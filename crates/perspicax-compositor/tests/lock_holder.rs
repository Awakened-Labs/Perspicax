//! Who may unlock the session, by a compositor with no screen:
//! `ext-session-lock-v1`, as swaylock speaks it (#126).
//!
//! One lock holds the session, and only it unlocks it. A second lock while
//! the holder's client lives is told `finished`, as swaylock is when
//! swayidle starts a second one, and neither its lock surfaces nor its
//! `unlock_and_destroy` take anything from the first: the unlock is the
//! protocol error it is, and the session stays locked. Before, any client
//! could lock and unlock straight away, and open a session its owner had
//! locked and walked away from.
//!
//! A holder whose client goes without unlocking leaves the session locked,
//! and the next lock takes it over: how a person recovers from a crashed
//! swaylock.
//!
//! What covers the monitors is read from the facts, where a lock surface is
//! a `LockCover`. Whether the session is locked is read from the page's
//! keyboard, which a window has only while it is not. Like the other live
//! tests these bind a real Wayland socket, so they need `XDG_RUNTIME_DIR`,
//! and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use std::net::Shutdown;

use common::{Desk, Ear, Session, client_over, heard, refused, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, SurfaceKind};
use perspicax_node::SurfaceId;
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{
    Connection, EventQueue, Proxy as _, QueueHandle, globals::GlobalList, protocol::wl_surface,
};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;

const PAGE: u32 = 0xff33_6699;
const COVER: u32 = 0xff20_2020;
const OTHER: u32 = 0xffcc_2020;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
}

/// A client with a window called "page", drawn, and a keyboard, which the
/// page has: it has it again only once the session is unlocked. Returns
/// what the keyboard hears.
fn page(session: &Session) -> (Desk, EventQueue<Desk>, Ear) {
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_coloured(&qh, "page", "page", PAGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let window = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });
    (desk, queue, ear)
}

/// Whether the page has the keyboard, as far as everything the compositor
/// has sent says.
fn page_has_keyboard(desk: &mut Desk, queue: &mut EventQueue<Desk>, ear: &Ear) -> bool {
    queue.roundtrip(desk).expect("dispatch");
    heard(ear).entered.is_some()
}

/// Ask `locker` to lock the session, as swaylock does, and wait for the
/// answer: `locked`, or `finished`.
fn ask_to_lock(
    locker: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    globals: &GlobalList,
) {
    until(queue, locker, |locker| !locker.outputs_empty());
    locker.lock(globals, qh);
    until(queue, locker, |locker| {
        locker.locked || locker.lock_finished
    });
}

/// A lock client, as swaylock is, on a connection of its own, its lock
/// answered.
fn locker(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList) {
    let (mut locker, mut queue, qh, globals) = session.client();
    ask_to_lock(&mut locker, &mut queue, &qh, &globals);
    (locker, queue, qh, globals)
}

/// Cover HEADLESS-1 with a lock surface of `locker`'s lock, and wait until
/// it is drawn: the compositor sizes even a refused lock's.
fn cover(
    locker: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    colour: u32,
) -> wl_surface::WlSurface {
    let drawn = locker.covers_drawn;
    let cover = locker.cover(qh, "HEADLESS-1", colour);
    until(queue, locker, |locker| locker.covers_drawn > drawn);
    cover
}

/// The lock surfaces the facts say cover a monitor.
fn covers(facts: &HostFacts) -> Vec<SurfaceId> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::LockCover))
        .map(|surface| surface.id)
        .collect()
}

/// The facts once the compositor has published everything it did so far.
/// Each lock request publishes before it is answered, so a client that has
/// heard its answer has nothing still to come.
fn now(session: &Session) -> HostFacts {
    session.wait_for(|_| true)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_second_lock_while_the_session_is_held_is_refused() {
    let session = session("lock-second-refused");
    let (mut holder, mut holder_queue, holder_qh, _) = locker(&session);
    assert!(holder.locked, "the first lock holds the session");
    cover(&mut holder, &mut holder_queue, &holder_qh, COVER);
    let held = covers(&session.wait_for(|facts| covers(facts).len() == 1));

    let (other, other_queue, ..) = locker(&session);
    assert!(
        other.lock_finished && !other.locked,
        "a second lock is told finished, and never locked"
    );
    assert_eq!(
        covers(&now(&session)),
        held,
        "the holder's cover still covers"
    );

    session.stop(((holder, holder_queue), (other, other_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn only_the_lock_that_holds_the_session_unlocks_it() {
    let session = session("lock-only-the-holder-unlocks");
    let (mut desk, mut queue, ear) = page(&session);
    let (mut holder, mut holder_queue, holder_qh, _) = locker(&session);
    cover(&mut holder, &mut holder_queue, &holder_qh, COVER);
    let held = covers(&session.wait_for(|facts| covers(facts).len() == 1));
    assert!(!page_has_keyboard(&mut desk, &mut queue, &ear), "locked");

    // A refused lock unlocks anyway, as a program that locked only to
    // unlock would. smithay 0.7.0 unlocked for it.
    let (mut other, mut other_queue, ..) = locker(&session);
    assert!(other.lock_finished, "refused");
    other.unlock();
    assert_eq!(
        refused(&mut other_queue, &mut other),
        ("ext_session_lock_v1".to_owned(), 1),
        "invalid_unlock"
    );
    assert_eq!(
        covers(&now(&session)),
        held,
        "the holder's cover still covers"
    );
    assert!(
        !page_has_keyboard(&mut desk, &mut queue, &ear),
        "the session is still locked"
    );

    // The holder unlocks.
    holder.unlock();
    holder_queue.roundtrip(&mut holder).expect("unlocked");
    session.wait_for(|facts| covers(facts).is_empty());
    assert!(page_has_keyboard(&mut desk, &mut queue, &ear), "unlocked");

    session.stop(((desk, queue), (holder, holder_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_refused_locks_surface_covers_nothing() {
    let session = session("lock-refused-cover");
    let (mut holder, mut holder_queue, holder_qh, _) = locker(&session);
    cover(&mut holder, &mut holder_queue, &holder_qh, COVER);
    let held = covers(&session.wait_for(|facts| covers(facts).len() == 1));

    // The protocol has a lock surface of a refused lock go undisplayed, not
    // refused: it is sized, and drawn, and covers nothing.
    let (mut other, mut other_queue, other_qh, _) = locker(&session);
    assert!(other.lock_finished, "refused");
    cover(&mut other, &mut other_queue, &other_qh, OTHER);
    assert_eq!(covers(&now(&session)), held, "only the holder's covers");

    session.stop(((holder, holder_queue), (other, other_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_lock_client_that_dies_leaves_the_session_locked_for_another_to_take() {
    let session = session("lock-taken-over");
    let (mut desk, mut queue, ear) = page(&session);

    // The holder, on a socket the test can cut, as a crash cuts swaylock's.
    let socket = common::stream(session.socket());
    let cut = socket.try_clone().expect("a second handle on the socket");
    let connection = Connection::from_socket(socket).expect("a Wayland connection");
    let (mut holder, mut holder_queue, holder_qh, holder_globals) = client_over(&connection);
    ask_to_lock(&mut holder, &mut holder_queue, &holder_qh, &holder_globals);
    assert!(holder.locked, "the first lock holds the session");
    cover(&mut holder, &mut holder_queue, &holder_qh, COVER);
    session.wait_for(|facts| covers(facts).len() == 1);

    // It dies. What it drew went with it, and the session stays locked.
    cut.shutdown(Shutdown::Both).expect("the connection cut");
    session.wait_for(|facts| covers(facts).is_empty());
    assert!(
        !page_has_keyboard(&mut desk, &mut queue, &ear),
        "the session is still locked"
    );

    // Another lock client takes the session over, and has the keyboard as
    // soon as its lock surface comes up.
    let (mut taker, mut taker_queue, taker_qh, taker_globals) = session.client();
    let taker_ear = taker.bind_keyboard(&taker_globals, &taker_qh);
    ask_to_lock(&mut taker, &mut taker_queue, &taker_qh, &taker_globals);
    assert!(taker.locked, "the session taken over");
    let taken = cover(&mut taker, &mut taker_queue, &taker_qh, COVER).id();
    until(&mut taker_queue, &mut taker, |_| {
        heard(&taker_ear).entered.as_ref() == Some(&taken)
    });
    assert_eq!(covers(&now(&session)).len(), 1, "its cover covers");

    // And unlocks.
    taker.unlock();
    taker_queue.roundtrip(&mut taker).expect("unlocked");
    session.wait_for(|facts| covers(facts).is_empty());
    assert!(page_has_keyboard(&mut desk, &mut queue, &ear), "unlocked");

    session.stop(((desk, queue), (taker, taker_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_holder_cannot_lock_twice() {
    let session = session("lock-holder-twice");
    let (mut holder, mut holder_queue, holder_qh, holder_globals) = locker(&session);
    cover(&mut holder, &mut holder_queue, &holder_qh, COVER);
    let held = covers(&session.wait_for(|facts| covers(facts).len() == 1));

    // A second lock of the holder's own, kept apart from the one it holds.
    let again = holder_globals
        .bind::<ExtSessionLockManagerV1, _, _>(&holder_qh, 1..=1, ())
        .expect("ext_session_lock_manager_v1")
        .lock(&holder_qh, ());
    until(&mut holder_queue, &mut holder, |holder| {
        holder.lock_finished
    });
    assert_eq!(covers(&now(&session)), held, "its cover still covers");
    again.destroy();

    // The lock it holds still unlocks.
    holder.unlock();
    holder_queue.roundtrip(&mut holder).expect("unlocked");
    session.wait_for(|facts| covers(facts).is_empty());

    session.stop((holder, holder_queue));
}
