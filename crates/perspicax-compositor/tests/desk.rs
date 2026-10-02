//! Window management across two monitors, on a compositor with no screen:
//! workspaces, and snapping.
//!
//! What a person sees when they switch workspace, checked through the facts
//! an agent reads: the windows of the workspace left behind are judged
//! `OtherWorkspace`, naming where they went, and the ones arriving are
//! visible again. Both ways monitors can share workspaces are run: spanning,
//! where one switch changes both monitors, and per output, where each monitor
//! flips on its own and a window sent to the other monitor joins whatever
//! that one is showing. And Logo and an arrow, giving a window half its
//! monitor and back.
//!
//! Driven by `Command::Perform`, the same code a person's key bindings run,
//! because a headless compositor has no keyboard. Like the other live tests
//! it binds a real Wayland socket, so it needs `XDG_RUNTIME_DIR`, and is
//! `#[ignore]`d for `ci/live-tests.sh` to run.

use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Command, Config, Facts, Requests, Stop, Virtual};
use perspicax_index::{HostFacts, judge};
use perspicax_node::{Rect, Visibility};
use perspicax_policy::{Action, Direction, Grid, Mode, Place, Shape, Side, Towards};
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
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
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

/// A button inside every window, window-relative.
const BUTTON: Rect = Rect {
    x0: 10.0,
    y0: 10.0,
    x1: 60.0,
    y1: 30.0,
};

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_spanning_switch_hides_one_workspace_and_shows_another_on_both_monitors() {
    let session = Session::start("spanning", Mode::Spanning);
    let (mut desk, mut queue, qh) = session.client();

    desk.open_window(&qh, "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let published = session.wait_for(|facts| seen(facts, "first") == Visibility::Visible);
    assert_eq!(published.outputs().len(), 2);

    session.perform(Action::Workspace(Direction::Right));
    session.wait_for(|facts| seen(facts, "first") == Visibility::OtherWorkspace { workspace: 1 });

    // A window opened now opens on workspace 2, where the person is.
    desk.open_window(&qh, "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    session.wait_for(|facts| seen(facts, "second") == Visibility::Visible);

    session.perform(Action::Workspace(Direction::Left));
    session.wait_for(|facts| {
        seen(facts, "first") == Visibility::Visible
            && seen(facts, "second") == Visibility::OtherWorkspace { workspace: 2 }
    });

    // The first window has the keyboard again, since the one that had it
    // left with its workspace. Carry it down to workspace 3 and go with it.
    session.perform(Action::CarryToWorkspace(Direction::Down));
    session.wait_for(|facts| {
        seen(facts, "first") == Visibility::Visible
            && seen(facts, "second") == Visibility::OtherWorkspace { workspace: 2 }
    });

    // Across to the other monitor, which is showing the same workspace, so
    // the window stays in front of the person.
    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    let published = session.wait_for(|facts| geometry(facts, "first").x0 >= 1280.0);
    assert_eq!(seen(&published, "first"), Visibility::Visible);

    session.stop(desk, queue);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn per_output_each_monitor_flips_alone_and_a_moved_window_joins_the_other() {
    let session = Session::start("per-output", Mode::PerOutput);
    let (mut desk, mut queue, qh) = session.client();

    desk.open_window(&qh, "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    session.wait_for(|facts| seen(facts, "first") == Visibility::Visible);

    // The left monitor, where the window is, flips to workspace 2.
    session.perform(Action::Workspace(Direction::Right));
    session.wait_for(|facts| seen(facts, "first") == Visibility::OtherWorkspace { workspace: 1 });

    desk.open_window(&qh, "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    session.wait_for(|facts| seen(facts, "second") == Visibility::Visible);

    // To the right monitor, still on workspace 1: the window joins it and
    // stays visible, and the left monitor still shows workspace 2.
    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    let published = session.wait_for(|facts| geometry(facts, "second").x0 >= 1280.0);
    assert_eq!(seen(&published, "second"), Visibility::Visible);
    assert_eq!(
        seen(&published, "first"),
        Visibility::OtherWorkspace { workspace: 1 },
        "the left monitor did not follow"
    );

    session.stop(desk, queue);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn logo_and_an_arrow_snap_the_focused_window_to_a_half_and_back() {
    let session = Session::start("snap", Mode::Spanning);
    let (mut desk, mut queue, qh) = session.client();

    desk.open_window(&qh, "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    session.wait_for(|facts| seen(facts, "first") == Visibility::Visible);

    // The right half of the left monitor, which is 1280 wide.
    session.perform(Action::Snap(Direction::Right));
    session.wait_for(|facts| geometry(facts, "first").x0 == 640.0);
    // And back from the right half to where it was, as Windows does.
    session.perform(Action::Snap(Direction::Left));
    session.wait_for(|facts| geometry(facts, "first").x0 == 0.0);

    session.stop(desk, queue);
}

/// A compositor on a thread: two virtual monitors side by side, a grid of
/// 2x2 workspaces, and a channel to perform bindings with.
struct Session {
    socket: String,
    facts: Facts,
    requests: Requests,
    stop: Stop,
    thread: thread::JoinHandle<Result<(), perspicax_compositor::Error>>,
}

impl Session {
    fn start(name: &str, mode: Mode) -> Self {
        let socket = format!("perspicax-desk-{name}-{}", std::process::id());
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
                    backend: Backend::Headless {
                        outputs: vec![
                            Virtual::numbered(1, (1280, 1024)),
                            Virtual {
                                place: Place::Beside {
                                    side: Side::RightOf,
                                    of: "HEADLESS-1".to_owned(),
                                    offset: 0,
                                },
                                ..Virtual::numbered(2, (1920, 1080))
                            },
                        ],
                        workspaces: Shape {
                            mode,
                            grid: Grid {
                                columns: 2,
                                rows: 2,
                                wrap: false,
                            },
                        },
                        access: perspicax_policy::Access::open(),
                    },
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
        Self {
            socket,
            facts,
            requests,
            stop,
            thread,
        }
    }

    fn client(&self) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>) {
        let client = connect(&self.socket);
        let (globals, queue) = registry_queue_init(&client).expect("the registry");
        let qh = queue.handle();
        (Desk::new(&globals, &qh), queue, qh)
    }

    fn perform(&self, action: Action) {
        self.requests
            .command(Command::Perform(action))
            .expect("the compositor is listening");
    }

    fn wait_for(&self, ready: impl Fn(&HostFacts) -> bool) -> HostFacts {
        wait_for(&self.facts, ready)
    }

    fn stop(self, desk: Desk, queue: EventQueue<Desk>) {
        self.stop.request();
        drop((desk, queue));
        self.thread
            .join()
            .expect("the compositor thread panicked")
            .expect("the compositor failed");
    }
}

/// How the index judges a button in the window titled `title`.
fn seen(facts: &HostFacts, title: &str) -> Visibility {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map_or(Visibility::Unknown, |surface| {
            judge(facts, surface.id, BUTTON).visibility
        })
}

fn geometry(facts: &HostFacts, title: &str) -> Rect {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map_or(Rect::ZERO, |surface| surface.geometry)
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

/// Dispatch until `done`, or fail after five seconds, then one more round
/// trip to flush the commit `done` was set beside.
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
/// it is configured.
struct Desk {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    shm: Shm,
    pool: SlotPool,
    windows: Vec<Window>,
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
            drawn: 0,
        }
    }

    fn open_window(&mut self, qh: &QueueHandle<Self>, title: &str) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, WindowDecorations::None, qh);
        window.set_title(title);
        window.commit();
        self.windows.push(window);
    }
}

impl WindowHandler for Desk {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        _: WindowConfigure,
        _: u32,
    ) {
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
        canvas.fill(0xff);
        let surface = window.wl_surface();
        buffer.attach_to(surface).expect("attach");
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        window.xdg_surface().set_window_geometry(
            0,
            0,
            i32::try_from(width).unwrap(),
            i32::try_from(height).unwrap(),
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
