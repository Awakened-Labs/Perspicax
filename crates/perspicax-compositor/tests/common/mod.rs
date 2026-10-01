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
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    backend::ObjectId,
    event_created_child,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_output, wl_shm, wl_surface},
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
};

pub const WINDOW: (u32, u32) = (400, 300);

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

    /// A client, its registry read.
    pub fn client(&self) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList) {
        let client = connect(&self.socket);
        let (globals, queue) = registry_queue_init(&client).expect("the registry");
        let qh = queue.handle();
        (Desk::new(&globals, &qh), queue, qh, globals)
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

/// The test's client: windows of its own, drawn once in a flat colour, and
/// whichever of the watching protocols a test binds.
pub struct Desk {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    shm: Shm,
    pool: SlotPool,
    pub windows: Vec<Window>,
    pub drawn: usize,
    pub list: Option<ExtForeignToplevelListV1>,
    pub list_finished: bool,
    pub listed: HashMap<ObjectId, Listed>,
    pub lock: Option<ExtSessionLockV1>,
    pub locked: bool,
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
            list: None,
            list_finished: false,
            listed: HashMap::new(),
            lock: None,
            locked: false,
        }
    }

    pub fn open_window(&mut self, qh: &QueueHandle<Self>, title: &str, app_id: &str) {
        let surface = self.compositor.create_surface(qh);
        let window = self.xdg.create_window(surface, WindowDecorations::None, qh);
        window.set_title(title);
        window.set_app_id(app_id);
        window.commit();
        self.windows.push(window);
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
