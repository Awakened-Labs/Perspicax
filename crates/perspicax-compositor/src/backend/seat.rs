//! A real session: libseat, one GPU, one DRM output per connected monitor,
//! GLES to composite, and the keyboard.
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
//! in the log. The pointer, xcursor and hit-testing arrive with slice 3; the
//! keyboard is here now because the escape hatches in [`super::hatch`] must
//! exist before anyone runs this on a TTY.

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
        input::{Event as _, InputEvent, KeyState, KeyboardKeyEvent as _},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            ImportDma as _, ImportMemWl as _, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer,
        },
        session::{Event as SessionEvent, Session as _, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    delegate_dmabuf,
    desktop::space::SpaceRenderElements,
    input::keyboard::{FilterResult, Keycode},
    output::{Mode as WlMode, Output, PhysicalProperties, Scale},
    reexports::{
        calloop::LoopHandle,
        drm::control::{Device as _, ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::backend::GlobalId,
    },
    utils::{DeviceFd, SERIAL_COUNTER, Transform},
    wayland::dmabuf::{
        DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
    },
};

use super::{
    Running, connectors,
    hatch::{self, Hatch},
};
use crate::{Error, state::Compositor};

type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;
type Elements = SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>;

/// What shows where no window is: a dark grey, so a working output is
/// distinguishable from a dead one. The wallpaper is the shell's job (W5).
const BACKDROP: [f32; 4] = [0.12, 0.12, 0.14, 1.0];

/// Scanout formats, in order of preference: 8 bits per channel with and
/// without alpha. Every GPU Mesa drives supports these; 10-bit formats are a
/// later refinement and a common source of black screens on older drivers.
const COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Argb8888, Fourcc::Xrgb8888];

/// Everything the seat backend owns.
pub(crate) struct Session {
    /// The seat itself: device access, and the VT. Dropping the last clone
    /// closes the seat, which is what gives the VT back.
    seat: LibSeatSession,
    libinput: Libinput,
    /// The node dmabufs are imported on, and the node client buffers must come
    /// from to be scanned out directly.
    render_node: DrmNode,
    outputs: DrmOutputManager<Allocator, Exporter, (), DrmDeviceFd>,
    renderer: GlesRenderer,
    heads: Vec<Head>,
    handle: LoopHandle<'static, Compositor>,
    /// Keys whose press was an escape hatch, so their release is swallowed
    /// too: a client must never see half of a chord it was not given.
    swallowed: Vec<Keycode>,
    /// False while another VT has the seat: no device may be touched then.
    active: bool,
    exit: bool,
}

/// One connected monitor, driven by one CRTC.
struct Head {
    connector: connector::Handle,
    crtc: crtc::Handle,
    output: Output,
    global: GlobalId,
    drm: DrmOutput<Allocator, Exporter, (), DrmDeviceFd>,
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
    pub(crate) fn open(handle: &LoopHandle<'static, Compositor>) -> Result<Self, Error> {
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
                |event, (), state| {
                    if let InputEvent::Keyboard { event } = event {
                        key(state, event.key_code(), event.state(), event.time_msec());
                    }
                },
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

        Ok(Self {
            seat,
            libinput,
            render_node,
            outputs,
            renderer,
            heads: Vec::new(),
            handle: handle.clone(),
            swallowed: Vec::new(),
            active: true,
            exit: false,
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
/// down what was unplugged, light what was plugged in.
fn rescan(state: &mut Compositor) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
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
        // The right edge of what is already mapped, so monitors line up left
        // to right in the order they were found. Real layout is config's job
        // (slice 5).
        let x = state
            .space
            .outputs()
            .filter_map(|output| state.space.output_geometry(output))
            .map(|geometry| geometry.loc.x + geometry.size.w)
            .max()
            .unwrap_or(0);
        match light(session, &state.display, info, crtc, x) {
            Ok(head) => {
                tracing::info!(output = head.output.name(), x, "monitor lit");
                state.space.map_output(&head.output, (x, 0));
                session.heads.push(head);
            }
            Err(error) => tracing::warn!(?handle, %error, "could not light monitor"),
        }
    }
    session.request_frames();
}

/// Bring one connector up on one CRTC, at its preferred mode.
fn light(
    session: &mut Session,
    display: &smithay::reexports::wayland_server::DisplayHandle,
    info: &connector::Info,
    crtc: crtc::Handle,
    x: i32,
) -> Result<Head, String> {
    let mode = info
        .modes()
        .iter()
        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
        .or_else(|| info.modes().first())
        .copied()
        .ok_or("the monitor offers no modes")?;
    let (width_mm, height_mm) = info.size().unwrap_or((0, 0));
    let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
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
    output.set_preferred(wl_mode);
    output.change_current_state(
        Some(wl_mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((x, 0).into()),
    );

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
    Ok(Head {
        connector: info.handle(),
        crtc,
        output,
        global,
        drm,
        dirty: true,
        scheduled: false,
        queued: false,
    })
}

/// Render one output, queue it for scanout if anything changed, and let the
/// clients on it draw again.
fn render(state: &mut Compositor, crtc: crtc::Handle) {
    let now = state.started_at().elapsed();
    let Compositor { backend, space, .. } = state;
    let Running::Seat(session) = backend else {
        return;
    };
    let Session {
        renderer,
        heads,
        active,
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

    let elements = match space.render_elements_for_output(renderer, &head.output, 1.0) {
        Ok(elements) => elements,
        Err(error) => {
            tracing::warn!(output = head.output.name(), %error, "output is not mapped");
            return;
        }
    };
    match head
        .drm
        .render_frame(renderer, &elements, BACKDROP, FrameFlags::DEFAULT)
    {
        Ok(frame) if !frame.is_empty => match head.drm.queue_frame(()) {
            Ok(()) => head.queued = true,
            Err(error) => tracing::warn!(output = head.output.name(), %error, "frame not queued"),
        },
        Ok(_) => {}
        Err(error) => tracing::warn!(output = head.output.name(), %error, "frame not rendered"),
    }

    // Whether or not anything changed: a client waiting on a callback for a
    // commit that damaged nothing still has to be told it may draw again.
    let output = head.output.clone();
    for window in space.elements_for_output(&output) {
        window.send_frame(&output, now, None, |_, _| Some(output.clone()));
    }
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
    for head in &mut session.heads {
        // Whatever was queued when we left will never see its vblank.
        head.queued = false;
    }
    rescan(state);
}

/// One key, from libinput. Escape hatches are taken here, before any client
/// or binding sees the key; everything else goes to the focused client.
fn key(state: &mut Compositor, keycode: Keycode, pressed: KeyState, time: u32) {
    let Some(keyboard) = state.keyboard.clone() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    let taken = keyboard.input(
        state,
        keycode,
        pressed,
        serial,
        time,
        |state, modifiers, keysym| {
            let Running::Seat(session) = &mut state.backend else {
                return FilterResult::Forward;
            };
            match pressed {
                KeyState::Pressed => {
                    match hatch::classify(modifiers, keysym.modified_sym(), &keysym.raw_syms()) {
                        Some(hatch) => {
                            session.swallowed.push(keycode);
                            FilterResult::Intercept(Some(hatch))
                        }
                        None => FilterResult::Forward,
                    }
                }
                KeyState::Released => match session.swallowed.iter().position(|&k| k == keycode) {
                    Some(at) => {
                        session.swallowed.swap_remove(at);
                        FilterResult::Intercept(None)
                    }
                    None => FilterResult::Forward,
                },
            }
        },
    );

    let (Some(Some(hatch)), Running::Seat(session)) = (taken, &mut state.backend) else {
        return;
    };
    match hatch {
        Hatch::Exit => {
            tracing::info!("Ctrl+Alt+Backspace: ending the session");
            session.exit = true;
        }
        Hatch::Vt(vt) => {
            tracing::info!(vt, "switching VT");
            if let Err(error) = session.seat.change_vt(vt) {
                tracing::warn!(vt, %error, "could not switch VT");
            }
        }
    }
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
