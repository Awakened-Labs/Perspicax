//! The windows, as a taskbar's list is told them, by a compositor with no
//! screen.
//!
//! A window opening, being retitled and closing reaches a client holding
//! `ext-foreign-toplevel-list-v1`. `[protocols]` decides who gets the list
//! at all: nobody, any client, or the programs it names. A rule narrowed
//! while a client holds the list takes it back. And while the session is
//! locked the list hears nothing, then everything at once when it is lifted.
//!
//! The client is this test binary, in this process, so an allowlist naming
//! `current_exe()` admits it and one naming anything else does not. Like the
//! other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Session, advertised, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_policy::{Access, Program, Protocol, Rule, Shape};

const LIST: &str = "ext_foreign_toplevel_list_v1";

fn backend(access: Access) -> Backend {
    Backend::Headless {
        outputs: vec![Virtual::numbered(1, (1280, 1024))],
        workspaces: Shape::default(),
        access,
        person: false,
        opacity: Default::default(),
    }
}

fn this_binary() -> String {
    std::env::current_exe()
        .expect("the test binary")
        .to_string_lossy()
        .into_owned()
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_list_announces_a_window_its_title_change_and_its_close() {
    let session = Session::start("list", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();

    desk.open_window(&qh, "first", "org.example.First");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.bind_list(&globals, &qh);
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["first"]
    });
    let first = desk.listed_by_title("first").unwrap().clone();
    assert_eq!(first.app_id, "org.example.First");
    assert_eq!(first.identifier.len(), 32);

    // A window opened while the list is held arrives on its own.
    desk.open_window(&qh, "second", "org.example.Second");
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["first", "second"]
    });

    // Retitled without drawing again: titles are not double-buffered.
    desk.windows[0].set_title("renamed");
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["renamed", "second"]
    });
    assert_eq!(
        desk.listed_by_title("renamed").unwrap().identifier,
        first.identifier,
        "the same window, under its new name"
    );
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("renamed"))
    });

    desk.windows.remove(1);
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["renamed"]
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_list_is_not_advertised_when_off() {
    let access = Access::open().with(Protocol::ForeignToplevelList, Rule::Off);
    let session = Session::start("list-off", backend(access));
    let (desk, queue, _qh, globals) = session.client();
    assert!(!advertised(&globals, LIST));
    assert!(
        advertised(&globals, "wl_compositor"),
        "the registry was read at all"
    );
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_allowlist_admits_the_programs_it_names_and_no_other() {
    let naming = |entry: String| {
        Access::open().with(
            Protocol::ForeignToplevelList,
            Rule::Only(vec![Program::parse(&entry)]),
        )
    };

    let session = Session::start("list-us", backend(naming(this_binary())));
    let (desk, queue, _qh, globals) = session.client();
    assert!(advertised(&globals, LIST), "named by full path");
    session.stop((desk, queue));

    let session = Session::start("list-them", backend(naming("waybar".to_owned())));
    let (desk, queue, _qh, globals) = session.client();
    assert!(!advertised(&globals, LIST), "not waybar");
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_narrowed_rule_takes_the_list_back() {
    let session = Session::start("list-narrowed", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "first", "first");
    desk.bind_list(&globals, &qh);
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["first"]
    });

    session.command(Command::Protocols(Access::open().with(
        Protocol::ForeignToplevelList,
        Rule::Only(vec![Program::parse("waybar")]),
    )));
    until(&mut queue, &mut desk, |desk| desk.list_finished);
    assert!(desk.listed_titles().is_empty(), "every window closed first");

    // And nothing new reaches it.
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    assert!(desk.listed_titles().is_empty());

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_list_hears_nothing_while_locked_and_everything_after() {
    let session = Session::start("list-locked", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "first", "first");
    desk.bind_list(&globals, &qh);
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["first"]
    });

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    desk.windows[0].set_title("behind the lock");
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("second"))
    });
    queue.roundtrip(&mut desk).expect("dispatch");
    assert_eq!(desk.listed_titles(), ["first"], "nothing while locked");

    desk.unlock();
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["behind the lock", "second"]
    });

    session.stop((desk, queue));
}
