//! A game's hold on the pointer, as a person's mouse meets it, by a
//! compositor with no screen.
//!
//! A game turns its camera by how far the mouse went, not by where the
//! pointer is, and reads that from `zwp_relative_pointer_v1`: every motion of
//! the mouse reaches the client under the pointer, even where the pointer
//! could not follow it, pinned against the edge of the screen.
//!
//! The person's mouse is [`Command::Motion`], which finds what is under the
//! pointer from the clients' buffers, so this needs the `capture` feature.
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Session, until};
use perspicax_compositor::{Backend, Command};
use wayland_client::{EventQueue, QueueHandle};

/// One monitor of `size`, with a person at it.
fn session(name: &str, size: (i32, i32)) -> Session {
    Session::start(name, Backend::headless(size).with_person())
}

/// One window, drawn at the top left of the monitor, with our pointer and
/// the mouse's own motion bound.
fn with_window(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.bind_relative(&globals, &qh);
    desk.open_window(&qh, "game", "game");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    // Drawn is not yet described: a motion before the window is in the
    // space would find nothing under the pointer.
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("game"))
    });
    (desk, queue, qh)
}

/// A person's mouse moving by `by`.
fn nudge(session: &Session, by: (i32, i32)) {
    session.command(Command::Motion { by });
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_mouse_s_own_motion_reaches_the_window_under_the_pointer() {
    let session = session("hold-relative", (1280, 800));
    let (mut desk, mut queue, _qh) = with_window(&session);

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
    let (mut desk, mut queue, _qh) = with_window(&session);

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
