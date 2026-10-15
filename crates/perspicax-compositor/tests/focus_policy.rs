//! The focus policy, as a person's hand meets it, by a compositor with no
//! screen.
//!
//! Headless has no focus policy of its own: nobody points at anything there,
//! and an agent's clicks go to the pointer without one. A test puts a seat's
//! in force with [`Command::FocusPolicy`], and the stand-in hand of
//! [`Command::Motion`] and [`Command::Click`] then moves the keyboard as a
//! person's does: under sloppy focus to the window it moves onto, under
//! click-to-focus to the window it clicks, which comes to the top. Until a
//! test does, a hand moves no keyboard, which every other test relies on.
//!
//! The hand finds what is under the pointer from the clients' buffers, so
//! this needs the `capture` feature. Like the other live tests it binds a
//! real Wayland socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d
//! for `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Session, until};
use perspicax_compositor::{Backend, Command};
use perspicax_index::HostFacts;
use perspicax_node::Rect;
use perspicax_policy::{Button, Focus, FocusModel, Mods};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::EventQueue;

/// "first", then "second", which opens down and right of it and so has the
/// keyboard and the top of the stack; with our pointer bound, at the origin
/// where it starts.
fn two_windows(name: &str) -> (Session, Desk, EventQueue<Desk>) {
    let session = Session::start(name, Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    session
        .wait_for(|facts| mapped(facts) == 2 && focused_last(facts).as_deref() == Some("second"));
    (session, desk, queue)
}

fn policy(model: FocusModel) -> Command {
    Command::FocusPolicy(Focus {
        model,
        autoraise: false,
    })
}

fn mapped(facts: &HostFacts) -> usize {
    facts.surfaces().iter().filter(|s| s.mapped).count()
}

fn geometry(facts: &HostFacts, title: &str) -> Rect {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .expect("a window of that title")
        .geometry
}

/// The title of the window that took the keyboard last.
fn focused_last(facts: &HostFacts) -> Option<String> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.focused_at.is_some())
        .max_by_key(|surface| surface.focused_at)?
        .title
        .clone()
}

/// The title of the window on top, the facts giving them bottom to top.
fn on_top(facts: &HostFacts) -> Option<String> {
    facts
        .surfaces()
        .iter()
        .rfind(|surface| surface.mapped && surface.title.is_some())?
        .title
        .clone()
}

/// A point of "first" that "second" does not cover: its top left, since
/// "second" opened down and right of it.
fn first_alone(facts: &HostFacts) -> (i32, i32) {
    let first = geometry(facts, "first");
    let second = geometry(facts, "second");
    assert!(
        second.x0 > first.x0 + 10.0 && second.y0 > first.y0 + 10.0,
        "cascaded: {first:?} under {second:?}"
    );
    ((first.x0 + 5.0) as i32, (first.y0 + 5.0) as i32)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_stand_in_hand_under_sloppy_focus_gives_the_keyboard_to_the_window_it_moves_onto() {
    let (session, mut desk, mut queue) = two_windows("focus-policy-sloppy");
    let facts = session.wait_for(|_| true);
    let to = first_alone(&facts);
    session.command(policy(FocusModel::Sloppy));

    session.command(Command::Motion { by: to });

    let facts = session.wait_for(|facts| focused_last(facts).as_deref() == Some("first"));
    assert_eq!(
        on_top(&facts).as_deref(),
        Some("second"),
        "sloppy focus without autoraise raises nothing"
    );
    let first = desk.windows[0].wl_surface().clone();
    until(&mut queue, &mut desk, |desk| {
        desk.pointed.as_ref() == Some(&first)
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_stand_in_click_under_click_to_focus_focuses_and_raises_what_it_clicks() {
    let (session, mut desk, mut queue) = two_windows("focus-policy-click");
    let facts = session.wait_for(|_| true);
    let at = first_alone(&facts);
    session.command(policy(FocusModel::Click));

    session.command(Command::Click {
        at,
        button: Button::Left,
        mods: Mods::default(),
    });

    session.wait_for(|facts| {
        focused_last(facts).as_deref() == Some("first") && on_top(facts).as_deref() == Some("first")
    });
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn with_no_focus_policy_in_force_a_stand_in_hand_moves_no_keyboard() {
    let (session, mut desk, mut queue) = two_windows("focus-policy-none");
    let facts = session.wait_for(|_| true);
    let at = first_alone(&facts);

    session.command(Command::Motion { by: at });
    session.command(Command::Click {
        at,
        button: Button::Left,
        mods: Mods::default(),
    });

    // The click reached the window under it: both commands are done.
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);
    let facts = session.wait_for(|_| true);
    assert_eq!(focused_last(&facts).as_deref(), Some("second"));
    assert_eq!(on_top(&facts).as_deref(), Some("second"));

    session.stop((desk, queue));
}
