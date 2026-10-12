//! The lock screen and the pointer, by a compositor with no screen:
//! `ext-session-lock-v1`, as swaylock speaks it.
//!
//! Locking takes the pointer from the window under it at once, with the
//! mouse at rest: a click, or a tap of the touchpad to wake the screen, must
//! not reach a window the lock hides (#82). The lock surface on the monitor
//! the pointer is on has it the moment it comes up, whichever monitor's came
//! first, so swaylock hides the cursor without a nudge. Unlocked, the window
//! under the pointer has it back at once, and the cursor swaylock hid is the
//! compositor's arrow until that window sets its own.
//!
//! Whatever held the keyboard or the pointer lets go as the session locks.
//! A window's open menu closes, as a click outside it would, and the lock
//! screen has the keys and the pointer: the menu would otherwise have kept
//! them, and taken the password. A resize under way is told it is over. And
//! no menu opened behind the lock takes anything from it.
//!
//! With no person at the seat, the first three see the lock alone move the
//! pointer: with one, a change of the keyboard's focus looks under the
//! pointer again too, a turn of the loop later. The last three need a
//! person, without whom no menu or resize takes the pointer. The hand is
//! [`Command::Motion`], and a click only where a resize needs one, since it
//! would leave a button's grab behind for a moment. Finding what is under
//! the pointer takes the clients' buffers, so this needs the `capture`
//! feature. Like the other live tests it binds a real Wayland socket, so it
//! needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to
//! run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Ear, Session, around, copy_all, heard, menu_with_the_keyboard, rgba, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::HostFacts;
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::{Access, Button, Mods, Place, Shape, Side};
use smithay_client_toolkit::shell::{WaylandSurface as _, xdg::XdgSurface as _};
use wayland_client::{
    EventQueue, Proxy as _, QueueHandle, globals::GlobalList, protocol::wl_surface,
};
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;

const PAGE: u32 = 0xff33_6699;
const COVER: u32 = 0xff20_2020;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
}

/// A session with a person at the seat, where a menu takes the keyboard and
/// pointer, and a client may resize its window.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)).with_person())
}

/// Two monitors side by side, HEADLESS-2 to the right of HEADLESS-1.
fn two_monitors(name: &str) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![
                Virtual::numbered(1, (1280, 800)),
                Virtual {
                    place: Place::Beside {
                        side: Side::RightOf,
                        of: "HEADLESS-1".to_owned(),
                        offset: 0,
                    },
                    ..Virtual::numbered(2, (1280, 800))
                },
            ],
            workspaces: Shape::default(),
            access: Access::open(),
            person: false,
            opacity: Default::default(),
        },
    )
}

/// A client with our pointer and a window called "page", drawn. Returns the
/// page's id and where it is on the desk.
fn with_page(
    session: &Session,
) -> (
    Desk,
    EventQueue<Desk>,
    QueueHandle<Desk>,
    GlobalList,
    (SurfaceId, Rect),
) {
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.open_coloured(&qh, "page", "page", PAGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| page(facts).is_some());
    let page = page(&facts).expect("waited for");
    (desk, queue, qh, globals, page)
}

/// [`with_page`], and the pointer resting on the page's middle. Returns
/// where that is on the desk.
fn hovered(
    session: &Session,
) -> (
    Desk,
    EventQueue<Desk>,
    QueueHandle<Desk>,
    GlobalList,
    (i32, i32),
) {
    let (mut desk, mut queue, qh, globals, (_, at)) = with_page(session);
    let middle = middle(at);
    // From the origin, where the pointer starts.
    session.command(Command::Motion { by: middle });
    let window = desk.windows[0].wl_surface().clone();
    until(&mut queue, &mut desk, |desk| {
        desk.pointed.as_ref() == Some(&window)
    });
    (desk, queue, qh, globals, middle)
}

fn page(facts: &HostFacts) -> Option<(SurfaceId, Rect)> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.mapped && surface.title.as_deref() == Some("page"))
        .map(|surface| (surface.id, surface.geometry))
}

fn middle(at: Rect) -> (i32, i32) {
    (
        ((at.x0 + at.x1) / 2.0).round() as i32,
        ((at.y0 + at.y1) / 2.0).round() as i32,
    )
}

/// A second client, as swaylock is, with a pointer and a keyboard of its
/// own, the session locked by it, and nothing yet covering any monitor.
/// Returns what its keyboard hears.
fn locked(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, Ear) {
    let (mut locker, mut queue, qh, globals) = session.client();
    locker.bind_pointer(&globals, &qh);
    let ear = locker.bind_keyboard(&globals, &qh);
    until(&mut queue, &mut locker, |locker| !locker.outputs_empty());
    locker.lock(&globals, &qh);
    until(&mut queue, &mut locker, |locker| locker.locked);
    (locker, queue, qh, ear)
}

/// Cover `monitor`, and wait until the cover has drawn.
fn cover(
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

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn locking_takes_the_pointer_from_the_window_under_it_though_the_mouse_never_moved() {
    let session = session("lock-takes-the-pointer");
    let (mut desk, mut queue, ..) = hovered(&session);

    // Locked, and nothing of the lock's up yet: the window has lost the
    // pointer all the same, so a tap at rest reaches nothing.
    let (mut locker, mut locker_queue, locker_qh, _) = locked(&session);
    until(&mut queue, &mut desk, |desk| desk.pointed.is_none());

    // The lock surface comes up under it, and has it.
    let cover = cover(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-1");
    until(&mut locker_queue, &mut locker, |locker| {
        locker.pointed.as_ref() == Some(&cover)
    });

    session.stop(((desk, queue), (locker, locker_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_lock_screen_on_the_monitor_under_the_pointer_has_it_though_it_came_second() {
    let session = two_monitors("lock-second-monitor");
    let (mut desk, mut queue, ..) = hovered(&session);
    // Across onto the second monitor, where no window is: the page's
    // leave says the pointer got there.
    session.command(Command::Motion { by: (1280, 0) });
    until(&mut queue, &mut desk, |desk| desk.pointed.is_none());

    let (mut locker, mut locker_queue, locker_qh, _) = locked(&session);
    cover(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-1");
    assert!(
        locker.pointed.is_none(),
        "the first monitor's lock surface is not under the pointer"
    );
    let second = cover(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-2");
    until(&mut locker_queue, &mut locker, |locker| {
        locker.pointed.as_ref() == Some(&second)
    });

    session.stop(((desk, queue), (locker, locker_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn unlocked_the_window_under_the_pointer_has_it_back_and_the_hidden_cursor_goes() {
    let session = session("lock-gives-the-pointer-back");
    let (mut desk, mut queue, qh, globals, at) = hovered(&session);
    desk.bind_screencopy(&globals, &qh);

    // swaylock hides the cursor the moment the pointer is over it.
    let (mut locker, mut locker_queue, locker_qh, _) = locked(&session);
    let cover = cover(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-1");
    until(&mut locker_queue, &mut locker, |locker| {
        locker.pointed.as_ref() == Some(&cover)
    });
    until(&mut queue, &mut desk, |desk| desk.pointed.is_none());
    locker.hide_cursor();
    locker.unlock();
    locker_queue.roundtrip(&mut locker).expect("unlocked");

    // The page has the pointer back, with the mouse still at rest.
    let window = desk.windows[0].wl_surface().clone();
    until(&mut queue, &mut desk, |desk| {
        desk.pointed.as_ref() == Some(&window)
    });
    // The page sets no cursor of its own, so the pointer is the
    // compositor's arrow, not the nothing swaylock asked for. Which arrow is
    // the machine's theme, so what is asked is only that one is drawn.
    let with = desk.grab(&qh, None, true);
    copy_all(&mut desk, &mut queue, &[with]);
    let at = (
        usize::try_from(at.0).expect("on the monitor"),
        usize::try_from(at.1).expect("on the monitor"),
    );
    assert!(
        around(&mut desk, with, at, 24)
            .iter()
            .any(|&pixel| pixel != rgba(PAGE)),
        "an arrow at the pointer"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn locking_closes_a_windows_menu_and_gives_the_lock_screen_the_keys_and_the_pointer() {
    let session = seated("lock-closes-the-menu");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_coloured(&qh, "page", "page", PAGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| page(facts).is_some());
    let (id, at) = page(&facts).expect("waited for");
    // A right-click menu, holding the keyboard and the pointer, and the
    // pointer resting on it: over its middle, from the origin.
    let menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, id);
    session.command(Command::Motion {
        by: (at.x0 as i32 + 40 + 60, at.y0 as i32 + 30 + 40),
    });
    until(&mut queue, &mut desk, |desk| {
        desk.pointed.as_ref() == Some(menu.wl_surface())
    });

    // Locked: the menu is closed, and neither the keys nor the pointer are
    // the page's any more, though nothing of the lock's is up yet. Nor were
    // the keys the page's on the way, as the menu let them go: the last
    // they arrived anywhere of the page's was in the menu.
    let in_menu = heard(&ear).enter_serial;
    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locked(&session);
    until(&mut queue, &mut desk, |desk| {
        desk.popups_done == 1 && desk.pointed.is_none() && heard(&ear).entered.is_none()
    });
    assert_eq!(
        heard(&ear).enter_serial,
        in_menu,
        "the keys went back to the page as the menu closed"
    );

    // The lock surface comes up, and has both.
    let cover = cover(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-1");
    let held = cover.id();
    until(&mut locker_queue, &mut locker, |locker| {
        locker.pointed.as_ref() == Some(&cover)
            && heard(&locker_ear).entered.as_ref() == Some(&held)
    });

    session.stop(((desk, queue, menu), (locker, locker_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_asked_for_behind_the_lock_screen_takes_nothing_from_it() {
    let session = seated("lock-refuses-a-menu");
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_coloured(&qh, "page", "page", PAGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let window = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });
    let serial = heard(&ear).enter_serial.expect("entered");

    let (mut locker, mut locker_queue, locker_qh, locker_ear) = locked(&session);
    let cover = cover(&mut locker, &mut locker_queue, &locker_qh, "HEADLESS-1");
    let held = cover.id();
    until(&mut locker_queue, &mut locker, |_| {
        heard(&locker_ear).entered.as_ref() == Some(&held)
    });

    // Behind the lock, the page opens a menu that asks for the keyboard
    // and pointer, naming the last input it had: it is dismissed at once.
    let menu = desk.open_menu(
        &qh,
        desk.windows[0].xdg_surface(),
        (40, 30),
        (120, 80),
        serial,
    );
    until(&mut queue, &mut desk, |desk| desk.popups_done == 1);
    locker_queue
        .roundtrip(&mut locker)
        .expect("whatever was sent");
    assert_eq!(
        heard(&locker_ear).entered.as_ref(),
        Some(&held),
        "the lock screen keeps the keys"
    );
    assert!(heard(&ear).entered.is_none(), "the page has none of them");

    session.stop(((desk, queue, menu), (locker, locker_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_resize_under_way_as_the_session_locks_is_told_it_is_over() {
    let session = seated("lock-ends-a-resize");
    let (mut desk, mut queue, _qh, _globals, (_, at)) = with_page(&session);
    // The person presses on the page, which asks to be resized from its
    // corner, as a toolkit drawing its own frame does, and drags.
    session.command(Command::Press {
        at: middle(at),
        button: Button::Left,
        mods: Mods::default(),
    });
    until(&mut queue, &mut desk, |desk| desk.pressed.is_some());
    let seat = desk.seat.clone().expect("our pointer's seat");
    desk.windows[0].resize(
        &seat,
        desk.pressed.expect("waited for"),
        ResizeEdge::BottomRight,
    );
    // A resize takes the pointer from every client.
    until(&mut queue, &mut desk, |desk| desk.pointed.is_none());
    session.command(Command::Motion { by: (20, 20) });
    until(&mut queue, &mut desk, |desk| desk.resizing);

    // Locked with the button still down: the resize is over.
    let (locker, locker_queue, ..) = locked(&session);
    until(&mut queue, &mut desk, |desk| !desk.resizing);

    session.stop(((desk, queue), (locker, locker_queue)));
}
