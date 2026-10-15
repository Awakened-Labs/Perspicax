//! What the live tests of the protocols that watch other windows share: a
//! compositor on a thread, a client of it, and the waiting.
//!
//! The client is one type for every such test, speaking whichever of the
//! protocols a test binds. Each test file compiles this module on its own and
//! uses part of it, hence the `dead_code` allowance.

#![allow(dead_code, reason = "each test file uses part of this module")]

use std::{
    collections::HashMap,
    fs::File,
    os::unix::{fs::FileExt as _, net::UnixStream},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Command, Config, Facts, Requests, Stop};
use perspicax_index::{HostFacts, Shot, SurfaceFacts};
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::Action;
use perspicax_protocols::shell::v1::client::perspicax_shell_v1::{self, PerspicaxShellV1};
use smithay::input::keyboard::{Keycode, xkb};
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
    shm::{Shm, ShmHandler, raw::RawPool, slot::SlotPool},
    subcompositor::SubcompositorState,
};
use wayland_client::{
    Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum,
    backend::{ObjectId, WaylandError},
    event_created_child,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_subsurface, wl_surface},
};
use wayland_protocols::ext::{
    foreign_toplevel_list::v1::client::{
        ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
        ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
    },
    session_lock::v1::client::{
        ext_session_lock_manager_v1::ExtSessionLockManagerV1,
        ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
        ext_session_lock_v1::{self, ExtSessionLockV1},
    },
    workspace::v1::client::{
        ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
        ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
        ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
    },
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    pointer_constraints::zv1::client::{
        zwp_confined_pointer_v1::ZwpConfinedPointerV1, zwp_locked_pointer_v1::ZwpLockedPointerV1,
        zwp_pointer_constraints_v1::Lifetime,
    },
    relative_pointer::zv1::client::zwp_relative_pointer_v1::ZwpRelativePointerV1,
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
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
        Self::start_with(name, backend, Vec::new(), false)
    }

    /// The same, with Xwayland: its display is in the facts once it is
    /// ready.
    pub fn start_with_xwayland(name: &str, backend: Backend) -> Self {
        Self::start_with(name, backend, Vec::new(), true)
    }

    /// The same, starting `spawn` as `--spawn` would: each a program and
    /// its arguments.
    pub fn start_spawning(name: &str, backend: Backend, spawn: Vec<Vec<String>>) -> Self {
        Self::start_with(name, backend, spawn, false)
    }

    fn start_with(name: &str, backend: Backend, spawn: Vec<Vec<String>>, xwayland: bool) -> Self {
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
                    spawn,
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
        client_on(&self.socket)
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

/// A client of the compositor on `socket`, its registry read: for a client
/// in a process of its own, which has a socket's name and no [`Session`].
pub fn client_on(socket: &str) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList) {
    let client = connect(socket);
    let (globals, queue) = registry_queue_init(&client).expect("the registry");
    let qh = queue.handle();
    (Desk::new(&globals, &qh), queue, qh, globals)
}

/// Connect by path rather than through `WAYLAND_DISPLAY`: setting an
/// environment variable in a test process races every other test in it.
pub fn connect(socket: &str) -> Connection {
    Connection::from_socket(stream(socket)).expect("a Wayland connection")
}

/// The socket a [`connect`] speaks over, for a test that wants to hold on to
/// it too.
pub fn stream(socket: &str) -> UnixStream {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(dir.join(socket)) {
            Ok(stream) => return stream,
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

/// The rectangles of the newest frame of damage, in the surface's own
/// coordinates.
pub fn newest(surface: &SurfaceFacts) -> Vec<Rect> {
    surface
        .damage
        .iter()
        .filter(|(generation, _)| *generation == surface.damage_generation)
        .map(|(_, region)| *region)
        .collect()
}

/// Once the compositor has taken in everything sent so far, wait until
/// `id`'s newest frame of damage is exactly `regions`.
pub fn landed(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    id: SurfaceId,
    regions: &[Rect],
) -> SurfaceFacts {
    queue.roundtrip(desk).expect("round trip");
    let facts = session.wait_for(|facts| facts.surface(id).is_some_and(|s| newest(s) == regions));
    facts.surface(id).expect("waited for").clone()
}

/// Wait until this client's popups have had `count` configures between
/// them, so the newest may draw.
pub fn configured(desk: &mut Desk, queue: &mut EventQueue<Desk>, count: usize) {
    until(queue, desk, |desk| desk.popups_configured == count);
}

/// Where [`menu_with_the_keyboard`] opens its menu, in its window: the
/// window's damage once the menu has drawn.
pub const MENU_AREA: Rect = Rect {
    x0: 40.0,
    y0: 30.0,
    x1: 160.0,
    y1: 110.0,
};

/// Once this client's first window, `page` to the facts, holds the keyboard
/// `ear` hears, open a menu of it that takes the keyboard, as a right-click
/// does, and draw it: the client's first popup, with a person at the seat,
/// where alone a menu takes the keyboard. Returns once the keyboard is in
/// the menu -- the grab took -- and what the menu drew has been published:
/// by then the window and the taskbars have been told whatever the menu
/// taking the keyboard changed, and the facts say it too.
pub fn menu_with_the_keyboard(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    ear: &Ear,
    page: SurfaceId,
) -> Popup {
    const MENU: u32 = 0xff33_6699;
    let window = desk.windows[0].wl_surface().id();
    until(queue, desk, |_| {
        heard(ear).entered.as_ref() == Some(&window)
    });
    let serial = heard(ear).enter_serial.expect("entered");
    let menu = desk.open_menu(
        qh,
        desk.windows[0].xdg_surface(),
        (40, 30),
        (120, 80),
        serial,
    );
    // In the menu: the grab took, and the window's configure, if any, was
    // sent with it.
    let grabbed = menu.wl_surface().id();
    until(queue, desk, |_| {
        heard(ear).entered.as_ref() == Some(&grabbed)
    });
    configured(desk, queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    landed(session, desk, queue, page, &[MENU_AREA]);
    menu
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

/// A colour as written, ARGB, as the pixel a screencopy reads back.
pub fn rgba(argb: u32) -> [u8; 4] {
    let [b, g, r, a] = argb.to_le_bytes();
    [r, g, b, a]
}

/// Where `shot` shows `colour`: the smallest rectangle around all of it, in
/// the picture's logical coordinates, or nothing. Read pixel by pixel in
/// the picture's own, so at any scale.
pub fn drawn_in(shot: &Shot, colour: u32) -> Vec<Rect> {
    let colour = rgba(colour);
    let width = usize::try_from(shot.width).expect("a width");
    let mut around: Option<Rect> = None;
    for (at, pixel) in shot.rgba.chunks_exact(4).enumerate() {
        if pixel == colour {
            let (x, y) = ((at % width) as f64, (at / width) as f64);
            let here = Rect::new(x, y, x + 1.0, y + 1.0);
            around = Some(around.map_or(here, |around| around.union(here)));
        }
    }
    around
        .map(|around| {
            Rect::new(
                around.x0 / shot.scale,
                around.y0 / shot.scale,
                around.x1 / shot.scale,
                around.y1 / shot.scale,
            )
        })
        .into_iter()
        .collect()
}

/// Give the monitor `name` the scale `scale` as a display tool does,
/// leaving every other monitor on as it is.
///
/// The protocol carries a scale in 256ths, so 0.9 arrives as 230/256.
pub fn rescale(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    globals: &GlobalList,
    name: &str,
    scale: f64,
) {
    if desk.displays.is_none() {
        desk.bind_displays(globals, qh);
    }
    until(queue, desk, |desk| desk.display_serial.is_some());
    let configuration = desk.configuration(qh, None);
    let on: Vec<String> = desk
        .shown
        .iter()
        .filter(|shown| shown.enabled && !shown.finished)
        .map(|shown| shown.name.clone())
        .collect();
    for head in &on {
        let enabled = desk.enable(&configuration, head, qh);
        if head == name {
            enabled.set_scale(scale);
        }
    }
    configuration.apply();
    until(queue, desk, |desk| desk.configured.is_some());
    assert_eq!(desk.configured, Some("succeeded"));
    until(queue, desk, |desk| {
        (desk.head_named(name).scale - scale).abs() < 1.0 / 256.0
    });
}

/// Copy each screencopy grab, at once, once each has been offered its
/// buffer.
pub fn copy_all(desk: &mut Desk, queue: &mut EventQueue<Desk>, grabs: &[usize]) {
    until(queue, desk, |desk| {
        grabs.iter().all(|&grab| desk.grabs[grab].buffer_done)
    });
    for &grab in grabs {
        desk.copy(grab, 0, false);
    }
    until(queue, desk, |desk| {
        grabs.iter().all(|&grab| desk.grabs[grab].ready)
    });
}

/// The pixels a grab copied in a square around `at`, out to `reach` on
/// every side.
pub fn around(desk: &mut Desk, grab: usize, at: (usize, usize), reach: usize) -> Vec<[u8; 4]> {
    let (x, y) = at;
    let mut pixels = Vec::new();
    for row in y - reach..=y + reach {
        for column in x - reach..=x + reach {
            pixels.push(desk.grabbed(grab, column, row));
        }
    }
    pixels
}

/// The protocol error the compositor ended this client's connection with:
/// the interface of the object it named, and the error's code. Fails if
/// the connection is still up, or ended any other way -- the compositor
/// gone, as one that panicked is.
pub fn refused<D>(queue: &mut EventQueue<D>, state: &mut D) -> (String, u32) {
    match queue.roundtrip(state) {
        Err(DispatchError::Backend(WaylandError::Protocol(error))) => {
            (error.object_interface, error.code)
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
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

/// A monitor bound again by `Desk::bind_outputs_again`, beside the one
/// `OutputState` holds.
pub struct Spare;

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

/// What the compositor said to one of the client's surfaces, in the order
/// it said it: which monitor it is on, when it may draw again, and, to a
/// window, how to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// It is on the monitor of this name now (`wl_surface.enter`).
    Enter(String),
    /// It is no longer on the monitor of this name (`wl_surface.leave`).
    Leave(String),
    /// It may draw again: a frame callback it asked for came back.
    Done,
    /// It should draw at this scale, in 120ths
    /// (`wp_fractional_scale_v1.preferred_scale`).
    PreferredScale(u32),
    /// A window's configure.
    Configure(Configured),
}

/// A window drawn at a size of its own and shown at another: see
/// [`Desk::open_viewed`].
pub struct Viewed {
    pub window: Window,
    pub viewport: WpViewport,
    /// Its buffer's size, in its pixels.
    pub pixels: (u32, u32),
    /// The size it is shown at, which is its window's.
    pub shown: (i32, i32),
}

/// One configure a window was sent, as the window read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Configured {
    pub serial: u32,
    /// The size it was offered, where it was offered one.
    pub size: Option<(u32, u32)>,
    pub activated: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    /// Tiled against any edge: snapped to a half or a quarter.
    pub tiled: bool,
}

impl Configured {
    fn read(serial: u32, configure: &WindowConfigure) -> Self {
        Self {
            serial,
            size: match configure.new_size {
                (Some(width), Some(height)) => Some((width.get(), height.get())),
                _ => None,
            },
            activated: configure.is_activated(),
            maximized: configure.is_maximized(),
            fullscreen: configure.is_fullscreen(),
            tiled: configure.is_tiled_left()
                || configure.is_tiled_right()
                || configure.is_tiled_top()
                || configure.is_tiled_bottom(),
        }
    }
}

/// A window that draws as GTK's do: every picture asks for a frame
/// callback, and no other is drawn until it comes back. A configure that
/// arrives while it waits is drawn when the callback comes.
struct Patience {
    window: Window,
    waiting: bool,
    /// A configure came while it waited, and is owed a picture.
    owed: bool,
}

/// What the client's keyboard was told, kept as it arrived. xkb's own types
/// stay on the test's side of the lock: they are not `Send`.
#[derive(Default)]
pub struct Heard {
    /// The last keymap, as text.
    pub keymap: Option<String>,
    /// How many layouts it has.
    pub layouts: u32,
    /// The modifiers and group as last sent: depressed, latched, locked,
    /// group.
    pub modifiers: [u32; 4],
    /// Each key pressed since the last keymap, with the modifiers in force.
    pub keys: Vec<(u32, [u32; 4])>,
    /// The surface the keyboard last entered, until it left.
    pub entered: Option<ObjectId>,
    /// The serial of the keyboard's last arrival: one a menu's grab can name.
    pub enter_serial: Option<u32>,
}

/// A keyboard's [`Heard`], shared with the dispatch that fills it.
pub type Ear = Arc<Mutex<Heard>>;

pub fn heard(ear: &Ear) -> MutexGuard<'_, Heard> {
    ear.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Heard {
    pub fn group(&self) -> u32 {
        self.modifiers[3]
    }

    /// The text the keys spell, read with the keymap and modifiers the
    /// client was sent.
    pub fn typed(&self) -> String {
        let Some(text) = self.keymap.clone() else {
            return String::new();
        };
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::COMPILE_NO_FLAGS,
        )
        .expect("the keymap the client was sent compiles");
        let mut state = xkb::State::new(&keymap);
        self.keys
            .iter()
            .map(|&(key, [depressed, latched, locked, group])| {
                state.update_mask(depressed, latched, locked, 0, 0, group);
                // Wayland's keycodes are evdev's; xkb's are 8 more.
                state.key_get_utf8(Keycode::new(key + 8))
            })
            .collect()
    }
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
    pub viewporter: Option<WpViewporter>,
    pub fractional_scales: Option<WpFractionalScaleManagerV1>,
    /// Windows drawn at a size of their own and shown at another through a
    /// viewport, as a client drawing at a fractional scale draws.
    pub viewed: Vec<Viewed>,
    /// Each window's colour, ARGB, in the order they opened.
    pub colours: Vec<u32>,
    pub drawn: usize,
    /// Each configure a window was sent, in order: which window, by its place
    /// in `windows`, and whether it was told it is the active one.
    pub activations: Vec<(usize, bool)>,
    /// Whether the last configure a window was sent said it is being
    /// resized.
    pub resizing: bool,
    /// The size the compositor last offered a window, where it named one.
    /// Windows are drawn at their own size whatever it offers.
    pub offered: Option<(u32, u32)>,
    /// Everything the compositor said to one of our surfaces, with which,
    /// in order. See [`Desk::said_to`].
    pub said: Vec<(ObjectId, Said)>,
    /// The windows opened with [`Desk::open_patient`].
    patient: Vec<Patience>,
    pub list: Option<ExtForeignToplevelListV1>,
    pub list_finished: bool,
    pub listed: HashMap<ObjectId, Listed>,
    pub lock: Option<ExtSessionLockV1>,
    pub locked: bool,
    /// The lock surfaces covering the monitors, as swaylock's do, each with
    /// its surface.
    pub covers: Vec<(ExtSessionLockSurfaceV1, wl_surface::WlSurface)>,
    /// How many times one of them has been configured, and drawn.
    pub covers_drawn: usize,
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
    /// Where on it the pointer last arrived, in its own coordinates.
    pub arrived: Option<(f64, f64)>,
    /// Every button the pointer pressed (`true`) or let go (`false`) on one
    /// of our surfaces, by Linux code, in order.
    pub buttons: Vec<(u32, bool)>,
    /// The serial of the last press on one of our surfaces: what a move or
    /// a resize has to name to be honoured.
    pub pressed: Option<u32>,
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
    /// Strips across the top of the screen, each with its colour and height,
    /// and wallpapers, whose height is 0 for the whole of what they are
    /// given.
    pub layers: Vec<(LayerSurface, u32, u32)>,
    pub layers_drawn: usize,
    /// How many configures the popups this client opened have had. Nothing
    /// is drawn in reply: a test paints a popup itself, once it may.
    pub popups_configured: usize,
    /// The serial of the last of them: one of the compositor's own, newer
    /// than any the client heard before it.
    pub popup_serial: Option<u32>,
    /// How many of this client's popups the compositor dismissed.
    pub popups_done: usize,
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
            viewporter: None,
            fractional_scales: None,
            viewed: Vec::new(),
            colours: Vec::new(),
            resizing: false,
            offered: None,
            said: Vec::new(),
            patient: Vec::new(),
            drawn: 0,
            activations: Vec::new(),
            list: None,
            list_finished: false,
            listed: HashMap::new(),
            lock: None,
            locked: false,
            covers: Vec::new(),
            covers_drawn: 0,
            taskbar: None,
            taskbar_finished: false,
            tasked: Vec::new(),
            seat: None,
            pointer: None,
            entered: None,
            pointed: None,
            buttons: Vec::new(),
            pressed: None,
            arrived: None,
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
            popup_serial: None,
            popups_done: 0,
        }
    }

    /// Whether the `nth` window has drawn: it draws its first configure as it
    /// comes. Not a count of everything drawn, which with a person at the
    /// seat includes the window in use redrawn as a new one takes over.
    pub fn has_drawn(&self, nth: usize) -> bool {
        self.activations.iter().any(|(window, _)| *window == nth)
    }

    /// Whether each configure the `nth` window was sent told it it is the
    /// active one, in order.
    pub fn activations_of(&self, nth: usize) -> Vec<bool> {
        self.activations
            .iter()
            .filter(|(window, _)| *window == nth)
            .map(|(_, active)| *active)
            .collect()
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

    /// A wallpaper: on `background`, over the whole of its output, drawn in
    /// one colour, ARGB, and moved by no panel, as a desktop's wallpaper is.
    pub fn open_wallpaper(&mut self, qh: &QueueHandle<Self>, colour: u32) {
        let surface = self.compositor.create_surface(qh);
        let wallpaper = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some("wallpaper"),
            None,
        );
        wallpaper.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        wallpaper.set_size(0, 0);
        wallpaper.set_exclusive_zone(-1);
        wallpaper.commit();
        self.layers.push((wallpaper, colour, 0));
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

    /// Open a window, drawn white, that asks for its frame to be drawn for
    /// it, as Qt and foot do.
    pub fn open_framed(&mut self, qh: &QueueHandle<Self>, title: &str) {
        self.colours.push(0xffff_ffff);
        let surface = self.compositor.create_surface(qh);
        let window = self
            .xdg
            .create_window(surface, WindowDecorations::RequestServer, qh);
        window.set_title(title);
        window.set_app_id(title);
        window.commit();
        self.windows.push(window);
    }

    /// Open a window that draws as GTK's do: it asks for a frame callback
    /// with every picture, and draws nothing more, a configure included,
    /// until that callback comes back. Left waiting, it stays as it was, which
    /// is how a window looks frozen.
    pub fn open_patient(&mut self, qh: &QueueHandle<Self>, title: &str, app_id: &str) {
        self.open_window(qh, title, app_id);
        let window = self.windows.last().expect("just opened").clone();
        self.patient.push(Patience {
            window,
            waiting: false,
            owed: false,
        });
    }

    /// Ask for a frame callback on `surface` and commit, drawing nothing
    /// new: a client that has drawn and now waits to be told it may draw
    /// again.
    pub fn ask_frame(&mut self, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface) {
        surface.frame(qh, surface.clone());
        surface.commit();
        if let Some(patience) = self.patience(surface) {
            patience.waiting = true;
        }
    }

    /// What the compositor said to `surface`, in order.
    pub fn said_to(&self, surface: &wl_surface::WlSurface) -> Vec<Said> {
        self.said
            .iter()
            .filter(|(to, _)| *to == surface.id())
            .map(|(_, said)| said.clone())
            .collect()
    }

    fn patience(&mut self, surface: &wl_surface::WlSurface) -> Option<&mut Patience> {
        self.patient
            .iter_mut()
            .find(|patience| patience.window.wl_surface() == surface)
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

    /// Bind the viewporter. Panics if it is not advertised.
    pub fn bind_viewporter(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.viewporter = Some(
            globals
                .bind::<WpViewporter, _, _>(qh, 1..=1, ())
                .expect("wp_viewporter"),
        );
    }

    /// `surface`'s viewport, through which it says what part of its buffer
    /// is shown and at what size.
    pub fn viewport(&self, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface) -> WpViewport {
        self.viewporter
            .as_ref()
            .expect("bind_viewporter first")
            .get_viewport(surface, qh, ())
    }

    /// Bind the fractional-scale manager. Panics if it is not advertised.
    pub fn bind_fractional_scale(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) {
        self.fractional_scales = Some(
            globals
                .bind::<WpFractionalScaleManagerV1, _, _>(qh, 1..=1, ())
                .expect("wp_fractional_scale_manager_v1"),
        );
    }

    /// Ask what scale `surface` should draw at. Each answer is said to it:
    /// [`Said::PreferredScale`], and [`Desk::scales_told`].
    pub fn fractional(
        &self,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
    ) -> WpFractionalScaleV1 {
        self.fractional_scales
            .as_ref()
            .expect("bind_fractional_scale first")
            .get_fractional_scale(surface, qh, surface.id())
    }

    /// The scales `surface` was told to draw at, in 120ths, in order.
    pub fn scales_told(&self, surface: &wl_surface::WlSurface) -> Vec<u32> {
        self.said_to(surface)
            .into_iter()
            .filter_map(|said| match said {
                Said::PreferredScale(scale) => Some(scale),
                _ => None,
            })
            .collect()
    }

    /// A surface with no role yet: what GTK asks its scale for, before the
    /// surface is a window.
    pub fn surface(&self, qh: &QueueHandle<Self>) -> wl_surface::WlSurface {
        self.compositor.create_surface(qh)
    }

    /// Open a window that draws a buffer `pixels` big, all in `colour`,
    /// ARGB, and says it is shown at `shown`, which is its window's size:
    /// what Firefox draws on a monitor at a fractional scale. It draws so
    /// at every configure, as a toolkit does.
    pub fn open_viewed(
        &mut self,
        qh: &QueueHandle<Self>,
        title: &str,
        colour: u32,
        pixels: (u32, u32),
        shown: (i32, i32),
    ) {
        self.open_coloured(qh, title, title, colour);
        let window = self.windows.last().expect("just opened").clone();
        let viewport = self.viewport(qh, window.wl_surface());
        self.viewed.push(Viewed {
            window,
            viewport,
            pixels,
            shown,
        });
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

    /// An empty region, for a test to add rectangles to and take them away
    /// from as it likes.
    pub fn region(&self) -> Region {
        Region::new(&self.compositor).expect("a region")
    }

    /// A pool of shared memory `len` bytes long, for a test to ask of as it
    /// likes.
    pub fn raw_pool(&self, len: usize) -> RawPool {
        RawPool::new(len, &self.shm).expect("a pool")
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
        self.open_popup_by(qh, parent, &self.positioner(at, size))
    }

    /// A popup of `parent`, placed by `positioner`: [`Self::open_popup`],
    /// with a positioner the test has shaped itself.
    pub fn open_popup_by(
        &self,
        qh: &QueueHandle<Self>,
        parent: &xdg_surface::XdgSurface,
        positioner: &XdgPositioner,
    ) -> Popup {
        Popup::new(parent, positioner, qh, &self.compositor, &self.xdg).expect("xdg_popup")
    }

    /// A menu of `parent` that takes the keyboard and pointer until it is
    /// dismissed, as a right-click opens one: placed as [`Self::open_popup`]
    /// places one, and grabbing on `seat` before its first commit, the only
    /// time xdg-shell allows. `serial` is the input event that opened it.
    pub fn open_menu(
        &self,
        qh: &QueueHandle<Self>,
        parent: &xdg_surface::XdgSurface,
        at: (i32, i32),
        size: (u32, u32),
        serial: u32,
    ) -> Popup {
        let positioner = self.positioner(at, size);
        let surface = self.compositor.create_surface(qh);
        let popup = Popup::from_surface(Some(parent), &positioner, qh, surface, &self.xdg)
            .expect("xdg_popup");
        let seat = self.seat.as_ref().expect("bind_keyboard first");
        popup.xdg_popup().grab(seat, serial);
        popup.wl_surface().commit();
        popup
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
    pub fn positioner(&self, at: (i32, i32), size: (u32, u32)) -> XdgPositioner {
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

    /// Attach a buffer `size` in its own pixels to `surface`, turned by
    /// `transform`, as a video player on a rotated monitor hands one over:
    /// all in `colour` but for a `corner` of its pixels at the top left in
    /// `marked`, and naming only that corner as damage, in the buffer's own
    /// coordinates. Not committed.
    pub fn paint_turned(
        &mut self,
        surface: &wl_surface::WlSurface,
        size: (u32, u32),
        transform: wl_output::Transform,
        colour: u32,
        corner: (u32, u32),
        marked: u32,
    ) {
        self.attach_marked(surface, size, colour, corner, marked);
        surface.set_buffer_transform(transform);
        surface.damage_buffer(
            0,
            0,
            i32::try_from(corner.0).unwrap(),
            i32::try_from(corner.1).unwrap(),
        );
    }

    /// Attach the picture [`Self::paint_turned`] attaches, unturned, naming
    /// only `named` as damage, `(x, y, width, height)` in the buffer's own
    /// coordinates: the same picture drawn again, with a change claimed
    /// where there is none to see. Not committed.
    pub fn paint_naming(
        &mut self,
        surface: &wl_surface::WlSurface,
        size: (u32, u32),
        colour: u32,
        corner: (u32, u32),
        marked: u32,
        named: Geometry,
    ) {
        self.attach_marked(surface, size, colour, corner, marked);
        surface.damage_buffer(named.0, named.1, named.2, named.3);
    }

    /// Attach a buffer `size` in its own pixels to `surface`, all in
    /// `colour` but for a `corner` of its pixels at the top left in
    /// `marked`, naming no damage.
    fn attach_marked(
        &mut self,
        surface: &wl_surface::WlSurface,
        size: (u32, u32),
        colour: u32,
        corner: (u32, u32),
        marked: u32,
    ) {
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
        for (at, pixel) in canvas.chunks_exact_mut(4).enumerate() {
            let at = u32::try_from(at).unwrap();
            let (x, y) = (at % width, at / width);
            let inside = x < corner.0 && y < corner.1;
            pixel.copy_from_slice(&if inside { marked } else { colour }.to_le_bytes());
        }
        buffer.attach_to(surface).expect("attach");
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

    /// Bind every monitor there was at the start once more, as a client that
    /// has just been told of a monitor does, and say how many. These are
    /// spares: the client's own monitors, the ones `OutputState` holds, are
    /// left as they are.
    pub fn bind_outputs_again(&self, globals: &GlobalList, qh: &QueueHandle<Self>) -> usize {
        let outputs: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|global| global.interface == "wl_output")
                .map(|global| (global.name, global.version))
                .collect()
        });
        for &(name, version) in &outputs {
            globals
                .registry()
                .bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, Spare);
        }
        outputs.len()
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

    /// A keyboard of our own, on the seat already bound or a new one, and
    /// what it hears.
    pub fn bind_keyboard(&mut self, globals: &GlobalList, qh: &QueueHandle<Self>) -> Ear {
        let seat = self.seat.get_or_insert_with(|| {
            globals
                .bind::<wl_seat::WlSeat, _, _>(qh, 1..=7, ())
                .expect("wl_seat")
        });
        let ear = Ear::default();
        seat.get_keyboard(qh, ear.clone());
        ear
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

    /// Cover the monitor called `monitor` with a lock surface drawn all in
    /// `colour`, ARGB, as swaylock covers each one: drawn at the size the
    /// compositor gives it, once it does. Returns its surface. Panics if the
    /// session was not locked by this client, or no monitor has that name.
    pub fn cover(
        &mut self,
        qh: &QueueHandle<Self>,
        monitor: &str,
        colour: u32,
    ) -> wl_surface::WlSurface {
        let output = self
            .outputs
            .outputs()
            .find(|output| self.output_name(output).as_deref() == Some(monitor))
            .unwrap_or_else(|| panic!("no monitor called {monitor}"));
        let surface = self.compositor.create_surface(qh);
        let cover = self
            .lock
            .as_ref()
            .expect("lock first")
            .get_lock_surface(&surface, &output, qh, colour);
        self.covers.push((cover, surface.clone()));
        surface
    }

    /// Hide the pointer while it is over us, as swaylock does the moment it
    /// arrives: a cursor of no surface at all.
    pub fn hide_cursor(&self) {
        let serial = self
            .entered
            .expect("the pointer is over one of our surfaces");
        self.pointer
            .as_ref()
            .expect("bind_pointer first")
            .set_cursor(serial, None, 0, 0);
    }

    /// Unlock, and take the lock surfaces down, as swaylock does once the
    /// password is right.
    pub fn unlock(&mut self) {
        if let Some(lock) = self.lock.take() {
            lock.unlock_and_destroy();
        }
        for (cover, surface) in self.covers.drain(..) {
            cover.destroy();
            surface.destroy();
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

impl Dispatch<WpViewporter, ()> for Desk {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for Desk {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for Desk {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Keyed by the surface it is of.
impl Dispatch<WpFractionalScaleV1, ObjectId> for Desk {
    fn event(
        desk: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            desk.said
                .push((surface.clone(), Said::PreferredScale(scale)));
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
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                desk.entered = Some(serial);
                desk.pointed = Some(surface);
                desk.arrived = Some((surface_x, surface_y));
            }
            wl_pointer::Event::Leave { .. } => {
                desk.entered = None;
                desk.pointed = None;
            }
            wl_pointer::Event::Button {
                serial,
                button,
                state,
                ..
            } => {
                let pressed = state == WEnum::Value(wl_pointer::ButtonState::Pressed);
                if pressed {
                    desk.pressed = Some(serial);
                }
                desk.buttons.push((button, pressed));
            }
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

impl Dispatch<wl_output::WlOutput, Spare> for Desk {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &Spare,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, Ear> for Desk {
    fn event(
        _: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        ear: &Ear,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut heard = heard(ear);
        match event {
            wl_keyboard::Event::Keymap { fd, size, .. } => {
                let mut bytes = vec![0; usize::try_from(size).expect("a size")];
                File::from(fd)
                    .read_exact_at(&mut bytes, 0)
                    .expect("the keymap reads");
                let text = String::from_utf8(bytes)
                    .expect("a keymap is text")
                    .trim_end_matches('\0')
                    .to_owned();
                let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                let keymap = xkb::Keymap::new_from_string(
                    &context,
                    text.clone(),
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::COMPILE_NO_FLAGS,
                )
                .expect("the keymap compiles");
                heard.layouts = keymap.num_layouts();
                heard.keymap = Some(text);
                heard.keys.clear();
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => heard.modifiers = [mods_depressed, mods_latched, mods_locked, group],
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                let modifiers = heard.modifiers;
                heard.keys.push((key, modifiers));
            }
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                heard.entered = Some(surface.id());
                heard.enter_serial = Some(serial);
            }
            wl_keyboard::Event::Leave { .. } => heard.entered = None,
            _ => {}
        }
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

/// A lock surface, its colour its user data: drawn at whatever size it is
/// configured to, as swaylock draws its own.
impl Dispatch<ExtSessionLockSurfaceV1, u32> for Desk {
    fn event(
        desk: &mut Self,
        cover: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        colour: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        else {
            return;
        };
        let Some(surface) = desk
            .covers
            .iter()
            .find(|(known, _)| known == cover)
            .map(|(_, surface)| surface.clone())
        else {
            return;
        };
        cover.ack_configure(serial);
        desk.paint(&surface, (width, height), *colour);
        surface.commit();
        desk.covers_drawn += 1;
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
        let height = if height == 0 {
            configure.new_size.1.max(1)
        } else {
            height
        };
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
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Popup,
        configure: PopupConfigure,
    ) {
        self.popups_configured += 1;
        self.popup_serial = Some(configure.serial);
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Popup) {
        self.popups_done += 1;
    }
}

impl WindowHandler for Desk {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.asked_to_close += 1;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        serial: u32,
    ) {
        if self.blank.contains(window) {
            window.commit();
            self.blank_configured += 1;
            return;
        }
        if let Some(nth) = self.windows.iter().position(|known| known == window) {
            self.activations.push((nth, configure.is_activated()));
        }
        self.resizing = configure.is_resizing();
        if let (Some(width), Some(height)) = configure.new_size {
            self.offered = Some((width.get(), height.get()));
        }
        let surface = window.wl_surface();
        self.said.push((
            surface.id(),
            Said::Configure(Configured::read(serial, &configure)),
        ));
        // Already acknowledged, by SCTK. A patient window draws it when it
        // may, as GTK does, and asks to be told when it may draw again.
        if let Some(patience) = self.patience(surface) {
            if patience.waiting {
                patience.owed = true;
                return;
            }
            patience.waiting = true;
            surface.frame(qh, surface.clone());
        }
        self.draw(window);
    }
}

impl Desk {
    /// Draw `window` in its colour at its own size, declare its geometry,
    /// and commit. A window viewed draws at the size of its buffer and is
    /// shown at its own.
    fn draw(&mut self, window: &Window) {
        let viewed = self.viewed.iter().find(|viewed| viewed.window == *window);
        let (width, height) = viewed.map_or(WINDOW, |viewed| viewed.pixels);
        let shown = viewed.map(|viewed| {
            viewed
                .viewport
                .set_destination(viewed.shown.0, viewed.shown.1);
            viewed.shown
        });
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
                Some(shown.map_or(
                    (
                        0,
                        0,
                        i32::try_from(width).unwrap(),
                        i32::try_from(height).unwrap(),
                    ),
                    |(width, height)| (0, 0, width, height),
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
    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.said.push((surface.id(), Said::Done));
        let Some(patience) = self.patience(surface) else {
            return;
        };
        patience.waiting = false;
        if std::mem::take(&mut patience.owed) {
            patience.waiting = true;
            let window = patience.window.clone();
            surface.frame(qh, surface.clone());
            self.draw(&window);
        }
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        let name = self.output_name(output).unwrap_or_default();
        self.said.push((surface.id(), Said::Enter(name)));
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        let name = self.output_name(output).unwrap_or_default();
        self.said.push((surface.id(), Said::Leave(name)));
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
