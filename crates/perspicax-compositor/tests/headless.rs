//! A headless compositor, brought up for real.
//!
//! These bind an actual Wayland socket, so they need `XDG_RUNTIME_DIR` -- which
//! a login session has and a bare `ssh host cmd` or a CI container does not.
//! They are `#[ignore]`d for the same reason `perspicax-atspi`'s live tests
//! are: the four gates have to stay green on a machine with no session, and a
//! test that silently passes by skipping itself is worse than one that has to
//! be asked for. `ci/live-tests.sh` runs them with `--include-ignored`.

use std::time::{Duration, Instant};

use perspicax_compositor::{Backend, Config, Error, Facts, Requests, Stop};

/// The smallest claim worth making automatically: it binds a socket, runs an
/// event loop, and stops when it is told to. Everything else in this milestone
/// is built on that not silently regressing.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_headless_compositor_comes_up_and_stops_when_told() {
    let config = Config {
        backend: Backend::Headless { size: (800, 600) },
        spawn: Vec::new(),
        env: Vec::new(),
        run_for: Some(Duration::from_millis(250)),
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
}

/// A misspelled `--spawn` has to say what it could not start. The failure this
/// guards against is a compositor that comes up, silently starts nothing, and
/// presents an empty desktop as a working one.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_command_that_does_not_exist_is_named_in_the_error() {
    let config = Config {
        backend: Backend::Headless { size: (800, 600) },
        spawn: vec![vec![
            "perspicax-no-such-program".to_owned(),
            "--flag".to_owned(),
        ]],
        env: Vec::new(),
        run_for: Some(Duration::from_millis(50)),
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
        backend: Backend::Headless { size: (800, 600) },
        spawn: Vec::new(),
        env: Vec::new(),
        run_for: Some(Duration::from_secs(60)),
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
