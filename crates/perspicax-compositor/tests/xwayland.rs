//! An X11 window under Xwayland, and who the index is told drew it.
//!
//! The claim this checks is the provenance one. The compositor's Wayland
//! credentials name only Xwayland, so the X client's pid has to come from
//! somewhere else: first from the window's own `_NET_WM_PID`, a claim, then
//! from the X-Resource extension, the X server's own word. The facts must end
//! up saying `XRes`, with the server attested separately.
//!
//! The X client is this test process, speaking X11 through x11rb, so nothing
//! beyond `Xwayland` itself has to be installed. Needs `XDG_RUNTIME_DIR` and
//! `Xwayland` on the `PATH`, and is `#[ignore]`d for `ci/live-tests.sh`.

#![cfg(feature = "xwayland")]

use std::{
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Command, Config, Error, Facts, Requests, Stop};
use perspicax_index::SurfaceFacts;
use perspicax_node::{Origin, X11Basis};
use perspicax_policy::Action;
use x11rb::{
    connection::Connection as _,
    protocol::{
        Event,
        xproto::{
            AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, MapState,
            PropMode, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

const TITLE: &str = "perspicax x11 provenance";

#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_is_attributed_to_its_client_through_xres() {
    let facts = Facts::new();
    let stop = Stop::new();
    let compositor = {
        let (facts, stop) = (facts.clone(), stop.clone());
        thread::spawn(move || {
            let config = Config {
                backend: Backend::headless((800, 600)),
                spawn: Vec::new(),
                env: Vec::new(),
                run_for: Some(Duration::from_secs(30)),
                config: None,
                socket: None,
                xwayland: true,
            };
            perspicax_compositor::run(&config, &facts, &Requests::new(), &stop)
        })
    };

    let display = eventually(Duration::from_secs(15), || facts.x11_display())
        .expect("Xwayland never became ready");
    let (x, screen) = x11rb::connect(Some(&format!(":{display}"))).expect("an X connection");
    let root = x.setup().roots[screen].root;
    let window = x.generate_id().expect("an X id");
    x.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        320,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().background_pixel(x.setup().roots[screen].white_pixel),
    )
    .expect("create_window");
    x.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        TITLE.as_bytes(),
    )
    .expect("WM_NAME");
    let net_wm_pid = x
        .intern_atom(false, b"_NET_WM_PID")
        .expect("intern")
        .reply()
        .expect("_NET_WM_PID")
        .atom;
    x.change_property32(
        PropMode::REPLACE,
        window,
        net_wm_pid,
        AtomEnum::CARDINAL,
        &[std::process::id()],
    )
    .expect("_NET_WM_PID");
    x.map_window(window).expect("map_window");
    x.flush().expect("flush");

    let described = eventually(Duration::from_secs(10), || {
        facts.read().surfaces().iter().find_map(|surface| {
            let Origin::X11(origin) = &surface.origin else {
                return None;
            };
            (surface.mapped
                && surface.title.as_deref() == Some(TITLE)
                && origin.basis == X11Basis::XRes)
                .then(|| (**origin).clone())
        })
    })
    .unwrap_or_else(|| panic!("never described with an XRes origin: {:?}", facts.read()));

    assert_eq!(
        described.client.map(|client| client.pid),
        Some(std::process::id()),
        "XRes names this test process as the window's client"
    );
    assert_ne!(
        described.server,
        std::process::id(),
        "and the attested server is Xwayland, a different process"
    );

    stop.request();
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
}

/// Issue #21: a `--spawn` program starts once Xwayland is ready, so it finds
/// `DISPLAY`, and so does whatever it starts in turn: the shell in a spawned
/// terminal, say. It used to start first, with `DISPLAY` removed.
///
/// And its toolkit may use it (H3). GTK and Qt are asked for Wayland first, as
/// they always were, but no longer for Wayland alone: Chromium and Electron in
/// X11 mode allow GTK only its X11 backend, and a strict `GDK_BACKEND=wayland`
/// left them nothing to open -- Teams, autostarted, said "cannot open display".
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_spawned_program_finds_xwaylands_display() {
    let written = std::env::temp_dir().join(format!("perspicax-display-{}", std::process::id()));
    let _ = std::fs::remove_file(&written);
    let facts = Facts::new();
    let stop = Stop::new();
    let compositor = {
        let (facts, stop, written) = (facts.clone(), stop.clone(), written.clone());
        thread::spawn(move || {
            let config = Config {
                backend: Backend::headless((800, 600)),
                spawn: vec![vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    r#"echo "$DISPLAY $GDK_BACKEND $QT_QPA_PLATFORM" > "$PERSPICAX_WRITE_DISPLAY_TO""#
                        .to_owned(),
                ]],
                env: vec![(
                    "PERSPICAX_WRITE_DISPLAY_TO".to_owned(),
                    written.display().to_string(),
                )],
                run_for: Some(Duration::from_secs(30)),
                config: None,
                socket: None,
                xwayland: true,
            };
            perspicax_compositor::run(&config, &facts, &Requests::new(), &stop)
        })
    };

    let display = eventually(Duration::from_secs(15), || facts.x11_display())
        .expect("Xwayland never became ready");
    // The whole line, not a file the shell has opened and not yet written.
    let seen = eventually(Duration::from_secs(10), || {
        std::fs::read_to_string(&written)
            .ok()
            .filter(|line| line.ends_with('\n'))
    });
    let _ = std::fs::remove_file(&written);

    stop.request();
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
    assert_eq!(
        seen.as_deref(),
        Some(format!(":{display} wayland,x11 wayland;xcb\n").as_str()),
        "the spawned program saw Xwayland's DISPLAY, and toolkits that may fall back to it"
    );
}

/// A `--spawn` command that will not start is still `run`'s error when it
/// was only tried once Xwayland was ready: not a warning in a log beside a
/// session that started nothing.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_command_that_does_not_exist_is_named_after_waiting_for_xwayland() {
    let config = Config {
        backend: Backend::headless((800, 600)),
        spawn: vec![vec![
            "perspicax-no-such-program".to_owned(),
            "--flag".to_owned(),
        ]],
        env: Vec::new(),
        run_for: Some(Duration::from_secs(20)),
        config: None,
        socket: None,
        xwayland: true,
    };

    match perspicax_compositor::run(&config, &Facts::new(), &Requests::new(), &Stop::new()) {
        Err(Error::Spawn { command, source }) => {
            assert_eq!(command, "perspicax-no-such-program --flag");
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected a spawn failure naming the command, got {other:?}"),
    }
}

fn eventually<T>(within: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(found) = probe() {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// The title of the window that took the keyboard last, as the facts say.
fn focused_last(facts: &Facts) -> Option<String> {
    let facts = facts.read();
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.focused_at.is_some())
        .max_by_key(|surface| surface.focused_at)?
        .title
        .clone()
}

/// Issue #8: an X11 window has no xdg maximized state, so maximizing one
/// snaps it to the whole monitor. It must be framed as maximized all the
/// same: the titlebar and no border, with the client sized for exactly that.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_maximized_x11_window_keeps_its_titlebar_and_loses_its_border() {
    let facts = Facts::new();
    let requests = Requests::new();
    let stop = Stop::new();
    let compositor = {
        let (facts, requests, stop) = (facts.clone(), requests.clone(), stop.clone());
        thread::spawn(move || {
            let config = Config {
                backend: Backend::headless((800, 600)),
                spawn: Vec::new(),
                env: Vec::new(),
                run_for: Some(Duration::from_secs(30)),
                config: None,
                socket: None,
                xwayland: true,
            };
            perspicax_compositor::run(&config, &facts, &requests, &stop)
        })
    };

    let display = eventually(Duration::from_secs(15), || facts.x11_display())
        .expect("Xwayland never became ready");
    let (x, screen) = x11rb::connect(Some(&format!(":{display}"))).expect("an X connection");
    let root = x.setup().roots[screen].root;
    // Two of them, so the one maximized is the one with the keyboard.
    for title in ["first", "second"] {
        let window = x.generate_id().expect("an X id");
        x.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            320,
            200,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(x.setup().roots[screen].white_pixel),
        )
        .expect("create_window");
        x.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )
        .expect("WM_NAME");
        x.map_window(window).expect("map_window");
    }
    x.flush().expect("flush");

    // The focused window with a frame of this many strips.
    let framed = |strips: usize| {
        eventually(Duration::from_secs(10), || {
            facts.read().surfaces().iter().find_map(|surface| {
                (surface.mapped && surface.focused_at.is_some() && surface.frame.len() == strips)
                    .then(|| surface.clone())
            })
        })
    };
    eventually(Duration::from_secs(10), || {
        let facts = facts.read();
        let framed = facts
            .surfaces()
            .iter()
            .filter(|s| s.mapped && s.frame.len() == 4);
        (framed.count() == 2).then_some(())
    })
    .expect("X11 windows with no Motif hints are framed: titlebar and border");

    // The second took the keyboard when it mapped, so it is the one
    // maximized.
    eventually(Duration::from_secs(5), || {
        (focused_last(&facts).as_deref() == Some("second")).then_some(())
    })
    .expect("the second X11 window never took the keyboard");

    requests
        .command(Command::Perform(Action::ToggleMaximize))
        .expect("the compositor is listening");
    let maximized = framed(1).unwrap_or_else(|| panic!("never titlebar-only: {:?}", facts.read()));
    let titlebar = maximized.frame[0];
    assert_eq!(
        (titlebar.x0, titlebar.y0, titlebar.x1, titlebar.y1),
        (0.0, 0.0, 800.0, 24.0),
        "the titlebar across the top of the monitor"
    );
    assert_eq!(
        (maximized.geometry.x0, maximized.geometry.y0),
        (0.0, 24.0),
        "and the client right under it, with no border"
    );

    requests
        .command(Command::Perform(Action::ToggleMaximize))
        .expect("the compositor is listening");
    framed(4).expect("restored, the border is back");

    stop.request();
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
}

/// A new X11 window takes the keyboard when it opens, as a Wayland one does,
/// and the next takes it from it. It maps before Xwayland has given it a
/// surface, so the keyboard waits for the surface: it used to be dropped, and
/// what was typed next went into the window the person had moved on from.
///
/// The X server's own answer is the one asked, since it decides which X
/// client hears a key.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_new_x11_window_takes_the_keyboard_when_it_maps() {
    let facts = Facts::new();
    let stop = Stop::new();
    let compositor = {
        let (facts, stop) = (facts.clone(), stop.clone());
        thread::spawn(move || {
            let config = Config {
                backend: Backend::headless((800, 600)),
                spawn: Vec::new(),
                env: Vec::new(),
                run_for: Some(Duration::from_secs(30)),
                config: None,
                socket: None,
                xwayland: true,
            };
            perspicax_compositor::run(&config, &facts, &Requests::new(), &stop)
        })
    };

    let display = eventually(Duration::from_secs(15), || facts.x11_display())
        .expect("Xwayland never became ready");
    let (x, screen) = x11rb::connect(Some(&format!(":{display}"))).expect("an X connection");
    let root = x.setup().roots[screen].root;
    for title in ["first", "second"] {
        let window = x.generate_id().expect("an X id");
        x.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            320,
            200,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(x.setup().roots[screen].white_pixel),
        )
        .expect("create_window");
        x.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )
        .expect("WM_NAME");
        x.map_window(window).expect("map_window");
        x.flush().expect("flush");

        eventually(Duration::from_secs(10), || {
            let focus = x.get_input_focus().ok()?.reply().ok()?.focus;
            (focus == window).then_some(())
        })
        .unwrap_or_else(|| panic!("{title} never took the keyboard: {:?}", facts.read()));
    }

    // And the facts agree: the second is the window focused last.
    eventually(Duration::from_secs(5), || {
        (focused_last(&facts).as_deref() == Some("second")).then_some(())
    })
    .unwrap_or_else(|| panic!("the facts never named the second: {:?}", facts.read()));

    stop.request();
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
}

x11rb::atom_manager! {
    /// The atoms an X11 window's state is asked for and told in.
    Atoms: AtomsCookie {
        WM_STATE,
        WM_CHANGE_STATE,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_FOCUSED,
    }
}

/// ICCCM's `WM_STATE`, as a window manager writes it.
const NORMAL: u32 = 1;
const ICONIC: u32 = 3;

/// A compositor with Xwayland, a person at it or not, and this test's own X
/// connection to it: an X client asking for states the way Wine does, and
/// reading back what the window manager told it.
struct X11 {
    facts: Facts,
    requests: Requests,
    stop: Stop,
    compositor: thread::JoinHandle<Result<(), Error>>,
    x: RustConnection,
    root: u32,
    white: u32,
    atoms: Atoms,
}

impl X11 {
    fn start(backend: Backend) -> Self {
        let facts = Facts::new();
        let requests = Requests::new();
        let stop = Stop::new();
        let compositor = {
            let (facts, requests, stop) = (facts.clone(), requests.clone(), stop.clone());
            thread::spawn(move || {
                let config = Config {
                    backend,
                    spawn: Vec::new(),
                    env: Vec::new(),
                    run_for: Some(Duration::from_secs(60)),
                    config: None,
                    socket: None,
                    xwayland: true,
                };
                perspicax_compositor::run(&config, &facts, &requests, &stop)
            })
        };
        let display = eventually(Duration::from_secs(15), || facts.x11_display())
            .expect("Xwayland never became ready");
        let (x, screen) = x11rb::connect(Some(&format!(":{display}"))).expect("an X connection");
        let root = x.setup().roots[screen].root;
        let white = x.setup().roots[screen].white_pixel;
        let atoms = Atoms::new(&x).expect("intern").reply().expect("the atoms");
        Self {
            facts,
            requests,
            stop,
            compositor,
            x,
            root,
            white,
            atoms,
        }
    }

    /// A 320 by 200 window titled `title`, mapped, once the compositor has
    /// it on screen. Its property changes are heard, so an answer can be
    /// waited for.
    fn open(&self, title: &str) -> u32 {
        self.open_with_state(title, &[])
    }

    /// The same, with `_NET_WM_STATE` set to `state` before it maps, as a
    /// client starting fullscreen sets it: EWMH lets a withdrawn window write
    /// its own.
    fn open_with_state(&self, title: &str, state: &[u32]) -> u32 {
        let window = self.x.generate_id().expect("an X id");
        self.x
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                self.root,
                0,
                0,
                320,
                200,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .background_pixel(self.white)
                    .event_mask(EventMask::PROPERTY_CHANGE),
            )
            .expect("create_window");
        self.x
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            )
            .expect("WM_NAME");
        if !state.is_empty() {
            self.x
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    self.atoms._NET_WM_STATE,
                    AtomEnum::ATOM,
                    state,
                )
                .expect("_NET_WM_STATE");
        }
        self.x.map_window(window).expect("map_window");
        self.x.flush().expect("flush");
        self.surface_where(title, |surface| surface.mapped)
            .unwrap_or_else(|| panic!("{title} never mapped: {:?}", self.facts.read()));
        window
    }

    /// Ask, as a client does: a client message to the root, which the window
    /// manager redirects to itself.
    fn ask(&self, window: u32, kind: u32, data: [u32; 5]) {
        self.drain();
        let event = ClientMessageEvent::new(32, window, kind, data);
        self.x
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .expect("send_event");
        self.x.flush().expect("flush");
    }

    /// Ask to be iconified or made normal, with ICCCM's `WM_CHANGE_STATE`.
    fn change_state(&self, window: u32, state: u32) {
        self.ask(window, self.atoms.WM_CHANGE_STATE, [state, 0, 0, 0, 0]);
    }

    /// Forget the property changes heard so far.
    fn drain(&self) {
        while let Ok(Some(_)) = self.x.poll_for_event() {}
    }

    /// Wait for the window manager to change `property` on `window`: the
    /// answer a client like Wine waits for after each request.
    fn answered(&self, window: u32, property: u32) -> bool {
        eventually(Duration::from_secs(5), || {
            loop {
                match self.x.poll_for_event().ok()? {
                    Some(Event::PropertyNotify(notify))
                        if notify.window == window && notify.atom == property =>
                    {
                        return Some(());
                    }
                    Some(_) => {}
                    None => return None,
                }
            }
        })
        .is_some()
    }

    /// The window's `WM_STATE`, as ICCCM spells it: the state, then the
    /// icon window.
    fn wm_state(&self, window: u32) -> Vec<u32> {
        self.x
            .get_property(
                false,
                window,
                self.atoms.WM_STATE,
                self.atoms.WM_STATE,
                0,
                2,
            )
            .expect("get_property")
            .reply()
            .expect("WM_STATE")
            .value32()
            .map(Iterator::collect)
            .unwrap_or_default()
    }

    /// The window's `_NET_WM_STATE`.
    fn net_wm_state(&self, window: u32) -> Vec<u32> {
        self.x
            .get_property(
                false,
                window,
                self.atoms._NET_WM_STATE,
                AtomEnum::ATOM,
                0,
                32,
            )
            .expect("get_property")
            .reply()
            .expect("_NET_WM_STATE")
            .value32()
            .map(Iterator::collect)
            .unwrap_or_default()
    }

    /// The window the X server sends keys to.
    fn focus(&self) -> u32 {
        self.x
            .get_input_focus()
            .expect("get_input_focus")
            .reply()
            .expect("the focus")
            .focus
    }

    /// Whether the X server still shows the window: its frame is mapped.
    fn viewable(&self, window: u32) -> bool {
        self.x
            .get_window_attributes(window)
            .expect("get_window_attributes")
            .reply()
            .expect("the attributes")
            .map_state
            == MapState::VIEWABLE
    }

    /// The facts of the window titled `title`, once `ready` holds of them.
    fn surface_where(
        &self,
        title: &str,
        ready: impl Fn(&SurfaceFacts) -> bool,
    ) -> Option<SurfaceFacts> {
        eventually(Duration::from_secs(10), || {
            self.facts
                .read()
                .surfaces()
                .iter()
                .find(|surface| surface.title.as_deref() == Some(title) && ready(surface))
                .cloned()
        })
    }

    fn perform(&self, action: Action) {
        self.requests
            .command(Command::Perform(action))
            .expect("the compositor is listening");
    }

    fn stop(self) {
        self.stop.request();
        self.compositor
            .join()
            .expect("the compositor thread panicked")
            .expect("the compositor failed");
    }
}

/// Issue #90: Wine minimizes a window by asking with `WM_CHANGE_STATE`, and
/// waits to be told it is iconic before it does anything more with it. The
/// window is minimized and told, with `WM_STATE` and `_NET_WM_STATE_HIDDEN`.
/// Its frame stays mapped, so Xwayland keeps its surface: minimized is a
/// state here, not an unmapping.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_that_iconifies_itself_is_minimized_and_told_it_is_iconic() {
    let x11 = X11::start(Backend::headless((1280, 1024)).with_person());
    let window = x11.open("game");

    x11.change_state(window, ICONIC);
    assert!(
        x11.answered(window, x11.atoms.WM_STATE),
        "never told it is iconic"
    );
    x11.surface_where("game", |surface| {
        !surface.mapped && surface.off_workspace.is_none()
    })
    .unwrap_or_else(|| panic!("never minimized: {:?}", x11.facts.read()));
    assert_eq!(x11.wm_state(window), [ICONIC, 0]);
    assert!(
        x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );
    assert!(x11.viewable(window), "minimized by state, not unmapped");

    x11.stop();
}

/// Issue #90: a window minimized by the person, then brought back, is told
/// both: iconic, then normal, which is the change Wine restores a window on.
/// Brought back, it takes the keyboard -- the surface Xwayland gave it is
/// still its own.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_minimized_x11_window_is_told_it_is_normal_and_takes_the_keyboard_when_restored() {
    let x11 = X11::start(Backend::headless((1280, 1024)).with_person());
    let first = x11.open("first");
    let second = x11.open("second");
    eventually(Duration::from_secs(10), || {
        (x11.focus() == second).then_some(())
    })
    .expect("the second never took the keyboard");

    x11.drain();
    x11.perform(Action::Minimize);
    assert!(x11.answered(second, x11.atoms.WM_STATE), "never told");
    assert_eq!(x11.wm_state(second), [ICONIC, 0]);
    assert!(
        x11.net_wm_state(second)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );
    eventually(Duration::from_secs(10), || {
        (x11.focus() == first).then_some(())
    })
    .expect("the keyboard never went to the window left on screen");

    x11.drain();
    x11.perform(Action::CycleFocus);
    assert!(x11.answered(second, x11.atoms.WM_STATE), "never told");
    assert_eq!(x11.wm_state(second), [NORMAL, 0]);
    assert!(
        !x11.net_wm_state(second)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );
    eventually(Duration::from_secs(10), || {
        (x11.focus() == second).then_some(())
    })
    .unwrap_or_else(|| {
        panic!(
            "restored, it never took the keyboard: {:?}",
            x11.facts.read()
        )
    });

    x11.stop();
}

/// Issue #90: a window on a workspace that is not showing is not minimized,
/// and is not told it is: Wine would minimize a game merely because its
/// workspace is not the one in front.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_on_a_workspace_not_showing_stays_normal() {
    let x11 = X11::start(two_workspaces().with_person());
    let window = x11.open("game");

    x11.perform(Action::Workspace(perspicax_policy::Direction::Right));
    x11.surface_where("game", |surface| surface.off_workspace.is_some())
        .unwrap_or_else(|| panic!("never left behind: {:?}", x11.facts.read()));
    assert_eq!(x11.wm_state(window), [NORMAL, 0]);
    assert!(
        !x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );

    x11.stop();
}

/// Issue #90: a minimized window asking to be normal again, with
/// `WM_CHANGE_STATE`, is brought back and told so.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_asking_to_be_normal_again_is_restored() {
    let x11 = X11::start(Backend::headless((1280, 1024)).with_person());
    let window = x11.open("game");
    x11.change_state(window, ICONIC);
    x11.surface_where("game", |surface| !surface.mapped)
        .expect("never minimized");

    x11.change_state(window, NORMAL);
    assert!(x11.answered(window, x11.atoms.WM_STATE), "never told");
    x11.surface_where("game", |surface| surface.mapped)
        .unwrap_or_else(|| panic!("never restored: {:?}", x11.facts.read()));
    assert_eq!(x11.wm_state(window), [NORMAL, 0]);

    x11.stop();
}

/// Issue #90: with nobody at the seat a window asking to be iconified stays
/// as it is -- an agent's desk does not rearrange itself -- and is still
/// answered, with the state it is in: Wine makes no other change to a window
/// while a request of its is unanswered.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn with_nobody_at_the_seat_an_x11_window_that_iconifies_itself_stays_and_is_told_so() {
    let x11 = X11::start(Backend::headless((1280, 1024)));
    let window = x11.open("game");

    x11.change_state(window, ICONIC);
    assert!(x11.answered(window, x11.atoms.WM_STATE), "never answered");
    assert_eq!(x11.wm_state(window), [NORMAL, 0]);
    assert!(
        x11.facts
            .read()
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("game") && surface.mapped),
        "an agent's desk rearranged itself"
    );

    x11.stop();
}

/// One 1280 by 1024 monitor with two workspaces side by side.
fn two_workspaces() -> Backend {
    Backend::Headless {
        outputs: vec![perspicax_compositor::Virtual::numbered(1, (1280, 1024))],
        workspaces: perspicax_policy::Shape {
            mode: perspicax_policy::Mode::Spanning,
            grid: perspicax_policy::Grid {
                columns: 2,
                rows: 1,
                wrap: false,
            },
        },
        access: perspicax_policy::Access::open(),
        person: false,
    }
}
