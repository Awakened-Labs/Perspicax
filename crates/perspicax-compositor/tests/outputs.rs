//! Monitors plugged in and out of a compositor with no screen.
//!
//! A person rearranging a desk does two things to the windows on it, and both
//! are claims this test checks through a real client and the published facts:
//!
//! - A monitor placed to the left of another pushes that one right, and the
//!   windows on it go with it. Nobody plugging in a second screen expects
//!   their windows to stay at the old coordinates, which now belong to the
//!   new monitor.
//! - A window left on a monitor that is unplugged is brought onto one that
//!   remains, instead of staying open, focused and somewhere nobody can see.
//!
//! The monitors are virtual, which is the point: this runs on a machine with
//! one monitor, or none. Like the other live tests it binds a real Wayland
//! socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Command, Config, Facts, Requests, Stop, Virtual};
use perspicax_index::HostFacts;
use perspicax_node::Rect;
use perspicax_policy::{Place, Side};
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

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn windows_move_with_their_monitor_and_are_rescued_when_it_is_unplugged() {
    let socket = format!("perspicax-outputs-{}", std::process::id());
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
                backend: Backend::Headless {
                    outputs: vec![Virtual::numbered(1, (1280, 1024))],
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

    let client = connect(&socket);
    let (globals, mut queue) = registry_queue_init(&client).expect("the registry");
    let qh = queue.handle();
    let mut desk = Desk::new(&globals, &qh);
    desk.open_window(&qh);
    until(&mut queue, &mut desk, |desk| desk.drawn);
    let published = wait_for(&facts, |facts| {
        facts.surfaces().iter().any(|surface| surface.mapped)
    });
    assert_eq!(published.outputs(), [rect(0, 0, 1280, 1024)]);
    assert_eq!(
        window(&published),
        rect(0, 0, 400, 300),
        "the cascade's first place"
    );

    // A bigger monitor to the left, its top 56 pixels above the first one's.
    // It takes the origin, and the first monitor and its window move right.
    requests
        .command(Command::Plug(Virtual {
            name: "HEADLESS-2".to_owned(),
            size: (1920, 1080),
            place: Place::Beside {
                side: Side::LeftOf,
                of: "HEADLESS-1".to_owned(),
                offset: -56,
            },
        }))
        .expect("the compositor is listening");
    let published = wait_for(&facts, |facts| facts.outputs().len() == 2);
    assert_eq!(
        published.outputs(),
        [rect(1920, 56, 1280, 1024), rect(0, 0, 1920, 1080)],
        "the first monitor slid right of the new one, 56 down"
    );
    assert_eq!(
        window(&published),
        rect(1920, 56, 400, 300),
        "the window went with its monitor"
    );

    // Unplug the window's monitor. The one left slides to the origin, and the
    // window comes onto it, against its right edge.
    requests
        .command(Command::Unplug("HEADLESS-1".to_owned()))
        .expect("the compositor is listening");
    let published = wait_for(&facts, |facts| facts.outputs().len() == 1);
    assert_eq!(published.outputs(), [rect(0, 0, 1920, 1080)]);
    assert_eq!(
        window(&published),
        rect(1520, 56, 400, 300),
        "rescued onto the monitor that remains, as near to where it was as fits"
    );

    stop.request();
    drop((desk, queue));
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(x + w),
        f64::from(y + h),
    )
}

/// The one window's rect in the global space.
fn window(facts: &HostFacts) -> Rect {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.mapped)
        .expect("the window")
        .geometry
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

/// The test's client: one window, drawn once in a flat colour as soon as it
/// is configured.
struct Desk {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    shm: Shm,
    pool: SlotPool,
    window: Option<Window>,
    drawn: bool,
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
            window: None,
            drawn: false,
        }
    }

    fn open_window(&mut self, qh: &QueueHandle<Self>) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, WindowDecorations::None, qh);
        window.set_title("on a monitor");
        window.commit();
        self.window = Some(window);
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
        self.drawn = true;
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
