//! The lock screen and the keyboard, by a compositor with no screen:
//! `ext-session-lock-v1`, as swaylock speaks it (#127, #197).
//!
//! While the session is locked, only the lock screen has the keys. A window
//! that opens behind it, asks to be brought forward, or comes forward as the
//! tab in front of it closes, takes nothing from it, nor does a launcher
//! asking for every key: whatever took them would get the password as it is
//! typed, and Enter would send it, as a chat message, a shell command or a
//! confirmed dialog. Before the lock screen is up there is nothing to take
//! the keys from, and a window closing then gives them to no other.
//! Unlocked, the window on top has them, often the one that opened meanwhile,
//! unless a launcher had them as the session locked (`launcher_keyboard.rs`).
//!
//! A lock screen that goes while the session is locked, as swaylock's does
//! when its monitor is unplugged, hands the keys to one still up, or to
//! nobody until one comes (#197).
//!
//! Each test has a page, a client with a window and a keyboard, and a
//! locker, a client of its own with a keyboard, as swaylock is. The person
//! types a password at the lock screen ([`Command::Type`]), and what each
//! keyboard was told is what is asserted, so none of this needs `capture`.
//! Each way the keys could leave the lock screen is asserted in the order
//! that says what held them: the window heard no key of the password, the
//! lock screen heard every one, and the window never had the keyboard since
//! the lock, not even for a moment. The first two are kept where keys go
//! out, even with every gate of #127 taken out; the third is the gate's.
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Ear, Session, advertised, heard, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{HostFacts, SurfaceKind};
use perspicax_policy::Action;
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{
    EventQueue, Proxy as _, QueueHandle, backend::ObjectId, globals::GlobalList,
    protocol::wl_surface,
};

const PAGE: u32 = 0xff33_6699;
const LATE: u32 = 0xff66_9933;
const COVER: u32 = 0xff20_2020;

/// What the person types at the lock screen: no capitals, so no Shift among
/// the presses, and an Enter, which would send it.
const PASSWORD: &str = "hunter2\n";

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
}

/// A session with a person at the seat, where a window that closes hands
/// the keyboard on.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)).with_person())
}

/// `count` monitors side by side, HEADLESS-1 leftmost.
fn monitors(name: &str, count: usize) -> Session {
    let mut backend = Backend::headless((1280, 800));
    if let Backend::Headless { outputs, .. } = &mut backend {
        *outputs = (1..=count)
            .map(|nth| Virtual::numbered(nth, (1280, 800)))
            .collect();
    }
    Session::start(name, backend)
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
    locker.bind_activation(&globals, &qh);
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

/// Cover `monitor` too, and wait until the cover has drawn. Returns its
/// surface.
fn cover_also(
    locker: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    monitor: &str,
) -> wl_surface::WlSurface {
    let drawn = locker.covers_drawn;
    let cover = locker.cover(qh, monitor, COVER);
    until(queue, locker, |locker| locker.covers_drawn > drawn);
    cover
}

/// The lock surfaces the facts say cover a monitor.
fn covers(facts: &HostFacts) -> usize {
    facts
        .surfaces()
        .iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::LockCover))
        .count()
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

/// The person types the password, and each client has heard all that came
/// of it when this returns. The compositor carries out a command between
/// two of the first client's round trips, as `xwayland.rs`'s typing waits,
/// and a round trip of each client after that is answered after anything
/// sent it.
fn type_password(session: &Session, clients: &mut [(&mut Desk, &mut EventQueue<Desk>)]) {
    session.command(Command::Type(PASSWORD.to_owned()));
    if let Some((desk, queue)) = clients.first_mut() {
        queue.roundtrip(desk).expect("dispatch");
    }
    for (desk, queue) in clients {
        queue.roundtrip(desk).expect("dispatch");
    }
}

/// Every key of the password went to the lock screen, and none of it, nor
/// the keyboard, to the window behind: in the order that says what held.
/// `before` is the window's keyboard's last `enter` as the lock came up.
fn the_lock_screen_heard_it(window: &Ear, lock_screen: &Ear, before: Option<u32>) {
    assert!(
        heard(window).keys.is_empty(),
        "the window behind the lock screen heard the password"
    );
    assert_eq!(
        heard(lock_screen).keys.len(),
        PASSWORD.chars().count(),
        "the lock screen heard every key of the password"
    );
    assert_eq!(
        heard(window).enter_serial,
        before,
        "the window behind the lock screen had the keyboard"
    );
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

/// The rule itself: what the person types at the lock screen goes to it,
/// and to no window behind it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_persons_keys_go_to_the_lock_screen_and_to_no_window() {
    let session = session("lock-keys-typed");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let before = heard(&ear).enter_serial;

    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    the_lock_screen_heard_it(&ear, &locker_ear, before);

    session.stop(((desk, queue), (locker, locker_queue)));
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
    let before = heard(&ear).enter_serial;

    desk.open_coloured(&qh, "late", "late", LATE);
    until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    the_lock_screen_heard_it(&ear, &locker_ear, before);
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
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

/// With a person at the seat, a window another program opens behind the
/// lock screen opens without the keys, the lock screen having them (#259).
/// Unlocked, the window the person was using has them back, over it; the
/// new one waits beneath until they give it them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_another_program_opens_behind_the_lock_screen_waits_for_the_person_once_unlocked() {
    let session = seated("lock-keys-another-seated");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    let (mut other, mut other_queue, other_qh, other_globals) = session.client();
    let other_ear = other.bind_keyboard(&other_globals, &other_qh);
    other.open_coloured(&other_qh, "late", "late", LATE);
    until(&mut other_queue, &mut other, |other| other.has_drawn(0));
    type_password(
        &session,
        &mut [
            (&mut other, &mut other_queue),
            (&mut locker, &mut locker_queue),
        ],
    );
    the_lock_screen_heard_it(&other_ear, &locker_ear, None);

    locker.unlock();
    locker_queue.roundtrip(&mut locker).expect("dispatch");
    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(
        heard(&other_ear).entered.is_none(),
        "the window that opened behind the lock screen has none"
    );
    session.wait_for(|facts| above(facts, "page", "late"));

    session.stop(((desk, queue), (locker, locker_queue), (other, other_queue)));
}

/// A window brought forward behind the lock screen, with a token the one
/// client that may make one then made: the lock screen, which has the keys.
/// It is raised, as asked, which says the token was good (#187), but the
/// lock screen keeps the keys. A token made before the screen locked would
/// not do: the keys going to the lock screen moved the person on (#257).
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_brought_forward_behind_the_lock_screen_takes_no_keys_from_it() {
    let session = session("lock-keys-activation");
    let (mut desk, mut queue, qh, globals, ear) = page(&session);
    desk.bind_activation(&globals, &qh);
    open_second(&mut desk, &mut queue, &qh, &ear, "top");
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    let held = cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let before = heard(&ear).enter_serial;
    let serial = heard(&locker_ear).enter_serial.expect("entered");
    let surface = locker.covers[0].1.clone();
    locker.make_token_from(&locker_qh, &surface, serial);
    until(&mut locker_queue, &mut locker, |locker| {
        !locker.tokens.is_empty()
    });
    let token = locker.tokens.remove(0);

    desk.spend_token(&token, 0);
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    assert!(
        above(&session.facts.read(), "page", "top"),
        "the page is brought forward"
    );
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    the_lock_screen_heard_it(&ear, &locker_ear, before);
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// Between the session locking and the lock screen's first surface, nothing
/// has the keys. A window that closes then gives them to the window beneath
/// it on any desktop, but not behind the lock, and what is typed then goes
/// to nobody. The lock screen has them as it comes up. Seated: with nobody
/// at the seat, a closing window hands the keyboard to no one anyway.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_closes_before_the_lock_screen_is_up_gives_the_keys_to_no_other() {
    let session = seated("lock-keys-close-in-the-gap");
    let (mut desk, mut queue, qh, _, ear) = page(&session);
    open_second(&mut desk, &mut queue, &qh, &ear, "top");
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let before = heard(&ear).enter_serial;

    desk.windows.remove(1);
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    assert!(
        heard(&ear).keys.is_empty(),
        "the window beneath heard what was typed behind the lock"
    );
    assert!(
        heard(&locker_ear).keys.is_empty(),
        "what was typed before the lock screen was up reached it"
    );
    assert_eq!(
        heard(&ear).enter_serial,
        before,
        "the keys went to the window beneath on the way"
    );

    // The lock screen comes up, has them, and hears the password.
    cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    the_lock_screen_heard_it(&ear, &locker_ear, before);

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
    let before = heard(&ear).enter_serial;

    desk.windows.remove(1);
    hear_all(&mut desk, &mut queue, &mut locker, &mut locker_queue);
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("page") && surface.mapped)
    });
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    the_lock_screen_heard_it(&ear, &locker_ear, before);
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
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
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    the_lock_screen_heard_it(&ear, &locker_ear, None);
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// The issue's other case: a locked laptop undocked. Its monitor goes, and
/// swaylock takes down the lock screen that covered it, the one with the
/// keys. The lock screen on the monitor left has them at once, and hears
/// the password. Before, they stayed on the surface that went, and no
/// other lock screen was ever given them. A monitor plugged back in is
/// covered again, and takes nothing from the lock screen that has them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_lock_screen_whose_monitor_goes_hands_its_keys_to_the_one_left() {
    let session = monitors("lock-keys-monitor-goes", 2);
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    let first = locker.covers[0].1.clone();
    let second = cover_also(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-2");
    session.wait_for(|facts| covers(facts) == 2);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let before = heard(&ear).enter_serial;
    assert_eq!(
        heard(&locker_ear).entered,
        Some(first.id()),
        "the first lock screen has the keys"
    );

    session.command(Command::Unplug("HEADLESS-1".to_owned()));
    until(&mut locker_queue, &mut locker, |locker| {
        !locker.has_monitor("HEADLESS-1")
    });
    locker.uncover(&first);
    until(&mut locker_queue, &mut locker, |_| {
        heard(&locker_ear).entered == Some(second.id())
    });
    session.wait_for(|facts| covers(facts) == 1);
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    the_lock_screen_heard_it(&ear, &locker_ear, before);

    // Plugged back in, and covered again.
    session.command(Command::Plug(Virtual::numbered(1, (1280, 800))));
    until(&mut locker_queue, &mut locker, |locker| {
        locker.has_monitor("HEADLESS-1")
    });
    cover_also(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-1");
    session.wait_for(|facts| covers(facts) == 2);
    locker_queue.roundtrip(&mut locker).expect("dispatch");
    assert_eq!(
        heard(&locker_ear).entered,
        Some(second.id()),
        "the lock screen with the keys keeps them"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// The lock client takes down its only lock screen and the session stays
/// locked, over the compositor's own: what is typed then reaches nobody,
/// neither a window nor the surface that went. The next lock screen up has
/// the keys, though it is not the lock's first. Before, the keys stayed on
/// the surface that went, and no later one was given them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_lock_screen_left_with_no_cover_gives_the_keys_to_nobody_until_one_comes() {
    let session = monitors("lock-keys-no-cover", 2);
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locker(&session);
    cover(&mut locker, &mut locker_queue, &locker_qh, &locker_ear);
    let first = locker.covers[0].1.clone();
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let before = heard(&ear).enter_serial;

    locker.uncover(&first);
    // Taking it down is only queued until the locker's next round trip.
    locker_queue.roundtrip(&mut locker).expect("dispatch");
    session.wait_for(|facts| covers(facts) == 0);
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    assert!(heard(&ear).keys.is_empty(), "the window heard the password");
    assert!(
        heard(&locker_ear).keys.is_empty(),
        "the lock screen that went heard the password"
    );

    let next = cover_also(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-2");
    until(&mut locker_queue, &mut locker, |_| {
        heard(&locker_ear).entered == Some(next.id())
    });
    type_password(
        &session,
        &mut [(&mut desk, &mut queue), (&mut locker, &mut locker_queue)],
    );
    the_lock_screen_heard_it(&ear, &locker_ear, before);

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// Neither protocol by which a program types for the person is offered. A
/// virtual keyboard's keys and an input method's are sent straight to the
/// client with the keyboard, past where perspicax keeps them on the lock
/// screen (#197): one offered would need that first.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn no_protocol_types_around_the_lock_screen() {
    let session = session("lock-keys-no-typing-protocols");
    let (desk, queue, _, globals) = session.client();
    for interface in [
        "zwp_virtual_keyboard_manager_v1",
        "zwp_input_method_manager_v2",
    ] {
        assert!(
            !advertised(&globals, interface),
            "{interface} is offered: its keys would go around the lock screen"
        );
    }
    session.stop((desk, queue));
}
