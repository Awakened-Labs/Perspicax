//! What the live tests of the protocols that watch other windows share: a
//! compositor on a thread, a client of it, and the waiting.
//!
//! The client is one type for every such test, speaking whichever of the
//! protocols a test binds. Each test file compiles this module on its own and
//! uses part of it, hence the `dead_code` allowance.

#![allow(dead_code, reason = "each test file uses part of this module")]

use std::{
    collections::HashMap,
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Command, Config, Facts, Requests, Stop};
use perspicax_index::HostFacts;
use perspicax_policy::Action;
use perspicax_protocols::shell::v1::client::perspicax_shell_v1::{self, PerspicaxShellV1};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer_constraints,
    delegate_registry, delegate_relative_pointer, delegate_shm, delegate_subcompositor,
    delegate_xdg_popup, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer_constraints::{PointerConstraintsHandler, PointerConstraintsState},
        relative_pointer::{RelativeMotionEvent, RelativePointerHandler, RelativePointerState},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        xdg::{
            XdgPositioner, XdgShell, XdgSurface as _,
            popup::{Popup, PopupConfigure, PopupHandler},
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
    subcompositor::SubcompositorState,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    backend::ObjectId,
    event_created_child,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_subsurface, wl_surface},
};
use wayland_protocols::ext::{
    foreign_toplevel_list::v1::client::{
        ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
        ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
    },
    session_lock::v1::client::{
        ext_session_lock_manager_v1::ExtSessionLockManagerV1,
        ext_session_lock_v1::{self, ExtSessionLockV1},
    },
    workspace::v1::client::{
        ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
        ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
        ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
    },
};
use wayland_protocols::wp::{
    pointer_constraints::zv1::client::{
        zwp_confined_pointer_v1::ZwpConfinedPointerV1, zwp_locked_pointer_v1::ZwpLockedPointerV1,
        zwp_pointer_constraints_v1::Lifetime,
    },
    relative_pointer::zv1::client::zwp_relative_pointer_v1::ZwpRelativePointerV1,
};
use wayland_protocols::xdg::shell::client::{xdg_positioner, xdg_surface};
use wayland_protocols_wlr::{
    foreign_toplevel::v1::client::{
        zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
        zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
    },
    output_management::v1::client::{
        zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
        zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
        zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
        zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
        zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
    },
    screencopy::v1::client::{
        zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
        zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    },
};

pub const WINDOW: (u32, u32) = (400, 300);

/// A rectangle as the protocol sends one: x, y, width, height.
pub type Geometry = (i32, i32, i32, i32);

/// A compositor on a thread, and a channel to drive it with.
pub struct Session {
    socket: String,
    pub facts: Facts,
    pub requests: Requests,
    stop: Stop,
    thread: thread::JoinHandle<Result<(), perspicax_compositor::Error>>,
}

impl Session {
    /// A headless compositor on `backend`, on a socket named for `name`.
    pub fn start(name: &str, backend: Backend) -> Self {
        Self::start_with(name, backend, false)
    }

    /// The same, with Xwayland: its display is in the facts once it is
    /// ready.
    pub fn start_with_xwayland(name: &str, backend: Backend) -> Self {
        Self::start_with(name, backend, true)
    }

    fn start_with(name: &str, backend: Backend, xwayland: bool) -> Self {
        let socket = format!("perspicax-{name}-{}", std::process::id());
        let facts = Facts::new();
        let requests = Requests::new();
        let stop = Stop::new();
        let thread = {
            let (facts, requests, stop, socket) = (
                facts.clone(),
                requests.clone(),
                stop.clone(),
                socket.clone(),
            );
            thread::spawn(move || {
                let config = Config {
                    backend,
                    spawn: Vec::new(),
                    env: Vec::new(),
                    run_for: Some(Duration::from_secs(30)),
                    config: None,
                    socket: Some(socket),
                    xwayland,
                };
                perspicax_compositor::run(&config, &facts, &requests, &stop)
            })
        };
        Self {
            socket,
            facts,
            requests,
            stop,
            thread,
        }
    }

    /// A client, its registry read.
    pub fn client(&self) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList) {
        let client = connect(&self.socket);
        let (globals, queue) = registry_queue_init(&client).expect("the registry");
        let qh = queue.handle();
        (Desk::new(&globals, &qh), queue, qh, globals)
    }

    /// The socket's name in `XDG_RUNTIME_DIR`, for a client of the test's
    /// own to [`connect`] to.
    pub fn socket(&self) -> &str {
        &self.socket
    }

    pub fn command(&self, command: Command) {
        self.requests
            .command(command)
            .expect("the compositor is listening");
    }

    pub fn perform(&self, action: Action) {
        self.command(Command::Perform(action));
    }

    pub fn wait_for(&self, ready: impl Fn(&HostFacts) -> bool) -> HostFacts {
        wait_for(&self.facts, ready)
    }

    /// Whether the compositor has stopped on its own: a session that ended.
    pub fn ended(&self) -> bool {
        self.thread.is_finished()
    }

    pub fn stop<D>(self, clients: D) {
        self.stop.request();
        drop(clients);
        self.thread
            .join()
            .expect("the compositor thread panicked")
            .expect("the compositor failed");
    }
}

/// Connect by path rather than through `WAYLAND_DISPLAY`: setting an
/// environment variable in a test process races every other test in it.
pub fn connect(socket: &str) -> Connection {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(dir.join(socket)) {
            Ok(stream) => return Connection::from_socket(stream).expect("a Wayland connection"),
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("the compositor never opened {socket}: {error}"),
        }
    }
}

pub fn wait_for(facts: &Facts, ready: impl Fn(&HostFacts) -> bool) -> HostFacts {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let published = facts.read();
        if ready(&published) {
            return published;
        }
        assert!(Instant::now() < deadline, "never published: {published:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Dispatch until `done`, or fail after five seconds, then one more round
/// trip to flush whatever was sent beside it.
pub fn until<D>(queue: &mut EventQueue<D>, state: &mut D, done: impl Fn(&D) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(state) {
        assert!(Instant::now() < deadline, "never happened");
        queue.roundtrip(state).expect("dispatch");
        if !done(state) {
            thread::sleep(Duration::from_millis(10));
        }
    }
    queue.roundtrip(state).expect("flush");
}

/// Whether the registry advertised `interface` to this client.
pub fn advertised(globals: &GlobalList, interface: &str) -> bool {
    globals
        .contents()
        .with_list(|list| list.iter().any(|global| global.interface == interface))
}

/// One window as a taskbar's list was told it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    pub identifier: String,
    pub title: String,
    pub app_id: String,
    pub closed: bool,
    /// How many `done`s it has had: each one a complete update.
    pub done: usize,
}

/// One window as a taskbar's controls were told it.
#[derive(Debug, Clone)]
pub struct Tasked {
    pub handle: ZwlrForeignToplevelHandleV1,
    pub title: String,
    pub app_id: String,
    pub states: Vec<zwlr_foreign_toplevel_handle_v1::State>,
    pub outputs: usize,
    pub parent: Option<ObjectId>,
    pub closed: bool,
    pub done: usize,
}

impl Tasked {
    pub fn is(&self, state: zwlr_foreign_toplevel_handle_v1::State) -> bool {
        self.states.contains(&state)
    }
}

/// One workspace as a pager was told it.
#[derive(Debug, Clone)]
pub struct Paged {
    pub handle: ExtWorkspaceHandleV1,
    pub id: String,
    pub name: String,
    pub coordinates: Vec<u32>,
    pub active: bool,
    pub removed: bool,
}

/// One group of workspaces as a pager was told it.
#[derive(Debug, Clone)]
pub struct PagedGroup {
    pub handle: ExtWorkspaceGroupHandleV1,
    pub outputs: usize,
    pub workspaces: Vec<ObjectId>,
    pub removed: bool,
}

/// One screencopy frame, as grim would see it.
pub struct Grab {
    pub frame: ZwlrScreencopyFrameV1,
    /// Format, width, height and stride, each buffer the frame offered.
    pub offered: Vec<(u32, u32, u32, u32)>,
    pub buffer_done: bool,
    pub ready: bool,
    pub failed: bool,
    pub damaged: bool,
    pub buffer: Option<smithay_client_toolkit::shm::slot::Buffer>,
}

/// One monitor as a display tool was told it.
#[derive(Debug, Clone)]
pub struct Shown {
    pub head: ZwlrOutputHeadV1,
    pub name: String,
    pub enabled: bool,
    pub modes: Vec<ObjectId>,
    pub current: Option<ObjectId>,
    pub position: (i32, i32),
    pub scale: f64,
    pub finished: bool,
}

/// One mode as a display tool was told it.
#[derive(Debug, Clone)]
pub struct ShownMode {
    pub mode: ZwlrOutputModeV1,
    pub size: (i32, i32),
    pub refresh: i32,
    pub finished: bool,
}

/// A hold on the pointer this client asked for.
pub enum Held {
    Locked(ZwpLockedPointerV1),
    Confined(ZwpConfinedPointerV1),
}

/// What the shell channel told the client, in the order it was told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Told {
    /// The start menu, on the monitor of this name, if the client knows it.
    StartMenu(Option<String>),
    /// The root menu, on the monitor of this name, at this point on it.
    RootMenu(Option<String>, i32, i32),
    /// The pie of this name, on the monitor of this name, at this point on
    /// it.
    PieMenu(Option<String>, i32, i32, String),
    Reconfigure,
}

/// The test's client: windows of its own, drawn once in a flat colour, and
/// whichever of the watching protocols a test binds.
pub struct Desk {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    subcompositor: SubcompositorState,
    xdg: XdgShell,
    shm: Shm,
    pool: SlotPool,
    pub windows: Vec<Window>,
    /// Windows that never draw, and how many configures they have had: each
    /// one acknowledged and committed with nothing attached.
    pub blank: Vec<Window>,
    pub blank_configured: usize,
    /// Windows that declare a geometry of their own when they draw, or
    /// none at all, in place of their whole buffer.
    pub declaring: Vec<(Window, Option<Geometry>)>,
    /// Each window's colour, ARGB, in the order they opened.
    pub colours: Vec<u32>,
    pub drawn: usize,
    /// The size the compositor last offered a window, where it named one.
    /// Windows are drawn at their own size whatever it offers.
    pub offered: Option<(u32, u32)>,
    pub list: Option<ExtForeignToplevelListV1>,
    pub list_finished: bool,
    pub listed: HashMap<ObjectId, Listed>,
    pub lock: Option<ExtSessionLockV1>,
    pub locked: bool,
    pub taskbar: Option<ZwlrForeignToplevelManagerV1>,
    pub taskbar_finished: bool,
    pub tasked: Vec<Tasked>,
    pub seat: Option<wl_seat::WlSeat>,
    pub pointer: Option<wl_pointer::WlPointer>,
    /// The serial of the pointer's last arrival on one of our surfaces: what
    /// `set_cursor` has to name to be heard.
    pub entered: Option<u32>,
    /// Which of our surfaces the pointer is over, from its last arrival
    /// until it leaves.
    pub pointed: Option<wl_surface::WlSurface>,
    /// Every button the pointer pressed (`true`) or let go (`false`) on one
    /// of our surfaces, by Linux code, in order.
    pub buttons: Vec<(u32, bool)>,
    /// How many scroll events the pointer brought one of our surfaces.
    pub scrolls: usize,
    /// Where the pointer moved to on one of our surfaces, in its own
    /// coordinates, each `wl_pointer.motion` in order. An arrival is not a
    /// motion and is not here.
    pub motions: Vec<(f64, f64)>,
    /// The mouse's own motion, as `bind_relative` hears it: how far, and
    /// how far before acceleration, each in order.
    pub relative_motions: Vec<((f64, f64), (f64, f64))>,
    relative: Option<(RelativePointerState, ZwpRelativePointerV1)>,
    constraints: Option<PointerConstraintsState>,
    /// The hold on the pointer asked for last, until `release`.
    pub held: Option<Held>,
    /// What the compositor said of it, in order: `locked`, `unlocked`,
    /// `confined`, `unconfined`.
    pub holds: Vec<&'static str>,
    /// How many times the compositor asked one of our windows to close.
    pub asked_to_close: usize,
    pub pager: Option<ExtWorkspaceManagerV1>,
    pub pager_done: usize,
    pub pager_finished: bool,
    pub groups: Vec<PagedGroup>,
    pub paged: Vec<Paged>,
    pub screencopy: Option<ZwlrScreencopyManagerV1>,
    pub grabs: Vec<Grab>,
    pub displays: Option<ZwlrOutputManagerV1>,
    pub display_serial: Option<u32>,
    pub shown: Vec<Shown>,
    pub shown_modes: Vec<ShownMode>,
    /// `succeeded`, `failed` or `cancelled`, for the last configuration.
    pub configured: Option<&'static str>,
    pub shell: Option<PerspicaxShellV1>,
    pub told: Vec<Told>,
    pub shell_finished: bool,
    /// The keyboard's layouts as the shell channel last listed them, each
    /// its name and short label, and the one in use.
    pub layouts: Vec<(String, String)>,
    listing: Vec<(String, String)>,
    pub active_layout: Option<u32>,
    /// How many of the channel's layout events arrived, of any kind.
    pub layout_events: usize,
    layer_shell: LayerShell,
    /// Strips across the top of the screen, each with its colour and height.
    pub layers: Vec<(LayerSurface, u32, u32)>,
    pub layers_drawn: usize,
    /// How many configures the popups this client opened have had. Nothing
    /// is drawn in reply: a test paints a popup itself, once it may.
    pub popups_configured: usize,
}

impl Desk {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>) -> Self {
        let shm = Shm::bind(globals, qh).expect("wl_shm");
        let pool = SlotPool::new(1 << 22, &shm).expect("a buffer pool");
        let compositor = CompositorState::bind(globals, qh).expect("wl_compositor");
        let subcompositor =
            SubcompositorState::bind(compositor.wl_compositor().clone(), globals, qh)
                .expect("wl_subcompositor");
        Self {
            registry: RegistryState::new(globals),
            outputs: OutputState::new(globals, qh),
            compositor,
            subcompositor,
            xdg: XdgShell::bind(globals, qh).expect("xdg_wm_base"),
            shm,
            pool,
            windows: Vec::new(),
            blank: Vec::new(),
            blank_configured: 0,
            declaring: Vec::new(),
            colours: Vec::new(),
            offered: None,
            drawn: 0,
            list: None,
            list_finished: false,
            listed: HashMap::new(),
            lock: None,
            locked: false,
            taskbar: None,
            taskbar_finished: false,
            tasked: Vec::new(),
            seat: None,
            pointer: None,
            entered: None,
            pointed: None,
            buttons: Vec::new(),
            scrolls: 0,
            motions: Vec::new(),
            relative_motions: Vec::new(),
            relative: None,
            constraints: None,
            held: None,
            holds: Vec::new(),
            asked_to_close: 0,
            pager: None,
            pager_done: 0,
            pager_finished: false,
            groups: Vec::new(),
            paged: Vec::new(),
            screencopy: None,
            grabs: Vec::new(),
            displays: None,
            display_serial: None,
            shown: Vec::new(),
            shown_modes: Vec::new(),
            configured: None,
            shell: None,
            told: Vec::new(),
            shell_finished: false,
            layouts: Vec::new(),
            listing: Vec::new(),
            active_layout: None,
            layout_events: 0,
            layer_shell: LayerShell::bind(globals, qh).expect("zwlr_layer_shell_v1"),
            layers: Vec::new(),
            layers_drawn: 0,
            popups_configured: 0,
        }
    }

    /// A strip `height` tall across the top of the screen on `layer`, drawn
    /// in one colour, ARGB, reserving no room: a panel, or a menu on
    /// `overlay`.
    pub fn open_strip(
        &mut self,
        qh: &QueueHandle<Self>,
        layer: Layer,
        namespace: &str,
        height: u32,
        colour: u32,
    ) {
        let surface = self.compositor.create_surface(qh);
        let strip =
            self.layer_shell
                .create_layer_surface(qh, surface, layer, Some(namespace), None);
        strip.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
        strip.set_size(0, height);
        strip.commit();
        self.layers.push((strip, colour, height));
    }

    /// The same on `overlay`, asking for every key as a launcher does: the
    /// compositor gives it the keyboard the moment it is up.
    pub fn open_launcher(&mut self, qh: &QueueHandle<Self>, height: u32, colour: u32) {
        self.open_strip(qh, Layer::Overlay, "launcher", height, colour);
        let (launcher, ..) = self.layers.last().expect("just opened");
        launcher.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        launcher.commit();
    }

    /// A panel `height` tall across the top of the screen on `top`, drawn
    /// in one colour, ARGB, that reserves its strip: windows are fitted
    /// below it, as a real panel's are.
    pub fn open_panel(
        &mut self,
        qh: &QueueHandle<Self>,
        namespace: &str,
        height: u32,
        colour: u32,
    ) {
        let surface = self.compositor.create_surface(qh);
        let panel =
            self.layer_shell
                .create_layer_surface(qh, surface, Layer::Top, Some(namespace), None);
        panel.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
        panel.set_size(0, height);
        panel.set_exclusive_zone(i32::try_from(height).expect("a panel's height"));
        panel.commit();
        self.layers.push((panel, colour, height));
    }

    pub fn open_window(&mut self, qh: &QueueHandle<Self>, title: &str, app_id: &str) {
        self.open_coloured(qh, title, app_id, 0xffff_ffff);
    }

    /// Open a window drawn all in one colour, ARGB.
    pub fn open_coloured(
        &mut self,
        qh: &QueueHandle<Self>,
        title: &str,
        app_id: &str,
        colour: u32,
    ) {
        self.colours.push(colour);
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, WindowDecorations::None, qh);
        window.set_title(title);
        window.set_app_id(app_id);
        window.commit();
        self.windows.push(window);
    }

    /// Open a window that declares its geometry and never draws.
    pub fn open_blank(&mut self, qh: &QueueHandle<Self>, title: &str) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, WindowDecorations::None, qh);
        window.set_title(title);
        let (width, height) = WINDOW;
        window.xdg_surface().set_window_geometry(
            0,
            0,
            i32::try_from(width).unwrap(),
            i32::try_from(height).unwrap(),
        );
        window.commit();
        self.blank.push(window);
    }

    /// Open a window drawn like any other that declares `geometry` as its
    /// window geometry, `(x, y, width, height)`, or none at all: what
    /// GStreamer's `waylandsink` declares.
    pub fn open_declaring(
        &mut self,
        qh: &QueueHandle<Self>,
        title: &str,
        geometry: Option<Geometry>,
    ) {
        self.open_window(qh, title, title);
        let window = self.windows.last().expect("just opened").clone();
        self.declaring.push((window, geometry));
    }

    /// Open a window that declares no geometry and never draws.
    pub fn open_bare(&mut self, qh: &QueueHandle<Self>, title: &str) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, WindowDecorations::None, qh);
        window.set_title(title);
        window.commit();
        self.blank.push(window);
    }

    /// Say that only `(x, y, width, height)` of `surface` is opaque, at its
    /// next commit.
    pub fn set_opaque(&self, surface: &wl_surface::WlSurface, rect: Geometry) {
        let region = Region::new(&self.compositor).expect("a region");
        region.add(rect.0, rect.1, rect.2, rect.3);
        surface.set_opaque_region(Some(region.wl_region()));
    }

    /// A subsurface of `parent`, at `at` in the parent's coordinates, with
    /// nothing in it yet. Synchronized, as every subsurface starts, unless
    /// `sync` is false.
    pub fn open_subsurface(
        &self,
        qh: &QueueHandle<Self>,
        parent: &wl_surface::WlSurface,
        at: (i32, i32),
        sync: bool,
    ) -> (wl_subsurface::WlSubsurface, wl_surface::WlSurface) {
        let (subsurface, surface) = self.subcompositor.create_subsurface(parent.clone(), qh);
        subsurface.set_position(at.0, at.1);
        if !sync {
            subsurface.set_desync();
        }
        (subsurface, surface)
    }

    /// A popup of `parent`, `size` big, with its corner at `at` in the
    /// parent's window geometry: a menu opened by a click there. Nothing is
    /// drawn in it: wait for it to be configured, then paint it.
    pub fn open_popup(
        &self,
        qh: &QueueHandle<Self>,
        parent: &xdg_surface::XdgSurface,
        at: (i32, i32),
        size: (u32, u32),
    ) -> Popup {
        let positioner = self.positioner(at, size);
        Popup::new(parent, &positioner, qh, &self.compositor, &self.xdg).expect("xdg_popup")
    }

    /// A popup of `panel`, as [`Self::open_popup`] opens one of a window:
    /// opened with no parent, and given to the panel before its first
    /// commit, as layer-shell asks.
    pub fn open_panel_popup(
        &self,
        qh: &QueueHandle<Self>,
        panel: &LayerSurface,
        at: (i32, i32),
        size: (u32, u32),
    ) -> Popup {
        let positioner = self.positioner(at, size);
        let surface = self.compositor.create_surface(qh);
        let popup =
            Popup::from_surface(None, &positioner, qh, surface, &self.xdg).expect("xdg_popup");
        panel.get_popup(popup.xdg_popup());
        popup.wl_surface().commit();
        popup
    }

    /// Place a popup of `size` with its corner exactly at `at`: hung from a
    /// one-pixel anchor there, growing down and to the right.
    fn positioner(&self, at: (i32, i32), size: (u32, u32)) -> XdgPositioner {
        let positioner = XdgPositioner::new(&self.xdg).expect("xdg_positioner");
        positioner.set_size(
            i32::try_from(size.0).unwrap(),
            i32::try_from(size.1).unwrap(),
        );
        positioner.set_anchor_rect(at.0, at.1, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        positioner
    }

    /// Attach a buffer of `size` to `surface`, all in one colour, ARGB, and
    /// damage all of it the way toolkits do, with a rectangle as large as
    /// the protocol allows. Not committed: the caller says which commit
    /// shows it.
    pub fn paint(&mut self, surface: &wl_surface::WlSurface, size: (u32, u32), colour: u32) {
        let (width, height) = size;
        let (buffer, canvas) = self
            .pool
            .create_buffer(
                i32::try_from(width).unwrap(),
                i32::try_from(height).unwrap(),
                i32::try_from(width * 4).unwrap(),
                wl_shm::Format::Argb8888,
            )
            .expect("a buffer");
        for pixel in canvas.chunks_exact_mut(4) {
            pixel.copy_from_slice(&colour.to_le_bytes());
        }
        buffer.attach_to(surface).expect("attach");
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
    }

    /// Bind the window list. Panics if it is not advertised.
    pub fn bind_list(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.list = Some(
            globals
                .bind::<ExtForeignToplevelListV1, _, _>(qh, 1..=1, ())
                .expect("ext_foreign_toplevel_list_v1"),
        );
    }

    /// The live windows the list knows, by title.
    pub fn listed_titles(&self) -> Vec<String> {
        let mut titles: Vec<_> = self
            .listed
            .values()
            .filter(|listed| !listed.closed && listed.done > 0)
            .map(|listed| listed.title.clone())
            .collect();
        titles.sort();
        titles
    }

    pub fn listed_by_title(&self, title: &str) -> Option<&Listed> {
        self.listed
            .values()
            .find(|listed| listed.title == title && !listed.closed)
    }

    /// Bind the taskbar's controls, and a seat to activate with. Panics if
    /// they are not advertised.
    pub fn bind_taskbar(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.taskbar = Some(
            globals
                .bind::<ZwlrForeignToplevelManagerV1, _, _>(qh, 1..=3, ())
                .expect("zwlr_foreign_toplevel_manager_v1"),
        );
        self.seat = Some(
            globals
                .bind::<wl_seat::WlSeat, _, _>(qh, 1..=7, ())
                .expect("wl_seat"),
        );
    }

    /// The window the taskbar knows by this title, open.
    pub fn task(&self, title: &str) -> Option<&Tasked> {
        self.tasked
            .iter()
            .find(|tasked| tasked.title == title && !tasked.closed && tasked.done > 0)
    }

    /// The open windows the taskbar knows, by title.
    pub fn task_titles(&self) -> Vec<String> {
        let mut titles: Vec<_> = self
            .tasked
            .iter()
            .filter(|tasked| !tasked.closed && tasked.done > 0)
            .map(|tasked| tasked.title.clone())
            .collect();
        titles.sort();
        titles
    }

    /// Click a window in the taskbar.
    pub fn activate(&self, title: &str) {
        let seat = self.seat.as_ref().expect("bind_taskbar first");
        self.task(title)
            .expect("a known window")
            .handle
            .activate(seat);
    }

    /// Bind the pager. Panics if it is not advertised.
    pub fn bind_pager(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.pager = Some(
            globals
                .bind::<ExtWorkspaceManagerV1, _, _>(qh, 1..=1, ())
                .expect("ext_workspace_manager_v1"),
        );
    }

    /// The groups the pager knows, not removed.
    pub fn live_groups(&self) -> Vec<&PagedGroup> {
        self.groups.iter().filter(|group| !group.removed).collect()
    }

    /// The names of the workspaces showing, in every group.
    pub fn active_workspaces(&self) -> Vec<String> {
        self.paged
            .iter()
            .filter(|paged| !paged.removed && paged.active)
            .map(|paged| paged.name.clone())
            .collect()
    }

    /// Ask to switch to the workspace named `name` in the first group that
    /// has one, and commit.
    pub fn switch_to(&self, name: &str) {
        let paged = self
            .paged
            .iter()
            .find(|paged| !paged.removed && paged.name == name)
            .expect("a workspace of that name");
        paged.handle.activate();
        self.pager.as_ref().expect("bind_pager first").commit();
    }

    /// Whether no monitor has been announced yet.
    pub fn outputs_empty(&self) -> bool {
        self.outputs.outputs().next().is_none()
    }

    /// A pointer of our own, on the seat `bind_taskbar` bound or a new one.
    pub fn bind_pointer(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        let seat = self.seat.get_or_insert_with(|| {
            globals
                .bind::<wl_seat::WlSeat, _, _>(qh, 1..=7, ())
                .expect("wl_seat")
        });
        self.pointer = Some(seat.get_pointer(qh, ()));
    }

    /// The mouse's own motion, for our pointer, as a game asks for it.
    /// Panics if `bind_pointer` has not been called, or the global is not
    /// advertised.
    pub fn bind_relative(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        let pointer = self.pointer.as_ref().expect("bind_pointer first");
        let manager = RelativePointerState::bind(globals, qh);
        let relative = manager
            .get_relative_pointer(pointer, qh)
            .expect("zwp_relative_pointer_manager_v1");
        self.relative = Some((manager, relative));
    }

    /// Ask to hold the pointer, as a game does: locked in place over the
    /// `nth` window, or confined to it, inside `region` (in the window
    /// surface's coordinates) or anywhere on it. Binds
    /// `zwp_pointer_constraints_v1` the first time; panics if it is not
    /// advertised, or `bind_pointer` has not been called.
    pub fn hold_pointer(
        &mut self,
        globals: &GlobalList,
        qh: &QueueHandle<Self>,
        nth: usize,
        locked: bool,
        region: Option<Geometry>,
        lifetime: Lifetime,
    ) {
        let constraints = self
            .constraints
            .get_or_insert_with(|| PointerConstraintsState::bind(globals, qh));
        let pointer = self.pointer.as_ref().expect("bind_pointer first");
        let surface = self.windows[nth].wl_surface();
        let region = region.map(|(x, y, w, h)| {
            let region = Region::new(&self.compositor).expect("a region");
            region.add(x, y, w, h);
            region
        });
        let region = region.as_ref().map(Region::wl_region);
        self.held = Some(if locked {
            Held::Locked(
                constraints
                    .lock_pointer(surface, pointer, region, lifetime, qh)
                    .expect("zwp_pointer_constraints_v1"),
            )
        } else {
            Held::Confined(
                constraints
                    .confine_pointer(surface, pointer, region, lifetime, qh)
                    .expect("zwp_pointer_constraints_v1"),
            )
        });
    }

    /// Give the confine asked for last a new region, applied with the
    /// `nth` window's next commit, which this makes.
    pub fn confine_to(&self, nth: usize, (x, y, w, h): Geometry) {
        let Some(Held::Confined(confined)) = &self.held else {
            panic!("confine the pointer first");
        };
        let region = Region::new(&self.compositor).expect("a region");
        region.add(x, y, w, h);
        confined.set_region(Some(region.wl_region()));
        self.windows[nth].wl_surface().commit();
    }

    /// Say where the `nth` window draws the pointer it holds in place,
    /// applied with the commit this makes.
    pub fn hint(&self, nth: usize, x: f64, y: f64) {
        let Some(Held::Locked(locked)) = &self.held else {
            panic!("lock the pointer first");
        };
        locked.set_cursor_position_hint(x, y);
        self.windows[nth].wl_surface().commit();
    }

    /// Let go of the hold asked for last, as a game leaving mouselook does.
    pub fn release(&mut self) {
        match self.held.take() {
            Some(Held::Locked(locked)) => locked.destroy(),
            Some(Held::Confined(confined)) => confined.destroy(),
            None => {}
        }
    }

    /// Draw the pointer ourselves while it is over us, as a toolkit does: a
    /// square of `side` in one colour, ARGB, its top-left corner the pointer.
    pub fn set_cursor(&mut self, qh: &QueueHandle<Self>, side: u32, colour: u32) {
        let serial = self
            .entered
            .expect("the pointer is over one of our surfaces");
        let surface = self.compositor.create_surface(qh);
        self.paint(&surface, (side, side), colour);
        surface.commit();
        self.pointer
            .as_ref()
            .expect("bind_pointer first")
            .set_cursor(serial, Some(&surface), 0, 0);
    }

    /// Bind screencopy. Panics if it is not advertised.
    pub fn bind_screencopy(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.screencopy = Some(
            globals
                .bind::<ZwlrScreencopyManagerV1, _, _>(qh, 1..=3, ())
                .expect("zwlr_screencopy_manager_v1"),
        );
    }

    /// Ask for a frame of the first monitor, or a region of it, with the
    /// pointer drawn in or not, as `grim -c` asks or plain `grim` does;
    /// returns which grab it is.
    pub fn grab(
        &mut self,
        qh: &QueueHandle<Self>,
        region: Option<(i32, i32, i32, i32)>,
        pointer: bool,
    ) -> usize {
        let manager = self.screencopy.as_ref().expect("bind_screencopy first");
        let output = self.outputs.outputs().next().expect("a monitor");
        let overlay = i32::from(pointer);
        let frame = match region {
            None => manager.capture_output(overlay, &output, qh, ()),
            Some((x, y, w, h)) => {
                manager.capture_output_region(overlay, &output, x, y, w, h, qh, ())
            }
        };
        self.grabs.push(Grab {
            frame,
            offered: Vec::new(),
            buffer_done: false,
            ready: false,
            failed: false,
            damaged: false,
            buffer: None,
        });
        self.grabs.len() - 1
    }

    /// Bring a buffer of the first format and size offered -- or one
    /// `narrower` pixels narrower, which is the wrong one -- and copy into
    /// it, waiting for damage or not.
    pub fn copy(&mut self, grab: usize, narrower: i32, damage: bool) {
        let (format, width, height, _) = self.grabs[grab].offered[0];
        let width = i32::try_from(width).unwrap() - narrower;
        let (buffer, _) = self
            .pool
            .create_buffer(
                width,
                i32::try_from(height).unwrap(),
                width * 4,
                wl_shm::Format::try_from(format).unwrap(),
            )
            .expect("a buffer");
        let grabbing = &mut self.grabs[grab];
        if damage {
            grabbing.frame.copy_with_damage(buffer.wl_buffer());
        } else {
            grabbing.frame.copy(buffer.wl_buffer());
        }
        grabbing.buffer = Some(buffer);
    }

    /// A copied pixel, as RGBA, from the grab's buffer.
    pub fn grabbed(&mut self, grab: usize, x: usize, y: usize) -> [u8; 4] {
        let (_, width, _, _) = self.grabs[grab].offered[0];
        let buffer = self.grabs[grab].buffer.as_ref().expect("copied");
        let canvas = self.pool.canvas(buffer).expect("the buffer's memory");
        let at = (y * width as usize + x) * 4;
        [canvas[at + 2], canvas[at + 1], canvas[at], canvas[at + 3]]
    }

    /// Bind output management. Panics if it is not advertised.
    pub fn bind_displays(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.displays = Some(
            globals
                .bind::<ZwlrOutputManagerV1, _, _>(qh, 1..=4, ())
                .expect("zwlr_output_manager_v1"),
        );
    }

    /// The monitor of this name, as last told.
    pub fn head_named(&self, name: &str) -> &Shown {
        self.shown
            .iter()
            .find(|shown| shown.name == name && !shown.finished)
            .expect("a head of that name")
    }

    /// The size of the mode a head is showing.
    pub fn current_size(&self, name: &str) -> Option<(i32, i32)> {
        let current = self.head_named(name).current.clone()?;
        self.shown_modes
            .iter()
            .find(|mode| mode.mode.id() == current)
            .map(|mode| mode.size)
    }

    /// A configuration against `serial`, or the last one announced.
    pub fn configuration(
        &mut self,
        qh: &QueueHandle<Self>,
        serial: Option<u32>,
    ) -> ZwlrOutputConfigurationV1 {
        self.configured = None;
        let serial = serial.or(self.display_serial).expect("a serial");
        self.displays
            .as_ref()
            .expect("bind_displays first")
            .create_configuration(serial, qh, ())
    }

    /// Enable a head in a configuration, to set what it should be.
    pub fn enable(
        &self,
        configuration: &ZwlrOutputConfigurationV1,
        name: &str,
        qh: &QueueHandle<Self>,
    ) -> ZwlrOutputConfigurationHeadV1 {
        configuration.enable_head(&self.head_named(name).head, qh, ())
    }

    /// Bind the shell channel, as perspicax-shell does. Panics if it is not
    /// advertised.
    pub fn bind_shell(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.bind_shell_up_to(globals, qh, 3);
    }

    /// Bind the channel as a shell from before pies were in it would.
    pub fn bind_shell_v2(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.bind_shell_up_to(globals, qh, 2);
    }

    /// Bind the channel as a shell from before the keyboard's layouts were
    /// in it would.
    pub fn bind_shell_v1(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.bind_shell_up_to(globals, qh, 1);
    }

    fn bind_shell_up_to(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>, version: u32) {
        self.shell = Some(
            globals
                .bind::<PerspicaxShellV1, _, _>(qh, 1..=version, ())
                .expect("perspicax_shell_v1"),
        );
    }

    /// The name of a monitor this client was told of.
    fn output_name(&self, output: &wl_output::WlOutput) -> Option<String> {
        self.outputs.info(output).and_then(|info| info.name)
    }

    /// Lock the session, as swaylock does.
    pub fn lock(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        let manager = globals
            .bind::<ExtSessionLockManagerV1, _, _>(qh, 1..=1, ())
            .expect("ext_session_lock_manager_v1");
        self.lock = Some(manager.lock(qh, ()));
    }

    pub fn unlock(&mut self) {
        if let Some(lock) = self.lock.take() {
            lock.unlock_and_destroy();
        }
        self.locked = false;
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } => {
                desk.listed.insert(toplevel.id(), Listed::default());
            }
            ext_foreign_toplevel_list_v1::Event::Finished => desk.list_finished = true,
            _ => {}
        }
    }

    event_created_child!(Desk, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let listed = desk.listed.entry(handle.id()).or_default();
        match event {
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => {
                listed.identifier = identifier;
            }
            ext_foreign_toplevel_handle_v1::Event::Title { title } => listed.title = title,
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => listed.app_id = app_id,
            ext_foreign_toplevel_handle_v1::Event::Done => listed.done += 1,
            ext_foreign_toplevel_handle_v1::Event::Closed => listed.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                desk.tasked.push(Tasked {
                    handle: toplevel,
                    title: String::new(),
                    app_id: String::new(),
                    states: Vec::new(),
                    outputs: 0,
                    parent: None,
                    closed: false,
                    done: 0,
                });
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => desk.taskbar_finished = true,
            _ => {}
        }
    }

    event_created_child!(Desk, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        let Some(tasked) = desk
            .tasked
            .iter_mut()
            .find(|tasked| tasked.handle == *handle)
        else {
            return;
        };
        match event {
            Event::Title { title } => tasked.title = title,
            Event::AppId { app_id } => tasked.app_id = app_id,
            Event::OutputEnter { .. } => tasked.outputs += 1,
            Event::OutputLeave { .. } => tasked.outputs -= 1,
            Event::State { state } => {
                tasked.states = state
                    .chunks_exact(4)
                    .filter_map(|bytes| {
                        let value = u32::from_le_bytes(bytes.try_into().ok()?);
                        zwlr_foreign_toplevel_handle_v1::State::try_from(value).ok()
                    })
                    .collect();
            }
            Event::Parent { parent } => tasked.parent = parent.map(|parent| parent.id()),
            Event::Done => tasked.done += 1,
            Event::Closed => tasked.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<PerspicaxShellV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        _: &PerspicaxShellV1,
        event: perspicax_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use perspicax_shell_v1::Event;
        match event {
            Event::StartMenu { output } => {
                let name = output.and_then(|output| desk.output_name(&output));
                desk.told.push(Told::StartMenu(name));
            }
            Event::RootMenu { output, x, y } => {
                let name = output.and_then(|output| desk.output_name(&output));
                desk.told.push(Told::RootMenu(name, x, y));
            }
            Event::PieMenu { output, x, y, name } => {
                let output = output.and_then(|output| desk.output_name(&output));
                desk.told.push(Told::PieMenu(output, x, y, name));
            }
            Event::Reconfigure => desk.told.push(Told::Reconfigure),
            Event::Finished => desk.shell_finished = true,
            Event::Layout { name, short, .. } => {
                desk.layout_events += 1;
                desk.listing.push((name, short));
            }
            Event::LayoutsDone => {
                desk.layout_events += 1;
                desk.layouts = std::mem::take(&mut desk.listing);
            }
            Event::ActiveLayout { index } => {
                desk.layout_events += 1;
                desk.active_layout = Some(index);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        _: &ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_workspace_manager_v1::Event;
        match event {
            Event::WorkspaceGroup { workspace_group } => desk.groups.push(PagedGroup {
                handle: workspace_group,
                outputs: 0,
                workspaces: Vec::new(),
                removed: false,
            }),
            Event::Workspace { workspace } => desk.paged.push(Paged {
                handle: workspace,
                id: String::new(),
                name: String::new(),
                coordinates: Vec::new(),
                active: false,
                removed: false,
            }),
            Event::Done => desk.pager_done += 1,
            Event::Finished => desk.pager_finished = true,
            _ => {}
        }
    }

    event_created_child!(Desk, ExtWorkspaceManagerV1, [
        ext_workspace_manager_v1::EVT_WORKSPACE_GROUP_OPCODE => (ExtWorkspaceGroupHandleV1, ()),
        ext_workspace_manager_v1::EVT_WORKSPACE_OPCODE => (ExtWorkspaceHandleV1, ())
    ]);
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        handle: &ExtWorkspaceGroupHandleV1,
        event: ext_workspace_group_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_workspace_group_handle_v1::Event;
        let Some(group) = desk.groups.iter_mut().find(|group| group.handle == *handle) else {
            return;
        };
        match event {
            Event::OutputEnter { .. } => group.outputs += 1,
            Event::OutputLeave { .. } => group.outputs -= 1,
            Event::WorkspaceEnter { workspace } => group.workspaces.push(workspace.id()),
            Event::WorkspaceLeave { workspace } => {
                group.workspaces.retain(|id| *id != workspace.id());
            }
            Event::Removed => group.removed = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        handle: &ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_workspace_handle_v1::Event;
        let Some(paged) = desk.paged.iter_mut().find(|paged| paged.handle == *handle) else {
            return;
        };
        match event {
            Event::Id { id } => paged.id = id,
            Event::Name { name } => paged.name = name,
            Event::Coordinates { coordinates } => {
                paged.coordinates = coordinates
                    .chunks_exact(4)
                    .filter_map(|bytes| Some(u32::from_le_bytes(bytes.try_into().ok()?)))
                    .collect();
            }
            Event::State { state } => {
                paged.active = state
                    .into_result()
                    .is_ok_and(|state| state.contains(ext_workspace_handle_v1::State::Active));
            }
            Event::Removed => paged.removed = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Desk {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: <ZwlrScreencopyManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        let Some(grab) = desk.grabs.iter_mut().find(|grab| grab.frame == *frame) else {
            return;
        };
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => grab.offered.push((
                format.into_result().map_or(0, u32::from),
                width,
                height,
                stride,
            )),
            Event::BufferDone => grab.buffer_done = true,
            Event::Ready { .. } => grab.ready = true,
            Event::Failed => grab.failed = true,
            Event::Damage { .. } => grab.damaged = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => desk.shown.push(Shown {
                head,
                name: String::new(),
                enabled: false,
                modes: Vec::new(),
                current: None,
                position: (0, 0),
                scale: 1.0,
                finished: false,
            }),
            zwlr_output_manager_v1::Event::Done { serial } => desk.display_serial = Some(serial),
            _ => {}
        }
    }

    event_created_child!(Desk, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ())
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_head_v1::Event;
        if let Event::Mode { mode } = &event {
            desk.shown_modes.push(ShownMode {
                mode: mode.clone(),
                size: (0, 0),
                refresh: 0,
                finished: false,
            });
        }
        let Some(shown) = desk.shown.iter_mut().find(|shown| shown.head == *head) else {
            return;
        };
        match event {
            Event::Name { name } => shown.name = name,
            Event::Mode { mode } => shown.modes.push(mode.id()),
            Event::Enabled { enabled } => shown.enabled = enabled != 0,
            Event::CurrentMode { mode } => shown.current = Some(mode.id()),
            Event::Position { x, y } => shown.position = (x, y),
            Event::Scale { scale } => shown.scale = scale,
            Event::Finished => shown.finished = true,
            _ => {}
        }
    }

    event_created_child!(Desk, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ())
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        mode: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(shown) = desk
            .shown_modes
            .iter_mut()
            .find(|shown| shown.mode == *mode)
        else {
            return;
        };
        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => shown.size = (width, height),
            zwlr_output_mode_v1::Event::Refresh { refresh } => shown.refresh = refresh,
            zwlr_output_mode_v1::Event::Finished => shown.finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        _: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        desk.configured = Some(match event {
            zwlr_output_configuration_v1::Event::Succeeded => "succeeded",
            zwlr_output_configuration_v1::Event::Failed => "failed",
            _ => "cancelled",
        });
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for Desk {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputConfigurationHeadV1,
        _: <ZwlrOutputConfigurationHeadV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Desk {
    fn event(
        desk: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial, surface, ..
            } => {
                desk.entered = Some(serial);
                desk.pointed = Some(surface);
            }
            wl_pointer::Event::Leave { .. } => {
                desk.entered = None;
                desk.pointed = None;
            }
            wl_pointer::Event::Button { button, state, .. } => desk.buttons.push((
                button,
                state == WEnum::Value(wl_pointer::ButtonState::Pressed),
            )),
            wl_pointer::Event::Axis { .. } => desk.scrolls += 1,
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => desk.motions.push((surface_x, surface_y)),
            _ => {}
        }
    }
}

impl RelativePointerHandler for Desk {
    fn relative_pointer_motion(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpRelativePointerV1,
        _: &wl_pointer::WlPointer,
        event: RelativeMotionEvent,
    ) {
        self.relative_motions
            .push((event.delta, event.delta_unaccel));
    }
}

impl PointerConstraintsHandler for Desk {
    fn confined(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpConfinedPointerV1,
        _: &wl_surface::WlSurface,
        _: &wl_pointer::WlPointer,
    ) {
        self.holds.push("confined");
    }

    fn unconfined(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpConfinedPointerV1,
        _: &wl_surface::WlSurface,
        _: &wl_pointer::WlPointer,
    ) {
        self.holds.push("unconfined");
    }

    fn locked(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpLockedPointerV1,
        _: &wl_surface::WlSurface,
        _: &wl_pointer::WlPointer,
    ) {
        self.holds.push("locked");
    }

    fn unlocked(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpLockedPointerV1,
        _: &wl_surface::WlSurface,
        _: &wl_pointer::WlPointer,
    ) {
        self.holds.push("unlocked");
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Desk {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for Desk {
    fn event(
        _: &mut Self,
        _: &ExtSessionLockManagerV1,
        _: <ExtSessionLockManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockV1, ()> for Desk {
    fn event(
        desk: &mut Self,
        _: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_v1::Event::Locked = event {
            desk.locked = true;
        }
    }
}

impl LayerShellHandler for Desk {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(&(_, colour, height)) = self.layers.iter().find(|(known, _, _)| known == layer)
        else {
            return;
        };
        let width = configure.new_size.0.max(1);
        let (buffer, canvas) = self
            .pool
            .create_buffer(
                i32::try_from(width).unwrap(),
                i32::try_from(height).unwrap(),
                i32::try_from(width * 4).unwrap(),
                wl_shm::Format::Argb8888,
            )
            .expect("a buffer");
        for pixel in canvas.chunks_exact_mut(4) {
            pixel.copy_from_slice(&colour.to_le_bytes());
        }
        let surface = layer.wl_surface();
        buffer.attach_to(surface).expect("attach");
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        layer.commit();
        self.layers_drawn += 1;
    }
}

impl PopupHandler for Desk {
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Popup, _: PopupConfigure) {
        self.popups_configured += 1;
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Popup) {}
}

impl WindowHandler for Desk {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.asked_to_close += 1;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        if self.blank.contains(window) {
            window.commit();
            self.blank_configured += 1;
            return;
        }
        if let (Some(width), Some(height)) = configure.new_size {
            self.offered = Some((width.get(), height.get()));
        }
        let (width, height) = WINDOW;
        let (buffer, canvas) = self
            .pool
            .create_buffer(
                i32::try_from(width).unwrap(),
                i32::try_from(height).unwrap(),
                i32::try_from(width * 4).unwrap(),
                wl_shm::Format::Argb8888,
            )
            .expect("a buffer");
        let colour = self
            .windows
            .iter()
            .position(|known| known == window)
            .and_then(|at| self.colours.get(at).copied())
            .unwrap_or(0xffff_ffff);
        for pixel in canvas.chunks_exact_mut(4) {
            pixel.copy_from_slice(&colour.to_le_bytes());
        }
        let surface = window.wl_surface();
        buffer.attach_to(surface).expect("attach");
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        let declared = self
            .declaring
            .iter()
            .find(|(known, _)| known == window)
            .map_or(
                Some((
                    0,
                    0,
                    i32::try_from(width).unwrap(),
                    i32::try_from(height).unwrap(),
                )),
                |(_, geometry)| *geometry,
            );
        if let Some((x, y, width, height)) = declared {
            window
                .xdg_surface()
                .set_window_geometry(x, y, width, height);
        }
        window.commit();
        self.drawn += 1;
    }
}

impl CompositorHandler for Desk {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Desk {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for Desk {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Desk {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Desk);
delegate_output!(Desk);
delegate_shm!(Desk);
delegate_subcompositor!(Desk);
delegate_xdg_shell!(Desk);
delegate_xdg_window!(Desk);
delegate_xdg_popup!(Desk);
delegate_layer!(Desk);
delegate_registry!(Desk);
delegate_relative_pointer!(Desk);
delegate_pointer_constraints!(Desk);
