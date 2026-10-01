//! Frames, negotiated and described, by a compositor with no screen.
//!
//! A client that asks for server-side decorations gets them, and the frame the
//! compositor would draw goes out in the facts, so that the index can see that
//! a titlebar covers what is under it -- though headless nothing is drawn. A
//! client that asks for nothing draws its own frame, as before.
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
use perspicax_index::{HostFacts, judge};
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

/// What the default `[decorations]` adds above a window: a 24-pixel
/// titlebar and the 2-pixel border over it.
const TOP: f64 = 26.0;

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_titlebar_is_negotiated_published_and_covers_what_is_under_it() {
    let socket = format!("perspicax-test-{}", std::process::id());
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
                backend: Backend::headless((800, 600)),
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

    let client = connect(&socket);
    let (globals, mut queue) = registry_queue_init(&client).expect("the registry");
    let qh = queue.handle();
    let mut desk = Desk::new(&globals, &qh);

    // Two framed windows, the second cascaded over the first.
    // The mode arrives in a configure of its own: a window is configured
    // when it is created, before its client has asked about decorations.
    let server = |desk: &Desk, n: usize| {
        desk.drawn == n
            && desk
                .modes
                .iter()
                .all(|mode| *mode == DecorationMode::Server)
    };
    desk.open_window(&qh, WindowDecorations::RequestServer);
    until(&mut queue, &mut desk, |desk| server(desk, 1));
    desk.open_window(&qh, WindowDecorations::RequestServer);
    until(&mut queue, &mut desk, |desk| server(desk, 2));

    let published = wait_for(&facts, |facts| mapped(facts) == 2 && framed(facts) == 2);
    let surfaces = published.surfaces();
    let (first, second) = (&surfaces[0], &surfaces[1]);

    // Opened at the corner of the screen, the first window was moved down
    // and right so its titlebar is on screen.
    assert_eq!(
        (first.geometry.x0, first.geometry.y0),
        (2.0, TOP),
        "the first window's client sits inside its frame"
    );
    let titlebar = first.frame[0];
    assert_eq!(
        (titlebar.y0, titlebar.y1),
        (0.0, TOP),
        "and its titlebar is the strip above it: {:?}",
        first.frame
    );

    // The strip of the first window just under the second one's titlebar,
    // and above the second one's client.
    let gap = second.geometry.y0 - first.geometry.y0;
    assert!(
        gap > 0.0 && gap < TOP,
        "the cascade overlaps titlebar and client: {gap}"
    );
    let under_titlebar = Rect::new(100.0, 0.0, 150.0, gap);
    let verdict = judge(&published, first.id, under_titlebar);
    assert_eq!(
        verdict.visibility,
        Visibility::Occluded { by: second.id },
        "a node under another window's titlebar is covered"
    );
    assert!(!verdict.unproven, "and the frame proves it");

    // Maximized, the second window keeps its titlebar, inside the monitor,
    // and loses its border; put back, it has both again.
    let perform = |action| {
        requests
            .command(Command::Perform(action))
            .expect("the compositor is listening");
    };
    perform(Action::ToggleMaximize);
    let published = wait_for(&facts, |facts| {
        facts.surfaces()[1].frame.len() == 1 && facts.surfaces()[1].geometry.y0 == TOP - 2.0
    });
    let maximized = &published.surfaces()[1];
    assert_eq!(maximized.geometry.x0, 0.0, "{:?}", maximized.geometry);
    assert_eq!(
        (maximized.frame[0].y0, maximized.frame[0].y1),
        (0.0, TOP - 2.0),
        "the titlebar is at the top of the monitor"
    );
    perform(Action::ToggleMaximize);
    wait_for(&facts, |facts| facts.surfaces()[1].frame.len() == 4);

    // A client that asks for nothing draws its own frame.
    desk.open_window(&qh, WindowDecorations::None);
    until(&mut queue, &mut desk, |desk| desk.drawn == 3);
    let published = wait_for(&facts, |facts| mapped(facts) == 3);
    let third = &published.surfaces()[2];
    assert!(third.frame.is_empty(), "no frame: {:?}", third.frame);

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

fn framed(facts: &HostFacts) -> usize {
    facts
        .surfaces()
        .iter()
        .filter(|surface| !surface.frame.is_empty())
        .count()
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

    fn open_window(&mut self, qh: &QueueHandle<Self>, decorations: WindowDecorations) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, decorations, qh);
        window.set_title("framed");
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
