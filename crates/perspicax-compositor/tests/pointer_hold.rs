//! A game's hold on the pointer, as a person's mouse meets it, by a
//! compositor with no screen.
//!
//! A game turns its camera by how far the mouse went, not by where the
//! pointer is, and reads that from `zwp_relative_pointer_v1`: every motion of
//! the mouse reaches the client under the pointer, even where the pointer
//! could not follow it, pinned against the edge of the screen.
//!
//! To keep the pointer from running off while it turns, it holds it with
//! `zwp_pointer_constraints_v1`: locked in place, when no client is told the
//! pointer moved and only the mouse's motion arrives, or confined to a
//! region of its window, which the pointer slides along the edge of. Only
//! the window in use may, and only once the pointer is in the region it
//! named. Let go, the pointer is where the game last said it drew it.
//!
//! The person takes it back as they leave any window: moving the keyboard
//! elsewhere, a launcher taking it, the lock screen, another workspace.
//! What is merely drawn over the game -- a notification -- takes nothing.
//!
//! The person's mouse is [`Command::Motion`], which finds what is under the
//! pointer from the clients' buffers, so this needs the `capture` feature.
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Session, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_policy::{Access, Action, Grid, Mode, Shape};
use smithay_client_toolkit::shell::{WaylandSurface, wlr_layer::Layer};
use wayland_client::{EventQueue, QueueHandle, globals::GlobalList};
use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1::Lifetime;

type Client = (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList);

/// One monitor of `size`, with a person at it.
fn session(name: &str, size: (i32, i32)) -> Session {
    Session::start(name, Backend::headless(size).with_person())
}

/// One window, drawn at the top left of the monitor, with our pointer and
/// the mouse's own motion bound.
fn with_window(session: &Session) -> Client {
    with_windows(session, &["game"])
}

/// Windows with these titles, opened in order, each drawn 400x300 and
/// cascaded 32 pixels on from the one before, the last on top with the
/// keyboard; with our pointer and the mouse's own motion bound.
fn with_windows(session: &Session, titles: &[&str]) -> Client {
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.bind_relative(&globals, &qh);
    for (n, title) in titles.iter().enumerate() {
        desk.open_window(&qh, title, title);
        until(&mut queue, &mut desk, |desk| desk.drawn == n + 1);
    }
    // Drawn is not yet described: a motion before a window is in the
    // space would find nothing under the pointer.
    session.wait_for(|facts| {
        titles.iter().all(|title| {
            facts
                .surfaces()
                .iter()
                .any(|surface| surface.title.as_deref() == Some(*title))
        })
    });
    (desk, queue, qh, globals)
}

/// Lock the pointer over the `nth` window, at `at` from the origin: it has
/// to be the window in use.
fn lock_over(
    client: &mut Client,
    session: &Session,
    nth: usize,
    at: (i32, i32),
    lifetime: Lifetime,
) {
    let (desk, queue, qh, globals) = client;
    nudge(session, at);
    until(queue, desk, |desk| desk.entered.is_some());
    desk.hold_pointer(globals, qh, nth, true, None, lifetime);
    until(queue, desk, |desk| desk.holds == ["locked"]);
}

/// Whether the last the client heard is that it holds the pointer.
///
/// What it heard is counted rather than matched: smithay tells a client its
/// hold let go once more when the pointer leaves its surface, whether it
/// held or not, so an `unlocked` may come twice.
fn holding(desk: &Desk) -> bool {
    matches!(desk.holds.last(), Some(&("locked" | "confined")))
}

/// How many times the client was told its hold took.
fn taken(desk: &Desk) -> usize {
    desk.holds
        .iter()
        .filter(|heard| matches!(**heard, "locked" | "confined"))
        .count()
}

/// A person's mouse moving by `by`.
fn nudge(session: &Session, by: (i32, i32)) {
    session.command(Command::Motion { by });
}

/// Wait until everything asked of the compositor so far has been done.
/// What the client asked comes first, by a roundtrip: a command reaches the
/// compositor on a channel of its own, and would overtake a request still
/// waiting in the client's buffer. Then the commands: a mouse motion of
/// nothing, which reaches the client however the pointer is held, is
/// answered after all of them.
fn settle(session: &Session, queue: &mut EventQueue<Desk>, desk: &mut Desk) {
    queue.roundtrip(desk).expect("a roundtrip");
    let heard = desk.relative_motions.len();
    nudge(session, (0, 0));
    until(queue, desk, |desk| desk.relative_motions.len() > heard);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_mouse_s_own_motion_reaches_the_window_under_the_pointer() {
    let session = session("hold-relative", (1280, 800));
    let (mut desk, mut queue, _qh, _globals) = with_window(&session);

    // From the origin, where the pointer starts, onto the window.
    nudge(&session, (100, 80));
    until(&mut queue, &mut desk, |desk| {
        desk.entered.is_some() && !desk.relative_motions.is_empty()
    });
    assert_eq!(desk.relative_motions, [((100.0, 80.0), (100.0, 80.0))]);

    nudge(&session, (5, -3));
    until(&mut queue, &mut desk, |desk| {
        desk.relative_motions.len() == 2
    });
    assert_eq!(desk.relative_motions[1], ((5.0, -3.0), (5.0, -3.0)));
    let moved = desk.motions.last().copied().expect("a motion");
    assert_eq!(moved, (105.0, 77.0), "the pointer went with the mouse");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn pinned_at_the_edge_of_the_screen_the_mouse_still_turns_the_game() {
    // A window bigger than the monitor, so it is under the pointer
    // wherever on the monitor the pointer is.
    let session = session("hold-edge", (300, 200));
    let (mut desk, mut queue, _qh, _globals) = with_window(&session);

    // Onto the window, which is an arrival, and then to the right edge.
    nudge(&session, (10, 10));
    until(&mut queue, &mut desk, |desk| desk.entered.is_some());
    nudge(&session, (1000, 90));
    until(&mut queue, &mut desk, |desk| {
        desk.relative_motions.len() == 2
    });
    let pinned = desk.motions.last().copied().expect("a motion");
    assert_eq!(pinned, (299.0, 100.0), "on the monitor's last pixel");

    // Further right: the pointer cannot go, and the game hears every bit
    // of it anyway.
    nudge(&session, (40, 0));
    nudge(&session, (40, 0));
    until(&mut queue, &mut desk, |desk| {
        desk.relative_motions.len() == 4
    });
    assert_eq!(
        desk.relative_motions[2..],
        [((40.0, 0.0), (40.0, 0.0)), ((40.0, 0.0), (40.0, 0.0))]
    );
    assert_eq!(desk.motions.last().copied(), Some(pinned));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_game_locks_the_pointer_in_place_and_the_mouse_still_turns_it() {
    let session = session("hold-lock", (1280, 800));
    let (mut desk, mut queue, qh, globals) = with_window(&session);
    nudge(&session, (100, 80));
    until(&mut queue, &mut desk, |desk| desk.entered.is_some());

    desk.hold_pointer(&globals, &qh, 0, true, None, Lifetime::Persistent);
    until(&mut queue, &mut desk, |desk| desk.holds == ["locked"]);
    let moved = desk.motions.len();
    nudge(&session, (300, 0));
    nudge(&session, (0, -50));
    until(&mut queue, &mut desk, |desk| {
        desk.relative_motions.len() == 3
    });
    assert_eq!(
        desk.relative_motions[1..],
        [((300.0, 0.0), (300.0, 0.0)), ((0.0, -50.0), (0.0, -50.0))]
    );
    assert_eq!(
        desk.motions.len(),
        moved,
        "no client is told a locked pointer moved"
    );

    // Let go, it is where it was held.
    desk.release();
    settle(&session, &mut queue, &mut desk);
    nudge(&session, (1, 0));
    until(&mut queue, &mut desk, |desk| desk.motions.len() > moved);
    assert_eq!(desk.motions.last().copied(), Some((101.0, 80.0)));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_confined_pointer_stops_at_the_edge_of_its_region_and_slides_along_it() {
    let session = session("hold-confine", (1280, 800));
    let (mut desk, mut queue, qh, globals) = with_window(&session);
    nudge(&session, (60, 60));
    until(&mut queue, &mut desk, |desk| desk.entered.is_some());

    desk.hold_pointer(
        &globals,
        &qh,
        0,
        false,
        Some((50, 50, 100, 100)),
        Lifetime::Persistent,
    );
    until(&mut queue, &mut desk, |desk| desk.holds == ["confined"]);

    // Out through the right side: to the edge, and down along it.
    nudge(&session, (200, 10));
    until(&mut queue, &mut desk, |desk| {
        desk.motions.last() == Some(&(149.0, 70.0))
    });
    // Straight down, past the bottom: into the corner.
    nudge(&session, (0, 500));
    until(&mut queue, &mut desk, |desk| {
        desk.motions.last() == Some(&(149.0, 149.0))
    });

    // The region shrinks away from the pointer: it comes back inside.
    desk.confine_to(0, (50, 50, 40, 40));
    settle(&session, &mut queue, &mut desk);
    nudge(&session, (1, 0));
    until(&mut queue, &mut desk, |desk| {
        desk.motions.last() == Some(&(89.0, 89.0))
    });
    assert_eq!(desk.holds, ["confined"], "held throughout");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_hold_waits_until_the_pointer_is_in_its_region() {
    let session = session("hold-region", (1280, 800));
    let (mut desk, mut queue, qh, globals) = with_window(&session);
    nudge(&session, (10, 10));
    until(&mut queue, &mut desk, |desk| desk.entered.is_some());

    desk.hold_pointer(
        &globals,
        &qh,
        0,
        true,
        Some((100, 100, 50, 50)),
        Lifetime::Persistent,
    );
    settle(&session, &mut queue, &mut desk);
    assert!(desk.holds.is_empty(), "not in the region: {:?}", desk.holds);

    nudge(&session, (110, 110));
    until(&mut queue, &mut desk, |desk| desk.holds == ["locked"]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_not_in_use_cannot_take_the_pointer() {
    let session = session("hold-not-in-use", (1280, 800));
    // "front" opens at (32, 32), over "back", and has the keyboard.
    let (mut desk, mut queue, qh, globals) = with_windows(&session, &["back", "front"]);
    nudge(&session, (10, 10));
    until(&mut queue, &mut desk, |desk| desk.entered.is_some());

    desk.hold_pointer(&globals, &qh, 0, true, None, Lifetime::Persistent);
    nudge(&session, (2, 2));
    settle(&session, &mut queue, &mut desk);
    assert!(
        desk.holds.is_empty(),
        "\"back\" is not in use: {:?}",
        desk.holds
    );

    // The person brings it forward: now it may.
    session.perform(Action::CycleFocus);
    nudge(&session, (1, 1));
    until(&mut queue, &mut desk, |desk| desk.holds == ["locked"]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn let_go_the_pointer_is_where_the_game_last_drew_it() {
    let session = session("hold-hint", (1280, 800));
    let (mut desk, mut queue, qh, globals) = with_window(&session);
    nudge(&session, (100, 80));
    until(&mut queue, &mut desk, |desk| desk.entered.is_some());
    desk.hold_pointer(&globals, &qh, 0, true, None, Lifetime::Persistent);
    until(&mut queue, &mut desk, |desk| desk.holds == ["locked"]);

    desk.hint(0, 250.0, 150.0);
    desk.release();
    settle(&session, &mut queue, &mut desk);
    let moved = desk.motions.len();
    nudge(&session, (1, 0));
    until(&mut queue, &mut desk, |desk| desk.motions.len() > moved);
    assert_eq!(desk.motions.last().copied(), Some((251.0, 150.0)));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn leaving_the_game_lets_go_and_coming_back_takes_hold_again() {
    let session = session("hold-leave", (1280, 800));
    // "other" opens over "game" at (32, 32) with the keyboard. The person
    // comes back to the game, which comes up over it, and the pointer rests
    // where the two overlap.
    let mut client = with_windows(&session, &["game", "other"]);
    session.perform(Action::CycleFocus);
    lock_over(&mut client, &session, 0, (100, 100), Lifetime::Persistent);
    let (mut desk, mut queue, ..) = client;
    let (game, other) = (
        desk.windows[0].wl_surface().clone(),
        desk.windows[1].wl_surface().clone(),
    );

    // Away: "other" comes up under the still pointer, and has it.
    session.perform(Action::CycleFocus);
    until(&mut queue, &mut desk, |desk| {
        !holding(desk) && desk.pointed.as_ref() == Some(&other)
    });
    // Back, and the game has it and holds it again with no nudge: the
    // camera turns at once.
    session.perform(Action::CycleFocus);
    until(&mut queue, &mut desk, |desk| {
        holding(desk) && taken(desk) == 2 && desk.pointed.as_ref() == Some(&game)
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_oneshot_hold_is_gone_once_it_lets_go() {
    let session = session("hold-oneshot", (1280, 800));
    let mut client = with_windows(&session, &["game", "other"]);
    session.perform(Action::CycleFocus);
    lock_over(&mut client, &session, 0, (10, 10), Lifetime::Oneshot);
    let (mut desk, mut queue, ..) = client;

    session.perform(Action::CycleFocus);
    until(&mut queue, &mut desk, |desk| !holding(desk));
    session.perform(Action::CycleFocus);
    nudge(&session, (1, 1));
    settle(&session, &mut queue, &mut desk);
    assert!(!holding(&desk) && taken(&desk) == 1, "{:?}", desk.holds);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_lock_screen_takes_the_pointer_back() {
    let session = session("hold-lock-screen", (1280, 800));
    let mut client = with_window(&session);
    lock_over(&mut client, &session, 0, (100, 80), Lifetime::Persistent);
    let (mut desk, mut queue, qh, globals) = client;

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked && !holding(desk));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn another_workspace_takes_the_pointer_back() {
    let session = Session::start(
        "hold-workspace",
        Backend::Headless {
            outputs: vec![Virtual::numbered(1, (1280, 800))],
            workspaces: Shape {
                mode: Mode::Spanning,
                grid: Grid {
                    columns: 2,
                    rows: 1,
                    wrap: false,
                },
            },
            access: Access::open(),
            person: true,
            opacity: Default::default(),
        },
    );
    let mut client = with_window(&session);
    lock_over(&mut client, &session, 0, (100, 80), Lifetime::Persistent);
    let (mut desk, mut queue, ..) = client;

    session.perform(Action::GoToWorkspace(2));
    until(&mut queue, &mut desk, |desk| {
        !holding(desk) && desk.pointed.is_none()
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn something_drawn_over_a_held_pointer_does_not_take_it() {
    let session = session("hold-overlay", (1280, 800));
    let mut client = with_window(&session);
    lock_over(&mut client, &session, 0, (100, 80), Lifetime::Persistent);
    let (mut desk, mut queue, qh, _globals) = client;
    let game = desk.windows[0].wl_surface().clone();

    // A notification across the top, over the pointer, taking no keys.
    desk.open_strip(&qh, Layer::Overlay, "toast", 150, 0xff_20_40_80);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    nudge(&session, (5, 5));
    settle(&session, &mut queue, &mut desk);
    assert_eq!(desk.holds, ["locked"]);
    assert_eq!(
        desk.pointed.as_ref(),
        Some(&game),
        "the game still has the pointer"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_that_takes_the_keyboard_takes_the_pointer_too() {
    let session = session("hold-launcher", (1280, 800));
    let mut client = with_window(&session);
    lock_over(&mut client, &session, 0, (100, 80), Lifetime::Persistent);
    let (mut desk, mut queue, qh, _globals) = client;

    desk.open_launcher(&qh, 150, 0xff_80_40_20);
    let launcher = desk.layers[0].0.wl_surface().clone();
    until(&mut queue, &mut desk, |desk| {
        !holding(desk) && desk.pointed.as_ref() == Some(&launcher)
    });

    session.stop((desk, queue));
}
