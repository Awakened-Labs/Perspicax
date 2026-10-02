//! Pictures, by a compositor with no screen: drawn in software when one is
//! asked for, from the buffers the clients last committed.
//!
//! A picture of a monitor shows a window's colour where the window is and
//! the backdrop where nothing is, and accounts for every surface in it. A
//! picture of one window shows it whole, even under another. And while the
//! session is locked there are no pictures.
//!
//! Only with the `capture` feature. Like the other live tests it binds a
//! real Wayland socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Session, until};
use perspicax_compositor::{ActError, Backend, Host};
use perspicax_index::{HostFacts, Shot, ShotTarget};
use perspicax_node::{Rect, SurfaceId};

/// ARGB, as the client writes it.
const BLUE: u32 = 0xff33_6699;
const ORANGE: u32 = 0xffee_8822;

fn rgba(argb: u32) -> [u8; 4] {
    let [b, g, r, a] = argb.to_le_bytes();
    [r, g, b, a]
}

fn pixel(shot: &Shot, x: f64, y: f64) -> [u8; 4] {
    let (x, y) = ((x * shot.scale) as usize, (y * shot.scale) as usize);
    let at = (y * shot.width as usize + x) * 4;
    shot.rgba[at..at + 4].try_into().unwrap()
}

fn geometry(facts: &HostFacts, title: &str) -> (SurfaceId, Rect) {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| (surface.id, surface.geometry))
        .expect("a window of that title")
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_picture_of_a_monitor_shows_the_window_and_says_who_drew_it() {
    let session = Session::start("capture-output", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| facts.surfaces().iter().any(|surface| surface.mapped));
    let (id, area) = geometry(&facts, "blue");

    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
    assert_eq!((shot.width, shot.height), (800, 600));
    assert_eq!(shot.output.as_deref(), Some("HEADLESS-1"));
    let middle = (area.x0 + area.x1) / 2.0;
    let centre = (area.y0 + area.y1) / 2.0;
    assert_eq!(pixel(&shot, middle, centre), rgba(BLUE));
    assert_ne!(pixel(&shot, 799.0, 599.0), rgba(BLUE), "the backdrop");
    let window = shot
        .drawn
        .iter()
        .find(|drawn| drawn.surface == id)
        .expect("the window is accounted for");
    assert_eq!(window.rect, area);
    assert!(shot.redacted.is_empty(), "headless consents to everything");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_picture_of_a_window_shows_it_whole_under_another() {
    let session = Session::start("capture-window", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_coloured(&qh, "orange", "orange", ORANGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let facts = session.wait_for(|facts| facts.surfaces().iter().filter(|s| s.mapped).count() == 2);
    let (blue, area) = geometry(&facts, "blue");

    // The orange window is cascaded over the blue one, so on the monitor
    // the blue one's bottom-right corner is orange.
    let monitor = host.capture(ShotTarget::Output(None)).expect("a picture");
    assert_eq!(
        pixel(&monitor, area.x1 - 10.0, area.y1 - 10.0),
        rgba(ORANGE),
        "covered on the monitor"
    );
    let shot = host.capture(ShotTarget::Window(blue)).expect("a picture");
    assert_eq!((shot.width, shot.height), (400, 300));
    assert_eq!(pixel(&shot, 10.0, 10.0), rgba(BLUE));
    assert_eq!(
        pixel(&shot, 390.0, 290.0),
        rgba(BLUE),
        "not the window over it"
    );
    assert_eq!(shot.drawn.len(), 1);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn there_are_no_pictures_while_locked() {
    let session = Session::start("capture-locked", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    assert_eq!(
        host.capture(ShotTarget::Output(None)),
        Err(ActError::Locked)
    );

    desk.unlock();
    session.stop((desk, queue));
}
