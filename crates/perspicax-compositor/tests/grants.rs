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

use common::Session;
use perspicax_compositor::Backend;
use perspicax_index::{Consent, HostFacts};

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
