//! Tab groups, by a compositor with no screen.
//!
//! Two windows grouped with the keyboard's binding share one place: the tab in
//! front has it and the other is parked, and an agent asking about the one
//! behind is told which tab is in front of it. Cycling swaps them in place,
//! and detaching puts both on screen. A window that draws its own frame gets
//! a strip of tabs from the compositor while it is grouped, and loses it after.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Command, Config, Facts, Requests, Stop};
use perspicax_index::{HostFacts, SurfaceFacts, judge};
use perspicax_node::{Rect, Visibility};
use perspicax_policy::Action;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_registry, delegate_shm, delegate_xdg_shell,
    delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell, XdgSurface as _,
            window::{DecorationMode, Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, EventQueue, QueueHandle,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_output, wl_shm, wl_surface},
};

const WINDOW: (u32, u32) = (400, 300);

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn two_windows_grouped_share_one_place_and_the_one_behind_says_which_is_in_front() {
    let socket = format!("perspicax-tabs-{}", std::process::id());
    let facts = Facts::new();
    let requests = Requests::new();
    let stop = Stop::new();
    let compositor = {
        let (facts, requests, stop, socket) = (
            facts.clone(),
            requests.clone(),
            stop.clone(),
            socket.clone(),
        );
        thread::spawn(move || {
            let config = Config {
                backend: Backend::headless((1280, 1024)),
                spawn: Vec::new(),
                env: Vec::new(),
                run_for: Some(Duration::from_secs(30)),
                config: None,
                socket: Some(socket),
                xwayland: false,
            };
            perspicax_compositor::run(&config, &facts, &requests, &stop)
        })
    };
    let perform = |action| {
        requests
            .command(Command::Perform(action))
            .expect("the compositor is listening");
    };

    let client = connect(&socket);
    let (globals, mut queue) = registry_queue_init(&client).expect("the registry");
    let qh = queue.handle();
    let mut desk = Desk::new(&globals, &qh);

    // One window the compositor frames, then one that draws its own, which
    // is focused because it opened last.
    desk.open_window(&qh, WindowDecorations::RequestServer, "framed");
    until(&mut queue, &mut desk, |desk| {
        desk.drawn == 1 && desk.modes[0] == DecorationMode::Server
    });
    desk.open_window(&qh, WindowDecorations::None, "own frame");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let before = wait_for(&facts, |facts| mapped(facts) == 2);
    let place = window(&before, "framed").geometry;
    assert!(window(&before, "own frame").frame.is_empty());

    // Grouped: the focused window joins the one focused before it, in front,
    // in its place, with a strip of tabs; the other is behind it.
    perform(Action::TabWithPrevious);
    let grouped = wait_for(&facts, |facts| {
        !window(facts, "framed").mapped && window(facts, "own frame").geometry == place
    });
    let (framed, own) = (window(&grouped, "framed"), window(&grouped, "own frame"));
    assert_eq!(own.frame.len(), 1, "a strip of tabs: {:?}", own.frame);
    assert_eq!(own.behind_tab, None);
    assert_eq!(
        judge(&grouped, framed.id, Rect::new(10.0, 10.0, 50.0, 30.0)).visibility,
        Visibility::InactiveTab { shown: own.id },
        "the tab behind names the tab in front"
    );

    // Cycled: the two swap, in the same place.
    perform(Action::CycleTab { forward: true });
    let cycled = wait_for(&facts, |facts| {
        window(facts, "framed").mapped && !window(facts, "own frame").mapped
    });
    assert_eq!(window(&cycled, "framed").geometry, place);
    assert_eq!(
        window(&cycled, "own frame").behind_tab,
        Some(window(&cycled, "framed").id)
    );

    // Detached: both on screen, and the one that draws its own frame is
    // back to drawing all of it.
    perform(Action::DetachTab);
    let apart = wait_for(&facts, |facts| mapped(facts) == 2);
    assert!(window(&apart, "own frame").frame.is_empty());
    assert_eq!(
        window(&apart, "own frame").geometry,
        place,
        "the heir took the place"
    );
    assert_ne!(
        window(&apart, "framed").geometry,
        place,
        "and the one detached moved aside"
    );

    stop.request();
    drop((desk, queue));
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
}

fn mapped(facts: &HostFacts) -> usize {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.mapped)
        .count()
}

fn window<'a>(facts: &'a HostFacts, title: &str) -> &'a SurfaceFacts {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .unwrap_or_else(|| panic!("no window titled {title:?}: {facts:?}"))
}

/// Connect by path rather than through `WAYLAND_DISPLAY`: setting an
/// environment variable in a test process races every other test in it.
fn connect(socket: &str) -> Connection {
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

fn wait_for(facts: &Facts, ready: impl Fn(&HostFacts) -> bool) -> HostFacts {
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

/// Dispatch until `done`, or fail after five seconds.
///
/// Then one more round trip. `done` becomes true inside a handler that has
/// just queued a commit, and a queued request sits in the client's buffer
/// until something flushes it -- without this, the last buffer this client
/// drew would never reach the compositor.
fn until(queue: &mut EventQueue<Desk>, desk: &mut Desk, done: impl Fn(&Desk) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(desk) {
        assert!(
            Instant::now() < deadline,
            "the compositor never configured us"
        );
        queue.roundtrip(desk).expect("dispatch");
    }
    queue.roundtrip(desk).expect("flush");
}

/// The test's client: windows, each drawn once in a flat colour as soon as
/// it is configured, remembering the decoration mode each was last told.
struct Desk {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    shm: Shm,
    pool: SlotPool,
    windows: Vec<Window>,
    modes: Vec<DecorationMode>,
    drawn: usize,
}

impl Desk {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>) -> Self {
        let shm = Shm::bind(globals, qh).expect("wl_shm");
        let pool = SlotPool::new(1 << 22, &shm).expect("a buffer pool");
        Self {
            registry: RegistryState::new(globals),
            outputs: OutputState::new(globals, qh),
            compositor: CompositorState::bind(globals, qh).expect("wl_compositor"),
            xdg: XdgShell::bind(globals, qh).expect("xdg_wm_base"),
            shm,
            pool,
            windows: Vec::new(),
            modes: Vec::new(),
            drawn: 0,
        }
    }

    fn open_window(&mut self, qh: &QueueHandle<Self>, decorations: WindowDecorations, title: &str) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, decorations, qh);
        window.set_title(title);
        window.commit();
        self.windows.push(window);
    }

    /// Attach one frame of flat colour at `size`.
    fn paint(&mut self, surface: &wl_surface::WlSurface, (width, height): (u32, u32)) {
        let stride = i32::try_from(width * 4).unwrap();
        let (buffer, canvas) = self
            .pool
            .create_buffer(
                i32::try_from(width).unwrap(),
                i32::try_from(height).unwrap(),
                stride,
                wl_shm::Format::Argb8888,
            )
            .expect("a buffer");
        canvas.fill(0xff);
        buffer.attach_to(surface).expect("attach");
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
    }
}

impl WindowHandler for Desk {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        // Drawn once; a later configure only changes the decoration mode.
        let index = self
            .windows
            .iter()
            .position(|known| known == window)
            .expect("one of ours");
        if let Some(mode) = self.modes.get_mut(index) {
            *mode = configure.decoration_mode;
            return;
        }
        self.modes.push(configure.decoration_mode);
        let surface = window.wl_surface().clone();
        self.paint(&surface, WINDOW);
        window.xdg_surface().set_window_geometry(
            0,
            0,
            i32::try_from(WINDOW.0).unwrap(),
            i32::try_from(WINDOW.1).unwrap(),
        );
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
delegate_xdg_shell!(Desk);
delegate_xdg_window!(Desk);
delegate_registry!(Desk);
