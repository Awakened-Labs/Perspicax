//! A taskbar's controls, by a compositor with no screen:
//! `wlr-foreign-toplevel-management-unstable-v1`, as waybar's `wlr/taskbar`
//! speaks it.
//!
//! The taskbar sees which window has the keyboard, or whose own menu has it,
//! follows it when it moves, and can move it: activating a window gives it the keyboard, restoring it
//! first if it was minimized, and activating a tab behind another brings it
//! forward in its group's place. Closing asks the window's client. While the
//! session is locked, the taskbar can do none of it.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Session, menu_with_the_keyboard, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{HostFacts, SurfaceFacts};
use perspicax_policy::{Access, Action, Program, Protocol, Rule, Shape};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::State;

fn session(name: &str) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![Virtual::numbered(1, (1280, 1024))],
            workspaces: Shape::default(),
            access: Access::open(),
            person: false,
            opacity: Default::default(),
        },
    )
}

fn window<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_taskbar_follows_the_keyboard_and_activating_moves_it() {
    let session = session("taskbar-focus");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);

    desk.open_window(&qh, "first", "org.example.First");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "second", "org.example.Second");
    until(&mut queue, &mut desk, |desk| {
        desk.task("second")
            .is_some_and(|task| task.is(State::Activated))
            && desk
                .task("first")
                .is_some_and(|task| !task.is(State::Activated))
    });
    let first = desk.task("first").unwrap();
    assert_eq!(first.app_id, "org.example.First");
    assert_eq!(first.outputs, 1, "on the one monitor");

    desk.activate("first");
    until(&mut queue, &mut desk, |desk| {
        desk.task("first")
            .is_some_and(|task| task.is(State::Activated))
            && desk
                .task("second")
                .is_some_and(|task| !task.is(State::Activated))
    });

    session.stop((desk, queue));
}

/// Issue #97: a window's own menu takes the keyboard, and the taskbar was
/// told no window was active for as long as the menu was open. The menu's
/// keys are the window's, so the window stays the active task.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_whose_own_menu_is_open_stays_the_active_task() {
    let session = Session::start(
        "taskbar-own-menu",
        Backend::headless((1280, 1024)).with_person(),
    );
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_window(&qh, "page", "org.example.Page");
    until(&mut queue, &mut desk, |desk| {
        desk.task("page")
            .is_some_and(|task| task.is(State::Activated))
    });
    let facts = session.wait_for(|facts| window(facts, "page").is_some());
    let page = window(&facts, "page").expect("waited for").id;

    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);
    queue.roundtrip(&mut desk).expect("the taskbar told");
    assert!(
        desk.task("page")
            .is_some_and(|task| task.is(State::Activated)),
        "the taskbar was told the window was not active while its menu was open"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn minimizing_parks_a_window_and_activating_brings_it_back() {
    let session = session("taskbar-minimize");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.task("first").is_some());

    desk.task("first").unwrap().handle.set_minimized();
    until(&mut queue, &mut desk, |desk| {
        desk.task("first")
            .is_some_and(|task| task.is(State::Minimized))
    });
    session.wait_for(|facts| window(facts, "first").is_some_and(|window| !window.mapped));

    desk.activate("first");
    until(&mut queue, &mut desk, |desk| {
        desk.task("first")
            .is_some_and(|task| !task.is(State::Minimized) && task.is(State::Activated))
    });
    session.wait_for(|facts| window(facts, "first").is_some_and(|window| window.mapped));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn activating_a_tab_behind_brings_it_forward() {
    let session = session("taskbar-tab");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);

    // The focused window joins the one focused before it, in front.
    session.perform(Action::TabWithPrevious);
    session.wait_for(|facts| window(facts, "first").is_some_and(|w| w.behind_tab.is_some()));

    until(&mut queue, &mut desk, |desk| desk.task("first").is_some());
    desk.activate("first");
    queue.roundtrip(&mut desk).expect("send it");
    let facts =
        session.wait_for(|facts| window(facts, "second").is_some_and(|w| w.behind_tab.is_some()));
    assert!(window(&facts, "first").unwrap().mapped);
    until(&mut queue, &mut desk, |desk| {
        desk.task("first")
            .is_some_and(|task| task.is(State::Activated))
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn close_asks_the_client_and_its_going_closes_the_handle() {
    let session = session("taskbar-close");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.task("first").is_some());

    desk.task("first").unwrap().handle.close();
    until(&mut queue, &mut desk, |desk| desk.asked_to_close == 1);

    // The client agrees, by closing it.
    desk.windows.clear();
    until(&mut queue, &mut desk, |desk| desk.task_titles().is_empty());

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_taskbar_can_do_nothing_while_locked() {
    let session = session("taskbar-locked");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.task("first").is_some());

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    desk.task("first").unwrap().handle.set_minimized();
    desk.task("first").unwrap().handle.close();
    queue.roundtrip(&mut desk).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert_eq!(desk.asked_to_close, 0);
    let facts = session.wait_for(|_| true);
    assert!(window(&facts, "first").unwrap().mapped, "not minimized");

    desk.unlock();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_narrowed_rule_takes_the_controls_back() {
    let session = session("taskbar-narrowed");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.task("first").is_some());

    session.command(Command::Protocols(Access::open().with(
        Protocol::ForeignToplevelManagement,
        Rule::Only(vec![Program::parse("waybar")]),
    )));
    until(&mut queue, &mut desk, |desk| desk.taskbar_finished);
    assert!(desk.task_titles().is_empty(), "every window closed first");

    session.stop((desk, queue));
}

/// A taskbar watching must not stop titles reaching it, or the facts, or the
/// titlebar: found on hardware with waybar, where a retitle went nowhere.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_retitle_reaches_the_taskbar_and_the_facts() {
    let session = session("taskbar-retitle");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.bind_list(&globals, &qh);
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.task("first").is_some());

    desk.windows[0].set_title("hello");
    until(&mut queue, &mut desk, |desk| desk.task("hello").is_some());
    session.wait_for(|facts| window(facts, "hello").is_some());

    session.stop((desk, queue));
}
