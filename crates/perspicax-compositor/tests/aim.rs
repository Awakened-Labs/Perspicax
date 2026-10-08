//! Where an agent's click lands inside a window that draws a shadow.
//!
//! An `Action::Click` names a rect in window space: relative to the window
//! geometry the client declared, inside any client-side shadow. The client is
//! told about the pointer in surface space, where its buffer begins, which
//! under a shadow is the shadow's width further out. So a click at window-space
//! `(50, 50)` on a window whose geometry starts at `(26, 23)` in its buffer must
//! arrive at `(76, 73)` -- Firefox's shape, on the seat where issue #45 found
//! every Firefox click a shadow's width from its target.
//!
//! A seat and a headless build with `capture` computed that shadow from
//! smithay's window geometry, which only a backend keeping buffers ever
//! measures; a plain headless build saw none and aimed short by it. Both now
//! take it from the geometry the client declared, the same place the
//! published facts' `buffer_origin` comes from, so this test means the same
//! thing in every build `ci/live-tests.sh` runs it in.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Config, Facts, Host, Requests, Stop};
use perspicax_index::{Action, HostFacts, PointerButton, SurfaceFacts};
use perspicax_node::{Rect, SurfaceId, Vec2};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
    delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
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
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

/// Each window's geometry: the visible frame, and window space's extent.
const WINDOW: (i32, i32) = (300, 200);
/// Where the shadowed window's geometry begins in its buffer: Firefox's
/// shadow, as #45 measured it.
const SHADOW: (i32, i32) = (26, 23);

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_in_window_space_lands_inside_the_shadow_a_window_declared() {
    let (socket, facts, requests, stop, compositor) = start("aim");
    let client = connect(&socket);
    let (globals, mut queue) = registry_queue_init(&client).expect("the registry");
    let qh = queue.handle();
    let mut desk = Desk::new(&globals, &qh);
    desk.open(&qh, "shadowed");
    desk.open(&qh, "flush");
    until(&mut queue, &mut desk, |desk| {
        desk.drawn == 2 && desk.pointer.is_some()
    });
    let published = wait_for(&facts, |facts| {
        facts
            .surfaces()
            .iter()
            .filter(|surface| surface.mapped)
            .count()
            == 2
    });
    let shadowed = titled(&published, "shadowed");
    let flush = titled(&published, "flush");

    // What the index is told, which is what the click has to agree with.
    let geometry = shadowed.geometry;
    assert_eq!(
        (geometry.width(), geometry.height()),
        (f64::from(WINDOW.0), f64::from(WINDOW.1)),
        "the geometry is the declared frame, not the buffer"
    );
    assert_eq!(
        shadowed.buffer_origin,
        geometry.origin().to_vec2() - Vec2::new(f64::from(SHADOW.0), f64::from(SHADOW.1)),
        "the buffer begins the shadow's width outside it"
    );
    assert_eq!(flush.buffer_origin, flush.geometry.origin().to_vec2());

    let host = Host::new(&facts, &requests);
    let click = |surface: SurfaceId| {
        host.act(
            surface,
            &Action::Click {
                at: Rect::new(40.0, 40.0, 60.0, 60.0),
                button: PointerButton::Left,
            },
        )
        .expect("dispatched");
    };

    click(shadowed.id);
    until(&mut queue, &mut desk, |desk| {
        desk.presses("shadowed").len() == 1
    });
    assert_eq!(
        desk.presses("shadowed"),
        [(76.0, 73.0)],
        "the rect's centre, (50, 50) in window space, is the shadow's width \
         further into the buffer"
    );

    click(flush.id);
    until(&mut queue, &mut desk, |desk| {
        desk.presses("flush").len() == 1
    });
    assert_eq!(
        desk.presses("flush"),
        [(50.0, 50.0)],
        "and a window with no shadow is pressed where it was asked"
    );

    stop.request();
    drop((desk, queue));
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
}

fn titled(facts: &HostFacts, title: &str) -> SurfaceFacts {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .unwrap_or_else(|| panic!("no window titled {title:?}: {facts:?}"))
        .clone()
}

/// A headless compositor on a thread, and the means to drive and stop it.
fn start(
    name: &str,
) -> (
    String,
    Facts,
    Requests,
    Stop,
    thread::JoinHandle<Result<(), perspicax_compositor::Error>>,
) {
    let socket = format!("perspicax-test-{name}-{}", std::process::id());
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
    (socket, facts, requests, stop, compositor)
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
/// trip to flush whatever the last handler queued.
fn until(queue: &mut EventQueue<Desk>, desk: &mut Desk, done: impl Fn(&Desk) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(desk) {
        assert!(Instant::now() < deadline, "the compositor never answered");
        queue.roundtrip(desk).expect("dispatch");
    }
    queue.roundtrip(desk).expect("flush");
}

/// The test's client: two windows of one size, one drawing a shadow around
/// its geometry and one drawing none, and where the pointer pressed each.
struct Desk {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    shm: Shm,
    pool: SlotPool,
    windows: Vec<(&'static str, Window)>,
    drawn: usize,
    seat: SeatState,
    pointer: Option<wl_pointer::WlPointer>,
    /// Each button press, surface-local, with the title of the window it
    /// landed on.
    pressed: Vec<(&'static str, (f64, f64))>,
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
            seat: SeatState::new(globals, qh),
            pointer: None,
            pressed: Vec::new(),
        }
    }

    fn open(&mut self, qh: &QueueHandle<Self>, title: &'static str) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, WindowDecorations::None, qh);
        window.set_title(title);
        window.commit();
        self.windows.push((title, window));
    }

    fn presses(&self, title: &str) -> Vec<(f64, f64)> {
        self.pressed
            .iter()
            .filter(|(on, _)| *on == title)
            .map(|(_, at)| *at)
            .collect()
    }

    fn title_of(&self, surface: &wl_surface::WlSurface) -> Option<&'static str> {
        self.windows
            .iter()
            .find(|(_, window)| window.wl_surface() == surface)
            .map(|(title, _)| *title)
    }
}

impl WindowHandler for Desk {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {}

    /// Draw once, at the size this test chose: the shadowed window a buffer
    /// the shadow's width larger on every side, and its geometry inside it.
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        _: WindowConfigure,
        _: u32,
    ) {
        let surface = window.wl_surface().clone();
        let (left, top) = match self.title_of(&surface) {
            Some("shadowed") => SHADOW,
            _ => (0, 0),
        };
        let (width, height) = (WINDOW.0 + 2 * left, WINDOW.1 + 2 * top);
        let (buffer, canvas) = self
            .pool
            .create_buffer(width, height, width * 4, wl_shm::Format::Argb8888)
            .expect("a buffer");
        canvas.fill(0xff);
        buffer.attach_to(&surface).expect("attach");
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        window
            .xdg_surface()
            .set_window_geometry(left, top, WINDOW.0, WINDOW.1);
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

impl SeatHandler for Desk {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for Desk {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if let PointerEventKind::Press { .. } = event.kind
                && let Some(title) = self.title_of(&event.surface)
            {
                self.pressed.push((title, event.position));
            }
        }
    }
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
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Desk);
delegate_output!(Desk);
delegate_shm!(Desk);
delegate_xdg_shell!(Desk);
delegate_xdg_window!(Desk);
delegate_seat!(Desk);
delegate_pointer!(Desk);
delegate_registry!(Desk);
