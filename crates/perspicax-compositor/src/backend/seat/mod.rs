//! A real session: libseat, one GPU, one DRM output per connected monitor,
//! GLES to composite, and the person's keyboard and pointer ([`input`]).
//!
//! # Shape
//!
//! [`Session::open`] negotiates device access through libseat, opens the
//! primary GPU, and builds the renderer -- everything that does not need the
//! [`Compositor`] to exist. [`attach`] then runs against the built compositor:
//! it advertises dmabuf, and does the first connector scan by the same path a
//! hotplugged monitor takes, so start-up and hotplug cannot drift apart.
//!
//! # Frames
//!
//! Driven by vblank, not a timer. An output is in one of three states: idle,
//! with a render *scheduled* for the next idle turn of the loop, or with a
//! frame *queued* on the CRTC awaiting its vblank. A commit marks every output
//! dirty and schedules the idle ones; a vblank renders again only if something
//! was committed meanwhile. A quiet desktop therefore costs nothing -- no
//! wakeups at the refresh rate -- and a busy one is paced by the monitor.
//! Clients get their frame callbacks when their output is rendered, which is
//! what makes them draw at the monitor's rate rather than the loop's.
//!
//! # What is not here yet
//!
//! One GPU: the primary. A second card's connectors are ignored, and said so
//! in the log.

use std::{path::PathBuf, sync::Once};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            dmabuf::Dmabuf,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            ImportDma as _, ImportMemWl as _,
            element::{
                Kind,
                memory::MemoryRenderBufferRenderElement,
                render_elements,
                solid::{SolidColorBuffer, SolidColorRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::GlesRenderer,
        },
        session::{Event as SessionEvent, Session as _, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    delegate_dmabuf,
    desktop::{space::SpaceRenderElements, utils::send_frames_surface_tree},
    input::{keyboard::Keycode, pointer::CursorImageStatus},
    output::{Mode as WlMode, Output, PhysicalProperties, Scale},
    reexports::{
        calloop::LoopHandle,
        drm::control::{Device as _, ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::backend::GlobalId,
    },
    utils::{DeviceFd, Transform},
    wayland::dmabuf::{
        DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
    },
};

use super::{Running, connectors, cursor::Cursor};
use crate::{Error, state::Compositor};

mod input;
mod settings;
mod supervise;
mod titles;

pub(crate) use settings::{populate, reload};

type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;

render_elements! {
    /// What one output shows, front to back: the pointer, a snap preview,
    /// then the windows.
    Elements<=GlesRenderer>;
    Space=SpaceRenderElements<GlesRenderer, crate::framed::FramedElement<GlesRenderer>>,
    Cursor=MemoryRenderBufferRenderElement<GlesRenderer>,
    CursorSurface=WaylandSurfaceRenderElement<GlesRenderer>,
    Preview=SolidColorRenderElement,
}

/// Where a dragged window would snap: a wash of the theme's `snap-preview`
/// over the zone, light enough to see the windows through.
const PREVIEW_ALPHA: f32 = 0.25;

/// The snap preview's colour, from the theme.
fn preview_colour(settings: &perspicax_config::Config) -> [f32; 4] {
    let colour = settings.theme.palette[perspicax_policy::Role::SnapPreview];
    let channel = |value: u8| f32::from(value) / 255.0;
    [channel(colour.r), channel(colour.g), channel(colour.b), 1.0]
}

/// Which hardware planes a frame may use: the primary plane only, with the
/// cursor and every window composited into it.
///
/// Not the default, which also puts the cursor on the cursor plane and client
/// buffers on overlay planes. On a VT switch the next session takes the
/// display and repaints the primary plane, but planes it does not use keep
/// whatever was on them. The first hardware run left perspicax's cursor frozen
/// on top of Enlightenment's. It cannot be cleared on the way out: libseat
/// disables the seat before the pause event reaches us, so by the time we hear
/// of it the device is gone. Composite everything, and nothing is left behind.
/// The GPU does a little more work per frame; direct scanout of a fullscreen
/// client on the primary plane is still allowed.
const PLANES: FrameFlags = FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT;

/// Scanout formats, in order of preference: 8 bits per channel with and
/// without alpha. Every GPU Mesa drives supports these; 10-bit formats are a
/// later refinement and a common source of black screens on older drivers.
const COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Argb8888, Fourcc::Xrgb8888];

/// Everything the seat backend owns.
pub(crate) struct Session {
    /// The seat itself: device access, and the VT. Dropping the last clone
    /// closes the seat, which is what gives the VT back.
    pub(super) seat: LibSeatSession,
    libinput: Libinput,
    /// The node dmabufs are imported on, and the node client buffers must come
    /// from to be scanned out directly.
    render_node: DrmNode,
    outputs: DrmOutputManager<Allocator, Exporter, (), DrmDeviceFd>,
    renderer: GlesRenderer,
    heads: Vec<Head>,
    handle: LoopHandle<'static, Compositor>,
    /// Keys whose press the compositor kept (an escape hatch or a binding),
    /// so their release is swallowed too: a client must never see half of a
    /// chord it was not given.
    pub(super) swallowed: Vec<Keycode>,
    /// The person's config: focus model, bindings, input and outputs. See
    /// [`settings`].
    pub(super) settings: perspicax_config::Config,
    /// Where it was read from, for the reload binding. `None` means the
    /// classic profile, with nothing to re-read.
    config_path: Option<PathBuf>,
    /// A save of the config was heard and its reload is waiting for the
    /// saves to settle.
    reload_pending: bool,
    /// Every input device libinput has handed us, so pointer settings can be
    /// applied to each, and again on reload.
    devices: Vec<smithay::reexports::input::Device>,
    /// Programs this session started for the person (autostart, bindings),
    /// stopped with it and reaped as they exit.
    children: Vec<std::process::Child>,
    /// perspicax-shell, started again when it stops. See [`supervise`].
    shell: supervise::Supervisor,
    cursor: Cursor,
    /// The fonts window titles are written in. See [`titles`].
    titles: titles::Titles,
    /// The last press on a titlebar, by window, time and place: the first
    /// half of a double-click.
    pub(super) title_press: Option<(Option<perspicax_node::SurfaceId>, perspicax_policy::Press)>,
    /// A titlebar button pressed and not yet let go.
    pub(super) button_press: Option<(crate::framed::Framed, perspicax_policy::FrameButton)>,
    /// The snap preview's colour and size, kept so the damage tracker can
    /// tell a preview that moved from one that did not.
    preview: SolidColorBuffer,
    /// The pointer resting against an edge of the desk, on its way to a
    /// workspace flip. See [`input`].
    pub(super) dwell: perspicax_policy::EdgeDwell,
    /// When the timer for `dwell` is armed for, so a pointer pressed against
    /// an edge arms one timer rather than one per motion event.
    pub(super) dwell_armed: Option<u64>,
    /// Scroll over the desktop, gathered into whole notches.
    pub(super) notches: perspicax_policy::Notches,
    /// The Logo key, down and on its way to being a tap, unless another
    /// key, a button or the wheel comes first. See [`input`].
    pub(super) logo_tap: perspicax_policy::LogoTap,
    /// False while another VT has the seat: no device may be touched then.
    active: bool,
    pub(super) exit: bool,
    /// The output rules a display tool put in force, instead of the
    /// config's, until a reload changes `[[output]]`. See `crate::heads`.
    pub(super) runtime: Option<Vec<perspicax_config::OutputRule>>,
    /// Connected monitors left dark, by name, with the modes each offers: so
    /// a display tool can turn one on.
    dark: Vec<(String, Vec<connectors::Offered>)>,
}

/// One connected monitor, driven by one CRTC.
struct Head {
    connector: connector::Handle,
    crtc: crtc::Handle,
    output: Output,
    global: GlobalId,
    drm: DrmOutput<Allocator, Exporter, (), DrmDeviceFd>,
    /// The modes the monitor offers, and which it is showing.
    modes: Vec<connectors::Offered>,
    mode: usize,
    /// Something was committed since this output last rendered.
    dirty: bool,
    /// A render is waiting for the loop's next idle turn.
    scheduled: bool,
    /// A frame is on the CRTC, awaiting its vblank.
    queued: bool,
}

impl Session {
    /// Take the seat and the primary GPU, and build the renderer.
    ///
    /// # Errors
    ///
    /// [`Error::Seat`] for each way a TTY session fails to come up, worded for
    /// the person who just typed `perspicax --seat` and is looking at a text
    /// console.
    pub(crate) fn open(
        handle: &LoopHandle<'static, Compositor>,
        config_path: Option<PathBuf>,
    ) -> Result<Self, Error> {
        // First, before the seat is taken: a config that cannot be used is
        // refused while the person is still looking at a text console that
        // can show them why.
        let settings = settings::load(config_path.as_deref())?;

        let (mut seat, seat_events) = LibSeatSession::new().map_err(|error| {
            Error::Seat(format!(
                "could not open a seat session ({error}); run from a TTY, with \
                 seatd or logind managing the seat"
            ))
        })?;
        let seat_name = seat.seat();

        let path = gpu_path(&seat_name)?;
        let node = DrmNode::from_path(&path).map_err(|error| {
            Error::Seat(format!("{} is not a DRM node: {error}", path.display()))
        })?;
        let render_node = node
            .node_with_type(NodeType::Render)
            .and_then(Result::ok)
            .unwrap_or(node);
        tracing::info!(gpu = %path.display(), %render_node, "primary GPU");

        let fd = seat
            .open(
                &path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(|error| {
                Error::Seat(format!("the seat refused {}: {error}", path.display()))
            })?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        // `true`: start from a known state, every connector and plane off,
        // rather than inheriting whatever the console or the last compositor
        // left lit. One modeset of flicker buys never debugging someone
        // else's CRTC configuration.
        let (drm, drm_events) = DrmDevice::new(fd.clone(), true)
            .map_err(|error| Error::Seat(format!("could not take DRM master: {error}")))?;
        let gbm = GbmDevice::new(fd)
            .map_err(|error| Error::Seat(format!("could not create a GBM device: {error}")))?;

        let renderer = renderer(&gbm)?;
        let render_formats = renderer.egl_context().dmabuf_render_formats().clone();
        let outputs = DrmOutputManager::new(
            drm,
            GbmAllocator::new(
                gbm.clone(),
                GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
            ),
            GbmFramebufferExporter::new(gbm.clone(), Some(render_node)),
            Some(gbm),
            COLOR_FORMATS,
            render_formats,
        );

        let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
            seat.clone().into(),
        );
        libinput
            .udev_assign_seat(&seat_name)
            .map_err(|()| Error::Seat(format!("libinput could not take seat {seat_name}")))?;
        let udev = UdevBackend::new(&seat_name)
            .map_err(|error| Error::Seat(format!("could not watch udev: {error}")))?;

        handle
            .insert_source(seat_events, |event, (), state| match event {
                SessionEvent::PauseSession => pause(state),
                SessionEvent::ActivateSession => resume(state),
            })
            .map_err(|error| Error::EventLoop(error.to_string()))?;
        handle
            .insert_source(drm_events, |event, _, state| match event {
                DrmEvent::VBlank(crtc) => vblank(state, crtc),
                DrmEvent::Error(error) => tracing::warn!(%error, "DRM device error"),
            })
            .map_err(|error| Error::EventLoop(error.to_string()))?;
        handle
            .insert_source(
                LibinputInputBackend::new(libinput.clone()),
                |event, (), state| input::handle(state, event),
            )
            .map_err(|error| Error::EventLoop(error.to_string()))?;
        handle
            .insert_source(udev, move |event, (), state| match event {
                UdevEvent::Changed { device_id } if device_id == node.dev_id() => rescan(state),
                UdevEvent::Changed { .. } => {}
                UdevEvent::Added { path, .. } => {
                    tracing::info!(gpu = %path.display(), "a second GPU is not driven yet");
                }
                UdevEvent::Removed { device_id } if device_id == node.dev_id() => {
                    tracing::error!("the primary GPU went away; ending the session");
                    if let Running::Seat(session) = &mut state.backend {
                        session.exit = true;
                    }
                }
                UdevEvent::Removed { .. } => {}
            })
            .map_err(|error| Error::EventLoop(error.to_string()))?;

        let dwell = perspicax_policy::EdgeDwell::new(settings.flipping.delay_ms);
        let preview = SolidColorBuffer::new((1, 1), preview_colour(&settings));
        let cursor = Cursor::load(settings.theme.cursor.as_deref(), settings.theme.cursor_size);
        settings::watch(handle, config_path.as_deref());
        Ok(Self {
            seat,
            libinput,
            render_node,
            outputs,
            renderer,
            heads: Vec::new(),
            handle: handle.clone(),
            swallowed: Vec::new(),
            settings,
            config_path,
            reload_pending: false,
            devices: Vec::new(),
            children: Vec::new(),
            shell: supervise::Supervisor::new(),
            cursor,
            titles: titles::Titles::new(),
            title_press: None,
            button_press: None,
            preview,
            dwell,
            dwell_armed: None,
            notches: perspicax_policy::Notches::default(),
            logo_tap: perspicax_policy::LogoTap::default(),
            active: true,
            exit: false,
            runtime: None,
            dark: Vec::new(),
        })
    }

    /// Something was committed: every output may have something new to show.
    ///
    /// Every output rather than the ones the surface is on, because the damage
    /// tracker behind each output already skips a frame with nothing new in
    /// it, and asking it is cheaper than being wrong about which output a
    /// surface overlaps.
    pub(crate) fn request_frames(&mut self) {
        for head in &mut self.heads {
            head.dirty = true;
            if !head.queued && !head.scheduled {
                head.scheduled = true;
                let crtc = head.crtc;
                self.handle.insert_idle(move |state| render(state, crtc));
            }
        }
    }

    pub(crate) fn exit_requested(&self) -> bool {
        self.exit
    }

    /// Every lit monitor, with where its `[[output]]` rule places it.
    pub(crate) fn placements(&self) -> Vec<(Output, perspicax_policy::Place)> {
        self.heads
            .iter()
            .map(|head| {
                let name = head.output.name();
                let place = self
                    .rules()
                    .iter()
                    .find(|rule| rule.name == name)
                    .map(|rule| rule.place.clone())
                    .unwrap_or_default();
                (head.output.clone(), place)
            })
            .collect()
    }
}

/// Finish bringing the seat up, now the compositor exists: advertise dmabuf,
/// tell `wl_shm` what the renderer can read, arm the panic hook, and light the
/// monitors.
pub(crate) fn attach(state: &mut Compositor) -> Result<(), Error> {
    let Running::Seat(session) = &mut state.backend else {
        return Ok(());
    };
    state.shm.update_formats(session.renderer.shm_formats());
    let feedback = DmabufFeedbackBuilder::new(
        session.render_node.dev_id(),
        session.renderer.dmabuf_formats(),
    )
    .build()
    .map_err(|error| Error::Seat(format!("no dmabuf feedback: {error}")))?;
    state
        .dmabuf
        .create_global_with_default_feedback::<Compositor>(&state.display, &feedback);

    arm_panic_hook();
    let numlock = session.settings.keyboard.numlock;
    settings::apply_keyboard(state, numlock);
    settings::tell_appearance(state);
    rescan(state);
    let Running::Seat(session) = &state.backend else {
        return Ok(());
    };
    if session.heads.is_empty() {
        return Err(Error::Seat(
            "no connected monitor could be lit on the primary GPU".to_owned(),
        ));
    }
    Ok(())
}

/// Log a panic where the person can find it, before unwinding gives the seat
/// back.
///
/// On a seat, stderr is the VT the session took over, and nothing written to
/// it is visible until the session ends -- if then. The VT itself is restored
/// by unwinding, not by this hook: the [`Session`] lives on `run`'s stack, so
/// a panic on the compositor thread drops it, which drops DRM master and
/// closes the libseat session, and seatd or logind switches the VT back to
/// text. (Unwinding is why the release profile leaves `panic` alone.) Even an
/// abort gets there: the kernel closes the descriptors and the seat daemon
/// notices. What only this hook can do is make the reason survive.
fn arm_panic_hook() {
    static ARMED: Once = Once::new();
    ARMED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!(%info, "panicked; the seat is released as the stack unwinds");
            previous(info);
        }));
    });
}

/// The GPU udev says the seat boots on, or failing that the first one it has.
fn gpu_path(seat: &str) -> Result<PathBuf, Error> {
    let primary = primary_gpu(seat)
        .map_err(|error| Error::Seat(format!("udev could not list GPUs: {error}")))?;
    match primary {
        Some(path) => Ok(path),
        None => all_gpus(seat)
            .map_err(|error| Error::Seat(format!("udev could not list GPUs: {error}")))?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Seat(format!("seat {seat} has no GPU"))),
    }
}

impl Session {
    /// The output rules in force: a display tool's, or the config's.
    fn rules(&self) -> &[perspicax_config::OutputRule] {
        self.runtime.as_deref().unwrap_or(&self.settings.outputs)
    }

    /// Every monitor, lit or dark, as a display tool sees it.
    pub(crate) fn heads(
        &self,
        space: &smithay::desktop::Space<crate::framed::Framed>,
    ) -> Vec<perspicax_policy::Head> {
        let modes = |offered: &[connectors::Offered]| -> Vec<perspicax_policy::HeadMode> {
            offered
                .iter()
                .map(|mode| perspicax_policy::HeadMode {
                    width: i32::from(mode.width),
                    height: i32::from(mode.height),
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a refresh rate in millihertz fits an i32 many times over"
                    )]
                    refresh: (mode.refresh * 1000.0).round() as i32,
                    preferred: mode.preferred,
                })
                .collect()
        };
        self.heads
            .iter()
            .map(|head| perspicax_policy::Head {
                name: head.output.name(),
                enabled: true,
                modes: modes(&head.modes),
                current: Some(head.mode),
                position: space
                    .output_geometry(&head.output)
                    .map_or((0, 0), |area| (area.loc.x, area.loc.y)),
                scale: head.output.current_scale().fractional_scale(),
            })
            .chain(
                self.dark
                    .iter()
                    .map(|(name, offered)| perspicax_policy::Head {
                        name: name.clone(),
                        enabled: false,
                        modes: modes(offered),
                        current: None,
                        position: (0, 0),
                        scale: 1.0,
                    }),
            )
            .collect()
    }

    /// The renderer and the pointer's images, for drawing something other
    /// than a frame: a picture, which may ask for the pointer.
    #[cfg(feature = "capture")]
    pub(crate) fn renderer_and_cursor(&mut self) -> (&mut GlesRenderer, &mut Cursor) {
        (&mut self.renderer, &mut self.cursor)
    }
}

/// EGL on the GBM device, and GLES on that.
fn renderer(gbm: &GbmDevice<DrmDeviceFd>) -> Result<GlesRenderer, Error> {
    // `eglGetPlatformDisplay` returns the same display for the same device to
    // every caller in a process, so Smithay requires that nothing else in it
    // creates EGL displays behind its back and terminates this one. Nothing
    // does: this crate is the only EGL user in the process, and this is the
    // only display it creates.
    #[allow(unsafe_code, reason = "eglGetPlatformDisplay: see the comment above")]
    let display = unsafe { EGLDisplay::new(gbm.clone()) }
        .map_err(|error| Error::Seat(format!("no EGL display on the GPU: {error}")))?;
    let context = EGLContext::new(&display)
        .map_err(|error| Error::Seat(format!("no EGL context: {error}")))?;
    // A GLES renderer is undefined behaviour if its context is current on
    // another thread. This one was created on the line above, on the
    // compositor thread, and is moved into the renderer without ever being
    // made current anywhere else.
    #[allow(unsafe_code, reason = "eglMakeCurrent: see the comment above")]
    let renderer = unsafe { GlesRenderer::new(context) }
        .map_err(|error| Error::Seat(format!("no GLES renderer: {error}")))?;
    Ok(renderer)
}

/// Scan the primary GPU's connectors and bring the outputs into line: tear
/// down what was unplugged, light what was plugged in, then place them all
/// (see [`Compositor::arrange_outputs`]).
fn rescan(state: &mut Compositor) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    // Every connector not lit is offered again below, and says again
    // whether it is dark.
    session.dark.clear();
    let device = session.outputs.device();
    let resources = match device.resource_handles() {
        Ok(resources) => resources,
        Err(error) => {
            tracing::warn!(%error, "could not list DRM resources");
            return;
        }
    };
    let connected: Vec<(connector::Info, Vec<crtc::Handle>)> = resources
        .connectors()
        .iter()
        .filter_map(|&handle| device.get_connector(handle, true).ok())
        .filter(|info| info.state() == connector::State::Connected)
        .map(|info| {
            let crtcs = info
                .encoders()
                .iter()
                .filter_map(|&encoder| device.get_encoder(encoder).ok())
                .flat_map(|encoder| resources.filter_crtcs(encoder.possible_crtcs()))
                .collect();
            (info, crtcs)
        })
        .collect();

    let driving: Vec<_> = session
        .heads
        .iter()
        .map(|head| (head.connector, head.crtc))
        .collect();
    let candidates: Vec<_> = connected
        .iter()
        .map(|(info, crtcs)| (info.handle(), crtcs.clone()))
        .collect();
    let changes = connectors::reconcile(&driving, &candidates);

    for (_, crtc) in changes.gone {
        if let Some(at) = session.heads.iter().position(|head| head.crtc == crtc) {
            let head = session.heads.remove(at);
            tracing::info!(output = head.output.name(), "monitor unplugged");
            crate::layers::close_on(&head.output);
            state.space.unmap_output(&head.output);
            state.display.remove_global::<Compositor>(head.global);
        }
    }
    for connector in changes.dark {
        tracing::warn!(
            ?connector,
            "no free CRTC can drive this monitor; it stays dark"
        );
    }
    for (handle, crtc) in changes.new {
        let Some((info, _)) = connected.iter().find(|(info, _)| info.handle() == handle) else {
            continue;
        };
        // Lit now, placed below once every new monitor's size is known.
        match light(session, &state.display, info, crtc) {
            Ok(Some(head)) => {
                tracing::info!(output = head.output.name(), "monitor lit");
                session.heads.push(head);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(?handle, %error, "could not light monitor"),
        }
    }
    session.request_frames();
    state.arrange_outputs();
}

/// Put a display tool's request in force: the rules it implies, as if the
/// config had said them. Only where monitors go changed, and they are moved;
/// anything else, and they are lit again, which is a modeset.
pub(crate) fn apply_heads(state: &mut Compositor, changes: &[perspicax_policy::HeadChange]) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    let current = session.heads(&state.space);
    let mut rules = session.rules().to_vec();
    let mut moved_only = true;
    for change in changes {
        let Some(head) = current.iter().find(|head| head.name == change.name) else {
            continue;
        };
        let at = match rules.iter().position(|rule| rule.name == change.name) {
            Some(at) => at,
            None => {
                rules.push(perspicax_config::OutputRule {
                    name: change.name.clone(),
                    enable: true,
                    mode: None,
                    place: perspicax_policy::Place::Auto,
                    scale: None,
                });
                rules.len() - 1
            }
        };
        let rule = &mut rules[at];
        if change.enabled != head.enabled {
            moved_only = false;
        }
        rule.enable = change.enabled;
        if let Some(perspicax_policy::ModeChoice::Listed(index)) = change.mode
            && let Some(mode) = head.modes.get(index)
        {
            if head.current != Some(index) {
                moved_only = false;
            }
            rule.mode = Some(perspicax_config::Mode {
                width: u16::try_from(mode.width).unwrap_or(0),
                height: u16::try_from(mode.height).unwrap_or(0),
                refresh: Some(f64::from(mode.refresh) / 1000.0),
            });
        }
        if let Some(scale) = change.scale {
            if (scale - head.scale).abs() > f64::EPSILON {
                moved_only = false;
            }
            rule.scale = Some(scale);
        }
        if let Some((x, y)) = change.position {
            rule.place = perspicax_policy::Place::At(x, y);
        }
    }
    session.runtime = Some(rules);
    if moved_only {
        state.arrange_outputs();
    } else {
        relight(state);
        state.refit_frames();
    }
}

/// Tear every output down and light them again from the config. A modeset
/// on every monitor, which is why reload only does it when the output rules
/// changed.
fn relight(state: &mut Compositor) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    for head in session.heads.drain(..) {
        crate::layers::close_on(&head.output);
        state.space.unmap_output(&head.output);
        state.display.remove_global::<Compositor>(head.global);
    }
    rescan(state);
}

/// Bring one connector up on one CRTC, as its config rule says: at the mode
/// and scale asked for, or the preferred mode. Where it goes is decided after,
/// with every other monitor. `Ok(None)` for a monitor the config turned off.
fn light(
    session: &mut Session,
    display: &smithay::reexports::wayland_server::DisplayHandle,
    info: &connector::Info,
    crtc: crtc::Handle,
) -> Result<Option<Head>, String> {
    let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
    let offered: Vec<_> = info
        .modes()
        .iter()
        .map(|mode| {
            let (width, height) = mode.size();
            connectors::Offered {
                width,
                height,
                refresh: f64::from(mode.vrefresh()),
                preferred: mode.mode_type().contains(ModeTypeFlags::PREFERRED),
            }
        })
        .collect();
    let rule = session
        .rules()
        .iter()
        .find(|rule| rule.name == name)
        .cloned();
    if rule.as_ref().is_some_and(|rule| !rule.enable) {
        tracing::info!(output = name, "left dark by config");
        session.dark.push((name, offered));
        return Ok(None);
    }
    let rule = rule.as_ref();
    let wanted = rule
        .and_then(|rule| rule.mode)
        .map(|mode| (mode.width, mode.height, mode.refresh));
    let (at, honoured) =
        connectors::pick_mode(wanted, &offered).ok_or("the monitor offers no modes")?;
    if !honoured {
        tracing::warn!(
            output = name,
            ?wanted,
            "the monitor does not offer that mode; using its preferred one"
        );
    }
    let mode = info.modes()[at];

    let (width_mm, height_mm) = info.size().unwrap_or((0, 0));
    let output = Output::new(
        name,
        PhysicalProperties {
            size: (
                i32::try_from(width_mm).unwrap_or(0),
                i32::try_from(height_mm).unwrap_or(0),
            )
                .into(),
            subpixel: info.subpixel().into(),
            // EDID is not parsed yet; `wl_output.name` above is what tells
            // two monitors apart until it is.
            make: "unknown".to_owned(),
            model: "unknown".to_owned(),
        },
    );
    let wl_mode = WlMode::from(mode);
    let scale = match rule.and_then(|rule| rule.scale) {
        Some(scale) if scale.fract() == 0.0 => Scale::Integer(scale as i32),
        Some(scale) => Scale::Fractional(scale),
        None => Scale::Integer(1),
    };
    output.set_preferred(wl_mode);
    output.change_current_state(Some(wl_mode), Some(Transform::Normal), Some(scale), None);

    let drm = session
        .outputs
        .initialize_output::<_, Elements>(
            crtc,
            mode,
            &[info.handle()],
            &output,
            None,
            &mut session.renderer,
            &DrmOutputRenderElements::default(),
        )
        .map_err(|error| error.to_string())?;
    let global = output.create_global::<Compositor>(display);
    Ok(Some(Head {
        connector: info.handle(),
        crtc,
        output,
        global,
        drm,
        modes: offered,
        mode: at,
        dirty: true,
        scheduled: false,
        queued: false,
    }))
}

/// Render one output, queue it for scanout if anything changed, and let the
/// clients on it draw again.
fn render(state: &mut Compositor, crtc: crtc::Handle) {
    state.dress_frames();
    // Every titlebar's labels, while the compositor can still be asked: a
    // window's tabs name its whole group.
    let labels: Vec<_> = state
        .space
        .elements()
        .map(|window| {
            let (labels, front) = state.tab_labels(window);
            (window.clone(), labels, front)
        })
        .collect();
    let now = state.started_at().elapsed();
    let pointer_at = state
        .pointer
        .as_ref()
        .map(|pointer| pointer.current_location());
    let Compositor {
        backend,
        space,
        cursor: status,
        lock,
        snap_preview,
        tab_drop,
        ..
    } = state;
    let Running::Seat(session) = backend else {
        return;
    };
    let Session {
        renderer,
        heads,
        active,
        cursor,
        preview,
        titles,
        settings,
        ..
    } = &mut **session;
    let Some(head) = heads.iter_mut().find(|head| head.crtc == crtc) else {
        return;
    };
    head.scheduled = false;
    if !*active || head.queued {
        // Asleep, or a frame is already on the way; the vblank (or the
        // resume) comes back here once it can do something.
        return;
    }
    head.dirty = false;

    // Titles before the windows are drawn: writing one needs the fonts,
    // which drawing a window has no way to reach.
    let whole = crate::framed::whole_scale(head.output.current_scale().fractional_scale());
    for (window, labels, front) in labels {
        if space.outputs_for_element(&window).contains(&head.output) {
            titles.prepare(&window, labels, front, whole, &settings.theme.font.family);
        }
    }

    // Front to back: the pointer over everything, then the windows.
    let mut elements: Vec<Elements> = match (pointer_at, space.output_geometry(&head.output)) {
        (Some(at), Some(geometry)) if geometry.to_f64().contains(at) => {
            let scale = head.output.current_scale().fractional_scale();
            super::cursor::elements(renderer, cursor, status, at - geometry.loc.to_f64(), scale)
        }
        _ => Vec::new(),
    };
    // Under the pointer and over the windows: where the window being dragged
    // would snap, on the output it would snap on, or the titlebar the tab
    // being dragged would join.
    let target = snap_preview
        .as_ref()
        .filter(|snap| snap.output == head.output)
        .map(|snap| snap.area)
        .or(*tab_drop);
    if let (Some(area), Some(geometry), None) =
        (target, space.output_geometry(&head.output), lock.as_ref())
        && area.overlaps(geometry)
    {
        let scale = head.output.current_scale().fractional_scale();
        preview.update(area.size, preview_colour(settings));
        let at = (area.loc - geometry.loc).to_physical_precise_round(scale);
        elements.push(Elements::Preview(SolidColorRenderElement::from_buffer(
            preview,
            at,
            scale,
            PREVIEW_ALPHA,
            Kind::Unspecified,
        )));
    }
    // Locked: the lock surface for this output and nothing else. An output
    // whose lock surface has not arrived yet shows only the backdrop, never
    // the windows it is covering.
    let scale = head.output.current_scale().fractional_scale();
    let cover = lock.as_ref().map(|locked| locked.on(&head.output));
    match cover {
        Some(Some(cover)) => elements.extend(
            render_elements_from_surface_tree(
                renderer,
                cover.surface.wl_surface(),
                (0, 0),
                scale,
                1.0,
                Kind::Unspecified,
            )
            .into_iter()
            .map(Elements::CursorSurface),
        ),
        Some(None) => {}
        None => match crate::shell::stack(space, &head.output) {
            Some(stack) => elements.extend(scene(renderer, &stack, scale)),
            None => {
                tracing::warn!(output = head.output.name(), "output is not mapped");
                return;
            }
        },
    }
    match head
        .drm
        .render_frame(renderer, &elements, crate::BACKDROP, PLANES)
    {
        Ok(frame) if !frame.is_empty => match head.drm.queue_frame(()) {
            Ok(()) => head.queued = true,
            Err(error) => tracing::warn!(output = head.output.name(), %error, "frame not queued"),
        },
        Ok(_) => {}
        Err(error) => tracing::warn!(output = head.output.name(), %error, "frame not rendered"),
    }

    // Whether or not anything changed: a client waiting on a callback for a
    // commit that damaged nothing still has to be told it may draw again. An
    // animated client cursor is a client drawing too.
    let output = head.output.clone();
    for window in space.elements_for_output(&output) {
        window.send_frame(&output, now, None, |_, _| Some(output.clone()));
    }
    for layer in smithay::desktop::layer_map_for_output(&output).layers() {
        layer.send_frame(&output, now, None, |_, _| Some(output.clone()));
    }
    if let Some(cover) = lock.as_ref().and_then(|locked| locked.on(&output)) {
        send_frames_surface_tree(cover.surface.wl_surface(), &output, now, None, |_, _| {
            Some(output.clone())
        });
    }
    if let CursorImageStatus::Surface(surface) = status {
        send_frames_surface_tree(surface, &output, now, None, |_, _| Some(output.clone()));
    }
}

/// What `stack` shows, front to back, at `scale`: each layer surface and
/// window where the stack puts it.
fn scene(renderer: &mut GlesRenderer, stack: &crate::shell::Stack, scale: f64) -> Vec<Elements> {
    use smithay::{
        backend::renderer::element::{AsRenderElements, Wrap},
        desktop::LayerSurface,
        utils::{Logical, Point},
    };

    use crate::framed::{Framed, FramedElement};

    let at_scale = smithay::utils::Scale::from(scale);
    let layers = |renderer: &mut GlesRenderer, pieces: &[(LayerSurface, Point<i32, Logical>)]| {
        pieces
            .iter()
            .flat_map(|(surface, at)| {
                AsRenderElements::<GlesRenderer>::render_elements::<
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(
                    surface,
                    renderer,
                    at.to_physical_precise_round(scale),
                    at_scale,
                    1.0,
                )
            })
            .map(|element| Elements::Space(SpaceRenderElements::Surface(element)))
            .collect::<Vec<_>>()
    };
    let windows = |renderer: &mut GlesRenderer, pieces: &[(Framed, Point<i32, Logical>)]| {
        pieces
            .iter()
            .flat_map(|(window, at)| {
                window.render_elements::<FramedElement<GlesRenderer>>(
                    renderer,
                    at.to_physical_precise_round(scale),
                    at_scale,
                    1.0,
                )
            })
            .map(|element| Elements::Space(SpaceRenderElements::Element(Wrap::from(element))))
            .collect::<Vec<_>>()
    };
    let mut elements = layers(renderer, &stack.overlay);
    elements.extend(windows(renderer, &stack.raised));
    elements.extend(layers(renderer, &stack.top));
    elements.extend(windows(renderer, &stack.windows));
    elements.extend(layers(renderer, &stack.lower));
    elements
}

/// A frame reached the screen. Draw again if anything was committed since.
fn vblank(state: &mut Compositor, crtc: crtc::Handle) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    let Some(head) = session.heads.iter_mut().find(|head| head.crtc == crtc) else {
        return;
    };
    if let Err(error) = head.drm.frame_submitted() {
        tracing::warn!(output = head.output.name(), %error, "frame not submitted");
    }
    head.queued = false;
    if head.dirty {
        render(state, crtc);
    }
}

/// Another VT took the seat. Touch no device until it comes back.
fn pause(state: &mut Compositor) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    tracing::info!("session paused");
    session.active = false;
    session.libinput.suspend();
    session.outputs.pause();
}

/// The seat came back. Reset DRM to a known state -- whoever had it meanwhile
/// may have rewired CRTCs to connectors -- then rescan, since monitors may
/// have come and gone while we were away, and redraw everything.
fn resume(state: &mut Compositor) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    tracing::info!("session resumed");
    if session.libinput.resume().is_err() {
        tracing::warn!("libinput did not resume; the keyboard may be dead");
    }
    if let Err(error) = session.outputs.activate(true) {
        tracing::warn!(%error, "DRM did not reactivate");
    }
    session.active = true;
    session.swallowed.clear();
    // A Logo key let go on another VT never came back up here.
    session.logo_tap = perspicax_policy::LogoTap::default();
    for head in &mut session.heads {
        // Whatever was queued when we left will never see its vblank.
        head.queued = false;
    }
    rescan(state);
}

impl DmabufHandler for Compositor {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf
    }

    /// Accept a client's dmabuf only if the renderer can actually import it,
    /// so a buffer the GPU cannot read is refused at creation rather than
    /// failing silently at the first frame.
    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let render_node = match &mut self.backend {
            Running::Seat(session) => session
                .renderer
                .import_dmabuf(&dmabuf, None)
                .is_ok()
                .then_some(session.render_node),
            Running::Headless { .. } => None,
        };
        match render_node {
            Some(node) => {
                dmabuf.set_node(node);
                let _ = notifier.successful::<Self>();
            }
            None => notifier.failed(),
        }
    }
}

delegate_dmabuf!(Compositor);
