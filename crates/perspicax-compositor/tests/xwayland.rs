//! X11 windows under Xwayland: who the index is told drew one, which takes
//! the keyboard, and what the window manager does and says when one asks
//! for a state.
//!
//! Provenance: the compositor's Wayland credentials name only Xwayland, so
//! the X client's pid has to come from somewhere else: first from the
//! window's own `_NET_WM_PID`, a claim, then from the X-Resource extension,
//! the X server's own word. The facts must end up saying `XRes`, with the
//! server attested separately.
//!
//! States (issues #90 and #95): the client asks as Wine does -- a client
//! message to the root, or `_NET_WM_STATE` or `WM_HINTS` written before it
//! maps -- and waits for the answer, a change to `WM_STATE` or
//! `_NET_WM_STATE`, as Wine does before it changes anything more. Most of these seat a person at the headless
//! compositor, since a window rearranges itself only for a person.
//!
//! Menus (issue #181): an application's menus bypass the window manager, made
//! here as Steam makes its own, and are left to their client. Pointing at
//! one, clicking it, cycling focus, closing a window, switching workspace, an
//! agent, and the menu's own requests never give it the keyboard or move it,
//! so the window it was opened from keeps the keyboard. The pointer's tests
//! need `capture`, and put a focus policy in force with
//! `Command::FocusPolicy`.
//!
//! The X client is this test process, speaking X11 through x11rb, so nothing
//! beyond `Xwayland` itself has to be installed. Needs `XDG_RUNTIME_DIR` and
//! `Xwayland` on the `PATH`, and is `#[ignore]`d for `ci/live-tests.sh`.

#![cfg(feature = "xwayland")]

mod common;

use std::{
    thread,
    time::{Duration, Instant},
};

use common::{Session, heard, until};
use perspicax_compositor::{
    ActError, Backend, Command, Config, Error, Facts, Host, Requests, Stop,
};
use perspicax_index::{Action as Verb, HostFacts, SurfaceFacts, judge};
use perspicax_node::{Origin, Rect, Visibility, X11Basis};
use perspicax_policy::{Action, Button, Focus, FocusModel, Mods, Opacity};
use wayland_client::Proxy as _;
use x11rb::{
    connection::Connection as _,
    properties::{WmHints, WmHintsState, WmSizeHints},
    protocol::{
        Event,
        randr::ConnectionExt as _,
        xproto::{
            AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux,
            ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, GrabStatus, MapState,
            NotifyMode, PropMode, WindowClass,
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

/// Issue #127: an X11 window that maps while the session is locked takes
/// the keyboard from nothing, as a Wayland one does not: the lock screen
/// keeps it, and with it the password typed there. Unlocked, the window has
/// it, being on top. Its surface comes after it maps, so the keyboard it
/// waits for is asked for only then: that is passed too by the time the
/// facts say it is on screen.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_that_maps_behind_the_lock_screen_takes_no_keys_from_it() {
    let x11 = X11::start("x11-behind-the-lock", Backend::headless((800, 600)));
    let (mut locker, mut queue, qh, globals) = x11.session.client();
    let ear = locker.bind_keyboard(&globals, &qh);
    until(&mut queue, &mut locker, |locker| !locker.outputs_empty());
    locker.lock(&globals, &qh);
    until(&mut queue, &mut locker, |locker| locker.locked);
    let held = locker.cover(&qh, "HEADLESS-1", 0xff20_2020).id();
    until(&mut queue, &mut locker, |_| {
        heard(&ear).entered.as_ref() == Some(&held)
    });

    let window = x11.open("behind");
    queue.roundtrip(&mut locker).expect("dispatch");
    assert_eq!(
        heard(&ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
    );
    assert_ne!(x11.focus(), window, "X gave the window the keys");

    locker.unlock();
    queue.roundtrip(&mut locker).expect("dispatch");
    eventually(Duration::from_secs(10), || {
        (x11.focus() == window).then_some(())
    })
    .unwrap_or_else(|| panic!("unlocked, it never took the keyboard: {:?}", x11.facts()));

    x11.session.stop((locker, queue));
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
        _NET_ACTIVE_WINDOW,
        _NET_WM_MOVERESIZE,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
    }
}

/// ICCCM's `WM_STATE`, as a window manager writes it.
const WITHDRAWN: u32 = 0;
const NORMAL: u32 = 1;
const ICONIC: u32 = 3;

/// `_NET_WM_STATE`'s actions.
const REMOVE: u32 = 0;
const ADD: u32 = 1;

/// EWMH's sources of a request: an application, or a pager or taskbar --
/// which Wine says it is when it asks to be active.
const APPLICATION: u32 = 1;
const PAGER: u32 = 2;

/// A panel's colour, ARGB.
const PANEL: u32 = 0xffee_8822;

/// A corner of a window that a panel across the top of the screen covers,
/// relative to the window.
const CORNER: Rect = Rect {
    x0: 5.0,
    y0: 5.0,
    x1: 25.0,
    y1: 25.0,
};

/// A window's place and size, as the facts give it.
fn rect_of(surface: &SurfaceFacts) -> (f64, f64, f64, f64) {
    let geometry = surface.geometry;
    (geometry.x0, geometry.y0, geometry.x1, geometry.y1)
}

/// A compositor with Xwayland, a person at it or not, and this test's own X
/// connection to it: an X client asking for states the way Wine does, and
/// reading back what the window manager told it.
struct X11 {
    session: Session,
    x: RustConnection,
    root: u32,
    white: u32,
    atoms: Atoms,
}

impl X11 {
    fn start(name: &str, backend: Backend) -> Self {
        let session = Session::start_with_xwayland(name, backend);
        let display = eventually(Duration::from_secs(15), || session.facts.x11_display())
            .expect("Xwayland never became ready");
        let (x, screen) = x11rb::connect(Some(&format!(":{display}"))).expect("an X connection");
        let root = x.setup().roots[screen].root;
        let white = x.setup().roots[screen].white_pixel;
        let atoms = Atoms::new(&x).expect("intern").reply().expect("the atoms");
        // The root's property changes are heard too: `_NET_ACTIVE_WINDOW`
        // is written there.
        x.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .expect("change_window_attributes")
        .check()
        .expect("the root's property changes");
        Self {
            session,
            x,
            root,
            white,
            atoms,
        }
    }

    /// What the compositor publishes now.
    fn facts(&self) -> HostFacts {
        self.session.facts.read()
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
        let window = self.create(title, state);
        self.x.map_window(window).expect("map_window");
        self.x.flush().expect("flush");
        self.surface_where(title, |surface| surface.mapped)
            .unwrap_or_else(|| panic!("{title} never mapped: {:?}", self.facts()));
        window
    }

    /// A 320 by 200 window titled `title` that maps asking to start
    /// minimized, as `xterm -iconic` and Wine ask: ICCCM's `WM_HINTS` initial
    /// state, iconic, set before it maps. Not waited for, since it may never
    /// be on screen.
    fn open_iconic(&self, title: &str) -> u32 {
        let window = self.create(title, &[]);
        WmHints {
            initial_state: Some(WmHintsState::Iconic),
            ..WmHints::new()
        }
        .set(&self.x, window)
        .expect("WM_HINTS");
        self.x.map_window(window).expect("map_window");
        self.x.flush().expect("flush");
        window
    }

    /// A window titled `title`, not yet mapped, with `_NET_WM_STATE` set to
    /// `state` if that is not empty. Its property changes are heard.
    fn create(&self, title: &str, state: &[u32]) -> u32 {
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
        window
    }

    /// A menu titled `title`, as Steam's are: a window that bypasses the
    /// window manager (override-redirect) and places itself at `at`, `size`
    /// big, typed a popup menu, with no `WM_TRANSIENT_FOR` and no word on
    /// input, so X would let it have the keyboard. Mapped, once the
    /// compositor has it on screen. Its presses are heard.
    fn menu(&self, title: &str, at: (i16, i16), size: (u16, u16)) -> u32 {
        let window = self.x.generate_id().expect("an X id");
        self.x
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                self.root,
                at.0,
                at.1,
                size.0,
                size.1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .background_pixel(self.white)
                    .override_redirect(1)
                    .event_mask(EventMask::PROPERTY_CHANGE | EventMask::BUTTON_PRESS),
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
        self.x
            .change_property32(
                PropMode::REPLACE,
                window,
                self.atoms._NET_WM_WINDOW_TYPE,
                AtomEnum::ATOM,
                &[self.atoms._NET_WM_WINDOW_TYPE_POPUP_MENU],
            )
            .expect("_NET_WM_WINDOW_TYPE");
        self.x.map_window(window).expect("map_window");
        self.x.flush().expect("flush");
        self.surface_where(title, |surface| surface.mapped)
            .unwrap_or_else(|| panic!("{title} never mapped: {:?}", self.facts()));
        window
    }

    /// Hear `window` gain and lose the keyboard, besides its property
    /// changes.
    fn hear_focus(&self, window: u32) {
        self.x
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::PROPERTY_CHANGE | EventMask::FOCUS_CHANGE),
            )
            .expect("change_window_attributes")
            .check()
            .expect("the window's focus changes");
    }

    /// Whether `window` is told, within a second, that it lost the keyboard:
    /// the `FocusOut` a client closes its menus on. Only what is heard after
    /// the last [`X11::drain`].
    fn told_focus_out(&self, window: u32) -> bool {
        eventually(Duration::from_secs(1), || {
            loop {
                match self.x.poll_for_event().ok()? {
                    Some(Event::FocusOut(out))
                        if out.event == window && out.mode == NotifyMode::NORMAL =>
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

    /// Whether the X client is told of a press of X's button `detail` on
    /// `window`. Only what is heard after the last [`X11::drain`].
    fn heard_press(&self, window: u32, detail: u8) -> bool {
        eventually(Duration::from_secs(5), || {
            loop {
                match self.x.poll_for_event().ok()? {
                    Some(Event::ButtonPress(press))
                        if press.event == window && press.detail == detail =>
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

    /// Whether the X server comes to have the pointer at `at` on `window`,
    /// in the window's own coordinates.
    fn pointer_reaches(&self, window: u32, at: (i16, i16)) -> bool {
        eventually(Duration::from_secs(5), || {
            let pointer = self.x.query_pointer(window).ok()?.reply().ok()?;
            (pointer.same_screen && (pointer.win_x, pointer.win_y) == at).then_some(())
        })
        .is_some()
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

    /// Ask for `_NET_WM_STATE` `first` (and `second`) to be added or removed.
    fn ask_state(&self, window: u32, action: u32, first: u32, second: u32) {
        // From an application, as Wine says it is.
        self.ask(
            window,
            self.atoms._NET_WM_STATE,
            [action, first, second, APPLICATION, 0],
        );
    }

    /// Ask for `window` to be made the active one, as EWMH has a client do:
    /// from `source`, with `stamp` the X time of the input behind the
    /// request, or 0 for none.
    fn activate(&self, window: u32, source: u32, stamp: u32) {
        self.ask(
            window,
            self.atoms._NET_ACTIVE_WINDOW,
            [source, stamp, 0, 0, 0],
        );
    }

    /// The X server's time now, as a client learns it: from a change of its
    /// own to a property, which the server stamps. Changes heard before it
    /// are forgotten.
    fn server_time(&self) -> u32 {
        self.drain();
        self.x
            .change_property8(
                PropMode::APPEND,
                self.root,
                AtomEnum::CUT_BUFFE_R0,
                AtomEnum::STRING,
                &[],
            )
            .expect("change_property8");
        self.x.flush().expect("flush");
        self.heard(self.root, AtomEnum::CUT_BUFFE_R0.into())
            .expect("the server never stamped the change")
    }

    /// Ask to be maximized, both ways at once as EWMH has it, or not.
    fn ask_maximized(&self, window: u32, action: u32) {
        self.ask_state(
            window,
            action,
            self.atoms._NET_WM_STATE_MAXIMIZED_VERT,
            self.atoms._NET_WM_STATE_MAXIMIZED_HORZ,
        );
    }

    /// Wait for every request sent so far to have been carried out: the
    /// window manager handles a client's requests in order, and answers this
    /// one, asking a window that is not minimized to be normal, by writing
    /// its `WM_STATE`.
    fn settled(&self, window: u32) {
        self.change_state(window, NORMAL);
        assert!(self.answered(window, self.atoms.WM_STATE), "never answered");
    }

    /// A panel across the top of the screen, `height` tall, reserving its
    /// strip, from a Wayland client of the test's own.
    fn panel(&self, height: u32) -> (common::Desk, wayland_client::EventQueue<common::Desk>) {
        let (mut desk, mut queue, qh, _) = self.session.client();
        desk.open_panel(&qh, "panel", height, PANEL);
        until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
        (desk, queue)
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
        self.heard(window, property).is_some()
    }

    /// The X time `property` on `window` changed at, once it has.
    fn heard(&self, window: u32, property: u32) -> Option<u32> {
        eventually(Duration::from_secs(5), || {
            loop {
                match self.x.poll_for_event().ok()? {
                    Some(Event::PropertyNotify(notify))
                        if notify.window == window && notify.atom == property =>
                    {
                        return Some(notify.time);
                    }
                    Some(_) => {}
                    None => return None,
                }
            }
        })
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

    /// The window the root's `_NET_ACTIVE_WINDOW` names: what an X client
    /// reads as the one with the keyboard, or `NONE`.
    fn active(&self) -> u32 {
        self.x
            .get_property(
                false,
                self.root,
                self.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .expect("get_property")
            .reply()
            .expect("_NET_ACTIVE_WINDOW")
            .value32()
            .and_then(|mut value| value.next())
            .unwrap_or(x11rb::NONE)
    }

    /// Whether the root comes to name `window` as active, and still does a
    /// moment later: Smithay writes the root there after every change of
    /// focus, a little after the change, and that must not be the last word.
    fn names_active(&self, window: u32) -> bool {
        let named = eventually(Duration::from_secs(5), || {
            (self.active() == window).then_some(())
        });
        named.is_some() && {
            thread::sleep(Duration::from_millis(300));
            self.active() == window
        }
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

    /// Whether the window titled `title` comes to be on top of the others on
    /// screen, as the facts give them, bottom to top.
    fn on_top(&self, title: &str) -> bool {
        eventually(Duration::from_secs(5), || {
            let facts = self.facts();
            let top = facts
                .surfaces()
                .iter()
                .rfind(|surface| surface.mapped && surface.title.is_some())?;
            (top.title.as_deref() == Some(title)).then_some(())
        })
        .is_some()
    }

    /// The facts of the window titled `title`, once `ready` holds of them.
    fn surface_where(
        &self,
        title: &str,
        ready: impl Fn(&SurfaceFacts) -> bool,
    ) -> Option<SurfaceFacts> {
        eventually(Duration::from_secs(10), || {
            self.facts()
                .surfaces()
                .iter()
                .find(|surface| surface.title.as_deref() == Some(title) && ready(surface))
                .cloned()
        })
    }

    fn perform(&self, action: Action) {
        self.session.perform(action);
    }

    fn stop(self) {
        self.session.stop(());
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
    let x11 = X11::start("x11-iconify", Backend::headless((1280, 1024)).with_person());
    let window = x11.open("game");

    x11.change_state(window, ICONIC);
    assert!(
        x11.answered(window, x11.atoms.WM_STATE),
        "never told it is iconic"
    );
    x11.surface_where("game", |surface| {
        !surface.mapped && surface.off_workspace.is_none()
    })
    .unwrap_or_else(|| panic!("never minimized: {:?}", x11.facts()));
    assert_eq!(x11.wm_state(window), [ICONIC, 0]);
    assert!(
        x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );
    assert!(x11.viewable(window), "minimized by state, not unmapped");

    x11.stop();
}

/// #78: consent for X11 is Xwayland's, so the person's key does not grant an
/// X11 window: every X client shares the one server and may read and drive
/// the others, and granting one would grant them all. What the key asked is
/// known to be done once the minimize after it is.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_is_not_granted_by_a_key() {
    let x11 = X11::start("x11-grant", Backend::headless((1280, 1024)).granting());
    let window = x11.open("x11");
    eventually(Duration::from_secs(10), || {
        (x11.focus() == window).then_some(())
    })
    .expect("the window never took the keyboard");

    x11.perform(Action::ToggleAgent);
    x11.perform(Action::Minimize);
    x11.surface_where("x11", |surface| !surface.mapped)
        .expect("the minimize after the key was done");
    assert_eq!(
        x11.facts().consent(),
        &perspicax_index::Consent::Granted(Vec::new()),
        "nothing granted"
    );
    x11.stop();
}

/// Issue #90: a window minimized by the person, then brought back, is told
/// both: iconic, then normal, which is the change Wine restores a window on.
/// Brought back, it takes the keyboard -- the surface Xwayland gave it is
/// still its own.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_minimized_x11_window_is_told_it_is_normal_and_takes_the_keyboard_when_restored() {
    let x11 = X11::start("x11-restore", Backend::headless((1280, 1024)).with_person());
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
    .unwrap_or_else(|| panic!("restored, it never took the keyboard: {:?}", x11.facts()));

    x11.stop();
}

/// Issue #90: a window on a workspace that is not showing is not minimized,
/// and is not told it is: Wine would minimize a game merely because its
/// workspace is not the one in front.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_on_a_workspace_not_showing_stays_normal() {
    let x11 = X11::start("x11-workspace", two_workspaces().with_person());
    let window = x11.open("game");

    x11.perform(Action::Workspace(perspicax_policy::Direction::Right));
    x11.surface_where("game", |surface| surface.off_workspace.is_some())
        .unwrap_or_else(|| panic!("never left behind: {:?}", x11.facts()));
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
    let x11 = X11::start("x11-normal", Backend::headless((1280, 1024)).with_person());
    let window = x11.open("game");
    x11.change_state(window, ICONIC);
    x11.surface_where("game", |surface| !surface.mapped)
        .expect("never minimized");

    x11.change_state(window, NORMAL);
    assert!(x11.answered(window, x11.atoms.WM_STATE), "never told");
    x11.surface_where("game", |surface| surface.mapped)
        .unwrap_or_else(|| panic!("never restored: {:?}", x11.facts()));
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
    let x11 = X11::start("x11-nobody-iconify", Backend::headless((1280, 1024)));
    let window = x11.open("game");

    x11.change_state(window, ICONIC);
    assert!(x11.answered(window, x11.atoms.WM_STATE), "never answered");
    assert_eq!(x11.wm_state(window), [NORMAL, 0]);
    assert!(
        x11.facts()
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("game") && surface.mapped),
        "an agent's desk rearranged itself"
    );

    x11.stop();
}

/// Issue #90: an X11 window asking to go fullscreen, as a Wine game does,
/// fills its whole monitor, over the panel, with no frame, and is told so.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_asking_for_fullscreen_fills_its_monitor_over_the_panel_with_no_frame() {
    let x11 = X11::start(
        "x11-fullscreen",
        Backend::headless((1280, 1024)).with_person(),
    );
    let panel = x11.panel(40);
    let window = x11.open("game");

    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    let game = x11
        .surface_where("game", |surface| {
            rect_of(surface) == (0.0, 0.0, 1280.0, 1024.0) && surface.frame.is_empty()
        })
        .unwrap_or_else(|| panic!("never filled its monitor: {:?}", x11.facts()));
    assert!(
        x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_FULLSCREEN)
    );
    eventually(Duration::from_secs(5), || {
        (judge(&x11.facts(), game.id, CORNER).visibility == Visibility::Visible).then_some(())
    })
    .unwrap_or_else(|| panic!("never over the panel: {:?}", x11.facts()));

    drop(panel);
    x11.stop();
}

/// Issue #90: an X11 window leaving fullscreen goes back where it was, at
/// the size it was, framed again.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_leaving_fullscreen_goes_back_where_it_was() {
    let x11 = X11::start(
        "x11-unfullscreen",
        Backend::headless((1280, 1024)).with_person(),
    );
    let window = x11.open("game");
    let before = x11
        .surface_where("game", |surface| surface.frame.len() == 4)
        .map(|surface| rect_of(&surface))
        .expect("framed");
    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    x11.surface_where("game", |surface| surface.frame.is_empty())
        .expect("never fullscreen");

    x11.ask_state(window, REMOVE, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    x11.surface_where("game", |surface| {
        rect_of(surface) == before && surface.frame.len() == 4
    })
    .unwrap_or_else(|| panic!("never back where it was, {before:?}: {:?}", x11.facts()));
    assert!(
        !x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_FULLSCREEN)
    );

    x11.stop();
}

/// Issue #90: a fullscreen X11 window asking for another size keeps its
/// monitor, and is told the size it has. Its size used to be granted, which
/// shrank a game in a corner of a monitor it was still meant to cover.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_fullscreen_x11_window_asking_for_another_size_keeps_its_monitor() {
    let x11 = X11::start(
        "x11-fullscreen-size",
        Backend::headless((1280, 1024)).with_person(),
    );
    let window = x11.open("game");
    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    x11.surface_where("game", |surface| {
        rect_of(surface) == (0.0, 0.0, 1280.0, 1024.0)
    })
    .expect("never fullscreen");

    x11.x
        .configure_window(window, &ConfigureWindowAux::new().width(320).height(200))
        .expect("configure_window");
    x11.settled(window);
    let geometry = x11
        .x
        .get_geometry(window)
        .expect("get_geometry")
        .reply()
        .expect("the geometry");
    assert_eq!((geometry.width, geometry.height), (1280, 1024));
    let game = x11.surface_where("game", |_| true).expect("still there");
    assert_eq!(rect_of(&game), (0.0, 0.0, 1280.0, 1024.0));

    x11.stop();
}

/// Issue #90: an X11 window asking to be maximized, both ways at once, fills
/// what the panel leaves under a titlebar; asking back puts it where it was.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_asking_to_be_maximized_fills_the_work_area_and_asking_back_restores_it() {
    let x11 = X11::start(
        "x11-maximize",
        Backend::headless((1280, 1024)).with_person(),
    );
    let panel = x11.panel(40);
    let window = x11.open("game");
    let before = x11
        .surface_where("game", |surface| surface.frame.len() == 4)
        .map(|surface| rect_of(&surface))
        .expect("framed");

    x11.ask_maximized(window, ADD);
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    x11.surface_where("game", |surface| {
        surface.frame.len() == 1 && rect_of(surface) == (0.0, 64.0, 1280.0, 1024.0)
    })
    .unwrap_or_else(|| panic!("never maximized under the panel: {:?}", x11.facts()));
    let state = x11.net_wm_state(window);
    assert!(state.contains(&x11.atoms._NET_WM_STATE_MAXIMIZED_VERT));
    assert!(state.contains(&x11.atoms._NET_WM_STATE_MAXIMIZED_HORZ));

    x11.ask_maximized(window, REMOVE);
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    x11.surface_where("game", |surface| {
        surface.frame.len() == 4 && rect_of(surface) == before
    })
    .unwrap_or_else(|| panic!("never back where it was, {before:?}: {:?}", x11.facts()));

    drop(panel);
    x11.stop();
}

/// Issue #90: fullscreen is over maximized, as EWMH has it: a maximized X11
/// window made fullscreen is maximized again when it leaves fullscreen.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_maximized_x11_window_made_fullscreen_is_maximized_again_when_it_leaves() {
    let x11 = X11::start(
        "x11-maximized-fullscreen",
        Backend::headless((1280, 1024)).with_person(),
    );
    let panel = x11.panel(40);
    let window = x11.open("game");
    x11.ask_maximized(window, ADD);
    x11.surface_where("game", |surface| surface.frame.len() == 1)
        .expect("never maximized");

    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    x11.surface_where("game", |surface| {
        surface.frame.is_empty() && rect_of(surface) == (0.0, 0.0, 1280.0, 1024.0)
    })
    .unwrap_or_else(|| panic!("never fullscreen: {:?}", x11.facts()));
    x11.ask_state(window, REMOVE, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    x11.surface_where("game", |surface| {
        surface.frame.len() == 1 && rect_of(surface) == (0.0, 64.0, 1280.0, 1024.0)
    })
    .unwrap_or_else(|| panic!("never maximized again: {:?}", x11.facts()));
    let state = x11.net_wm_state(window);
    assert!(state.contains(&x11.atoms._NET_WM_STATE_MAXIMIZED_VERT));
    assert!(!state.contains(&x11.atoms._NET_WM_STATE_FULLSCREEN));

    drop(panel);
    x11.stop();
}

/// Issue #90: snapping a fullscreen X11 window to half its monitor takes it
/// out of fullscreen, and it is told so: framed by a half, it is not
/// fullscreen any more.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn snapping_a_fullscreen_x11_window_takes_it_out_of_fullscreen() {
    let x11 = X11::start(
        "x11-snap-fullscreen",
        Backend::headless((1280, 1024)).with_person(),
    );
    let window = x11.open("game");
    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    x11.surface_where("game", |surface| surface.frame.is_empty())
        .expect("never fullscreen");

    x11.perform(Action::Snap(perspicax_policy::Direction::Left));
    x11.surface_where("game", |surface| {
        surface.geometry.x1 <= 640.0 && surface.frame.len() == 4
    })
    .unwrap_or_else(|| panic!("never snapped to the left half: {:?}", x11.facts()));
    assert!(
        !x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_FULLSCREEN)
    );

    x11.stop();
}

/// Issue #90: a window asking for a state it already has is still answered.
/// Nothing changes, but Wine waits for the answer all the same, and changes
/// nothing more about the window until it comes.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_asking_again_for_a_state_it_has_is_still_answered() {
    let x11 = X11::start(
        "x11-asked-again",
        Backend::headless((1280, 1024)).with_person(),
    );
    let window = x11.open("game");
    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    x11.surface_where("game", |surface| surface.frame.is_empty())
        .expect("never fullscreen");

    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "asked again, never answered"
    );
    assert!(
        x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_FULLSCREEN)
    );

    x11.stop();
}

/// Issue #90: a window on a workspace that is not showing, asking to go
/// fullscreen, is told it is, and fills its monitor when its workspace is
/// shown -- not before.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_on_a_workspace_not_showing_that_asks_for_fullscreen_stays_there_until_shown() {
    let x11 = X11::start("x11-parked-fullscreen", two_workspaces().with_person());
    let window = x11.open("game");
    x11.perform(Action::Workspace(perspicax_policy::Direction::Right));
    x11.surface_where("game", |surface| surface.off_workspace.is_some())
        .expect("never left behind");

    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    assert!(
        x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_FULLSCREEN)
    );
    x11.settled(window);
    assert!(
        x11.surface_where("game", |surface| surface.off_workspace.is_some())
            .is_some_and(|surface| !surface.mapped),
        "shown on a workspace not showing"
    );

    x11.perform(Action::Workspace(perspicax_policy::Direction::Left));
    x11.surface_where("game", |surface| {
        surface.mapped && rect_of(surface) == (0.0, 0.0, 1280.0, 1024.0)
    })
    .unwrap_or_else(|| panic!("never filled its monitor once shown: {:?}", x11.facts()));

    x11.stop();
}

/// Issue #90: with nobody at the seat a window asking to go fullscreen stays
/// as it was placed, and is told so.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn with_nobody_at_the_seat_an_x11_window_asking_for_fullscreen_stays_as_placed_and_is_answered() {
    let x11 = X11::start("x11-nobody-fullscreen", Backend::headless((1280, 1024)));
    let window = x11.open("game");
    let before = x11
        .surface_where("game", |surface| surface.frame.len() == 4)
        .map(|surface| rect_of(&surface))
        .expect("framed");

    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    assert!(
        !x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_FULLSCREEN)
    );
    x11.settled(window);
    let game = x11.surface_where("game", |_| true).expect("still there");
    assert_eq!(rect_of(&game), before, "an agent's desk rearranged itself");

    x11.stop();
}

/// Issue #90: a window that maps already asking to be fullscreen -- set in
/// its own `_NET_WM_STATE` before it maps, as EWMH lets a withdrawn window
/// and as Wine does for a game that starts fullscreen -- opens filling its
/// monitor. And it is still told it is fullscreen once it has the keyboard:
/// the window manager's first write of `_NET_WM_STATE` used to replace the
/// request with `FOCUSED` alone.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_that_maps_fullscreen_opens_filling_its_monitor_and_keeps_saying_so_once_focused() {
    let x11 = X11::start(
        "x11-maps-fullscreen",
        Backend::headless((1280, 1024)).with_person(),
    );
    let window = x11.open_with_state("game", &[x11.atoms._NET_WM_STATE_FULLSCREEN]);

    x11.surface_where("game", |surface| {
        rect_of(surface) == (0.0, 0.0, 1280.0, 1024.0) && surface.frame.is_empty()
    })
    .unwrap_or_else(|| panic!("never filled its monitor: {:?}", x11.facts()));
    eventually(Duration::from_secs(10), || {
        (x11.focus() == window).then_some(())
    })
    .expect("never took the keyboard");
    let state = eventually(Duration::from_secs(5), || {
        let state = x11.net_wm_state(window);
        state
            .contains(&x11.atoms._NET_WM_STATE_FOCUSED)
            .then_some(state)
    })
    .expect("never told it is focused");
    assert!(
        state.contains(&x11.atoms._NET_WM_STATE_FULLSCREEN),
        "told it is focused, and no longer fullscreen: {state:?}"
    );

    x11.stop();
}

/// Issue #90: a window that maps already asking to be maximized, both ways,
/// opens maximized.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_that_maps_maximized_opens_maximized() {
    let x11 = X11::start(
        "x11-maps-maximized",
        Backend::headless((1280, 1024)).with_person(),
    );
    let window = x11.open_with_state(
        "game",
        &[
            x11.atoms._NET_WM_STATE_MAXIMIZED_VERT,
            x11.atoms._NET_WM_STATE_MAXIMIZED_HORZ,
        ],
    );

    x11.surface_where("game", |surface| {
        surface.frame.len() == 1 && rect_of(surface) == (0.0, 24.0, 1280.0, 1024.0)
    })
    .unwrap_or_else(|| panic!("never maximized: {:?}", x11.facts()));
    let state = x11.net_wm_state(window);
    assert!(state.contains(&x11.atoms._NET_WM_STATE_MAXIMIZED_VERT));
    assert!(state.contains(&x11.atoms._NET_WM_STATE_MAXIMIZED_HORZ));

    x11.stop();
}

/// Issue #90: with nobody at the seat a window that maps asking to be
/// fullscreen is placed as any other is, framed, at its own size.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn with_nobody_at_the_seat_an_x11_window_that_maps_fullscreen_is_placed_as_any_other() {
    let x11 = X11::start(
        "x11-nobody-maps-fullscreen",
        Backend::headless((1280, 1024)),
    );
    x11.open_with_state("game", &[x11.atoms._NET_WM_STATE_FULLSCREEN]);

    let game = x11
        .surface_where("game", |surface| surface.frame.len() == 4)
        .unwrap_or_else(|| panic!("never framed: {:?}", x11.facts()));
    let (x0, y0, x1, y1) = rect_of(&game);
    assert_eq!((x1 - x0, y1 - y0), (320.0, 200.0), "not at its own size");

    x11.stop();
}

/// Issue #95: a window that maps asking to start minimized -- ICCCM's
/// `WM_HINTS` initial state, as `xterm -iconic` and Wine set it -- starts
/// minimized, leaving the keyboard where it was, and is told it is iconic,
/// which Wine waits for. Its frame is mapped all the same, so it has its
/// surface when it is brought back, and takes the keyboard then.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_that_maps_iconic_starts_minimized_and_is_told_so() {
    let x11 = X11::start(
        "x11-maps-iconic",
        Backend::headless((1280, 1024)).with_person(),
    );
    let first = x11.open("first");
    eventually(Duration::from_secs(10), || {
        (x11.focus() == first).then_some(())
    })
    .expect("the first never took the keyboard");

    let game = x11.open_iconic("game");
    eventually(Duration::from_secs(5), || {
        (x11.wm_state(game) == [ICONIC, 0]).then_some(())
    })
    .unwrap_or_else(|| panic!("never told it is iconic: {:?}", x11.wm_state(game)));
    assert!(
        x11.net_wm_state(game)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );
    assert!(x11.viewable(game), "minimized by state, not unmapped");
    x11.surface_where("game", |surface| {
        !surface.mapped && surface.off_workspace.is_none()
    })
    .unwrap_or_else(|| panic!("not minimized: {:?}", x11.facts()));
    assert_eq!(
        x11.focus(),
        first,
        "took the keyboard as it started minimized"
    );
    assert!(x11.names_active(first));

    x11.drain();
    x11.perform(Action::CycleFocus);
    x11.surface_where("game", |surface| surface.mapped)
        .unwrap_or_else(|| panic!("never brought back: {:?}", x11.facts()));
    assert_eq!(x11.wm_state(game), [NORMAL, 0]);
    eventually(Duration::from_secs(10), || {
        (x11.focus() == game).then_some(())
    })
    .unwrap_or_else(|| panic!("restored, it never took the keyboard: {:?}", x11.facts()));

    x11.stop();
}

/// Issue #95: with nobody at the seat a window that maps asking to start
/// minimized is mapped as any other is, and told it is normal: an agent's
/// desk does not rearrange itself.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn with_nobody_at_the_seat_an_x11_window_that_maps_iconic_is_placed_as_any_other() {
    let x11 = X11::start("x11-nobody-maps-iconic", Backend::headless((1280, 1024)));
    let game = x11.open_iconic("game");

    x11.surface_where("game", |surface| surface.mapped)
        .unwrap_or_else(|| panic!("never mapped: {:?}", x11.facts()));
    assert_eq!(x11.wm_state(game), [NORMAL, 0]);
    assert!(
        !x11.net_wm_state(game)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );

    x11.stop();
}

/// Whether a window is told it has the keyboard, once the compositor has had
/// time to say so either way.
fn says_focused(x11: &X11, window: u32, focused: bool) -> bool {
    eventually(Duration::from_secs(5), || {
        (x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_FOCUSED)
            == focused)
            .then_some(())
    })
    .is_some()
}

/// Issue #90: only the window with the keyboard is told it is focused. A
/// window minimized while it had the keyboard used to keep saying so, beside
/// the one that took it -- Steam, the EVE launcher and EVE all at once.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn only_the_x11_window_with_the_keyboard_says_it_is_focused_after_another_is_minimized() {
    let x11 = X11::start(
        "x11-focused-minimized",
        Backend::headless((1280, 1024)).with_person(),
    );
    let first = x11.open("first");
    let second = x11.open("second");
    eventually(Duration::from_secs(10), || {
        (x11.focus() == second).then_some(())
    })
    .expect("the second never took the keyboard");
    assert!(says_focused(&x11, second, true));

    x11.perform(Action::Minimize);
    eventually(Duration::from_secs(10), || {
        (x11.focus() == first).then_some(())
    })
    .expect("the keyboard never went to the first");
    assert!(says_focused(&x11, first, true), "the first is not told");
    assert!(
        says_focused(&x11, second, false),
        "minimized, the second still says it is focused"
    );

    x11.stop();
}

/// Issue #90: a window left on a workspace no longer showing is not told it
/// is still focused.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_left_on_another_workspace_no_longer_says_it_is_focused() {
    let x11 = X11::start("x11-focused-workspace", two_workspaces().with_person());
    let window = x11.open("game");
    assert!(says_focused(&x11, window, true));

    x11.perform(Action::Workspace(perspicax_policy::Direction::Right));
    x11.surface_where("game", |surface| surface.off_workspace.is_some())
        .expect("never left behind");
    assert!(
        says_focused(&x11, window, false),
        "left behind, it still says it is focused"
    );

    x11.stop();
}

/// Issue #90: a window its client withdraws is told it is withdrawn, as
/// ICCCM has a window manager do -- Wine maps a window again only once it has
/// seen that -- and no longer says it is focused.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_withdrawn_x11_window_is_told_it_is_withdrawn_and_no_longer_says_it_is_focused() {
    let x11 = X11::start(
        "x11-withdrawn",
        Backend::headless((1280, 1024)).with_person(),
    );
    let first = x11.open("first");
    let second = x11.open("second");
    assert!(says_focused(&x11, second, true));

    x11.drain();
    x11.x.unmap_window(second).expect("unmap_window");
    x11.x.flush().expect("flush");
    assert!(
        x11.answered(second, x11.atoms.WM_STATE),
        "never told it is withdrawn"
    );
    assert_eq!(x11.wm_state(second), [WITHDRAWN, 0]);
    assert!(says_focused(&x11, second, false));
    assert!(says_focused(&x11, first, true), "the first never took over");

    x11.stop();
}

/// Issue #90: a fullscreen window withdrawn and mapped again comes back as a
/// plain window, framed, as a new one would: the window manager clears the
/// states it gave a window when it is withdrawn.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_withdrawn_from_fullscreen_maps_again_as_a_plain_window() {
    let x11 = X11::start(
        "x11-withdrawn-fullscreen",
        Backend::headless((1280, 1024)).with_person(),
    );
    let window = x11.open("game");
    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    x11.surface_where("game", |surface| surface.frame.is_empty())
        .expect("never fullscreen");

    x11.drain();
    x11.x.unmap_window(window).expect("unmap_window");
    x11.x.flush().expect("flush");
    assert!(x11.answered(window, x11.atoms.WM_STATE), "never told");
    x11.x.map_window(window).expect("map_window");
    x11.x.flush().expect("flush");
    x11.surface_where("game", |surface| surface.mapped && surface.frame.len() == 4)
        .unwrap_or_else(|| panic!("never mapped again, framed: {:?}", x11.facts()));
    assert!(
        !x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_FULLSCREEN)
    );

    x11.stop();
}

/// Issue #91: the root's `_NET_ACTIVE_WINDOW` names the X11 window with the
/// keyboard, and none once a Wayland window has it, as EWMH has a window
/// manager say. Smithay wrote the root there after every change of focus, so
/// a client reading it -- Wine, which goes by it for which window is in
/// front -- read that the desktop was.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn the_root_names_the_x11_window_with_the_keyboard_and_none_once_a_wayland_one_has_it() {
    let x11 = X11::start("x11-active", Backend::headless((1280, 1024)).with_person());
    let first = x11.open("first");
    assert!(x11.names_active(first), "the first is not named");
    let second = x11.open("second");
    assert!(x11.names_active(second), "the second is not named");

    let (mut desk, mut queue, qh, _) = x11.session.client();
    desk.open_window(&qh, "wayland", "perspicax.test.wayland");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    assert!(
        x11.names_active(x11rb::NONE),
        "still names {:#x} with a Wayland window at the keyboard",
        x11.active()
    );

    x11.stop();
}

/// Issue #91: the keyboard moving between X11 windows, here because the one
/// with it is minimized, moves the name with it -- and it stays, after
/// Smithay has written the root there.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn the_root_goes_on_naming_the_x11_window_with_the_keyboard_after_focus_moves() {
    let x11 = X11::start(
        "x11-active-moves",
        Backend::headless((1280, 1024)).with_person(),
    );
    let first = x11.open("first");
    let second = x11.open("second");
    assert!(x11.names_active(second), "the second is not named");

    x11.perform(Action::Minimize);
    eventually(Duration::from_secs(10), || {
        (x11.focus() == first).then_some(())
    })
    .expect("the keyboard never went to the first");
    assert!(
        x11.names_active(first),
        "names {:#x}, not the first",
        x11.active()
    );

    x11.stop();
}

/// Issue #91: with nobody at the seat, the keyboard does not move on when
/// the window with it is withdrawn, and the root names no window rather than
/// one that is gone.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn with_nobody_at_the_seat_the_root_names_no_window_once_the_active_one_is_withdrawn() {
    let x11 = X11::start("x11-nobody-active", Backend::headless((1280, 1024)));
    let first = x11.open("first");
    let second = x11.open("second");
    assert!(x11.names_active(second), "the second is not named");

    x11.x.unmap_window(second).expect("unmap_window");
    x11.x.flush().expect("flush");
    assert!(x11.answered(second, x11.atoms.WM_STATE), "never withdrawn");
    assert!(
        x11.names_active(x11rb::NONE),
        "names {:#x}, though the second is gone and the first never took the keyboard",
        x11.active()
    );
    assert_ne!(
        x11.focus(),
        first,
        "the keyboard moved on with nobody at the seat"
    );

    x11.stop();
}

/// Issue #91: a window asking to be active with no input behind the request
/// -- Wine asks so, as a pager, at time 0 -- is raised, but does not take
/// the keyboard from the window that has it, as an xdg window may not
/// without a token from the person's input. Answered, by the root naming the
/// active window again: Wine holds back its idea of which window is in front
/// until it is.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_asking_to_be_active_without_input_is_raised_but_not_focused() {
    let x11 = X11::start(
        "x11-activate-raised",
        Backend::headless((1280, 1024)).with_person(),
    );
    let first = x11.open("first");
    let second = x11.open("second");
    assert!(x11.names_active(second), "the second is not named");

    x11.activate(first, PAGER, 0);
    assert!(
        x11.answered(x11.root, x11.atoms._NET_ACTIVE_WINDOW),
        "never answered"
    );
    assert!(x11.on_top("first"), "never raised: {:?}", x11.facts());
    assert_eq!(
        x11.focus(),
        second,
        "took the keyboard with no input behind it"
    );
    assert!(x11.names_active(second));

    x11.stop();
}

/// Issue #91: a minimized window asking to be active with no input behind
/// the request -- a Wine program bringing itself back -- comes back, and is
/// told it is normal again, but does not take the keyboard.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_minimized_x11_window_asking_to_be_active_without_input_comes_back() {
    let x11 = X11::start(
        "x11-activate-minimized",
        Backend::headless((1280, 1024)).with_person(),
    );
    let first = x11.open("first");
    let second = x11.open("second");
    x11.perform(Action::Minimize);
    x11.surface_where("second", |surface| !surface.mapped)
        .expect("never minimized");
    eventually(Duration::from_secs(10), || {
        (x11.focus() == first).then_some(())
    })
    .expect("the keyboard never went to the first");

    x11.activate(second, PAGER, 0);
    x11.surface_where("second", |surface| surface.mapped)
        .unwrap_or_else(|| panic!("never came back: {:?}", x11.facts()));
    assert_eq!(x11.wm_state(second), [NORMAL, 0]);
    assert!(x11.on_top("second"), "came back underneath");
    assert_eq!(
        x11.focus(),
        first,
        "took the keyboard with no input behind it"
    );
    assert!(x11.names_active(first));

    x11.stop();
}

/// Issue #91: a window asking to be active with fresh input behind the
/// request -- a program the person just started, raising its first
/// instance -- takes the keyboard, as an xdg window does with a fresh token.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_asking_to_be_active_with_fresh_input_takes_the_keyboard() {
    let x11 = X11::start(
        "x11-activate-input",
        Backend::headless((1280, 1024)).with_person(),
    );
    let first = x11.open("first");
    let second = x11.open("second");
    assert!(x11.names_active(second), "the second is not named");

    x11.activate(first, APPLICATION, x11.server_time());
    eventually(Duration::from_secs(10), || {
        (x11.focus() == first).then_some(())
    })
    .unwrap_or_else(|| panic!("never took the keyboard: {:?}", x11.facts()));
    assert!(x11.on_top("first"), "never raised");
    assert!(x11.names_active(first), "not named");

    x11.stop();
}

/// Issue #91: a minimized window on a workspace that is not showing, asking
/// to be active with no input behind the request, is minimized no longer
/// but waits on its own workspace: showing that one would take the keyboard
/// from the window that has it.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_on_a_workspace_not_showing_asking_to_be_active_without_input_waits_there() {
    let x11 = X11::start("x11-activate-elsewhere", two_workspaces().with_person());
    let window = x11.open("game");
    x11.perform(Action::Minimize);
    x11.surface_where("game", |surface| !surface.mapped)
        .expect("never minimized");
    x11.perform(Action::Workspace(perspicax_policy::Direction::Right));

    x11.activate(window, PAGER, 0);
    assert!(x11.answered(window, x11.atoms.WM_STATE), "never told");
    assert_eq!(x11.wm_state(window), [NORMAL, 0]);
    x11.surface_where("game", |surface| {
        !surface.mapped && surface.off_workspace.is_some()
    })
    .unwrap_or_else(|| panic!("not left on its own workspace: {:?}", x11.facts()));
    assert_ne!(
        x11.focus(),
        window,
        "took the keyboard with no input behind it"
    );

    x11.stop();
}

/// Issue #91: a window on a workspace that is not showing, asking to be
/// active with fresh input behind the request, is shown there with the
/// keyboard, as an xdg window with a fresh token is.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_on_a_workspace_not_showing_asking_to_be_active_with_fresh_input_is_shown() {
    let x11 = X11::start("x11-activate-shown", two_workspaces().with_person());
    let window = x11.open("game");
    x11.perform(Action::Workspace(perspicax_policy::Direction::Right));
    x11.surface_where("game", |surface| surface.off_workspace.is_some())
        .expect("never left behind");

    x11.activate(window, APPLICATION, x11.server_time());
    x11.surface_where("game", |surface| surface.mapped)
        .unwrap_or_else(|| panic!("its workspace never showed: {:?}", x11.facts()));
    eventually(Duration::from_secs(10), || {
        (x11.focus() == window).then_some(())
    })
    .unwrap_or_else(|| panic!("never took the keyboard: {:?}", x11.facts()));

    x11.stop();
}

/// Issue #91: with nobody at the seat a window asking to be active stays as
/// it is, even with input behind the request -- an agent's desk does not
/// rearrange itself -- and is still answered.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn with_nobody_at_the_seat_an_x11_window_asking_to_be_active_is_answered_and_nothing_moves() {
    let x11 = X11::start("x11-nobody-activate", Backend::headless((1280, 1024)));
    let first = x11.open("first");
    let second = x11.open("second");
    assert!(x11.names_active(second), "the second is not named");

    x11.activate(first, APPLICATION, x11.server_time());
    assert!(
        x11.answered(x11.root, x11.atoms._NET_ACTIVE_WINDOW),
        "never answered"
    );
    assert!(x11.on_top("second"), "an agent's desk rearranged itself");
    assert_eq!(x11.focus(), second, "an agent's desk moved the keyboard");
    assert!(x11.names_active(second));

    x11.stop();
}

/// Issue #91: a window asking with `_NET_WM_STATE_HIDDEN` is minimized, and
/// asking back brings it back, as with `WM_CHANGE_STATE`. Smithay dropped the
/// request, unanswered.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_asking_to_be_hidden_is_minimized_and_asking_back_restores_it() {
    let x11 = X11::start("x11-hidden", Backend::headless((1280, 1024)).with_person());
    let window = x11.open("game");

    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_HIDDEN, 0);
    x11.surface_where("game", |surface| {
        !surface.mapped && surface.off_workspace.is_none()
    })
    .unwrap_or_else(|| panic!("never minimized: {:?}", x11.facts()));
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    assert_eq!(x11.wm_state(window), [ICONIC, 0]);
    assert!(
        x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );

    x11.ask_state(window, REMOVE, x11.atoms._NET_WM_STATE_HIDDEN, 0);
    x11.surface_where("game", |surface| surface.mapped)
        .unwrap_or_else(|| panic!("never brought back: {:?}", x11.facts()));
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    assert_eq!(x11.wm_state(window), [NORMAL, 0]);
    assert!(
        !x11.net_wm_state(window)
            .contains(&x11.atoms._NET_WM_STATE_HIDDEN)
    );

    x11.stop();
}

/// Issue #91: with nobody at the seat a window asking to be hidden stays as
/// it is, and is still answered.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn with_nobody_at_the_seat_an_x11_window_asking_to_be_hidden_stays_and_is_answered() {
    let x11 = X11::start("x11-nobody-hidden", Backend::headless((1280, 1024)));
    let window = x11.open("game");

    x11.ask_state(window, ADD, x11.atoms._NET_WM_STATE_HIDDEN, 0);
    assert!(
        x11.answered(window, x11.atoms._NET_WM_STATE),
        "never answered"
    );
    assert_eq!(x11.wm_state(window), [NORMAL, 0]);
    assert!(
        x11.facts()
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("game") && surface.mapped),
        "an agent's desk rearranged itself"
    );

    x11.stop();
}

/// Issue #91: a minimized window on a workspace that is not showing, asking
/// with `WM_CHANGE_STATE` to be normal again, is brought back but waits on
/// its own workspace: showing that one would take the keyboard from the
/// window that has it, with no input behind the request.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_on_a_workspace_not_showing_asking_to_be_normal_waits_there() {
    let x11 = X11::start("x11-normal-elsewhere", two_workspaces().with_person());
    let window = x11.open("game");
    x11.perform(Action::Minimize);
    x11.surface_where("game", |surface| !surface.mapped)
        .expect("never minimized");
    x11.perform(Action::Workspace(perspicax_policy::Direction::Right));

    x11.change_state(window, NORMAL);
    assert!(x11.answered(window, x11.atoms.WM_STATE), "never told");
    assert_eq!(x11.wm_state(window), [NORMAL, 0]);
    x11.surface_where("game", |surface| {
        !surface.mapped && surface.off_workspace.is_some()
    })
    .unwrap_or_else(|| panic!("not left on its own workspace: {:?}", x11.facts()));
    assert_ne!(
        x11.focus(),
        window,
        "took the keyboard with no input behind it"
    );

    x11.stop();
}

/// One 1280 by 1024 monitor with two workspaces side by side.
/// Issue #106: an X client's `WM_NORMAL_HINTS` may say its window is at
/// least, or at most, a negative size, and smithay builds a `Size` of the
/// hints as they are, which a debug build asserts against. A person resizing
/// such a window resizes it as one that gave no limits.
#[cfg(feature = "capture")]
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_whose_size_hints_are_negative_resizes_as_one_with_none() {
    let x11 = X11::start(
        "x11-negative-hints",
        Backend::headless((1280, 1024)).with_person(),
    );
    let window = x11.create("hinted", &[]);
    let mut hints = WmSizeHints::new();
    hints.min_size = Some((-1, -1));
    hints.max_size = Some((-5, -5));
    hints
        .set_normal_hints(&x11.x, window)
        .expect("WM_NORMAL_HINTS")
        .check()
        .expect("WM_NORMAL_HINTS set");
    x11.x
        .change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::BUTTON_PRESS),
        )
        .expect("change_window_attributes")
        .check()
        .expect("the window's presses");
    x11.x.map_window(window).expect("map_window");
    x11.x.flush().expect("flush");
    let hinted = x11
        .surface_where("hinted", |surface| surface.mapped)
        .expect("mapped");
    let (x0, y0, x1, _) = rect_of(&hinted);

    // The person's button down on the window, and once the X client has
    // seen it, a request to be resized from its bottom right corner, as a
    // client drawing its own frame asks.
    x11.drain();
    x11.session.command(Command::Press {
        at: (x0 as i32 + 20, y0 as i32 + 20),
        button: Button::Left,
        mods: Mods::default(),
    });
    assert!(
        eventually(Duration::from_secs(5), || {
            matches!(x11.x.poll_for_event(), Ok(Some(Event::ButtonPress(_)))).then_some(())
        })
        .is_some(),
        "the X client never saw the press"
    );
    let bottom_right = 4;
    x11.ask(
        window,
        x11.atoms._NET_WM_MOVERESIZE,
        [x0 as u32 + 20, y0 as u32 + 20, bottom_right, 1, 1],
    );

    // Pulled out a little at a time until the resize has begun: the window
    // grows, with no limit to stop it.
    let grown = eventually(Duration::from_secs(10), || {
        x11.session.command(Command::Motion { by: (8, 6) });
        thread::sleep(Duration::from_millis(50));
        let facts = x11.facts();
        let hinted = facts
            .surfaces()
            .iter()
            .find(|surface| surface.title.as_deref() == Some("hinted"))?;
        (rect_of(hinted).2 > x1 + 30.0).then_some(())
    });
    x11.session.command(Command::Release {
        button: Button::Left,
    });
    assert!(grown.is_some(), "the window never grew: {:?}", x11.facts());
    assert!(!x11.session.ended(), "the compositor is still running");

    x11.stop();
}

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
        opacity: Default::default(),
        agent: Default::default(),
    }
}

/// Issue #108: Wine's `ClipCursor` is a pointer grab confined to a window,
/// and Xwayland turns that into a `zwp_confined_pointer_v1` on the window's
/// surface -- given that the compositor offers it. Then the person's mouse
/// cannot take the pointer off the X game onto the Wayland window beside it.
///
/// Two things Xwayland does are waited out, not retried (issue #120):
/// - A grab made while Xwayland counts the X cursor hidden is held with a
///   lock, its stand-in for a game's mouselook, not a confine. And it shows
///   the cursor of a window the pointer has just come onto only once the
///   pointer moves on it, or 5 ms later. So the pointer arrives a pixel off
///   and, once X has it there, moves onto its spot.
/// - Xwayland asks for the confine on a connection of its own, after it has
///   answered the grab, and gives it up if the pointer leaves the window
///   first. Here nothing would make it ask again. So once it has the grab,
///   the game draws a mark. By the time the compositor has the mark, it has
///   the confine, asked for before the mark on the same connection.
///
/// A push that gets out after that is a failure, as it is without the
/// protocol, when none ever holds.
#[cfg(feature = "capture")]
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_pointer_grab_confined_to_its_window_keeps_the_mouse_in_it() {
    let x11 = X11::start("x11-confine", Backend::headless((1280, 800)).with_person());
    // A Wayland window at the top left, and the X game cascaded over it,
    // with the keyboard.
    let (mut desk, mut queue, qh, globals) = x11.session.client();
    desk.bind_pointer(&globals, &qh);
    desk.open_window(&qh, "beside", "beside");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    x11.surface_where("beside", |surface| surface.mapped)
        .expect("the Wayland window on screen");
    let window = x11.open("game");
    assert!(
        eventually(Duration::from_secs(5), || {
            (focused_last(&x11.session.facts).as_deref() == Some("game")).then_some(())
        })
        .is_some(),
        "the X game never took the keyboard"
    );
    let game = x11
        .surface_where("game", |surface| surface.mapped)
        .expect("mapped");
    let (x0, y0, _, _) = rect_of(&game);
    assert!(
        x0 > 0.0,
        "the game must leave some of the Wayland window beside it: {x0}"
    );

    // Where on the game the X server has the pointer, once it has it there.
    let pointed = |x11: &X11| {
        let pointer = x11.x.query_pointer(window).ok()?.reply().ok()?;
        pointer
            .same_screen
            .then_some((pointer.win_x, pointer.win_y))
    };
    let reaches = |x11: &X11, at: (i16, i16)| {
        eventually(Duration::from_secs(5), || {
            pointed(x11).filter(|seen| *seen == at)
        })
    };

    // The mouse onto the game, from the origin where the pointer starts, a
    // pixel right of its spot. Once X has it there, the pixel back: a move
    // on the window, which shows X's cursor.
    x11.session.command(Command::Motion {
        by: (x0 as i32 + 21, y0 as i32 + 20),
    });
    assert_eq!(
        reaches(&x11, (21, 20)),
        Some((21, 20)),
        "the X server never saw the pointer arrive"
    );
    x11.session.command(Command::Motion { by: (-1, 0) });
    assert_eq!(
        reaches(&x11, (20, 20)),
        Some((20, 20)),
        "the X server never saw the pointer move on the game"
    );
    // Whatever the arrival told the Wayland window has been heard. From
    // here on, the pointer arriving there is the pointer getting out.
    queue.roundtrip(&mut desk).expect("a roundtrip");
    desk.arrived = None;

    let grab = x11
        .x
        .grab_pointer(
            false,
            window,
            EventMask::POINTER_MOTION,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            window,
            x11rb::NONE,
            x11rb::CURRENT_TIME,
        )
        .expect("grab_pointer")
        .reply()
        .expect("the grab's answer");
    assert_eq!(grab.status, GrabStatus::SUCCESS);

    // The game's mark: a patch of its background drawn again, where nothing
    // else draws, so damage there can only be this.
    let (x, y, width, height) = (3, 5, 7, 11);
    x11.x
        .clear_area(false, window, x, y, width, height)
        .expect("clear_area");
    x11.x.flush().expect("flush");
    let mark = Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(x) + f64::from(width),
        f64::from(y) + f64::from(height),
    );
    x11.surface_where("game", |game| {
        game.damage.iter().any(|(_, drawn)| *drawn == mark)
    })
    .unwrap_or_else(|| panic!("the game's mark never arrived: {:?}", x11.facts()));

    // Far to the left, over the Wayland window: the pointer stops at the
    // game's left edge, and however far the mouse goes on, slides along it.
    x11.session.command(Command::Motion { by: (-1000, 0) });
    assert_eq!(
        reaches(&x11, (0, 20)),
        Some((0, 20)),
        "the pointer left the X game"
    );
    x11.session.command(Command::Motion { by: (-1000, 50) });
    assert_eq!(
        reaches(&x11, (0, 70)),
        Some((0, 70)),
        "slid down along the left edge"
    );
    queue.roundtrip(&mut desk).expect("a roundtrip");
    assert_eq!(desk.arrived, None, "the Wayland window had the pointer");

    drop((desk, queue));
    x11.stop();
}

/// Xwayland is not offered `wp_viewporter`, through which it would fake for
/// an X game a resolution the monitor does not have and scale the game's
/// window up to fill it. Where an X window is and how big comes from the X
/// server, here and in smithay's search for what is under the pointer, so
/// neither would follow (issue #135). Offered one, Xwayland lists nine
/// resolutions more than the monitor's own.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_game_is_offered_only_the_monitors_own_resolution() {
    let x11 = X11::start("x11-modes", Backend::headless((800, 600)));
    x11.x
        .randr_query_version(1, 3)
        .expect("RandR")
        .reply()
        .expect("RandR's version");
    let resources = x11
        .x
        .randr_get_screen_resources(x11.root)
        .expect("RandR")
        .reply()
        .expect("the screen's resources");
    let modes: Vec<(u16, u16)> = resources
        .modes
        .iter()
        .map(|mode| (mode.width, mode.height))
        .collect();
    assert_eq!(modes, [(800, 600)]);
    x11.stop();
}

/// Headless at 1280 by 800 with a person at it, a window without the
/// keyboard drawn at `unfocused` percent as a seat's `[opacity]` says, and a
/// focus policy of `model` in force for the person's hand.
fn seated(name: &str, unfocused: u8, model: FocusModel) -> X11 {
    let mut backend = Backend::headless((1280, 800)).with_person();
    if let Backend::Headless { opacity, .. } = &mut backend {
        *opacity = Opacity {
            unfocused,
            ..Opacity::default()
        };
    }
    let x11 = X11::start(name, backend);
    x11.session.command(Command::FocusPolicy(Focus {
        model,
        autoraise: false,
    }));
    x11
}

/// "steam", with the keyboard, and a menu it opened over itself, as Steam
/// opens its own: where it is, and the menu.
fn steam_with_a_menu(x11: &X11) -> (u32, u32, (i16, i16)) {
    let steam = x11.open("steam");
    assert!(
        eventually(Duration::from_secs(5), || (x11.focus() == steam)
            .then_some(()))
        .is_some(),
        "steam never took the keyboard"
    );
    x11.hear_focus(steam);
    let (x0, y0, _, _) = rect_of(
        &x11.surface_where("steam", |surface| surface.mapped)
            .expect("mapped"),
    );
    let at = (x0 as i16 + 40, y0 as i16 + 40);
    let menu = x11.menu("menu", at, (120, 100));
    (steam, menu, at)
}

/// Issue #181: under sloppy focus the pointer moving onto an X application's
/// menu gave the menu the keyboard. smithay told X the window that opened
/// it had lost it, the application closed its menu on the `FocusOut`, and
/// for that moment the window was drawn as one without the keyboard. Steam's
/// menus closed as soon as the pointer reached them.
#[cfg(feature = "capture")]
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn pointing_at_an_x11_menu_under_sloppy_focus_leaves_the_keyboard_with_its_window_and_its_window_undimmed()
 {
    let x11 = seated("x11-menu-sloppy", 90, FocusModel::Sloppy);
    let (steam, menu, at) = steam_with_a_menu(&x11);
    x11.drain();

    // The pointer from the origin, where it starts, onto the menu.
    x11.session.command(Command::Motion {
        by: (i32::from(at.0) + 20, i32::from(at.1) + 20),
    });
    assert!(
        x11.pointer_reaches(menu, (20, 20)),
        "the X server never saw the pointer reach the menu"
    );

    assert!(
        !x11.told_focus_out(steam),
        "steam was told it lost the keyboard"
    );
    assert_eq!(x11.focus(), steam, "the menu took the keyboard");
    assert_eq!(focused_last(&x11.session.facts).as_deref(), Some("steam"));
    let facts = x11.facts();
    for title in ["steam", "menu"] {
        let surface = facts
            .surfaces()
            .iter()
            .find(|surface| surface.title.as_deref() == Some(title))
            .expect("on screen");
        assert_eq!(surface.drawn_opacity, None, "{title} was drawn dimmed");
    }

    x11.stop();
}

/// Issue #181: under strict focus too, and the keyboard is not taken from
/// the window either, as it is over the bare desktop: the menu is the
/// window's own.
#[cfg(feature = "capture")]
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn pointing_at_an_x11_menu_under_strict_focus_keeps_the_keyboard_with_its_window() {
    let x11 = seated("x11-menu-strict", 100, FocusModel::Strict);
    let (steam, menu, at) = steam_with_a_menu(&x11);
    x11.drain();

    x11.session.command(Command::Motion {
        by: (i32::from(at.0) + 20, i32::from(at.1) + 20),
    });
    assert!(
        x11.pointer_reaches(menu, (20, 20)),
        "the X server never saw the pointer reach the menu"
    );

    assert!(
        !x11.told_focus_out(steam),
        "steam was told it lost the keyboard"
    );
    assert_eq!(x11.focus(), steam);

    x11.stop();
}

/// Issue #181: a click focuses and raises the window clicked under every
/// focus model, and an X application's menu was taken for one: an item
/// clicked lost the window the keyboard, and its menu, before the click
/// was even delivered.
#[cfg(feature = "capture")]
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn clicking_an_x11_menu_reaches_it_and_leaves_the_keyboard_with_its_window() {
    let x11 = seated("x11-menu-click", 100, FocusModel::Click);
    let (steam, menu, at) = steam_with_a_menu(&x11);
    x11.drain();

    x11.session.command(Command::Click {
        at: (i32::from(at.0) + 20, i32::from(at.1) + 20),
        button: Button::Left,
        mods: Mods::default(),
    });
    // Heard after the keyboard moved, if it moved, so its `FocusOut` is
    // gone by the time the press is: X's focus says it instead.
    assert!(x11.heard_press(menu, 1), "the menu never heard the click");

    assert_eq!(x11.focus(), steam, "the menu took the keyboard");
    assert_eq!(focused_last(&x11.session.facts).as_deref(), Some("steam"));

    x11.stop();
}

/// Issue #181: an X application's menu is no window to a window binding,
/// any more than a panel's menu is: a binding on a window's thumb button
/// left a click on the menu to the menu, rather than flipping the workspace
/// out from under it.
#[cfg(feature = "capture")]
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn a_window_binding_is_not_set_off_by_a_click_on_an_x11_menu() {
    let x11 = X11::start("x11-menu-binding", two_workspaces().with_person());
    x11.session.command(Command::MouseBindings(
        perspicax_policy::MouseBindings::default().bind(
            perspicax_policy::Context::Window,
            perspicax_policy::MouseChord {
                mods: Mods::default(),
                gesture: perspicax_policy::Gesture::Press(Button::Side),
            },
            Action::CycleWorkspace { forward: true },
        ),
    ));
    let (_, menu, at) = steam_with_a_menu(&x11);
    x11.drain();

    x11.session.command(Command::Click {
        at: (i32::from(at.0) + 20, i32::from(at.1) + 20),
        button: Button::Side,
        mods: Mods::default(),
    });

    // X's button 8 is the thumb button nearer the wrist.
    assert!(
        x11.heard_press(menu, 8),
        "the binding took the menu's click"
    );
    assert!(
        x11.surface_where("steam", |surface| surface.mapped)
            .is_some(),
        "the workspace flipped: {:?}",
        x11.facts()
    );

    x11.stop();
}

/// Issue #181: cycling focus walks the stack from the bottom, and an X
/// application's menu was in it: the second press gave the menu the
/// keyboard, and its window lost it and the menu with it.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn cycling_focus_never_lands_on_an_x11_menu() {
    let x11 = X11::start("x11-menu-cycle", Backend::headless((1280, 800)));
    let (steam, _, _) = steam_with_a_menu(&x11);
    x11.drain();

    x11.perform(Action::CycleFocus);
    x11.perform(Action::CycleFocus);

    assert!(
        !x11.told_focus_out(steam),
        "steam was told it lost the keyboard"
    );
    assert_eq!(x11.focus(), steam, "the menu took the keyboard");

    x11.stop();
}

/// Issue #181: the window with the keyboard closing gives it to the window
/// on top, and an X application's menu is on top of everything when it
/// opens. The keyboard went to the menu rather than to the window beneath.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn closing_the_x11_window_with_the_keyboard_while_a_menu_is_up_gives_the_keyboard_to_the_window_beneath()
 {
    let x11 = X11::start(
        "x11-menu-close",
        Backend::headless((1280, 800)).with_person(),
    );
    let (steam, menu, _) = steam_with_a_menu(&x11);
    let dialog = x11.open("dialog");
    assert!(
        eventually(Duration::from_secs(5), || (x11.focus() == dialog)
            .then_some(()))
        .is_some(),
        "the dialog never took the keyboard"
    );

    x11.x.unmap_window(dialog).expect("unmap_window");
    x11.x.flush().expect("flush");

    // Through none: X drops the focus as the dialog unmaps, and smithay
    // takes it from the dialog before giving it on.
    let to = eventually(Duration::from_secs(5), || {
        let focus = x11.focus();
        (focus != dialog && focus != x11rb::NONE).then_some(focus)
    });
    assert_eq!(
        to,
        Some(steam),
        "the keyboard went to {to:#x?}, the menu is {menu:#x}"
    );

    x11.stop();
}

/// Issue #181: leaving the workspace whose window had the keyboard gives
/// the keyboard to the window on top of what is showing, and an X
/// application's menu shows on every workspace, since it belongs to none.
/// It was given the keyboard there.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn switching_to_an_empty_workspace_while_an_x11_menu_is_up_gives_the_menu_no_keyboard() {
    let x11 = X11::start("x11-menu-workspace", two_workspaces());
    let (_, menu, _) = steam_with_a_menu(&x11);

    x11.perform(Action::GoToWorkspace(2));
    x11.surface_where("steam", |surface| !surface.mapped)
        .expect("steam left behind on the first workspace");

    let took = eventually(Duration::from_secs(1), || {
        (x11.focus() == menu).then_some(())
    });
    assert!(took.is_none(), "the menu took the keyboard");

    x11.stop();
}

/// The id the facts give the window titled `title`.
fn id_of(x11: &X11, title: &str) -> perspicax_node::SurfaceId {
    x11.surface_where(title, |surface| surface.mapped)
        .unwrap_or_else(|| panic!("{title} never mapped"))
        .id
}

/// Issue #181: an X application's menu has the keyboard only through its
/// own client, which moves X's focus to it from the window that has the
/// keyboard. An agent's `Focus` on one is refused as on a Wayland menu:
/// given it, the menu's window would lose the keyboard, and the menu.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_agent_cannot_give_an_x11_menu_the_keyboard() {
    let x11 = X11::start("x11-menu-agent-focus", Backend::headless((1280, 800)));
    let host = Host::new(&x11.session.facts, &x11.session.requests);
    let (steam, _, _) = steam_with_a_menu(&x11);
    let menu = id_of(&x11, "menu");

    assert_eq!(
        host.act(menu, &Verb::Focus).map(|_| ()),
        Err(ActError::TakesNoKeyboard(menu.0))
    );
    assert_eq!(x11.focus(), steam);

    x11.stop();
}

/// Issue #181: closing a window is asking its client to, and an X
/// application's menu has no `WM_DELETE_WINDOW` to be asked by, so smithay
/// destroys it outright. An agent's `Close` on one finds no window to close,
/// as on a Wayland menu.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_agent_cannot_close_an_x11_menu() {
    let x11 = X11::start("x11-menu-agent-close", Backend::headless((1280, 800)));
    let host = Host::new(&x11.session.facts, &x11.session.requests);
    let (_, menu_window, _) = steam_with_a_menu(&x11);
    let menu = id_of(&x11, "menu");

    assert_eq!(
        host.act(menu, &Verb::Close).map(|_| ()),
        Err(ActError::NoSuchSurface(menu.0))
    );
    assert!(x11.viewable(menu_window), "the menu was destroyed");

    x11.stop();
}

/// Issue #181: a window that places itself is not the window manager's to
/// minimize, even when it asks: it was parked, taken off screen while X
/// still showed it.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_menu_asking_to_be_minimized_stays_where_it_put_itself() {
    let x11 = X11::start(
        "x11-menu-iconic",
        Backend::headless((1280, 800)).with_person(),
    );
    let (steam, menu, _) = steam_with_a_menu(&x11);

    x11.change_state(menu, ICONIC);
    x11.settled(steam);

    let hidden = eventually(Duration::from_secs(1), || {
        x11.surface_where("menu", |surface| !surface.mapped)
    });
    assert!(hidden.is_none(), "the menu was minimized");

    x11.stop();
}

/// Issue #181: nor to make fullscreen: it was told it was, and moved on
/// screen while X still had it where it put itself.
#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_menu_asking_for_fullscreen_stays_where_it_put_itself() {
    let x11 = X11::start(
        "x11-menu-fullscreen",
        Backend::headless((1280, 800)).with_person(),
    );
    let (steam, menu, _) = steam_with_a_menu(&x11);

    x11.ask_state(menu, ADD, x11.atoms._NET_WM_STATE_FULLSCREEN, 0);
    x11.settled(steam);

    assert!(
        !x11.net_wm_state(menu)
            .contains(&x11.atoms._NET_WM_STATE_FULLSCREEN),
        "the menu was made fullscreen"
    );

    x11.stop();
}
