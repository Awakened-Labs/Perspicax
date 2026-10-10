//! A window whose client never declares its geometry is as big as what it
//! draws.
//!
//! A client says where its window is inside its surface with
//! `xdg_surface.set_window_geometry`. GStreamer's `waylandsink` never does,
//! and xdg-shell says what that means: "If never set, the value is the full
//! bounds of the surface, including any subsurfaces." Until issue #44 such a
//! window was left out of the facts altogether: missing from `window_list`,
//! and covering nothing, so a node of another window under it was judged
//! visible, and any client could hide a control that way on purpose.
//!
//! The rest is the same question asked from the other side: a geometry
//! declared without area is no geometry, one declared larger than what is
//! drawn is clipped to it, as smithay draws it, and what a window's
//! subsurfaces draw covers what is under them even where its own surface
//! said it was see-through.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.
//! They mean the same in a plain headless build, where smithay measures
//! nothing, and in one with `capture`, where it does and the compositor
//! checks at every commit that the two agree.

mod common;

use common::{Desk, Geometry, Session, WINDOW, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, SurfaceFacts, judge};
use perspicax_node::{Rect, Vec2, Visibility};
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::{EventQueue, QueueHandle};

const VIDEO: u32 = 0xff22_3344;
const PICTURE: u32 = 0xff99_6633;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

fn width(surface: &SurfaceFacts) -> f64 {
    surface.geometry.x1 - surface.geometry.x0
}

fn height(surface: &SurfaceFacts) -> f64 {
    surface.geometry.y1 - surface.geometry.y0
}

fn origin(surface: &SurfaceFacts) -> Vec2 {
    Vec2::new(surface.geometry.x0, surface.geometry.y0)
}

/// Open a window called `title` that declares `geometry` -- `None` for
/// none at all -- and wait until it has drawn and the facts show it.
fn settle(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
    geometry: Option<Geometry>,
) -> SurfaceFacts {
    let drawn = desk.drawn;
    desk.open_declaring(qh, title, geometry);
    until(queue, desk, |desk| desk.drawn == drawn + 1);
    let facts = session.wait_for(|facts| titled(facts, title).is_some_and(|window| window.mapped));
    titled(&facts, title).expect("waited for").clone()
}

/// Once the compositor has taken in everything sent so far, wait until the
/// window titled `title` is as `ready` says.
fn becomes(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    title: &str,
    ready: impl Fn(&SurfaceFacts) -> bool,
) -> SurfaceFacts {
    queue.roundtrip(desk).expect("round trip");
    let facts = session.wait_for(|facts| titled(facts, title).is_some_and(&ready));
    titled(&facts, title).expect("waited for").clone()
}

/// A rect of `under`'s window space that lies at `at` in `over`'s.
fn beneath(under: &SurfaceFacts, over: &SurfaceFacts, at: Rect) -> Rect {
    let (dx, dy) = (
        over.geometry.x0 - under.geometry.x0,
        over.geometry.y0 - under.geometry.y0,
    );
    let rect = Rect::new(at.x0 + dx, at.y0 + dy, at.x1 + dx, at.y1 + dy);
    assert!(
        rect.x1 <= width(under) && rect.y1 <= height(under),
        "{rect:?} is inside the window under"
    );
    rect
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_declares_no_geometry_is_described_at_the_size_it_draws() {
    let session = session("undeclared-described");
    let (mut desk, mut queue, qh, _) = session.client();

    let video = settle(&session, &mut desk, &mut queue, &qh, "video", None);

    assert_eq!(
        (width(&video), height(&video)),
        (f64::from(WINDOW.0), f64::from(WINDOW.1)),
        "as big as its buffer"
    );
    assert_eq!(
        video.buffer_origin,
        origin(&video),
        "no shadow: the surface begins where the window does"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_declares_no_geometry_covers_the_window_under_it() {
    let session = session("undeclared-covers");
    let (mut desk, mut queue, qh, _) = session.client();
    let under = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "under",
        Some((0, 0, 400, 300)),
    );
    let over = settle(&session, &mut desk, &mut queue, &qh, "over", None);
    let facts = session.wait_for(|_| true);

    let hidden = beneath(&under, &over, Rect::new(10.0, 10.0, 30.0, 30.0));
    assert_eq!(
        judge(&facts, under.id, hidden).visibility,
        Visibility::Occluded { by: over.id },
        "under a window that declared nothing"
    );
    // Each window opens a cascade step down and right of the one before, so
    // the top left of the one under is still in view.
    assert_eq!(
        judge(&facts, under.id, Rect::new(5.0, 5.0, 20.0, 20.0)).visibility,
        Visibility::Visible
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_declares_no_geometry_takes_in_its_subsurfaces() {
    let session = session("undeclared-subsurfaces");
    let (mut desk, mut queue, qh, _) = session.client();
    let video = settle(&session, &mut desk, &mut queue, &qh, "video", None);
    let root = desk.windows[0].wl_surface().clone();
    let (w, h) = (f64::from(WINDOW.0), f64::from(WINDOW.1));

    // `waylandsink`'s shape: the picture in a subsurface of its own, here
    // reaching out past the window's own surface on the right.
    let (_placed, child) = desk.open_subsurface(&qh, &root, (360, 20), false);
    root.commit();
    desk.paint(&child, (120, 80), VIDEO);
    child.commit();
    let wider = becomes(&session, &mut desk, &mut queue, "video", |video| {
        width(video) == 480.0
    });
    assert_eq!(height(&wider), h, "no taller: the subsurface ends inside");
    assert_eq!(origin(&wider), origin(&video), "where it was placed");

    // Its picture taken away, it shows nothing, and reaches nowhere.
    child.attach(None, 0, 0);
    child.commit();
    becomes(&session, &mut desk, &mut queue, "video", |video| {
        width(video) == w
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_subsurface_above_and_left_of_a_window_moves_where_its_surface_begins() {
    let session = session("undeclared-above-left");
    let (mut desk, mut queue, qh, _) = session.client();
    let video = settle(&session, &mut desk, &mut queue, &qh, "video", None);
    let root = desk.windows[0].wl_surface().clone();

    let (_placed, child) = desk.open_subsurface(&qh, &root, (-20, -10), false);
    root.commit();
    desk.paint(&child, (60, 40), VIDEO);
    child.commit();
    let grown = becomes(&session, &mut desk, &mut queue, "video", |video| {
        width(video) == f64::from(WINDOW.0) + 20.0
    });

    assert_eq!(height(&grown), f64::from(WINDOW.1) + 10.0);
    // The window stays where it was put, and now begins 20 left and 10 up
    // of its own surface -- which is where smithay draws that surface from.
    assert_eq!(origin(&grown), origin(&video));
    assert_eq!(grown.buffer_origin - origin(&grown), Vec2::new(20.0, 10.0));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_subsurface_covers_what_its_window_said_was_see_through() {
    let session = session("undeclared-opaque");
    let (mut desk, mut queue, qh, _) = session.client();
    let under = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "under",
        Some((0, 0, 400, 300)),
    );
    settle(&session, &mut desk, &mut queue, &qh, "over", None);
    let root = desk.windows[1].wl_surface().clone();

    // Its own surface says only a 10 px corner of it is opaque, and a
    // subsurface that says nothing is drawn over the rest.
    desk.set_opaque(&root, (0, 0, 10, 10));
    let (_placed, child) = desk.open_subsurface(&qh, &root, (100, 100), false);
    root.commit();
    desk.paint(&child, (100, 100), PICTURE);
    child.commit();
    let over = becomes(&session, &mut desk, &mut queue, "over", |over| {
        over.opaque.as_ref().is_some_and(|opaque| opaque.len() == 2)
    });
    let facts = session.wait_for(|_| true);

    let under_picture = beneath(&under, &over, Rect::new(120.0, 120.0, 140.0, 140.0));
    assert_eq!(
        judge(&facts, under.id, under_picture).visibility,
        Visibility::Occluded { by: over.id },
        "under the subsurface's picture"
    );
    let see_through = beneath(&under, &over, Rect::new(250.0, 20.0, 270.0, 40.0));
    assert_eq!(
        judge(&facts, under.id, see_through).visibility,
        Visibility::Visible,
        "under the part of the window it said was see-through, with nothing drawn over it"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_has_drawn_nothing_and_declares_nothing_is_not_described() {
    let session = session("undeclared-blank");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_bare(&qh, "bare");
    until(&mut queue, &mut desk, |desk| desk.blank_configured >= 1);
    // A window opened after it, to know the facts have caught up with it.
    settle(&session, &mut desk, &mut queue, &qh, "after", None);
    let facts = session.wait_for(|_| true);
    assert!(
        titled(&facts, "bare").is_none(),
        "nothing on screen, nothing to describe"
    );

    let surface = desk.blank[0].wl_surface().clone();
    desk.paint(&surface, WINDOW, VIDEO);
    surface.commit();
    let bare = becomes(&session, &mut desk, &mut queue, "bare", |bare| bare.mapped);
    assert_eq!(
        (width(&bare), height(&bare)),
        (f64::from(WINDOW.0), f64::from(WINDOW.1))
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_geometry_declared_without_area_is_no_geometry() {
    let session = session("undeclared-no-area");
    let (mut desk, mut queue, qh, _) = session.client();
    // Smithay takes either without complaint, and would clip either to
    // nothing. A negative size is refused here the same way, but only a
    // release build lets one through to be refused: in a debug build, such
    // as this one, smithay asserts against it as it reads the request.
    for (title, geometry) in [("empty", (0, 0, 0, 0)), ("flat", (0, 0, 400, 0))] {
        let window = settle(&session, &mut desk, &mut queue, &qh, title, Some(geometry));
        assert_eq!(
            (width(&window), height(&window)),
            (f64::from(WINDOW.0), f64::from(WINDOW.1)),
            "{title}: as big as what it draws"
        );
    }

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_geometry_declared_larger_than_what_is_drawn_is_clipped_to_it() {
    let session = session("undeclared-clipped");
    let (mut desk, mut queue, qh, _) = session.client();
    let (w, h) = (
        i32::try_from(WINDOW.0).unwrap(),
        i32::try_from(WINDOW.1).unwrap(),
    );
    let window = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "generous",
        Some((-10, -10, w + 20, h + 20)),
    );

    // As smithay clips it to draw it: the surface begins where the window
    // does, not 10 px inside a margin that was never drawn.
    assert_eq!(
        (width(&window), height(&window)),
        (f64::from(WINDOW.0), f64::from(WINDOW.1))
    );
    assert_eq!(window.buffer_origin, origin(&window));

    session.stop((desk, queue));
}
