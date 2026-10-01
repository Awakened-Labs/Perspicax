//! An agent's window verbs, as the compositor carries them out: `Close`
//! and `Forward` through `Host::act`, the path `window_close` and
//! `tab_forward` take.
//!
//! A tab behind another comes forward in its group's place; a close reaches
//! the client as a request; and while the session is locked neither is done.
//! The gate that decides whether an agent may ask is `perspicax-index`'s and
//! is tested there; this is what happens once it has said yes.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Session, until};
use perspicax_compositor::{ActError, Backend, Host};
use perspicax_index::{Action as Verb, HostFacts};
use perspicax_node::SurfaceId;
use perspicax_policy::Action;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

fn id(facts: &HostFacts, title: &str) -> SurfaceId {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| surface.id)
        .expect("a window of that title")
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn forward_brings_a_tab_behind_to_the_front_of_its_group() {
    let session = session("verbs-forward");
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    session.perform(Action::TabWithPrevious);
    let facts = session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.behind_tab.is_some())
    });
    let (first, second) = (id(&facts, "first"), id(&facts, "second"));
    assert_eq!(facts.surface(first).unwrap().tabs, [first, second]);
    assert_eq!(facts.surface(first).unwrap().behind_tab, Some(second));

    let dispatched = host.act(first, &Verb::Forward).expect("dispatched");
    assert_eq!(dispatched.focus_after, Some(first));
    session.wait_for(|facts| {
        facts.surface(second).unwrap().behind_tab == Some(first)
            && facts.surface(first).unwrap().mapped
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn close_asks_the_client() {
    let session = session("verbs-close");
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "first", "org.example.First");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| !facts.surfaces().is_empty());
    let first = id(&facts, "first");
    assert_eq!(
        facts.surface(first).unwrap().app_id.as_deref(),
        Some("org.example.First")
    );
    assert_eq!(facts.surface(first).unwrap().workspace, Some(1));

    host.act(first, &Verb::Close).expect("dispatched");
    until(&mut queue, &mut desk, |desk| desk.asked_to_close == 1);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn neither_is_done_while_locked() {
    let session = session("verbs-locked");
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let first = id(
        &session.wait_for(|facts| !facts.surfaces().is_empty()),
        "first",
    );

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    assert_eq!(host.act(first, &Verb::Close), Err(ActError::Locked));
    assert_eq!(host.act(first, &Verb::Forward), Err(ActError::Locked));
    queue.roundtrip(&mut desk).expect("dispatch");
    assert_eq!(desk.asked_to_close, 0);

    desk.unlock();
    session.stop((desk, queue));
}
