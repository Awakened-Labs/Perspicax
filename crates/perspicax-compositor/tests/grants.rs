//! What an agent is granted where a person has to agree to it, and how long
//! a grant lasts: on a headless desk that consents as a seat does.
//!
//! A grant names a pid, and the kernel gives an exited process's pid to the
//! next program that asks once the exited one is reaped. So every grant must
//! end with its process, or a program nobody granted could arrive holding
//! one. A reused pid can't be arranged on demand, so what is tested is the
//! condition that makes reuse harmless: a grant whose process has exited is
//! no longer in the consent the compositor publishes.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use std::process::{Child, Command, Stdio};

use common::{Session, until};
use perspicax_compositor::Backend;
use perspicax_index::{Consent, HostFacts, SurfaceFacts};
use perspicax_policy::Action;

/// Set, to the compositor's socket, for the copy of this binary that
/// [`window_probe`] runs in.
const PROBE: &str = "PERSPICAX_WINDOW_PROBE";

/// A desk that consents as a seat does.
fn granting(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).granting())
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

/// A window titled "probe", put up by a process of its own: a copy of this
/// binary running [`window_probe`]. It stays until it is killed.
fn probe(session: &Session) -> Child {
    let child = Command::new(std::env::current_exe().expect("this test's binary"))
        .args(["--exact", "window_probe", "--ignored", "--test-threads=1"])
        .env(PROBE, session.socket())
        .stdout(Stdio::null())
        .spawn()
        .expect("the probe starts");
    session.wait_for(|facts| titled(facts, "probe").is_some_and(|window| window.mapped));
    child
}

/// The pids granted, once the compositor has published any consent. Before
/// its first snapshot a reader sees the default, nobody.
fn granted(facts: &HostFacts) -> Option<&[u32]> {
    match facts.consent() {
        Consent::Granted(pids) => Some(pids),
        Consent::Nobody => None,
        Consent::Everyone => panic!("consent is not by grant"),
    }
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_spawned_programs_grant_ends_when_it_exits() {
    let session = Session::start_spawning(
        "grant-ends",
        Backend::headless((800, 600)).granting(),
        vec![vec!["sleep".to_owned(), "1".to_owned()]],
    );
    let facts = session.wait_for(|facts| granted(facts).is_some_and(|pids| pids.len() == 1));
    let pid = granted(&facts).expect("published")[0];
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "granted while it runs"
    );

    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop(());
}

/// The person's key on the focused window: its process granted, and taken
/// back by the same key.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_key_grants_the_focused_windows_process_and_takes_it_back() {
    let session = granting("grant-key");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "mine", "mine");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    session.wait_for(|facts| titled(facts, "mine").is_some_and(|window| window.mapped));
    let me = std::process::id();

    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts) == Some(&[me][..]));

    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop((desk, queue));
}

/// `revoke-agent-all` takes back a key's grant and `--spawn`'s alike.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn revoking_every_grant_leaves_nothing_consented() {
    let session = Session::start_spawning(
        "grant-revoke-all",
        Backend::headless((1280, 1024)).granting(),
        vec![vec!["sleep".to_owned(), "30".to_owned()]],
    );
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "mine", "mine");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    session.wait_for(|facts| titled(facts, "mine").is_some_and(|window| window.mapped));
    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts).is_some_and(|pids| pids.len() == 2));

    session.perform(Action::RevokeAgents);
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop((desk, queue));
}

/// A process this compositor did not start is not reaped by it, so nothing
/// here would notice it exit but its pidfd. The grant goes when the process
/// does, before anyone reaps it: before its pid could be anyone else's.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_in_a_process_of_its_own_loses_its_grant_when_the_process_exits() {
    let session = granting("grant-exits");
    let mut probe = probe(&session);
    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts) == Some(&[probe.id()][..]));

    probe.kill().expect("the probe is ours to kill");
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    probe
        .wait()
        .expect("and ours to reap, after its grant has gone");
    session.stop(());
}

/// Not a test: the window [`probe`] puts up, in a process of its own.
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

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_desk_that_does_not_consent_by_grant_consents_to_everyone_whatever_it_spawns() {
    let session = Session::start_spawning(
        "grant-everyone",
        Backend::headless((800, 600)),
        vec![vec!["sleep".to_owned(), "1".to_owned()]],
    );
    let facts = session.wait_for(|facts| facts.consent() != &Consent::Nobody);
    assert_eq!(facts.consent(), &Consent::Everyone);
    session.stop(());
}
