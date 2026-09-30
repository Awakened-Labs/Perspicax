//! A panel over a window, judged by a compositor with no screen.
//!
//! The claim layer-shell support makes to the index is small and exact: a
//! surface on the `top` layer sits over every window, so the part of a window
//! under a panel is occluded *by the panel*. This test puts a real window and a
//! real panel on a headless compositor, through the real protocols, and asks
//! the index's own judge.
//!
//! The client is written here with smithay-client-toolkit, not borrowed from
//! waybar, so the test needs nothing installed beyond what CI already has. Like
//! the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Config, Facts, Requests, Stop};
use perspicax_index::{HostFacts, judge};
use perspicax_node::{Rect, Visibility};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
        },
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
const PANEL_HEIGHT: u32 = 40;

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_on_the_top_layer_occludes_the_window_beneath_it() {
    let socket = format!("perspicax-test-{}", std::process::id());
    let facts = Facts::new();
    let stop = Stop::new();
    let compositor = {
        let (facts, stop, socket) = (facts.clone(), stop.clone(), socket.clone());
        thread::spawn(move || {
            let config = Config {
                backend: Backend::Headless { size: (800, 600) },
                spawn: Vec::new(),
                env: Vec::new(),
                run_for: Some(Duration::from_secs(30)),
                config: None,
                socket: Some(socket),
                xwayland: false,
            };
            perspicax_compositor::run(&config, &facts, &Requests::new(), &stop)
        })
    };

    let client = connect(&socket);
    let (globals, mut queue) = registry_queue_init(&client).expect("the registry");
    let qh = queue.handle();
    let mut desk = Desk::new(&globals, &qh);
    // The window first, so the cascade puts it at the origin under where the
    // panel will go -- a panel mapped first would make the window open below
    // its exclusive zone, which is correct and not what this test is about.
    desk.open_window(&qh);
    until(&mut queue, &mut desk, |desk| desk.windows_drawn == 1);
    desk.open_panel(&qh);
    until(&mut queue, &mut desk, |desk| desk.panel_drawn);

    let published = wait_for(&facts, |facts| {
        facts
            .surfaces()
            .iter()
            .filter(|surface| surface.mapped)
            .count()
            == 2
    });
    let surfaces = published.surfaces();
    let (window, panel) = (&surfaces[0], &surfaces[1]);
    assert!(
        panel.geometry.y1 - panel.geometry.y0 < 100.0,
        "the topmost surface is the panel, a strip across the top: {:?}",
        panel.geometry
    );

    let under_panel = Rect::new(10.0, 10.0, 60.0, 30.0);
    let below_panel = Rect::new(10.0, 100.0, 60.0, 120.0);
    assert_eq!(
        judge(&published, window.id, under_panel).visibility,
        Visibility::Occluded { by: panel.id },
        "a button in the window's top strip is under the panel"
    );
    assert_eq!(
        judge(&published, window.id, below_panel).visibility,
        Visibility::Visible,
        "and one further down is in the clear"
    );

    // And a window opened once the panel is up is placed below its exclusive
    // zone, never under it.
    desk.open_window(&qh);
    until(&mut queue, &mut desk, |desk| desk.windows_drawn == 2);
    let published = wait_for(&facts, |facts| {
        facts
            .surfaces()
            .iter()
            .filter(|surface| surface.mapped)
            .count()
            == 3
    });
    let second = published
        .surfaces()
        .iter()
        .find(|surface| surface.id != window.id && surface.id != panel.id)
        .expect("the second window");
    assert!(
        second.geometry.y0 >= f64::from(PANEL_HEIGHT),
        "the second window opened under the panel: {:?}",
        second.geometry
    );

    stop.request();
    drop((desk, queue));
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
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

/// The test's client: one window and one panel, each drawn once in a flat
/// colour as soon as it is configured.
struct Desk {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    layers: LayerShell,
    shm: Shm,
    pool: SlotPool,
    windows: Vec<Window>,
    panel: Option<LayerSurface>,
    windows_drawn: usize,
    panel_drawn: bool,
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
            layers: LayerShell::bind(globals, qh).expect("zwlr_layer_shell_v1"),
            shm,
            pool,
            windows: Vec::new(),
            panel: None,
            windows_drawn: 0,
            panel_drawn: false,
        }
    }

    fn open_window(&mut self, qh: &QueueHandle<Self>) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, WindowDecorations::None, qh);
        window.set_title("under the panel");
        window.commit();
        self.windows.push(window);
    }

    fn open_panel(&mut self, qh: &QueueHandle<Self>) {
        let surface = self.compositor.create_surface(qh);
        let panel = self
            .layers
            .create_layer_surface(qh, surface, Layer::Top, Some("panel"), None);
        panel.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
        panel.set_size(0, PANEL_HEIGHT);
        panel.set_exclusive_zone(i32::try_from(PANEL_HEIGHT).unwrap());
        panel.commit();
        self.panel = Some(panel);
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
        _: WindowConfigure,
        _: u32,
    ) {
        // The size this test chose, whatever was suggested: the geometry is
        // part of what is being asserted.
        let surface = window.wl_surface().clone();
        self.paint(&surface, WINDOW);
        window.xdg_surface().set_window_geometry(
            0,
            0,
            i32::try_from(WINDOW.0).unwrap(),
            i32::try_from(WINDOW.1).unwrap(),
        );
        window.commit();
        self.windows_drawn += 1;
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
        // Anchored left and right with no width of its own: the compositor
        // says how wide the output is.
        let width = configure.new_size.0.max(1);
        let surface = layer.wl_surface().clone();
        self.paint(&surface, (width, PANEL_HEIGHT));
        layer.commit();
        self.panel_drawn = true;
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
delegate_layer!(Desk);
delegate_registry!(Desk);
