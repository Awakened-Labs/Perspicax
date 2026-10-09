//! The channel to the desktop shell, `perspicax-shell-v1`, by a compositor
//! with no screen.
//!
//! A binding that asks for the start menu reaches a client holding the
//! channel with the monitor under the pointer; one that asks for the root
//! menu says where on that monitor the pointer is. The shell may end the
//! session. `[protocols] shell` decides who holds the channel, a narrowed
//! rule takes it back, and while the session is locked the shell is told
//! nothing and may end nothing.
//!
//! From version 2 the shell is told the keyboard's layouts and the one in
//! use, whatever switched it, and may switch it; a shell bound at version 1
//! hears none of it.
//!
//! The client is this test binary, so a rule naming `current_exe()` admits
//! it. Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use std::{
    thread,
    time::{Duration, Instant},
};

use common::{Session, Told, advertised, until};
use perspicax_compositor::{Backend, Command, Host, Keymap, Virtual};
use perspicax_index::{Action as Verb, PointerButton};
use perspicax_node::Rect;
use perspicax_policy::{Access, Action, Place, Program, Protocol, Rule, Shape, Side};

const SHELL: &str = "perspicax_shell_v1";

fn backend(access: Access) -> Backend {
    Backend::Headless {
        outputs: vec![
            Virtual::numbered(1, (1280, 1024)),
            Virtual {
                place: Place::Beside {
                    side: Side::RightOf,
                    of: "HEADLESS-1".to_owned(),
                    offset: 0,
                },
                ..Virtual::numbered(2, (1920, 1080))
            },
        ],
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
fn the_start_menu_action_reaches_the_shell_with_its_monitor() {
    let session = Session::start("shell-start", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_shell(&globals, &qh);
    until(&mut queue, &mut desk, |desk| !desk.outputs_empty());

    session.perform(Action::StartMenu);
    until(&mut queue, &mut desk, |desk| !desk.told.is_empty());
    // The pointer has not moved from the desk's top-left corner.
    assert_eq!(desk.told, [Told::StartMenu(Some("HEADLESS-1".to_owned()))]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_root_menu_action_says_where_the_pointer_is() {
    let session = Session::start("shell-root", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "target", "target");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.bind_shell(&globals, &qh);
    queue.roundtrip(&mut desk).expect("bound");
    let facts = session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("target") && surface.mapped)
    });
    let window = facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some("target"))
        .expect("the window");

    // An agent's click puts the pointer on the window, 20 pixels in.
    let host = Host::new(&session.facts, &session.requests);
    host.act(
        window.id,
        &Verb::Click {
            at: Rect::new(10.0, 10.0, 30.0, 30.0),
            button: PointerButton::Left,
        },
    )
    .expect("dispatched");
    session.perform(Action::RootMenu);
    until(&mut queue, &mut desk, |desk| !desk.told.is_empty());

    let (x, y) = (
        window.geometry.x0 as i32 + 20,
        window.geometry.y0 as i32 + 20,
    );
    assert_eq!(
        desk.told,
        [Told::RootMenu(Some("HEADLESS-1".to_owned()), x, y)]
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_pie_is_asked_for_by_name_where_the_pointer_is_of_a_shell_that_knows_pies() {
    let session = Session::start("shell-pie", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "target", "target");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.bind_shell(&globals, &qh);
    queue.roundtrip(&mut desk).expect("bound");
    // A shell from before pies, which must not hear of one.
    let (mut old, mut old_queue, old_qh, old_globals) = session.client();
    old.bind_shell_v2(&old_globals, &old_qh);
    old_queue.roundtrip(&mut old).expect("bound");
    let facts = session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("target") && surface.mapped)
    });
    let window = facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some("target"))
        .expect("the window");

    let host = Host::new(&session.facts, &session.requests);
    host.act(
        window.id,
        &Verb::Click {
            at: Rect::new(30.0, 10.0, 50.0, 30.0),
            button: PointerButton::Left,
        },
    )
    .expect("dispatched");
    session.perform(Action::Pie("launchers".to_owned()));
    until(&mut queue, &mut desk, |desk| !desk.told.is_empty());

    let (x, y) = (
        window.geometry.x0 as i32 + 40,
        window.geometry.y0 as i32 + 20,
    );
    assert_eq!(
        desk.told,
        [Told::PieMenu(
            Some("HEADLESS-1".to_owned()),
            x,
            y,
            "launchers".to_owned()
        )]
    );
    settle(&mut old_queue, &mut old);
    assert!(old.told.is_empty(), "{:?}", old.told);

    session.stop(((desk, queue), (old, old_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_shell_is_told_nothing_and_ends_nothing_while_locked() {
    let session = Session::start("shell-locked", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_shell(&globals, &qh);
    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);

    session.perform(Action::StartMenu);
    session.perform(Action::RootMenu);
    session.perform(Action::Pie("launchers".to_owned()));
    desk.shell.as_ref().expect("bound").exit_session();
    queue.roundtrip(&mut desk).expect("dispatch");
    thread::sleep(Duration::from_millis(200));
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(desk.told.is_empty(), "{:?}", desk.told);
    assert!(!session.ended(), "a locked session cannot be logged out of");

    desk.unlock();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn there_is_no_shell_global_when_the_rule_is_off() {
    let access = Access::open().with(Protocol::Shell, Rule::Off);
    let session = Session::start("shell-off", backend(access));
    let (desk, queue, _qh, globals) = session.client();
    assert!(!advertised(&globals, SHELL));
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_shell_rule_admits_the_program_it_names_and_no_other() {
    let naming =
        |entry: &str| Access::open().with(Protocol::Shell, Rule::Only(vec![Program::parse(entry)]));

    let session = Session::start("shell-us", backend(naming(&this_binary())));
    let (desk, queue, _qh, globals) = session.client();
    assert!(advertised(&globals, SHELL), "named by full path");
    session.stop((desk, queue));

    let session = Session::start("shell-them", backend(naming("perspicax-shell")));
    let (desk, queue, _qh, globals) = session.client();
    assert!(!advertised(&globals, SHELL), "this is not perspicax-shell");
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_narrowed_rule_takes_the_shell_channel_back() {
    let session = Session::start("shell-narrowed", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_shell(&globals, &qh);
    queue.roundtrip(&mut desk).expect("dispatch");

    session.command(Command::Protocols(Access::open().with(
        Protocol::Shell,
        Rule::Only(vec![Program::parse("perspicax-shell")]),
    )));
    until(&mut queue, &mut desk, |desk| desk.shell_finished);
    session.perform(Action::StartMenu);
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(desk.told.is_empty(), "{:?}", desk.told);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn exit_session_ends_a_headless_session() {
    let session = Session::start("shell-exit", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_shell(&globals, &qh);
    queue.roundtrip(&mut desk).expect("dispatch");

    desk.shell.as_ref().expect("bound").exit_session();
    queue.flush().expect("sent");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !session.ended() {
        assert!(Instant::now() < deadline, "the session never ended");
        thread::sleep(Duration::from_millis(20));
    }

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_shell_is_told_the_layouts_and_the_one_in_use_and_may_switch_it() {
    let session = Session::start("shell-layouts", backend(Access::open()));
    session.command(Command::Keymap(Keymap {
        layout: "us,ru".to_owned(),
        ..Keymap::default()
    }));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_shell(&globals, &qh);
    until(&mut queue, &mut desk, |desk| {
        desk.layouts.len() == 2 && desk.active_layout == Some(0)
    });
    let named = |name: &str, short: &str| (name.to_owned(), short.to_owned());
    assert_eq!(
        desk.layouts,
        [named("English (US)", "US"), named("Russian", "RU")]
    );

    // A switch from anywhere reaches it: a binding's, then its own.
    session.perform(Action::Layout(2));
    until(&mut queue, &mut desk, |desk| desk.active_layout == Some(1));
    let shell = desk.shell.clone().expect("bound");
    shell.set_layout(0);
    until(&mut queue, &mut desk, |desk| desk.active_layout == Some(0));
    // One past the last is no layout: nothing happens.
    let before = desk.layout_events;
    shell.set_layout(2);
    settle(&mut queue, &mut desk);
    assert_eq!(desk.layout_events, before, "no layout 2 to switch to");
    session.perform(Action::Layout(2));
    until(&mut queue, &mut desk, |desk| desk.active_layout == Some(1));

    // While locked it is told nothing; once unlocked, what changed.
    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    let before = desk.layout_events;
    session.perform(Action::Layout(1));
    settle(&mut queue, &mut desk);
    assert_eq!(desk.layout_events, before, "nothing while locked");
    desk.unlock();
    until(&mut queue, &mut desk, |desk| desk.active_layout == Some(0));
    assert_eq!(
        desk.layout_events,
        before + 1,
        "only the one in use changed"
    );

    // A shell bound at version 1 hears none of it.
    let (mut old, mut old_queue, old_qh, old_globals) = session.client();
    old.bind_shell_v1(&old_globals, &old_qh);
    session.perform(Action::Layout(2));
    until(&mut queue, &mut desk, |desk| desk.active_layout == Some(1));
    settle(&mut old_queue, &mut old);
    assert_eq!(old.layout_events, 0);

    session.stop(((desk, queue), (old, old_queue)));
}

/// A few turns of the compositor's loop, for what would be sent to arrive.
fn settle<D>(queue: &mut wayland_client::EventQueue<D>, state: &mut D) {
    for _ in 0..3 {
        queue.roundtrip(state).expect("dispatch");
        thread::sleep(Duration::from_millis(30));
    }
}
