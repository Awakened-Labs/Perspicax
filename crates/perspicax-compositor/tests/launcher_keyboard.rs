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
//! The session locking takes the keys from it, and unlocking gives them back
//! to the launcher that had them, if it is still up: not to the window beneath
//! it, nor to another launcher that happens to be top-most by then (#203).
//!
//! Each test has a page, a client with a window and a keyboard, and a
//! launcher, a client of its own with a keyboard, as the shell is. What each
//! keyboard was told is what is asserted: `enter` and `leave` alone, so none
//! of this needs `capture`. Like the other live tests these bind a real
//! Wayland socket, so they need `XDG_RUNTIME_DIR`, and are `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use std::net::Shutdown;

use common::{Desk, Ear, Session, heard, until};
use perspicax_compositor::{ActError, Backend, Host};
use perspicax_index::{Action as Verb, HostFacts, SurfaceKind};
use perspicax_node::SurfaceId;
use smithay_client_toolkit::shell::{WaylandSurface as _, wlr_layer::KeyboardInteractivity};
use wayland_client::{
    Connection, EventQueue, Proxy as _, QueueHandle, backend::ObjectId, globals::GlobalList,
};

const PAGE: u32 = 0xff33_6699;
const LATE: u32 = 0xff66_9933;
const LAUNCHER: u32 = 0xff44_4444;
const COVER: u32 = 0xff20_2020;
/// How tall each launcher is.
const STRIP: u32 = 120;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
}

/// A session with a person at the seat, where a window opens with the keys
/// only if the person asked for it (#259).
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

/// With a person at the seat, a window another program opens under the
/// launcher, which they did not start from it, opens without the keys
/// (#259). Once the launcher goes, the window they were using has them
/// back, over it; the new one waits beneath until they give it them. The
/// launcher is the page's own, a run box it opened say: one another
/// program showed would have taken nothing from the page (#267).
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_another_program_opens_under_a_launcher_waits_for_the_person_once_it_goes() {
    let session = seated("launcher-keys-another-seated");
    let (mut desk, mut queue, qh, _, ear) = page(&session);
    open_launcher(&mut desk, &mut queue, &qh, &ear);

    let (mut other, mut other_queue, other_qh, other_globals) = session.client();
    let other_ear = other.bind_keyboard(&other_globals, &other_qh);
    other.open_coloured(&other_qh, "late", "late", LATE);
    until(&mut other_queue, &mut other, |other| other.has_drawn(0));

    // The launcher goes, and the page stays.
    desk.layers.clear();
    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(
        heard(&other_ear).entered.is_none(),
        "the window that opened under the launcher has none"
    );
    session.wait_for(|facts| above(facts, "page", "late"));

    session.stop(((desk, queue), (other, other_queue)));
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
/// A launcher has every key until it goes, and they go then to the window on
/// top. So an agent raises nothing under one, seen by the person or not: the
/// window would be given what they typed next, for picking from the launcher.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_raise_under_a_launcher_is_refused_naming_it() {
    let session = session("launcher-raise");
    let (mut desk, mut queue, qh, _, _) = page(&session);
    desk.open_window(&qh, "over", "over");
    until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
    session.wait_for(|facts| above(facts, "over", "page"));
    let (mut launcher, mut launcher_queue, _, launcher_ear) = launcher(&session);

    let facts = session.facts.read();
    let first = |wanted: &dyn Fn(&SurfaceKind, Option<&str>) -> bool| -> SurfaceId {
        facts
            .surfaces()
            .iter()
            .find(|surface| wanted(&surface.kind, surface.title.as_deref()))
            .map(|surface| surface.id)
            .expect("on the desk")
    };
    let page_window = first(&|_, title| title == Some("page"));
    let up = first(&|kind, _| matches!(kind, SurfaceKind::Layer { .. }));

    let host = Host::new(&session.facts, &session.requests);
    assert_eq!(
        host.act(page_window, &Verb::Raise).map(|_| ()),
        Err(ActError::MenuOpen { popup: up })
    );
    hear_all(&mut desk, &mut queue, &mut launcher, &mut launcher_queue);
    assert!(
        above(&session.facts.read(), "over", "page"),
        "nothing was raised"
    );
    let launcher_surface = launcher.layers[0].0.wl_surface().id();
    assert_eq!(
        heard(&launcher_ear).entered,
        Some(launcher_surface),
        "the launcher still has the keys"
    );

    session.stop(((desk, queue), (launcher, launcher_queue)));
}

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

// What a launcher has as the session unlocks (#203). `clear_focus` took the
// keys from it as the session locked, and only the one that had them then has
// them back. Each test hears the keys leave it before unlocking: `until` asks
// before it dispatches, so a wait for its `enter` would otherwise pass on the
// one from before the lock.

/// A lock client, as swaylock is, on a connection of its own, with a keyboard:
/// the session locked by it, and HEADLESS-1 covered by a lock screen that has
/// the keys.
fn lock(session: &Session) -> (Desk, EventQueue<Desk>) {
    let (mut locker, mut queue, qh, globals) = session.client();
    let ear = locker.bind_keyboard(&globals, &qh);
    until(&mut queue, &mut locker, |locker| !locker.outputs_empty());
    locker.lock(&globals, &qh);
    until(&mut queue, &mut locker, |locker| locker.locked);
    let cover = locker.cover(&qh, "HEADLESS-1", COVER).id();
    until(&mut queue, &mut locker, |_| {
        heard(&ear).entered.as_ref() == Some(&cover)
    });
    (locker, queue)
}

/// Unlock, as swaylock does, and have the compositor take it in.
fn unlock(locker: &mut Desk, queue: &mut EventQueue<Desk>) {
    locker.unlock();
    queue.roundtrip(locker).expect("unlocked");
}

/// Show a launcher on a client of its own, the session having nobody's say
/// on it: told nothing of the keys. Returns the client and what it hears.
fn show_launcher(session: &Session) -> (Desk, EventQueue<Desk>, Ear) {
    let (mut other, mut queue, qh, globals) = session.client();
    let ear = other.bind_keyboard(&globals, &qh);
    other.open_launcher(&qh, STRIP, LATE);
    until(&mut queue, &mut other, |other| other.layers_drawn == 1);
    (other, queue, ear)
}

/// The issue's own case: a launcher up as the session locks, swayidle having
/// locked it with the launcher still open. It is drawn over everything once
/// the session unlocks, so it is what the person types at, and it has the
/// keys. Before, they went to the window beneath, unseen.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_up_as_the_session_locks_has_the_keys_back_as_it_unlocks() {
    let session = session("launcher-keys-unlock");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut launcher, mut launcher_queue, _, launcher_ear) = launcher(&session);
    let held = launcher.layers[0].0.wl_surface().id();
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    let (mut locker, mut locker_queue) = lock(&session);
    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.is_none()
    });
    unlock(&mut locker, &mut locker_queue);

    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.as_ref() == Some(&held)
    });
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(
        heard(&ear).entered.is_none(),
        "the page took the keys from the launcher"
    );

    session.stop((
        (desk, queue),
        (launcher, launcher_queue),
        (locker, locker_queue),
    ));
}

/// A launcher that goes behind the lock screen has nothing to come back to:
/// the window on top has the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_that_goes_behind_the_lock_screen_leaves_the_keys_to_the_window_on_top() {
    let session = session("launcher-keys-unlock-gone");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut launcher, mut launcher_queue, _, _) = launcher(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let (mut locker, mut locker_queue) = lock(&session);

    launcher.layers.clear();
    launcher_queue.roundtrip(&mut launcher).expect("closed");
    unlock(&mut locker, &mut locker_queue);

    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });

    session.stop((
        (desk, queue),
        (launcher, launcher_queue),
        (locker, locker_queue),
    ));
}

/// Unmap the `nth` layer as layer-shell has a client do it: attach no buffer,
/// and commit. Sent, and taken in, before this returns.
fn unmap_layer(desk: &mut Desk, queue: &mut EventQueue<Desk>, nth: usize) {
    let surface = desk.layers[nth].0.wl_surface();
    surface.attach(None, 0, 0);
    surface.commit();
    queue.roundtrip(desk).expect("the commit taken in");
}

/// A launcher its client unmapped behind the lock screen, and has not asked
/// for again, is not on screen to type at: the window on top has the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_unmapped_behind_the_lock_screen_leaves_the_keys_to_the_window_on_top() {
    let session = session("launcher-keys-unlock-unmapped");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut launcher, mut launcher_queue, _, launcher_ear) = launcher(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let (mut locker, mut locker_queue) = lock(&session);
    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.is_none()
    });

    unmap_layer(&mut launcher, &mut launcher_queue, 0);
    unlock(&mut locker, &mut locker_queue);

    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });
    launcher_queue.roundtrip(&mut launcher).expect("dispatch");
    assert!(
        heard(&launcher_ear).entered.is_none(),
        "a launcher nobody can see has the keys"
    );

    session.stop((
        (desk, queue),
        (launcher, launcher_queue),
        (locker, locker_queue),
    ));
}

/// A launcher unmapped and mapped again behind the lock screen comes back as
/// a new layer, with a new id, and a new layer that shows while the session
/// is locked had no keys to lose: the window on top has them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_mapped_again_behind_the_lock_screen_is_new_and_has_no_keys_back() {
    let session = session("launcher-keys-unlock-mapped-again");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut launcher, mut launcher_queue, _, launcher_ear) = launcher(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let (mut locker, mut locker_queue) = lock(&session);
    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.is_none()
    });

    unmap_layer(&mut launcher, &mut launcher_queue, 0);
    let drawn = launcher.layers_drawn;
    launcher.layers[0].0.commit();
    until(&mut launcher_queue, &mut launcher, |launcher| {
        launcher.layers_drawn > drawn
    });
    unlock(&mut locker, &mut locker_queue);

    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });
    launcher_queue.roundtrip(&mut launcher).expect("dispatch");
    assert!(
        heard(&launcher_ear).entered.is_none(),
        "the launcher mapped again was taken for the one that had the keys"
    );

    session.stop((
        (desk, queue),
        (launcher, launcher_queue),
        (locker, locker_queue),
    ));
}

/// A layer that no longer asks for every key by the time the session unlocks
/// is no launcher, and the keys are not what it asked for.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_that_stops_wanting_every_key_behind_the_lock_screen_has_none_back() {
    let session = session("launcher-keys-unlock-stops-wanting");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut launcher, mut launcher_queue, _, launcher_ear) = launcher(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    let (mut locker, mut locker_queue) = lock(&session);
    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.is_none()
    });

    let (layer, ..) = &launcher.layers[0];
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.commit();
    launcher_queue.roundtrip(&mut launcher).expect("taken in");
    unlock(&mut locker, &mut locker_queue);

    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });
    launcher_queue.roundtrip(&mut launcher).expect("dispatch");
    assert!(
        heard(&launcher_ear).entered.is_none(),
        "a layer that wants no keys has them"
    );

    session.stop((
        (desk, queue),
        (launcher, launcher_queue),
        (locker, locker_queue),
    ));
}

/// A launcher another program showed while the person used the page took no
/// keys as it showed (#267), and being top-most as the session unlocks earns
/// it none: it never had them to get back, and it would have what was typed
/// next at the page, Enter and all.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_nobody_asked_for_takes_no_keys_as_the_session_unlocks() {
    let session = seated("launcher-keys-unlock-unasked");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut other, mut other_queue, other_ear) = show_launcher(&session);
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(heard(&ear).entered.is_some(), "the page keeps the keys");

    let (mut locker, mut locker_queue) = lock(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());
    unlock(&mut locker, &mut locker_queue);

    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(
        heard(&other_ear).entered.is_none(),
        "the launcher nobody asked for has the keys"
    );

    session.stop(((desk, queue), (other, other_queue), (locker, locker_queue)));
}

/// A launcher that opens behind the lock screen, nobody there to ask for it,
/// takes no keys as the session unlocks either: any program could otherwise
/// show one while the person is away, and have what they type on their return.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_that_opens_behind_the_lock_screen_takes_no_keys_as_the_session_unlocks() {
    let session = seated("launcher-keys-unlock-behind");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut locker, mut locker_queue) = lock(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    let (mut other, mut other_queue, other_ear) = show_launcher(&session);
    unlock(&mut locker, &mut locker_queue);

    let page = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&page)
    });
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(
        heard(&other_ear).entered.is_none(),
        "the launcher that opened behind the lock has the keys"
    );

    session.stop(((desk, queue), (other, other_queue), (locker, locker_queue)));
}

/// A launcher that opens behind the lock screen over the one that had the
/// keys is the top-most, but the keys are the first's: it is what the person
/// was typing at, and nobody asked for the other.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_that_opens_behind_the_lock_screen_takes_nothing_from_the_one_that_had_the_keys() {
    let session = seated("launcher-keys-unlock-over");
    let (mut desk, mut queue, qh, _, ear) = page(&session);
    let held = open_launcher(&mut desk, &mut queue, &qh, &ear);
    let (mut locker, mut locker_queue) = lock(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    let (mut other, mut other_queue, other_ear) = show_launcher(&session);
    unlock(&mut locker, &mut locker_queue);

    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&held)
    });
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(
        heard(&other_ear).entered.is_none(),
        "the launcher that opened over it has the keys"
    );

    session.stop(((desk, queue), (other, other_queue), (locker, locker_queue)));
}

/// A launcher whose own menu has the keys as the session locks counts as
/// having them. A menu has them only by a click on it while its launcher has
/// none, one shown without them (#267): a launcher that has them keeps them
/// from everything but a launcher (#131). A layer's menu holds no grab, so it
/// stays up through the lock, and the launcher has the keys back itself. Needs
/// `capture`, for the click.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_whose_menu_has_the_keys_as_the_session_locks_has_them_back_as_it_unlocks() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Mods};

    let session = seated("launcher-keys-unlock-menu");
    let (mut desk, mut queue, _, _, ear) = page(&session);
    let (mut launcher, mut launcher_queue, launcher_qh, launcher_globals) = session.client();
    let launcher_ear = launcher.bind_keyboard(&launcher_globals, &launcher_qh);
    launcher.open_launcher(&launcher_qh, STRIP, LAUNCHER);
    until(&mut launcher_queue, &mut launcher, |launcher| {
        launcher.layers_drawn == 1
    });
    let held = launcher.layers[0].0.wl_surface().id();
    let menu = launcher.open_panel_popup(&launcher_qh, &launcher.layers[0].0, (20, 20), (100, 60));
    common::configured(&mut launcher, &mut launcher_queue, 1);
    launcher.paint(menu.wl_surface(), (100, 60), LATE);
    menu.wl_surface().commit();
    launcher_queue.roundtrip(&mut launcher).expect("drawn");
    assert!(
        heard(&launcher_ear).entered.is_none(),
        "shown without the keys"
    );
    session.command(Command::Click {
        at: (40, 40),
        button: Button::Left,
        mods: Mods::default(),
    });
    let in_menu = menu.wl_surface().id();
    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.as_ref() == Some(&in_menu)
    });

    let (mut locker, mut locker_queue) = lock(&session);
    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.is_none()
    });
    unlock(&mut locker, &mut locker_queue);

    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.as_ref() == Some(&held)
    });
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(
        heard(&ear).entered.is_none(),
        "the page took the keys from the launcher"
    );

    session.stop((
        (desk, queue),
        menu,
        (launcher, launcher_queue),
        (locker, locker_queue),
    ));
}

/// The lock client dies, and another takes the session over and unlocks it.
/// Who finishes the lock is no matter to what the person left open.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_has_the_keys_back_from_a_lock_that_took_the_session_over() {
    let session = session("launcher-keys-unlock-takeover");
    let (mut launcher, mut launcher_queue, _, launcher_ear) = launcher(&session);
    let held = launcher.layers[0].0.wl_surface().id();

    // The holder, on a socket the test can cut, as a crash cuts swaylock's.
    let socket = common::stream(session.socket());
    let cut = socket.try_clone().expect("a second handle on the socket");
    let connection = Connection::from_socket(socket).expect("a Wayland connection");
    let (mut holder, mut holder_queue, holder_qh, holder_globals) =
        common::client_over(&connection);
    until(&mut holder_queue, &mut holder, |holder| {
        !holder.outputs_empty()
    });
    holder.lock(&holder_globals, &holder_qh);
    until(&mut holder_queue, &mut holder, |holder| holder.locked);
    holder.cover(&holder_qh, "HEADLESS-1", COVER);
    until(&mut holder_queue, &mut holder, |holder| {
        holder.covers_drawn > 0
    });
    session.wait_for(|facts| covers(facts) == 1);
    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.is_none()
    });

    cut.shutdown(Shutdown::Both).expect("the connection cut");
    session.wait_for(|facts| covers(facts) == 0);

    let (mut taker, mut taker_queue) = lock(&session);
    unlock(&mut taker, &mut taker_queue);

    until(&mut launcher_queue, &mut launcher, |_| {
        heard(&launcher_ear).entered.as_ref() == Some(&held)
    });

    session.stop(((launcher, launcher_queue), (taker, taker_queue)));
}

/// The lock surfaces the facts say cover a monitor.
fn covers(facts: &HostFacts) -> usize {
    facts
        .surfaces()
        .iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::LockCover))
        .count()
}
