//! Windows drawn see-through, as the person's `[opacity]` says, by a
//! compositor with no screen.
//!
//! A window under its application's rule is drawn that opaque, so a picture
//! shows what is behind it mixed in; one without the keyboard is dimmed by a
//! share of its own; a fullscreen window, and a picture of one window alone,
//! are drawn whole. The facts say how opaque each window is drawn, and an
//! agent is still told what is behind one is covered: translucency is for the
//! person's eyes.
//!
//! The clients here draw ARGB buffers. Pictures are drawn with pixman, which
//! draws a buffer without an alpha channel by copying rather than blending,
//! so an XRGB window under a rule would come out darkened, not mixed; on a
//! seat, GLES blends either. Headless only draws see-through when told to, as
//! here, since an agent's desk has no `[opacity]`.
//!
//! Only with the `capture` feature. Like the other live tests it binds a
//! real Wayland socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Session, until};
use perspicax_compositor::{Backend, Host};
use perspicax_index::{HostFacts, Shot, ShotTarget, SurfaceFacts, judge};
use perspicax_node::{Rect, Visibility};
use perspicax_policy::Opacity;

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

/// `over` drawn `percent` opaque on `under`, as a renderer mixes them.
fn mixed(over: [u8; 4], under: [u8; 4], percent: u8) -> [u8; 4] {
    let share = f64::from(percent) / 100.0;
    let channel =
        |at: usize| (f64::from(over[at]) * share + f64::from(under[at]) * (1.0 - share)).round();
    [channel(0) as u8, channel(1) as u8, channel(2) as u8, 0xff]
}

/// Whether two colours are the same within rounding.
fn near(seen: [u8; 4], wanted: [u8; 4]) -> bool {
    seen.iter()
        .zip(wanted)
        .all(|(&seen, wanted)| seen.abs_diff(wanted) <= 2)
}

fn surface<'a>(facts: &'a HostFacts, title: &str) -> &'a SurfaceFacts {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .expect("a window of that title")
}

/// Headless at 800x600, with `opacity` as a seat's `[opacity]` would say.
fn see_through(opacity: Opacity) -> Backend {
    let mut backend = Backend::headless((800, 600));
    if let Backend::Headless { opacity: into, .. } = &mut backend {
        *into = opacity;
    }
    backend
}

fn ruled(app: &str, percent: u8) -> Opacity {
    Opacity {
        apps: [(app.to_owned(), percent)].into(),
        ..Opacity::default()
    }
}

/// A point of the blue window that the orange one, under it, also covers:
/// its top left, since each window opens down and right of the one before.
fn over_orange(facts: &HostFacts) -> (f64, f64) {
    let blue = surface(facts, "blue").geometry;
    let orange = surface(facts, "orange").geometry;
    assert!(blue.x0 > orange.x0 && blue.y0 > orange.y0, "cascaded");
    (blue.x0 + 10.0, blue.y0 + 10.0)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_under_its_applications_rule_shows_what_is_behind_it_and_still_covers_it() {
    let session = Session::start("opacity-rule", see_through(ruled("blue", 50)));
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_coloured(&qh, "orange", "orange", ORANGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let facts = session.wait_for(|facts| {
        facts.surfaces().iter().filter(|s| s.mapped).count() == 2
            && surface(facts, "blue").drawn_opacity == Some(50)
    });
    assert_eq!(surface(&facts, "orange").drawn_opacity, None, "no rule");

    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
    let (x, y) = over_orange(&facts);
    let seen = pixel(&shot, x, y);
    assert!(
        near(seen, mixed(rgba(BLUE), rgba(ORANGE), 50)),
        "half blue, half the orange behind: {seen:?}"
    );

    // And yet covered, as far as an agent is told, on the same footing as
    // if blue were opaque: it declared nothing, so on policy.
    let orange = surface(&facts, "orange");
    let blue = surface(&facts, "blue");
    let (dx, dy) = (
        blue.geometry.x0 - orange.geometry.x0,
        blue.geometry.y0 - orange.geometry.y0,
    );
    let behind = Rect::new(dx + 5.0, dy + 5.0, dx + 25.0, dy + 25.0);
    assert_eq!(
        judge(&facts, orange.id, behind).visibility,
        Visibility::Occluded { by: blue.id }
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_without_the_keyboard_is_dimmed_and_the_one_with_it_is_not() {
    let session = Session::start(
        "opacity-unfocused",
        see_through(Opacity {
            unfocused: 50,
            ..Opacity::default()
        }),
    );
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_coloured(&qh, "orange", "orange", ORANGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    // Opened last, so it has the keyboard.
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let facts = session.wait_for(|facts| {
        facts.surfaces().iter().filter(|s| s.mapped).count() == 2
            && surface(facts, "orange").drawn_opacity == Some(50)
    });
    assert_eq!(surface(&facts, "blue").drawn_opacity, None, "in use");

    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
    let backdrop = pixel(&shot, 799.0, 599.0);
    let orange = surface(&facts, "orange").geometry;
    let seen = pixel(&shot, orange.x0 + 10.0, orange.y0 + 10.0);
    assert!(
        near(seen, mixed(rgba(ORANGE), backdrop, 50)),
        "orange dimmed over the backdrop {backdrop:?}: {seen:?}"
    );
    let (x, y) = over_orange(&facts);
    assert_eq!(pixel(&shot, x, y), rgba(BLUE), "in use, undimmed");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_fullscreen_window_is_drawn_whole_and_its_own_opacity_returns_when_it_leaves() {
    let session = Session::start("opacity-fullscreen", see_through(ruled("blue", 50)));
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_coloured(&qh, "orange", "orange", ORANGE);
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| {
        desk.task("orange").is_some() && desk.task("blue").is_some()
    });
    session.wait_for(|facts| surface(facts, "blue").drawn_opacity == Some(50));

    desk.task("blue")
        .expect("a known window")
        .handle
        .set_fullscreen(None);
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| {
        let blue = surface(facts, "blue");
        (blue.geometry.x0, blue.geometry.y0) == (0.0, 0.0) && blue.drawn_opacity.is_none()
    });
    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
    let orange = surface(&facts, "orange").geometry;
    assert_eq!(
        pixel(&shot, orange.x0 + 10.0, orange.y0 + 10.0),
        rgba(BLUE),
        "nothing shows through a fullscreen window"
    );

    desk.task("blue")
        .expect("a known window")
        .handle
        .unset_fullscreen();
    queue.flush().expect("sent");
    session.wait_for(|facts| surface(facts, "blue").drawn_opacity == Some(50));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_picture_of_a_see_through_window_alone_shows_it_whole() {
    let session = Session::start("opacity-window", see_through(ruled("blue", 50)));
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_coloured(&qh, "blue", "blue", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| surface(facts, "blue").drawn_opacity == Some(50));

    let shot = host
        .capture(ShotTarget::Window(surface(&facts, "blue").id))
        .expect("a picture");
    assert_eq!(pixel(&shot, 10.0, 10.0), rgba(BLUE));
    assert_eq!(pixel(&shot, 390.0, 290.0), rgba(BLUE));

    session.stop((desk, queue));
}
