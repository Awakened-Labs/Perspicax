//! A screenshot tool's view, by a compositor with no screen:
//! `wlr-screencopy-unstable-v1`, as grim speaks it.
//!
//! A frame of the monitor, copied into the tool's own shared memory, shows a
//! window's colour where the window is; a region shows just that part. A
//! buffer that is not what the frame described is a protocol error. A copy
//! that waits for damage waits for a commit. While the session is locked
//! every copy fails, and with `[protocols] screencopy = "off"` there is no
//! global to ask.
//!
//! The pointer is in a frame that asks for it, as `grim -c` and a recorder
//! with its cursor on do, and in no other: the client's own cursor where it
//! set one, the compositor's arrow where it did not. A frame like that,
//! waiting for damage, is copied when the pointer moves, with nothing
//! committed; one that did not ask goes on waiting.
//!
//! Only with the `capture` feature. Like the other live tests it binds a
//! real Wayland socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Session, advertised, until};
use perspicax_compositor::{Backend, Host, Virtual};
use perspicax_index::{Action as Verb, HostFacts};
use perspicax_node::Rect;
use perspicax_policy::{Access, Protocol, Rule, Shape};
use wayland_client::{EventQueue, QueueHandle, globals::GlobalList};

const BLUE: u32 = 0xff33_6699;
const RED: u32 = 0xffcc_2222;

fn rgba(argb: u32) -> [u8; 4] {
    let [b, g, r, a] = argb.to_le_bytes();
    [r, g, b, a]
}

fn session(name: &str, access: Access) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![Virtual::numbered(1, (800, 600))],
            workspaces: Shape::default(),
            access,
        },
    )
}

fn area(facts: &HostFacts) -> Rect {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.mapped)
        .map(|surface| surface.geometry)
        .expect("a window on screen")
}

/// A blue window, with screencopy bound and the pointer moved onto it at
/// `local`, in the window's own coordinates: an agent's scroll, which moves
/// the pointer and changes nothing on screen. Returns the window, and where
/// the pointer is on the monitor.
fn point_into_a_window(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    globals: &GlobalList,
    local: (f64, f64),
) -> (Rect, (usize, usize)) {
    desk.open_coloured(qh, "blue", "blue", BLUE);
    until(queue, desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| facts.surfaces().iter().any(|s| s.mapped));
    let area = area(&facts);
    desk.bind_screencopy(globals, qh);
    desk.bind_pointer(globals, qh);
    until(queue, desk, |desk| !desk.outputs_empty());
    let pointer = move_pointer(session, &facts, area, local);
    until(queue, desk, |desk| desk.entered.is_some());
    (area, pointer)
}

/// Move the pointer to `local` in the window's own coordinates, and say
/// where on the monitor that is.
fn move_pointer(
    session: &Session,
    facts: &HostFacts,
    area: Rect,
    local: (f64, f64),
) -> (usize, usize) {
    let window = facts
        .surfaces()
        .iter()
        .find(|surface| surface.mapped)
        .expect("a window on screen")
        .id;
    let (x, y) = local;
    Host::new(&session.facts, &session.requests)
        .act(
            window,
            &Verb::Scroll {
                at: Rect::new(x - 1.0, y - 1.0, x + 1.0, y + 1.0),
                dx: 0.0,
                dy: 1.0,
            },
        )
        .expect("the pointer moved");
    ((area.x0 + x) as usize, (area.y0 + y) as usize)
}

/// Copy each grab, at once, once each has been offered its buffer.
fn copy_all(desk: &mut Desk, queue: &mut EventQueue<Desk>, grabs: &[usize]) {
    until(queue, desk, |desk| {
        grabs.iter().all(|&grab| desk.grabs[grab].buffer_done)
    });
    for &grab in grabs {
        desk.copy(grab, 0, false);
    }
    until(queue, desk, |desk| {
        grabs.iter().all(|&grab| desk.grabs[grab].ready)
    });
}

/// The pixels of a square around `at`, out to `reach` on every side.
fn around(desk: &mut Desk, grab: usize, at: (usize, usize), reach: usize) -> Vec<[u8; 4]> {
    let (x, y) = at;
    let mut pixels = Vec::new();
    for row in y - reach..=y + reach {
        for column in x - reach..=x + reach {
            pixels.push(desk.grabbed(grab, column, row));
        }
    }
    pixels
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn grim_gets_the_monitor_with_the_window_on_it() {
    let session = session("screencopy-output", Access::open());
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let area = area(&session.wait_for(|facts| facts.surfaces().iter().any(|s| s.mapped)));
    desk.bind_screencopy(&globals, &qh);
    until(&mut queue, &mut desk, |desk| !desk.outputs_empty());

    let grab = desk.grab(&qh, None, false);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].buffer_done);
    // One offer: a client older than version 3 copies on every one, and a
    // second copy of a frame is a protocol error (grim 1.4 hit it).
    assert_eq!(desk.grabs[grab].offered.len(), 1);
    let (_, width, height, stride) = desk.grabs[grab].offered[0];
    assert_eq!((width, height, stride), (800, 600, 3200));

    desk.copy(grab, 0, false);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].ready);
    let (x, y) = (
        ((area.x0 + area.x1) / 2.0) as usize,
        ((area.y0 + area.y1) / 2.0) as usize,
    );
    assert_eq!(desk.grabbed(grab, x, y), rgba(BLUE));
    assert_ne!(desk.grabbed(grab, 799, 599), rgba(BLUE));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_region_is_just_that_part_of_the_monitor() {
    let session = session("screencopy-region", Access::open());
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let area = area(&session.wait_for(|facts| facts.surfaces().iter().any(|s| s.mapped)));
    desk.bind_screencopy(&globals, &qh);
    until(&mut queue, &mut desk, |desk| !desk.outputs_empty());

    // A 20x20 square at the window's top-left corner.
    let (x, y) = (area.x0 as i32, area.y0 as i32);
    let grab = desk.grab(&qh, Some((x, y, 20, 20)), false);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].buffer_done);
    let (_, width, height, _) = desk.grabs[grab].offered[0];
    assert_eq!((width, height), (20, 20));
    desk.copy(grab, 0, false);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].ready);
    assert_eq!(desk.grabbed(grab, 0, 0), rgba(BLUE));
    assert_eq!(desk.grabbed(grab, 19, 19), rgba(BLUE));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_buffer_of_the_wrong_size_is_a_protocol_error() {
    let session = session("screencopy-wrong", Access::open());
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_screencopy(&globals, &qh);
    until(&mut queue, &mut desk, |desk| !desk.outputs_empty());
    let grab = desk.grab(&qh, None, false);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].buffer_done);

    desk.copy(grab, 1, false);
    assert!(
        queue.roundtrip(&mut desk).is_err(),
        "the compositor ends the connection"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_copy_with_damage_waits_for_something_to_change() {
    let session = session("screencopy-damage", Access::open());
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_screencopy(&globals, &qh);
    until(&mut queue, &mut desk, |desk| !desk.outputs_empty());
    let grab = desk.grab(&qh, None, false);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].buffer_done);

    desk.copy(grab, 0, true);
    queue.roundtrip(&mut desk).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(!desk.grabs[grab].ready, "nothing has changed yet");

    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].ready);
    assert!(desk.grabs[grab].damaged);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn every_copy_fails_while_locked() {
    let session = session("screencopy-locked", Access::open());
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_screencopy(&globals, &qh);
    until(&mut queue, &mut desk, |desk| !desk.outputs_empty());

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    let grab = desk.grab(&qh, None, false);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].buffer_done);
    desk.copy(grab, 0, false);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].failed);
    assert!(!desk.grabs[grab].ready);

    desk.unlock();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn there_is_nothing_to_ask_when_off() {
    let session = session(
        "screencopy-off",
        Access::open().with(Protocol::Screencopy, Rule::Off),
    );
    let (desk, queue, _qh, globals) = session.client();
    assert!(!advertised(&globals, "zwlr_screencopy_manager_v1"));
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_frame_shows_the_pointer_only_when_asked() {
    let session = session("screencopy-pointer", Access::open());
    let (mut desk, mut queue, qh, globals) = session.client();
    let (_, (x, y)) = point_into_a_window(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        &globals,
        (100.0, 80.0),
    );
    desk.set_cursor(&qh, 8, RED);

    let with = desk.grab(&qh, None, true);
    let without = desk.grab(&qh, None, false);
    let (left, top) = (
        i32::try_from(x).unwrap() - 10,
        i32::try_from(y).unwrap() - 10,
    );
    let region = desk.grab(&qh, Some((left, top, 20, 20)), true);
    copy_all(&mut desk, &mut queue, &[with, without, region]);

    assert_eq!(
        desk.grabbed(with, x, y),
        rgba(RED),
        "the client's cursor, at the pointer"
    );
    assert_eq!(desk.grabbed(with, x + 7, y + 7), rgba(RED), "all of it");
    assert_eq!(desk.grabbed(with, x + 8, y + 8), rgba(BLUE), "and no more");
    assert_eq!(
        desk.grabbed(with, x - 1, y - 1),
        rgba(BLUE),
        "its hotspot is its corner"
    );
    assert_eq!(
        desk.grabbed(without, x, y),
        rgba(BLUE),
        "not asked for, not drawn"
    );
    assert_eq!(
        desk.grabbed(region, 10, 10),
        rgba(RED),
        "where it is in the region"
    );
    assert_eq!(desk.grabbed(region, 9, 9), rgba(BLUE));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn without_a_cursor_of_its_own_the_pointer_is_the_compositors_arrow() {
    let session = session("screencopy-arrow", Access::open());
    let (mut desk, mut queue, qh, globals) = session.client();
    let (_, at) = point_into_a_window(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        &globals,
        (100.0, 80.0),
    );

    let with = desk.grab(&qh, None, true);
    let without = desk.grab(&qh, None, false);
    copy_all(&mut desk, &mut queue, &[with, without]);

    // The theme's arrow, or the generated one where there is no theme: which
    // is the machine's, so what is asked is only that something is drawn.
    assert!(
        around(&mut desk, with, at, 24)
            .iter()
            .any(|&pixel| pixel != rgba(BLUE)),
        "an arrow at the pointer"
    );
    assert!(
        around(&mut desk, without, at, 24)
            .iter()
            .all(|&pixel| pixel == rgba(BLUE)),
        "nothing but the window, not asked for"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_frame_waiting_for_damage_with_the_pointer_is_copied_when_the_pointer_moves() {
    let session = session("screencopy-pointer-moves", Access::open());
    let (mut desk, mut queue, qh, globals) = session.client();
    let (area, from) = point_into_a_window(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        &globals,
        (100.0, 80.0),
    );

    let shown = desk.grab(&qh, None, true);
    let plain = desk.grab(&qh, None, false);
    until(&mut queue, &mut desk, |desk| {
        desk.grabs[shown].buffer_done && desk.grabs[plain].buffer_done
    });
    desk.copy(shown, 0, true);
    desk.copy(plain, 0, true);
    queue.roundtrip(&mut desk).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(
        !desk.grabs[shown].ready && !desk.grabs[plain].ready,
        "nothing has changed yet"
    );

    let facts = session.wait_for(|facts| facts.surfaces().iter().any(|s| s.mapped));
    let to = move_pointer(&session, &facts, area, (200.0, 150.0));
    until(&mut queue, &mut desk, |desk| desk.grabs[shown].ready);
    assert!(desk.grabs[shown].damaged);
    assert!(
        around(&mut desk, shown, to, 24)
            .iter()
            .any(|&pixel| pixel != rgba(BLUE)),
        "the pointer where it went"
    );
    assert!(
        around(&mut desk, shown, from, 24)
            .iter()
            .all(|&pixel| pixel == rgba(BLUE)),
        "and not where it was"
    );

    // The control: had something committed, this frame would be ready too.
    queue.roundtrip(&mut desk).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(
        !desk.grabs[plain].ready,
        "the pointer moving changes nothing in a picture without it"
    );

    session.stop((desk, queue));
}
