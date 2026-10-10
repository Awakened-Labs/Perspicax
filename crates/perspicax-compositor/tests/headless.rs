//! A headless compositor, brought up for real.
//!
//! These bind an actual Wayland socket, so they need `XDG_RUNTIME_DIR` -- which
//! a login session has and a bare `ssh host cmd` or a CI container does not.
//! They are `#[ignore]`d for the same reason `perspicax-atspi`'s live tests
//! are: the four gates have to stay green on a machine with no session, and a
//! test that silently passes by skipping itself is worse than one that has to
//! be asked for. `ci/live-tests.sh` runs them with `--include-ignored`.

mod common;

use std::time::{Duration, Instant};

use common::{Session, until};
use perspicax_compositor::{Backend, Config, Error, Facts, Requests, Stop};
use perspicax_index::Consent;

/// The smallest claim worth making automatically: it binds a socket, runs an
/// event loop, and stops when it is told to. Everything else in this milestone
/// is built on that not silently regressing.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_headless_compositor_comes_up_and_stops_when_told() {
    let config = Config {
        backend: Backend::headless((800, 600)),
        spawn: Vec::new(),
        env: Vec::new(),
        run_for: Some(Duration::from_millis(250)),
        config: None,
        socket: None,
        xwayland: false,
    };

    let facts = Facts::new();
    let started = Instant::now();
    perspicax_compositor::run(&config, &facts, &Requests::new(), &Stop::new())
        .expect("a headless compositor needs nothing but a runtime dir");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(250),
        "returned before its deadline, in {elapsed:?} -- the loop is not running"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "overshot its deadline by an order of magnitude, in {elapsed:?}"
    );
    assert!(
        facts.read().surfaces().is_empty(),
        "a compositor nobody connected to has no surfaces to describe"
    );
    assert_eq!(
        facts.read().consent(),
        &Consent::Everyone,
        "headless consents to every client -- without this every act in CI is refused"
    );
}

/// A misspelled `--spawn` has to say what it could not start. The failure this
/// guards against is a compositor that comes up, silently starts nothing, and
/// presents an empty desktop as a working one.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_command_that_does_not_exist_is_named_in_the_error() {
    let config = Config {
        backend: Backend::headless((800, 600)),
        spawn: vec![vec![
            "perspicax-no-such-program".to_owned(),
            "--flag".to_owned(),
        ]],
        env: Vec::new(),
        run_for: Some(Duration::from_millis(50)),
        config: None,
        socket: None,
        xwayland: false,
    };

    match perspicax_compositor::run(&config, &Facts::new(), &Requests::new(), &Stop::new()) {
        Err(Error::Spawn { command, source }) => {
            assert_eq!(command, "perspicax-no-such-program --flag");
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected a spawn failure naming the command, got {other:?}"),
    }
}

/// Being asked to stop beats a deadline that has not arrived. The read this
/// exists for takes seconds it cannot predict, so the only honest deadline is
/// "when the reader says so".
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_stop_request_ends_the_loop_before_its_deadline() {
    let config = Config {
        backend: Backend::headless((800, 600)),
        spawn: Vec::new(),
        env: Vec::new(),
        run_for: Some(Duration::from_secs(60)),
        config: None,
        socket: None,
        xwayland: false,
    };

    let stop = Stop::new();
    let asker = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        asker.request();
    });

    let started = Instant::now();
    perspicax_compositor::run(&config, &Facts::new(), &Requests::new(), &stop)
        .expect("stopping is not a failure");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "waited for the deadline instead of the request"
    );
}

/// The socket's name is published as the compositor comes up, before anything
/// it starts could ask for it: it is what the session bus is told, so that a
/// program D-Bus starts on request -- a keyring's unlock prompt, a portal --
/// can find this compositor (issue #27).
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_socket_is_published_to_whoever_watches_the_session() {
    let socket = format!("perspicax-test-session-{}", std::process::id());
    let config = Config {
        backend: Backend::headless((800, 600)),
        spawn: Vec::new(),
        env: Vec::new(),
        run_for: Some(Duration::from_millis(250)),
        config: None,
        socket: Some(socket.clone()),
        xwayland: false,
    };

    let facts = Facts::new();
    let watch = facts.watch_session();
    perspicax_compositor::run(&config, &facts, &Requests::new(), &Stop::new())
        .expect("a headless compositor needs nothing but a runtime dir");

    let heard: Vec<_> = watch.try_iter().collect();
    assert_eq!(
        heard.first().map(|facts| facts.wayland_display.clone()),
        Some(None),
        "a watcher hears first that there is no socket yet"
    );
    assert_eq!(
        heard.last().and_then(|facts| facts.wayland_display.clone()),
        Some(socket),
        "the socket was never published: {heard:?}"
    );
    assert_eq!(facts.session().x11_display, None);
}

/// Issue #90: a headless compositor can seat a person, so that what only a
/// person gets -- here a window maximizing itself -- is tested where CI runs.
/// The window is offered its whole monitor.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_asking_to_be_maximized_is_when_a_person_sits_at_the_seat() {
    let session = Session::start(
        "person-maximize",
        Backend::headless((1280, 1024)).with_person(),
    );
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "asking", "asking");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);

    desk.windows[0].set_maximized();
    until(&mut queue, &mut desk, |desk| {
        desk.offered == Some((1280, 1024))
    });

    session.stop((desk, queue));
}

/// The other half of the above: with nobody at the seat, a window asking to
/// be maximized is left as it was, the deterministic behaviour an agent's
/// desk is built on.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_asking_to_be_maximized_is_left_as_it_was_with_nobody_at_the_seat() {
    let session = Session::start("nobody-maximize", Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "asking", "asking");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let offered = desk.offered;

    desk.windows[0].set_maximized();
    queue.roundtrip(&mut desk).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert_eq!(desk.offered, offered, "nobody asked for it to be maximized");

    session.stop((desk, queue));
}

/// What a game reaches for to turn its camera with the mouse: the mouse's
/// own motion, wherever the pointer is, and a hold on the pointer while it
/// turns. Xwayland looks for both before it turns an X game's pointer warps
/// into the same.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_game_finds_what_it_turns_its_camera_with() {
    let session = Session::start("game-globals", Backend::headless((800, 600)));
    let (desk, queue, _qh, globals) = session.client();
    for global in [
        "zwp_relative_pointer_manager_v1",
        "zwp_pointer_constraints_v1",
    ] {
        assert!(common::advertised(&globals, global), "{global}");
    }
    session.stop((desk, queue));
}
