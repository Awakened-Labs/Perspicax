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
use perspicax_node::{Origin, X11Basis};
use perspicax_policy::Action;
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass},
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
    // Two of them, so that Alt+Tab's cycle has one to go to.
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

    // Focused first, as a person's click would: a window maps before
    // Xwayland has given it a surface, so mapping it cannot focus it.
    requests
        .command(Command::Perform(Action::CycleFocus))
        .expect("the compositor is listening");
    eventually(Duration::from_secs(5), || {
        facts
            .read()
            .surfaces()
            .iter()
            .any(|surface| surface.focused_at.is_some())
            .then_some(())
    })
    .expect("the X11 window never took focus");

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
