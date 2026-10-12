//! A headless compositor, brought up for real.
//!
//! These bind an actual Wayland socket, so they need `XDG_RUNTIME_DIR` -- which
//! a login session has and a bare `ssh host cmd` or a CI container does not.
//! They are `#[ignore]`d for the same reason `perspicax-atspi`'s live tests
//! are: the four gates have to stay green on a machine with no session, and a
//! test that silently passes by skipping itself is worse than one that has to
//! be asked for. `ci/live-tests.sh` runs them with `--include-ignored`.

mod common;

use std::{
    io::ErrorKind,
    time::{Duration, Instant},
};

use common::{Configured, Desk, Said, Session, until};
use perspicax_compositor::{Backend, Config, Error, Facts, Requests, Stop};
use perspicax_index::Consent;
use perspicax_policy::{Action, Direction};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::backend::WaylandError;

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

/// #99: a window opening takes the keyboard from the window in use, which is
/// told it no longer is. It used to go on believing it was: its state was
/// unset unsaid as the new window was mapped, and then there was nothing
/// left to tell it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_opening_over_the_one_in_use_tells_that_one_it_no_longer_is() {
    let session = Session::start(
        "person-new-window",
        Backend::headless((1280, 1024)).with_person(),
    );
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);

    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| {
        desk.activations_of(0) == [true, false]
    });
    assert_eq!(desk.activations_of(1), [true], "the new window is in use");

    session.stop((desk, queue));
}

/// With nobody at the seat every window is active for as long as it is
/// open: a toolkit that believes it is unfocused draws differently, and an
/// agent reads what is drawn. A window opening used to unset the others'
/// state unsaid, so the next configure any of them was sent told it it was
/// not active. Here that is the one a request to be maximized is answered
/// with, which with nobody at the seat changes nothing else.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn with_nobody_at_the_seat_a_window_opening_leaves_the_others_active() {
    let session = Session::start("nobody-new-window", Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);

    desk.windows[0].set_maximized();
    until(&mut queue, &mut desk, |desk| {
        desk.activations_of(0).len() == 2
    });
    assert_eq!(desk.activations_of(0), [true, true]);

    session.stop((desk, queue));
}

/// xdg-shell says a request to be unmaximized, or to leave fullscreen, is
/// answered with a configure, whatever it changes. With nobody at the seat
/// it changes nothing, and it used to be answered with nothing at all.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_asking_to_stop_filling_is_answered_with_nobody_at_the_seat() {
    let session = Session::start("nobody-unmaximize", Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "asking", "asking");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);

    desk.windows[0].unset_maximized();
    until(&mut queue, &mut desk, |desk| configures(desk).len() == 2);
    desk.windows[0].unset_fullscreen();
    until(&mut queue, &mut desk, |desk| configures(desk).len() == 3);

    session.stop((desk, queue));
}

/// #99: a window snapped to a half, which is not maximized, asks to be
/// unmaximized, as a toolkit may. It is answered, and stays as it is. It
/// used to be sent back to the size it had before it was snapped, still
/// tiled, and moved there.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_snapped_window_asking_to_be_unmaximized_keeps_its_half_and_its_tiling() {
    let session = Session::start(
        "person-unmaximize-snapped",
        Backend::headless((1280, 1024)).with_person(),
    );
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "snapped", "snapped");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    session.perform(Action::Snap(Direction::Left));
    until(&mut queue, &mut desk, |desk| {
        configures(desk)
            .last()
            .is_some_and(|last| last.tiled && last.size == Some((640, 1024)))
    });
    let snapped = configures(&desk).len();

    desk.windows[0].unset_maximized();
    until(&mut queue, &mut desk, |desk| {
        configures(desk).len() > snapped
    });
    let answer = *configures(&desk).last().expect("answered");
    assert!(
        answer.tiled && !answer.maximized && answer.size == Some((640, 1024)),
        "no longer as it was snapped: {answer:?}"
    );

    session.stop((desk, queue));
}

/// Every configure the client's first window was sent, in order.
fn configures(desk: &Desk) -> Vec<Configured> {
    desk.said_to(desk.windows[0].wl_surface())
        .into_iter()
        .filter_map(|said| match said {
            Said::Configure(configured) => Some(configured),
            _ => None,
        })
        .collect()
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

/// #101: a client that falls behind is held for, not cut off at once. This
/// one asks for 30 000 frame callbacks in one commit, as the shell's pie
/// could have, then reads nothing for a moment: the answers, a `done` and a
/// `delete_id` for each, are some 720 KB, far past what the kernel holds for
/// it. wayland-backend's buffer of 4 KB cut it off; a megabyte holds them
/// until it reads, and it is sent every one.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_client_that_reads_late_is_sent_every_frame_callback_it_asked_for() {
    const ASKED: usize = 30_000;
    let read = read_late("reads-late", ASKED);
    assert_eq!(read, Ok(ASKED), "cut off for reading late");
}

/// The other half: what waits for a client is held to a megabyte, so one
/// that has fallen further behind is still cut off, and costs no more. These
/// 80 000 answers are some 1.9 MB.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_client_further_behind_than_a_megabyte_is_cut_off() {
    let read = read_late("reads-too-late", 80_000);
    assert!(read.is_err(), "held past a megabyte: {read:?}");
}

/// Open a window, ask for `asked` frame callbacks on it in one commit, read
/// nothing for a moment while the compositor answers, then read: how many
/// were answered, or what the connection said instead.
fn read_late(name: &str, asked: usize) -> Result<usize, String> {
    let session = Session::start(name, Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "late", "late");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let surface = desk.windows[0].wl_surface().clone();
    let done = |desk: &Desk| {
        desk.said_to(&surface)
            .iter()
            .filter(|said| **said == Said::Done)
            .count()
    };
    let before = done(&desk);

    for _ in 0..asked {
        surface.frame(&qh, surface.clone());
    }
    surface.commit();
    // The asking is 12 bytes a callback, more than the socket takes at once.
    loop {
        match queue.flush() {
            Ok(()) => break,
            Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("could not ask: {error}"),
        }
    }
    std::thread::sleep(Duration::from_millis(200));

    let read = queue
        .roundtrip(&mut desk)
        .map(|_| done(&desk) - before)
        .map_err(|error| error.to_string());
    session.stop((desk, queue));
    read
}
