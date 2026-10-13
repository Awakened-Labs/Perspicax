//! A window in the person's hand, as the workspaces meet it, by a compositor
//! with no screen.
//!
//! A window being dragged goes where the person goes. Switch workspace while
//! holding it -- by a key, by a number, by a pager -- and it comes along, as
//! resting it against an edge of the desk always took it: it is on screen
//! because it is in the person's hand, and the workspaces say so, rather than
//! drawing it on one workspace while they keep it on another, to vanish at
//! the next switch. Sent away while held, it takes the person with it,
//! because nothing held can be left behind. Per output, it goes with the
//! monitor under it, which is not always the one it left.
//!
//! The person's hand is [`Command::Press`], a client asking to be moved, and
//! [`Command::Motion`] and [`Command::Release`], which find what is under the
//! pointer from the clients' buffers, so this needs the `capture` feature.
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Session, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::HostFacts;
use perspicax_node::Rect;
use perspicax_policy::{Access, Action, Button, Direction, Grid, Mode, Mods, Place, Shape, Side};
use wayland_client::{EventQueue, QueueHandle};

/// Where a window is, as the facts an agent reads tell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// On screen.
    Shown,
    /// On another workspace, counting from 1.
    Away(u16),
    /// Off screen, and on no other workspace: minimized.
    Hidden,
}

/// Two monitors side by side, a 2x2 grid of workspaces, and a person at
/// them: only a person's press may start a move.
fn session(name: &str, mode: Mode) -> Session {
    Session::start(
        name,
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
            workspaces: Shape {
                mode,
                grid: Grid {
                    columns: 2,
                    rows: 2,
                    wrap: false,
                },
            },
            access: Access::open(),
            person: true,
            opacity: Default::default(),
            agent: Default::default(),
        },
    )
}

/// Two windows on the first monitor, "stays" and, cascaded over it with
/// the keyboard, "held", drawn and described, with our pointer bound.
fn with_windows(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.bind_pager(&globals, &qh);
    for (n, title) in ["stays", "held"].into_iter().enumerate() {
        desk.open_window(&qh, title, title);
        until(&mut queue, &mut desk, |desk| desk.has_drawn(n));
    }
    until(&mut queue, &mut desk, |desk| desk.pager_done > 0);
    session.wait_for(|facts| {
        ["stays", "held"]
            .iter()
            .all(|title| seen(facts, title) == Seen::Shown)
    });
    (desk, queue, qh)
}

/// The person takes "held" by its client, which asks to be moved, as a
/// toolkit drawing its own titlebar does, and keeps the button down.
fn take_held(session: &Session, desk: &mut Desk, queue: &mut EventQueue<Desk>) {
    let at = centre(geometry(&session.facts.read(), "held"));
    desk.pressed = None;
    session.command(Command::Press {
        at,
        button: Button::Left,
        mods: Mods::default(),
    });
    until(queue, desk, |desk| desk.pressed.is_some());
    let seat = desk.seat.clone().expect("our pointer's seat");
    desk.windows[1].move_(&seat, desk.pressed.expect("waited for"));
    // A move takes the pointer from every client.
    until(queue, desk, |desk| desk.pointed.is_none());
}

/// The person letting go of it.
fn release(session: &Session) {
    session.command(Command::Release {
        button: Button::Left,
    });
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_in_hand_comes_along_when_a_key_switches_workspace() {
    let session = session("in-hand-key", Mode::Spanning);
    let (mut desk, mut queue, _qh) = with_windows(&session);
    let from = geometry(&session.facts.read(), "held");

    take_held(&session, &mut desk, &mut queue);
    session.perform(Action::Workspace(Direction::Right));
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Shown && seen(facts, "stays") == Seen::Away(1)
    });

    // Still in hand on workspace 2, and put down there.
    session.command(Command::Motion { by: (40, 30) });
    release(&session);
    session.wait_for(|facts| geometry(facts, "held").x0 == from.x0 + 40.0);

    // Where it was put down, it stays: workspace 2's, not 1's.
    session.perform(Action::Workspace(Direction::Left));
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Away(2) && seen(facts, "stays") == Seen::Shown
    });
    session.perform(Action::Workspace(Direction::Right));
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Shown && seen(facts, "stays") == Seen::Away(1)
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_in_hand_comes_along_whatever_switches_workspace() {
    let session = session("in-hand-any", Mode::Spanning);
    let (mut desk, mut queue, _qh) = with_windows(&session);

    take_held(&session, &mut desk, &mut queue);
    // Straight to one by its number.
    session.perform(Action::GoToWorkspace(4));
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Shown && seen(facts, "stays") == Seen::Away(1)
    });
    // A pager, as waybar's buttons switch.
    desk.switch_to("3");
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["3"]
    });
    session.wait_for(|facts| seen(facts, "held") == Seen::Shown);
    // A step back through them, as a scroll over the desktop takes.
    session.perform(Action::CycleWorkspace { forward: false });
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["2"]
    });
    session.wait_for(|facts| seen(facts, "held") == Seen::Shown);
    release(&session);

    // It was put down on the last of them.
    session.perform(Action::GoToWorkspace(1));
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Away(2) && seen(facts, "stays") == Seen::Shown
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_sent_away_in_hand_takes_the_person_along() {
    let session = session("in-hand-sent", Mode::Spanning);
    let (mut desk, mut queue, _qh) = with_windows(&session);

    take_held(&session, &mut desk, &mut queue);
    session.perform(Action::SendToWorkspace(Direction::Right));
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Shown && seen(facts, "stays") == Seen::Away(1)
    });
    release(&session);

    session.perform(Action::Workspace(Direction::Left));
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Away(2) && seen(facts, "stays") == Seen::Shown
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn per_output_a_window_in_hand_goes_with_the_monitor_under_it() {
    let session = session("in-hand-per-output", Mode::PerOutput);
    let (mut desk, mut queue, _qh) = with_windows(&session);
    let from = geometry(&session.facts.read(), "held");

    // Across onto the right monitor, which flips on its own, while the
    // window is still on the left monitor's workspace until it is put down.
    take_held(&session, &mut desk, &mut queue);
    session.command(Command::Motion { by: (1280, 0) });
    session.perform(Action::Workspace(Direction::Right));
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["1", "2"]
    });
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Shown && seen(facts, "stays") == Seen::Shown
    });
    release(&session);
    session.wait_for(|facts| geometry(facts, "held").x0 == from.x0 + 1280.0);

    // It is on the right monitor's workspace 2, and the left one never moved.
    session.perform(Action::Workspace(Direction::Left));
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["1", "1"]
    });
    session.wait_for(|facts| {
        seen(facts, "held") == Seen::Away(2) && seen(facts, "stays") == Seen::Shown
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_minimized_in_hand_stays_where_it_was_put_away() {
    let session = session("in-hand-minimized", Mode::Spanning);
    let (mut desk, mut queue, _qh) = with_windows(&session);
    let from = geometry(&session.facts.read(), "held");

    take_held(&session, &mut desk, &mut queue);
    session.perform(Action::Minimize);
    session.wait_for(|facts| seen(facts, "held") == Seen::Hidden);
    // The hand goes on with nothing in it, and lets go of nothing.
    session.command(Command::Motion { by: (40, 30) });
    release(&session);

    // Brought back, it is where it was minimized, not where the hand went.
    session.perform(Action::CycleFocus);
    let back = session.wait_for(|facts| seen(facts, "held") == Seen::Shown);
    assert_eq!(geometry(&back, "held"), from);

    session.stop((desk, queue));
}

/// Where the window titled `title` is.
fn seen(facts: &HostFacts, title: &str) -> Seen {
    let surface = facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .unwrap_or_else(|| panic!("no window titled {title}"));
    match (surface.mapped, surface.off_workspace) {
        (true, _) => Seen::Shown,
        (false, Some(workspace)) => Seen::Away(workspace),
        (false, None) => Seen::Hidden,
    }
}

fn geometry(facts: &HostFacts, title: &str) -> Rect {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map_or(Rect::ZERO, |surface| surface.geometry)
}

/// A rect's centre, in whole pixels, for a person's hand to aim at.
fn centre(at: Rect) -> (i32, i32) {
    (
        ((at.x0 + at.x1) / 2.0).round() as i32,
        ((at.y0 + at.y1) / 2.0).round() as i32,
    )
}
