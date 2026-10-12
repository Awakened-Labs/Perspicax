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
//! With no person at the seat, these see the lock alone move the pointer:
//! with one, a change of the keyboard's focus looks under the pointer again
//! too, a turn of the loop later. The hand is [`Command::Motion`], never a
//! click, which would leave a button's grab behind for a moment. Finding
//! what is under the pointer takes the clients' buffers, so this needs the
//! `capture` feature. Like the other live tests it binds a real Wayland
//! socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Session, around, copy_all, rgba, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::HostFacts;
use perspicax_node::Rect;
use perspicax_policy::{Access, Place, Shape, Side};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{EventQueue, QueueHandle, globals::GlobalList, protocol::wl_surface};

const PAGE: u32 = 0xff33_6699;
const COVER: u32 = 0xff20_2020;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
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

/// A client with a window called "page", drawn, and the pointer resting on
/// its middle. Returns where that is on the desk.
fn hovered(
    session: &Session,
) -> (
    Desk,
    EventQueue<Desk>,
    QueueHandle<Desk>,
    GlobalList,
    (i32, i32),
) {
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.open_coloured(&qh, "page", "page", PAGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| page(facts).is_some());
    let at = page(&facts).expect("waited for");
    let middle = (
        ((at.x0 + at.x1) / 2.0).round() as i32,
        ((at.y0 + at.y1) / 2.0).round() as i32,
    );
    // From the origin, where the pointer starts.
    session.command(Command::Motion { by: middle });
    let window = desk.windows[0].wl_surface().clone();
    until(&mut queue, &mut desk, |desk| {
        desk.pointed.as_ref() == Some(&window)
    });
    (desk, queue, qh, globals, middle)
}

fn page(facts: &HostFacts) -> Option<Rect> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.mapped && surface.title.as_deref() == Some("page"))
        .map(|surface| surface.geometry)
}

/// A second client, as swaylock is, with a pointer of its own, the session
/// locked by it, and nothing yet covering any monitor.
fn locked(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (mut locker, mut queue, qh, globals) = session.client();
    locker.bind_pointer(&globals, &qh);
    until(&mut queue, &mut locker, |locker| !locker.outputs_empty());
    locker.lock(&globals, &qh);
    until(&mut queue, &mut locker, |locker| locker.locked);
    (locker, queue, qh)
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
    let (mut locker, mut locker_queue, locker_qh) = locked(&session);
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

    let (mut locker, mut locker_queue, locker_qh) = locked(&session);
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
    let (mut locker, mut locker_queue, locker_qh) = locked(&session);
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
