//! The settings app's channel, `perspicax-settings-v1`, by a compositor with
//! no screen.
//!
//! A capture waits for the next chord only while a surface of the process
//! that asked holds the keyboard, from any of its connections. It is
//! cancelled when the keyboard goes to another process, when another capture
//! is asked for, when the session locks, and when a reload's `[protocols]
//! settings` rule no longer admits the client. `[protocols] settings`
//! decides who sees the channel at all. And whatever an agent was given, it
//! may not act on the settings app, which edits the file that holds it back.
//!
//! Headless has no person's keys, so what a capture hears is not tested
//! here: `perspicax_policy::take` decides it, and the seat kit presses the
//! keys.
//!
//! The client is this test binary, so a rule naming `current_exe()` admits
//! it. Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use std::{
    process::{Child, Command as Process, Stdio},
    thread,
    time::Duration,
};

use common::{Desk, Renamed, Session, advertised, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{Consent, HostFacts, Refusal, SurfaceFacts, WindowVerb, check_window};
use perspicax_policy::{Access, Program, Protocol, Rule, Shape};
use perspicax_protocols::settings::v1::client::perspicax_chord_capture_v1::Reason;
use wayland_client::EventQueue;

const SETTINGS: &str = "perspicax_settings_v1";

/// Set, to the compositor's socket, for the copy of this binary that
/// [`window_probe`] runs in.
const PROBE: &str = "PERSPICAX_CAPTURE_PROBE";

fn this_binary() -> String {
    std::env::current_exe()
        .expect("the test binary")
        .to_string_lossy()
        .into_owned()
}

fn backend(access: Access) -> Backend {
    Backend::Headless {
        outputs: vec![Virtual::numbered(1, (1280, 1024))],
        workspaces: Shape::default(),
        access,
        person: false,
        opacity: Default::default(),
        agent: Default::default(),
    }
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

/// A window titled `title` from `desk`, mapped and holding the keyboard.
fn focused_window(session: &Session, desk: &mut Desk, queue: &mut EventQueue<Desk>, title: &str) {
    let qh = queue.handle();
    desk.open_window(&qh, title, title);
    let drawn = desk.drawn + 1;
    until(queue, desk, |desk| desk.drawn == drawn);
    session.wait_for(|facts| {
        titled(facts, title).is_some_and(|window| window.mapped && window.focused_at.is_some())
    });
}

/// Long enough for anything the compositor was going to say to arrive.
fn settle(queue: &mut EventQueue<Desk>, desk: &mut Desk) {
    queue.roundtrip(desk).expect("dispatch");
    thread::sleep(Duration::from_millis(200));
    queue.roundtrip(desk).expect("dispatch");
}

/// A window titled "probe", put up by `exe` running [`window_probe`]: a
/// process of its own. It stays until it is killed.
fn probe(session: &Session, exe: &str) -> Child {
    let child = Process::new(exe)
        .args(["--exact", "window_probe", "--ignored", "--test-threads=1"])
        .env(PROBE, session.socket())
        .stdout(Stdio::null())
        .spawn()
        .expect("the probe starts");
    session.wait_for(|facts| titled(facts, "probe").is_some_and(|window| window.mapped));
    child
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_settings_protocol_is_offered_only_to_what_its_rule_admits() {
    let naming = |rule: Rule| Access::open().with(Protocol::Settings, rule);
    for (name, rule, offered) in [
        ("capture-any", Rule::Any, true),
        (
            "capture-us",
            Rule::Only(vec![Program::parse(&this_binary())]),
            true,
        ),
        (
            "capture-them",
            Rule::Only(vec![Program::parse("perspicax-settings")]),
            false,
        ),
        ("capture-off", Rule::Off, false),
    ] {
        let session = Session::start(name, backend(naming(rule)));
        let (desk, queue, _qh, globals) = session.client();
        assert_eq!(advertised(&globals, SETTINGS), offered, "{name}");
        session.stop((desk, queue));
    }
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_capture_asked_without_the_keyboard_is_cancelled_as_unfocused() {
    let session = Session::start("capture-unfocused", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_settings(&globals, &qh);
    let first = desk.capture_chord(&qh);
    until(&mut queue, &mut desk, |desk| {
        desk.cancelled(first).is_some()
    });
    assert_eq!(desk.cancelled(first), Some(Reason::Unfocused));
    session.stop((desk, queue));
}

/// The app's toolkit draws on one connection and asks on another: the
/// capture waits while either's surface has the keyboard, and ends when a
/// window of another process takes it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_capture_ends_when_the_keyboard_leaves_the_process_that_asked() {
    let session = Session::start("capture-leaves", backend(Access::open()));
    let (mut toolkit, mut toolkit_queue, _, _) = session.client();
    focused_window(&session, &mut toolkit, &mut toolkit_queue, "app");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_settings(&globals, &qh);
    let capture = desk.capture_chord(&qh);
    settle(&mut queue, &mut desk);
    assert!(desk.caught.is_empty(), "waits: {:?}", desk.caught);

    // Another window of the same process takes the keyboard: still waiting.
    focused_window(&session, &mut toolkit, &mut toolkit_queue, "app's dialog");
    settle(&mut queue, &mut desk);
    assert!(desk.caught.is_empty(), "still waits: {:?}", desk.caught);

    let mut other = probe(&session, &this_binary());
    until(&mut queue, &mut desk, |desk| {
        desk.cancelled(capture).is_some()
    });
    assert_eq!(desk.cancelled(capture), Some(Reason::Unfocused));
    assert_eq!(desk.caught.len(), 1, "told once: {:?}", desk.caught);

    other.kill().expect("the probe is ours to kill");
    other.wait().expect("and ours to reap");
    session.stop(((desk, queue), (toolkit, toolkit_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_second_capture_replaces_the_first() {
    let session = Session::start("capture-replaced", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    focused_window(&session, &mut desk, &mut queue, "app");
    desk.bind_settings(&globals, &qh);
    let first = desk.capture_chord(&qh);
    let second = desk.capture_chord(&qh);
    until(&mut queue, &mut desk, |desk| {
        desk.cancelled(first).is_some()
    });
    assert_eq!(desk.cancelled(first), Some(Reason::Replaced));
    settle(&mut queue, &mut desk);
    assert_eq!(desk.cancelled(second), None, "the second waits");
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_capture_is_cancelled_when_the_session_locks() {
    let session = Session::start("capture-locked", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    focused_window(&session, &mut desk, &mut queue, "app");
    desk.bind_settings(&globals, &qh);
    let before = desk.capture_chord(&qh);
    settle(&mut queue, &mut desk);
    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    until(&mut queue, &mut desk, |desk| {
        desk.cancelled(before).is_some()
    });
    assert_eq!(
        desk.cancelled(before),
        Some(Reason::Locked),
        "locked, not merely unfocused"
    );

    let during = desk.capture_chord(&qh);
    until(&mut queue, &mut desk, |desk| {
        desk.cancelled(during).is_some()
    });
    assert_eq!(desk.cancelled(during), Some(Reason::Locked));

    desk.unlock();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_reload_that_no_longer_admits_it_withdraws_the_capture() {
    let session = Session::start("capture-withdrawn", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    focused_window(&session, &mut desk, &mut queue, "app");
    desk.bind_settings(&globals, &qh);
    let before = desk.capture_chord(&qh);
    settle(&mut queue, &mut desk);
    assert!(desk.caught.is_empty(), "waits: {:?}", desk.caught);

    session.command(Command::Protocols(Access::open().with(
        Protocol::Settings,
        Rule::Only(vec![Program::parse("perspicax-settings")]),
    )));
    until(&mut queue, &mut desk, |desk| {
        desk.cancelled(before).is_some()
    });
    assert_eq!(desk.cancelled(before), Some(Reason::Withdrawn));

    let after = desk.capture_chord(&qh);
    until(&mut queue, &mut desk, |desk| {
        desk.cancelled(after).is_some()
    });
    assert_eq!(desk.cancelled(after), Some(Reason::Withdrawn));
    session.stop((desk, queue));
}

/// Started as an agent's program is, by `--spawn`, on a desk that consents
/// to everyone: the settings app's window is still refused, and this
/// binary's, beside it, is not.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_settings_app_started_with_spawn_is_still_refused_by_act() {
    let renamed = Renamed::called("perspicax-settings", "settings-app");
    let socket = format!("perspicax-capture-act-{}", std::process::id());
    let session = Session::start_spawning(
        "capture-act",
        Backend::headless((1280, 1024)),
        vec![vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!(
                "{PROBE}={socket} '{}' --exact window_probe --ignored --test-threads=1 >/dev/null",
                renamed.0.display()
            ),
        ]],
    );
    assert_eq!(session.socket(), socket, "the socket the probe is told");
    let (mut desk, mut queue, _qh, _globals) = session.client();
    focused_window(&session, &mut desk, &mut queue, "mine");
    let facts =
        session.wait_for(|facts| titled(facts, "probe").is_some_and(|window| window.mapped));
    assert_eq!(facts.consent(), &Consent::Everyone);

    let probe = titled(&facts, "probe").expect("the settings app's window");
    assert_eq!(
        check_window(&facts, probe.id, &WindowVerb::Close).err(),
        Some(Refusal::NeverGranted {
            program: "perspicax-settings".to_owned()
        }),
        "{:?}",
        probe.origin
    );
    let mine = titled(&facts, "mine").expect("this binary's window");
    assert!(check_window(&facts, mine.id, &WindowVerb::Close).is_ok());

    session.stop((desk, queue));
    drop(renamed);
}

/// Not a test: the window [`probe`] and the spawned settings app put up, in
/// a process of their own.
#[test]
#[ignore = "a window in a process of its own, for the tests that need one"]
fn window_probe() {
    let Ok(socket) = std::env::var(PROBE) else {
        return;
    };
    let (mut desk, mut queue, qh, _) = common::client_on(&socket);
    desk.open_window(&qh, "probe", "probe");
    loop {
        queue
            .blocking_dispatch(&mut desk)
            .expect("the compositor is there");
    }
}
