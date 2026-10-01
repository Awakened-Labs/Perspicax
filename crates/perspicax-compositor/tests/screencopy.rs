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
//! Only with the `capture` feature. Like the other live tests it binds a
//! real Wayland socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Session, advertised, until};
use perspicax_compositor::{Backend, Virtual};
use perspicax_index::HostFacts;
use perspicax_node::Rect;
use perspicax_policy::{Access, Protocol, Rule, Shape};

const BLUE: u32 = 0xff33_6699;

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

    let grab = desk.grab(&qh, None);
    until(&mut queue, &mut desk, |desk| desk.grabs[grab].buffer_done);
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
    let grab = desk.grab(&qh, Some((x, y, 20, 20)));
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
    let grab = desk.grab(&qh, None);
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
    let grab = desk.grab(&qh, None);
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
    let grab = desk.grab(&qh, None);
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
