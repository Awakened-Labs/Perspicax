//! The lock screen and the keyboard, by a compositor with no screen:
//! `ext-session-lock-v1`, as swaylock speaks it (#127).
//!
//! While the session is locked, only the lock screen has the keys. A window
//! that opens behind it, asks to be brought forward, or comes forward as the
//! tab in front of it closes, takes nothing from it, nor does a launcher
//! asking for every key: whatever took them would get the password as it is
//! typed, and Enter would send it, as a chat message, a shell command or a
//! confirmed dialog. Before the lock screen is up there is nothing to take
//! the keys from, and a window closing then gives them to no other.
//! Unlocked, the window on top has them, often the one that opened meanwhile.
//!
//! Each test has a page, a client with a window and a keyboard, and a
//! locker, a client of its own with a keyboard, as swaylock is. What each
//! keyboard was told is what is asserted: `enter` and `leave` alone, so none
//! of this needs `capture`. Like the other live tests these bind a real
//! Wayland socket, so they need `XDG_RUNTIME_DIR`, and are `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Ear, Session, heard, until};
use perspicax_compositor::Backend;
use perspicax_index::HostFacts;
use perspicax_policy::Action;
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{EventQueue, Proxy as _, QueueHandle, backend::ObjectId, globals::GlobalList};

const PAGE: u32 = 0xff33_6699;
const LATE: u32 = 0xff66_9933;
const COVER: u32 = 0xff20_2020;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
}

/// A session with a person at the seat, where a window that closes hands
/// the keyboard on.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)).with_person())
}

/// A client with a window called "page", drawn, and a keyboard, which the
/// page has. Returns what the keyboard hears.
fn page(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList, Ear) {
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_coloured(&qh, "page", "page", PAGE);
    until(&mut queue, &mut desk, |desk| desk.has_drawn(0));
    let window = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });
    (desk, queue, qh, globals, ear)
}

/// Open the window `title` beside the page's, drawn in `LATE`, and wait
/// until it has the keyboard, as a new window has. Returns its surface.
fn open_second(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    ear: &Ear,
    title: &str,
) -> ObjectId {
    desk.open_coloured(qh, title, title, LATE);
    until(queue, desk, |desk| desk.has_drawn(1));
    let window = desk.windows[1].wl_surface().id();
    until(queue, desk, |_| {
        heard(ear).entered.as_ref() == Some(&window)
    });
    window
}

/// A lock client, as swaylock is, on a connection of its own, with a
/// keyboard: the session locked by it, and nothing yet covering any monitor.
/// Returns what its keyboard hears.
fn locker(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, Ear) {
    let (mut locker, mut queue, qh, globals) = session.client();
    let ear = locker.bind_keyboard(&globals, &qh);
    until(&mut queue, &mut locker, |locker| !locker.outputs_empty());
    locker.lock(&globals, &qh);
    until(&mut queue, &mut locker, |locker| locker.locked);
    (locker, queue, qh, ear)
}

/// Cover HEADLESS-1, and wait until the cover has the keyboard, as the
/// lock's first surface does. Returns the cover.
fn cover(
    locker: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    ear: &Ear,
) -> ObjectId {
    let cover = locker.cover(qh, "HEADLESS-1", COVER).id();
    until(queue, locker, |_| {
        heard(ear).entered.as_ref() == Some(&cover)
    });
    cover
}

/// Hear everything the compositor has sent so far: the page first, since
/// what it just asked for is what might have moved the keyboard, and so the
/// lock screen hears any `leave` that came of it.
fn hear_all(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    locker: &mut Desk,
    locker_queue: &mut EventQueue<Desk>,
) {
    queue.roundtrip(desk).expect("dispatch");
    locker_queue.roundtrip(locker).expect("dispatch");
}

/// Whether the window `upper` is above the window `lower`, the facts giving
/// them bottom to top.
fn above(facts: &HostFacts, upper: &str, lower: &str) -> bool {
    let at = |title: &str| {
        facts
            .surfaces()
            .iter()
            .position(|surface| surface.title.as_deref() == Some(title))
    };
    matches!((at(upper), at(lower)), (Some(upper), Some(lower)) if upper > lower)
}

/// The issue's own case: a chat window, a terminal or a dialog that opens
/// behind the lock screen while the person types their password.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_opens_behind_the_lock_screen_takes_no_keys_from_it() {
    let session = session("lock-keys-new-window");
    let (mut desk, mut queue, qh, _, ear) = page(&session);
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    let held = cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    desk.open_coloured(&qh, "late", "late", LATE);
    until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
    );
    assert!(
        heard(&ear).entered.is_none(),
        "the window that opened behind it has none of them"
    );

    // Unlocked, the window on top has them: the one that opened.
    locker.unlock();
    locker_queue.roundtrip(&mut locker).expect("dispatch");
    let late = desk.windows[1].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&late)
    });

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// A window brought forward behind the lock screen, with a token its program
/// made while another of its windows had the keys, a moment before the
/// screen locked: a program started again just then, raising its first
/// window. It is raised, as asked, which says the token was good (#187), but
/// the lock screen keeps the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_brought_forward_behind_the_lock_screen_takes_no_keys_from_it() {
    let session = session("lock-keys-activation");
    let (mut desk, mut queue, qh, globals, ear) = page(&session);
    desk.bind_activation(&globals, &qh);
    open_second(&mut desk, &mut queue, &qh, &ear, "top");
    let serial = heard(&ear).enter_serial.expect("entered");
    desk.make_token(&qh, 1, serial);
    until(&mut queue, &mut desk, |desk| !desk.tokens.is_empty());
    let token = desk.tokens.remove(0);
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    let held = cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    desk.spend_token(&token, 0);
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    assert!(
        above(&session.facts.read(), "page", "top"),
        "the page is brought forward"
    );
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
    );
    assert!(
        heard(&ear).entered.is_none(),
        "the window brought forward behind it has none of them"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// Between the session locking and the lock screen's first surface, nothing
/// has the keys. A window that closes then gives them to the window beneath
/// it on any desktop, but not behind the lock, and the lock screen has them
/// as it comes up. Seated: with nobody at the seat, a closing window hands
/// the keyboard to no one anyway.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_closes_before_the_lock_screen_is_up_gives_the_keys_to_no_other() {
    let session = seated("lock-keys-close-in-the-gap");
    let (mut desk, mut queue, qh, _, ear) = page(&session);
    open_second(&mut desk, &mut queue, &qh, &ear, "top");
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let last = heard(&ear).enter_serial;

    desk.windows.remove(1);
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    assert!(
        heard(&ear).entered.is_none(),
        "the window beneath took the keys behind the lock"
    );
    assert_eq!(
        heard(&ear).enter_serial,
        last,
        "the keys went to the window beneath on the way"
    );

    // The lock screen comes up, and has them.
    cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    assert!(heard(&ear).entered.is_none(), "the page has none of them");

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// Two windows grouped as tabs, and the one in front closes behind the lock
/// screen: the tab behind it comes forward into its place, and takes
/// nothing from the lock screen.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tab_that_comes_forward_behind_the_lock_screen_takes_no_keys_from_it() {
    let session = session("lock-keys-tab");
    let (mut desk, mut queue, qh, _, ear) = page(&session);
    open_second(&mut desk, &mut queue, &qh, &ear, "front");
    session.perform(Action::TabWithPrevious);
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("page") && !surface.mapped)
    });
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    let held = cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    desk.windows.remove(1);
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("page") && surface.mapped)
    });
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
    );
    assert!(
        heard(&ear).entered.is_none(),
        "the tab that came forward behind it has none of them"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// A launcher asking for every key, as fuzzel does, opened behind the lock
/// screen: started by a key held as the screen locked, say.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_that_opens_behind_the_lock_screen_takes_no_keys_from_it() {
    let session = session("lock-keys-launcher");
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    let held = cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);

    desk.open_launcher(&qh, 40, LATE);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
    );
    assert!(
        heard(&ear).entered.is_none(),
        "the launcher behind it has none of them"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}
