//! A launcher and the keyboard, by a compositor with no screen: a layer
//! surface over the windows that asks for every key, as fuzzel does, and as
//! the shell's menus and pies do (#131).
//!
//! layer-shell has the top-most such layer keep the keys while it is up. A
//! window that opens under it, asks to be brought forward, or is picked in
//! the taskbar takes nothing from it: what is typed at the launcher would go
//! to the window instead, unseen beneath it. The window is raised all the
//! same, and has the keys once the launcher goes. Another launcher may take
//! them, as the newest is the one on top.
//!
//! Each test has a page, a client with a window and a keyboard, and a
//! launcher, a client of its own with a keyboard, as the shell is. What each
//! keyboard was told is what is asserted: `enter` and `leave` alone, so none
//! of this needs `capture`. Like the other live tests these bind a real
//! Wayland socket, so they need `XDG_RUNTIME_DIR`, and are `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Ear, Session, heard, until};
use perspicax_compositor::Backend;
use perspicax_index::HostFacts;
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{EventQueue, Proxy as _, QueueHandle, backend::ObjectId, globals::GlobalList};

const PAGE: u32 = 0xff33_6699;
const LATE: u32 = 0xff66_9933;
const LAUNCHER: u32 = 0xff44_4444;
/// How tall each launcher is.
const STRIP: u32 = 120;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
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

/// A launcher, as the shell is, on a connection of its own, with a keyboard
/// and `xdg_activation_v1`, for the token it hands what it starts: up, and
/// holding the keys. Returns what its keyboard hears.
fn launcher(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, Ear) {
    let (mut launcher, mut queue, qh, globals) = session.client();
    let ear = launcher.bind_keyboard(&globals, &qh);
    launcher.bind_activation(&globals, &qh);
    open_launcher(&mut launcher, &mut queue, &qh, &ear);
    (launcher, queue, qh, ear)
}

/// Open one more launcher on `launcher`'s client, and wait until it has the
/// keys, as one that wants every key has once it is up. Returns its surface.
fn open_launcher(
    launcher: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    ear: &Ear,
) -> ObjectId {
    let before = launcher.layers_drawn;
    launcher.open_launcher(qh, STRIP, LAUNCHER);
    let (layer, ..) = launcher.layers.last().expect("just opened");
    let surface = layer.wl_surface().id();
    until(queue, launcher, |launcher| launcher.layers_drawn > before);
    until(queue, launcher, |_| {
        heard(ear).entered.as_ref() == Some(&surface)
    });
    surface
}

/// Hear everything the compositor has sent so far: the page first, since
/// what it just asked for is what might have moved the keyboard, and so the
/// launcher hears any `leave` that came of it.
fn hear_all(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    launcher: &mut Desk,
    launcher_queue: &mut EventQueue<Desk>,
) {
    queue.roundtrip(desk).expect("dispatch");
    launcher_queue.roundtrip(launcher).expect("dispatch");
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

/// The issue's own case: a program started a moment ago, a chat window, a
/// dialog from a background job, opens while the person types at the
/// launcher. Once the launcher goes, as one does when it has started what was
/// asked of it, the window has the keys, being on top.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_opens_under_a_launcher_takes_no_keys_from_it() {
    let session = session("launcher-keys-new-window");
    let (mut desk, mut queue, qh, _, ear) = page(&session);
    let (mut launcher, mut launcher_queue, _, launcher_ear) = launcher(&session);
    let held = launcher.layers[0].0.wl_surface().id();
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    desk.open_coloured(&qh, "late", "late", LATE);
    until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
    hear_all(&mut desk, &mut queue, &mut launcher, &mut launcher_queue);
    assert_eq!(
        heard(&launcher_ear).entered.as_ref(),
        Some(&held),
        "the launcher keeps the keys"
    );
    assert!(
        heard(&ear).entered.is_none(),
        "the window that opened under it has none of them"
    );

    // The launcher goes, and the window on top has them: the one that opened.
    drop((launcher, launcher_queue));
    let late = desk.windows[1].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&late)
    });

    session.stop((desk, queue));
}

/// A window brought forward under the launcher, with the token the
/// launcher made as it started the window's program again: the program was
/// running already, and raises its first window before the launcher has
/// gone. It is raised, as asked, which says the token was good (#187), but
/// the launcher keeps the keys. A token made before the launcher came up
/// would not do: the keys going to it moved the person on (#257).
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_brought_forward_under_a_launcher_takes_no_keys_from_it() {
    let session = session("launcher-keys-activation");
    let (mut desk, mut queue, qh, globals, ear) = page(&session);
    desk.bind_activation(&globals, &qh);
    desk.open_coloured(&qh, "top", "top", LATE);
    until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
    let top = desk.windows[1].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&top)
    });
    let (mut launcher, mut launcher_queue, launcher_qh, launcher_ear) = launcher(&session);
    let held = launcher.layers[0].0.wl_surface().clone();
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let serial = heard(&launcher_ear).enter_serial.expect("entered");
    launcher.make_token_from(&launcher_qh, &held, serial);
    until(&mut launcher_queue, &mut launcher, |launcher| {
        !launcher.tokens.is_empty()
    });
    let token = launcher.tokens.remove(0);
    let held = held.id();

    desk.spend_token(&token, 0);
    hear_all(&mut desk, &mut queue, &mut launcher, &mut launcher_queue);
    assert!(
        above(&session.facts.read(), "page", "top"),
        "the page is brought forward"
    );
    assert_eq!(
        heard(&launcher_ear).entered.as_ref(),
        Some(&held),
        "the launcher keeps the keys"
    );
    assert!(
        heard(&ear).entered.is_none(),
        "the window brought forward under it has none of them"
    );

    session.stop(((desk, queue), (launcher, launcher_queue)));
}

/// What a launcher is for (#257): the person picks a program that is
/// running already, the launcher makes a token for it and goes, and the
/// keys go back to the window that was on top. Only then does the program
/// spend the token, and its window comes forward with the keys. The keys
/// leaving a launcher do not move the person on, as their going anywhere
/// else would.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_the_launcher_made_brings_a_window_forward_once_it_has_gone() {
    let session = session("launcher-keys-gone");
    let (mut desk, mut queue, qh, globals, ear) = page(&session);
    desk.bind_activation(&globals, &qh);
    desk.open_coloured(&qh, "top", "top", LATE);
    until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
    let top = desk.windows[1].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&top)
    });
    let (mut launcher, mut launcher_queue, launcher_qh, launcher_ear) = launcher(&session);
    let held = launcher.layers[0].0.wl_surface().clone();
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let serial = heard(&launcher_ear).enter_serial.expect("entered");
    launcher.make_token_from(&launcher_qh, &held, serial);
    until(&mut launcher_queue, &mut launcher, |launcher| {
        !launcher.tokens.is_empty()
    });
    let token = launcher.tokens.remove(0);

    drop((launcher, launcher_queue));
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&top)
    });
    desk.spend_token(&token, 0);
    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });
    assert!(
        above(&session.facts.read(), "page", "top"),
        "the page is brought forward"
    );

    session.stop((desk, queue));
}

/// The person picks a window in the taskbar while the launcher is up: the
/// window is raised, as asked, but the keys stay with the launcher until it
/// goes, when the window picked, now on top, has them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_picked_in_the_taskbar_under_a_launcher_takes_no_keys_from_it() {
    let session = session("launcher-keys-taskbar");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_coloured(&qh, "page", "page", PAGE);
    desk.open_coloured(&qh, "top", "top", LATE);
    let top = desk.windows[1].wl_surface().id();
    until(&mut queue, &mut desk, |desk| {
        desk.task("page").is_some() && heard(&ear).entered.as_ref() == Some(&top)
    });
    let (mut launcher, mut launcher_queue, _, launcher_ear) = launcher(&session);
    let held = launcher.layers[0].0.wl_surface().id();
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    desk.activate("page");
    hear_all(&mut desk, &mut queue, &mut launcher, &mut launcher_queue);
    assert_eq!(
        heard(&launcher_ear).entered.as_ref(),
        Some(&held),
        "the launcher keeps the keys"
    );
    assert!(
        heard(&ear).entered.is_none(),
        "the window picked under it has none of them"
    );

    // Raised all the same: once the launcher goes, it is the window on top.
    drop((launcher, launcher_queue));
    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });

    session.stop((desk, queue));
}

/// A second launcher opened over the first, a run box from a menu say, has
/// the keys: the newest launcher is the one on top, and layer-shell gives
/// them to that one.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_second_launcher_takes_the_keys_from_the_first() {
    let session = session("launcher-keys-second");
    let (mut launcher, mut queue, qh, ear) = launcher(&session);
    let first = launcher.layers[0].0.wl_surface().id();

    // Waits until the second has the keys, and fails if it never does.
    let second = open_launcher(&mut launcher, &mut queue, &qh, &ear);
    assert_ne!(first, second, "the second is a launcher of its own");

    session.stop((launcher, queue));
}
