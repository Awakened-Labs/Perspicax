//! A headless compositor, brought up for real.
//!
//! These bind an actual Wayland socket, so they need `XDG_RUNTIME_DIR` -- which
//! a login session has and a bare `ssh host cmd` or a CI container does not.
//! They are `#[ignore]`d for the same reason `wm-atspi`'s live tests are: the
//! four gates have to stay green on a machine with no session, and a test that
//! silently passes by skipping itself is worse than one that has to be asked
//! for. `ci/live-tests.sh` runs them with `--include-ignored`.

use std::time::{Duration, Instant};

use wm_compositor::{Config, Error};

/// The smallest claim worth making automatically: it binds a socket, runs an
/// event loop, and stops when it is told to. Everything else in this milestone
/// is built on that not silently regressing.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_headless_compositor_comes_up_and_stops_when_told() {
    let config = Config {
        size: (800, 600),
        spawn: Vec::new(),
        run_for: Some(Duration::from_millis(250)),
    };

    let started = Instant::now();
    wm_compositor::run(&config).expect("a headless compositor needs nothing but a runtime dir");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(250),
        "returned before its deadline, in {elapsed:?} -- the loop is not running"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "overshot its deadline by an order of magnitude, in {elapsed:?}"
    );
}

/// A misspelled `--spawn` has to say what it could not start. The failure this
/// guards against is a compositor that comes up, silently starts nothing, and
/// presents an empty desktop as a working one.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_command_that_does_not_exist_is_named_in_the_error() {
    let config = Config {
        size: (800, 600),
        spawn: vec![vec!["wm-no-such-program".to_owned(), "--flag".to_owned()]],
        run_for: Some(Duration::from_millis(50)),
    };

    match wm_compositor::run(&config) {
        Err(Error::Spawn { command, source }) => {
            assert_eq!(command, "wm-no-such-program --flag");
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected a spawn failure naming the command, got {other:?}"),
    }
}
