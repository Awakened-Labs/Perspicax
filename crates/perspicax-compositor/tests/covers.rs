//! What a surface draws, and so what it covers.
//!
//! A client says where its window is inside what it draws, with
//! `xdg_surface.set_window_geometry`: the visible frame, without the shadow
//! a toolkit draws around it. That is where the window is, and where an
//! agent's coordinates are measured from. It is not how far the window
//! reaches. What a client draws past its geometry -- a shadow, a subsurface
//! hanging out of it -- is still drawn over whatever is under it, and still
//! takes the person's clicks there, and a client can declare its window as
//! small as it likes (issue #103). So the facts say both: `geometry`, where
//! the window is, and `drawn`, everything it draws.
//!
//! And a window covers everything it draws. Where its client says it is
//! opaque, a node under it is covered on the client's word; anywhere else
//! it draws, on policy, because a client that leaves part of itself out of
//! its opaque region has not said that part is see-through. One opaque
//! pixel declared far off the screen used to prove a whole window
//! see-through, and is here too.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.
//! They mean the same in a plain headless build, where smithay measures
//! nothing, and in one with `capture`, where it does and the compositor
//! checks at every commit that the two agree.

mod common;

use common::{Desk, Geometry, Session, WINDOW, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, Judgement, SurfaceFacts, SurfaceKind, judge};
use perspicax_node::{Rect, Vec2, Visibility};
use smithay_client_toolkit::shell::{WaylandSurface, wlr_layer::Layer};
use wayland_client::{EventQueue, QueueHandle};

const PICTURE: u32 = 0xff99_6633;
const STRIP: u32 = 0xff33_6699;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
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

/// `rect`, in a surface's own coordinates, in global space: placed from
/// where the surface begins, which under a shadow is not where its window
/// does.
fn from_buffer(surface: &SurfaceFacts, (x, y, w, h): Geometry) -> Rect {
    let Vec2 { x: x0, y: y0 } = surface.buffer_origin;
    Rect::new(
        x0 + f64::from(x),
        y0 + f64::from(y),
        x0 + f64::from(x + w),
        y0 + f64::from(y + h),
    )
}

/// A rect of `under`'s window space that lies at `at` in `over`'s.
fn beneath(under: &SurfaceFacts, over: &SurfaceFacts, at: Rect) -> Rect {
    within(
        under,
        Rect::new(
            at.x0 + over.geometry.x0,
            at.y0 + over.geometry.y0,
            at.x1 + over.geometry.x0,
            at.y1 + over.geometry.y0,
        ),
    )
}

/// A rect given in global space, in `window`'s own window space: where a
/// node of it there would be.
fn within(window: &SurfaceFacts, global: Rect) -> Rect {
    let area = window.geometry;
    assert!(
        area.x0 <= global.x0
            && area.y0 <= global.y0
            && global.x1 <= area.x1
            && global.y1 <= area.y1,
        "{global:?} is inside the window at {area:?}"
    );
    Rect::new(
        global.x0 - area.x0,
        global.y0 - area.y0,
        global.x1 - area.x0,
        global.y1 - area.y0,
    )
}

/// Covered by `by`, and whether on its client's word or on policy.
fn covered(by: &SurfaceFacts, proven: bool) -> Judgement {
    Judgement {
        visibility: Visibility::Occluded { by: by.id },
        unproven: !proven,
    }
}

/// The whole picture the harness draws for every window, where it is drawn.
fn picture(surface: &SurfaceFacts) -> Rect {
    let (w, h) = WINDOW;
    from_buffer(
        surface,
        (0, 0, i32::try_from(w).unwrap(), i32::try_from(h).unwrap()),
    )
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_declaring_a_one_pixel_geometry_is_described_as_drawing_all_its_picture() {
    let session = session("covers-one-pixel");
    let (mut desk, mut queue, qh, _) = session.client();

    let tiny = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "tiny",
        Some((0, 0, 1, 1)),
    );

    assert_eq!(
        (tiny.geometry.width(), tiny.geometry.height()),
        (1.0, 1.0),
        "its window is where its client said"
    );
    assert_eq!(
        tiny.drawn,
        picture(&tiny),
        "and what it draws is all of its picture"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_shadow_is_part_of_what_a_window_draws() {
    let session = session("covers-shadow");
    let (mut desk, mut queue, qh, _) = session.client();

    // Firefox's shape: a frame 26 px in and 23 down from the outside of a
    // shadow drawn all round it.
    let shadowed = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "shadowed",
        Some((26, 23, 300, 200)),
    );

    assert_eq!(
        shadowed.geometry,
        from_buffer(&shadowed, (26, 23, 300, 200)),
        "the window is the frame inside the shadow"
    );
    assert_eq!(
        shadowed.drawn,
        picture(&shadowed),
        "and it draws the shadow too"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_subsurface_hanging_outside_a_window_is_part_of_what_it_draws() {
    let session = session("covers-subsurface");
    let (mut desk, mut queue, qh, _) = session.client();
    let window = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "window",
        Some((0, 0, 400, 300)),
    );
    let root = desk.windows[0].wl_surface().clone();

    // A picture hanging out of the window's top left corner, outside the
    // geometry its client declared.
    let (_placed, child) = desk.open_subsurface(&qh, &root, (-30, -30), false);
    root.commit();
    desk.paint(&child, (60, 40), PICTURE);
    child.commit();
    let hanging = becomes(&session, &mut desk, &mut queue, "window", |window| {
        window.drawn != window.geometry
    });
    assert_eq!(
        hanging.geometry, window.geometry,
        "the window is where it was"
    );
    assert_eq!(
        hanging.drawn,
        from_buffer(&hanging, (-30, -30, 430, 330)),
        "and it draws past its corner"
    );

    // Its picture taken away, it draws only its window again.
    child.attach(None, 0, 0);
    child.commit();
    becomes(&session, &mut desk, &mut queue, "window", |window| {
        window.drawn == window.geometry
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_layer_surfaces_subsurface_is_part_of_what_it_draws_on_every_backend() {
    let session = session("covers-layer-subsurface");
    let (mut desk, mut queue, qh, _) = session.client();
    let strip = |facts: &HostFacts| {
        facts
            .surfaces()
            .iter()
            .find(|surface| {
                surface.mapped
                    && matches!(&surface.kind, SurfaceKind::Layer { namespace, .. } if namespace == "strip")
            })
            .cloned()
    };
    desk.open_strip(&qh, Layer::Overlay, "strip", 40, STRIP);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let placed = strip(&session.wait_for(|facts| strip(facts).is_some())).expect("waited for");
    assert_eq!(placed.drawn, placed.geometry, "a strip draws itself");

    // A picture hanging below the strip, as a launcher's list might.
    let root = desk.layers[0].0.wl_surface().clone();
    let (_placed, child) = desk.open_subsurface(&qh, &root, (100, 30), false);
    root.commit();
    desk.paint(&child, (60, 40), PICTURE);
    child.commit();
    queue.roundtrip(&mut desk).expect("round trip");
    let hanging =
        strip(&session.wait_for(|facts| {
            strip(facts).is_some_and(|strip| strip.drawn.y1 > placed.geometry.y1)
        }))
        .expect("waited for");

    // Only `drawn`: a build that keeps buffers places the strip at the size
    // smithay measures, subsurface and all, and a plain one at the size it
    // was configured to. What it draws is the same either way.
    assert_eq!(
        hanging.drawn,
        Rect::new(
            placed.geometry.x0,
            placed.geometry.y0,
            placed.geometry.x1,
            placed.geometry.y0 + 70.0
        )
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_declares_a_one_pixel_geometry_covers_the_window_under_it() {
    let session = session("covers-one-pixel-over");
    let (mut desk, mut queue, qh, _) = session.client();
    let under = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "under",
        Some((0, 0, 400, 300)),
    );
    // The issue's own: a 400x300 picture over the window under it, from a
    // client that says its window is one pixel at the picture's corner.
    let tiny = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "tiny",
        Some((0, 0, 1, 1)),
    );
    let facts = session.facts.read();

    let hidden = within(
        &under,
        Rect::new(
            tiny.drawn.x0 + 100.0,
            tiny.drawn.y0 + 100.0,
            tiny.drawn.x0 + 120.0,
            tiny.drawn.y0 + 120.0,
        ),
    );
    assert_eq!(
        judge(&facts, under.id, hidden),
        covered(&tiny, false),
        "100 px inside its picture, and nowhere near its one pixel"
    );
    let clear = Rect::new(1.0, 1.0, 5.0, 5.0);
    assert!(
        under.geometry.x0 + clear.x1 <= tiny.drawn.x0,
        "the top left of the one under is outside the picture"
    );
    assert_eq!(
        judge(&facts, under.id, clear).visibility,
        Visibility::Visible
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_declares_one_opaque_pixel_off_the_screen_covers_the_window_under_it() {
    let session = session("covers-far-pixel");
    let (mut desk, mut queue, qh, _) = session.client();
    let under = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "under",
        Some((0, 0, 400, 300)),
    );
    settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "over",
        Some((0, 0, 400, 300)),
    );
    let root = desk.windows[1].wl_surface().clone();

    // The issue's second way: an ordinary window says it is opaque in one
    // pixel, far off any screen, and nowhere else.
    desk.set_opaque(&root, (10000, 10000, 1, 1));
    root.commit();
    let over = becomes(&session, &mut desk, &mut queue, "over", |over| {
        over.opaque == [Rect::new(10000.0, 10000.0, 10001.0, 10001.0)]
    });
    let facts = session.facts.read();

    let hidden = beneath(&under, &over, Rect::new(100.0, 100.0, 120.0, 120.0));
    assert_eq!(
        judge(&facts, under.id, hidden),
        covered(&over, false),
        "a pixel declared opaque over there proves nothing see-through here"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_shadow_covers_the_window_under_it() {
    let session = session("covers-shadow-over");
    let (mut desk, mut queue, qh, _) = session.client();
    let under = settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "under",
        Some((0, 0, 400, 300)),
    );
    settle(
        &session,
        &mut desk,
        &mut queue,
        &qh,
        "over",
        Some((26, 23, 300, 200)),
    );
    let root = desk.windows[1].wl_surface().clone();

    // Firefox's shape, as a toolkit that says what it can: the frame is
    // opaque, and the shadow around it is not said to be.
    desk.set_opaque(&root, (26, 23, 300, 200));
    root.commit();
    let over = becomes(&session, &mut desk, &mut queue, "over", |over| {
        !over.opaque.is_empty()
    });
    let facts = session.facts.read();

    // Each window cascades a step down and right of the one before, so the
    // shadow over's left edge throws lies on the window under it.
    let in_shadow = within(
        &under,
        Rect::new(
            over.drawn.x0 + 5.0,
            over.geometry.y0 + 5.0,
            over.drawn.x0 + 15.0,
            over.geometry.y0 + 15.0,
        ),
    );
    assert_eq!(
        judge(&facts, under.id, in_shadow),
        covered(&over, false),
        "under the shadow, on policy"
    );
    let in_frame = beneath(&under, &over, Rect::new(10.0, 10.0, 30.0, 30.0));
    assert_eq!(
        judge(&facts, under.id, in_frame),
        covered(&over, true),
        "under the frame, on its client's word"
    );
    let clear = Rect::new(1.0, 1.0, 5.0, 5.0);
    assert!(
        under.geometry.x0 + clear.x1 <= over.drawn.x0
            && under.geometry.y0 + clear.y1 <= over.drawn.y0,
        "the top left of the one under is outside the shadow"
    );
    assert_eq!(
        judge(&facts, under.id, clear).visibility,
        Visibility::Visible,
        "and past the shadow, nothing is drawn over it"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_subsurface_proves_it_covers_what_is_under_it_only_where_it_says_it_is_opaque() {
    let session = session("covers-subsurfaces");
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

    // Its own surface says a 10 px corner of it is opaque. One subsurface
    // says it is opaque all over, and another says nothing.
    let (_said, opaque) = desk.open_subsurface(&qh, &root, (100, 100), false);
    let (_unsaid, silent) = desk.open_subsurface(&qh, &root, (250, 150), false);
    desk.set_opaque(&root, (0, 0, 10, 10));
    root.commit();
    desk.paint(&opaque, (100, 100), PICTURE);
    desk.set_opaque(&opaque, (0, 0, 100, 100));
    opaque.commit();
    desk.paint(&silent, (60, 60), PICTURE);
    silent.commit();
    let over = becomes(&session, &mut desk, &mut queue, "over", |over| {
        over.opaque.len() == 2
    });
    let facts = session.facts.read();

    for (at, proven, said) in [
        (
            (2.0, 2.0),
            true,
            "under the corner its surface said is opaque",
        ),
        (
            (120.0, 120.0),
            true,
            "under the subsurface that said it is opaque",
        ),
        (
            (260.0, 160.0),
            false,
            "under the subsurface that said nothing",
        ),
        ((250.0, 20.0), false, "under the rest of its own surface"),
    ] {
        let (x, y) = at;
        let node = beneath(&under, &over, Rect::new(x, y, x + 6.0, y + 6.0));
        assert_eq!(
            judge(&facts, under.id, node),
            covered(&over, proven),
            "{said}"
        );
    }

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_subsurface_that_says_it_is_opaque_proves_it_though_its_window_said_nothing() {
    let session = session("covers-subsurface-alone");
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

    let (_said, opaque) = desk.open_subsurface(&qh, &root, (100, 100), false);
    root.commit();
    desk.paint(&opaque, (100, 100), PICTURE);
    desk.set_opaque(&opaque, (0, 0, 100, 100));
    opaque.commit();
    let over = becomes(&session, &mut desk, &mut queue, "over", |over| {
        over.opaque.len() == 1
    });
    let facts = session.facts.read();

    let under_it = beneath(&under, &over, Rect::new(120.0, 120.0, 140.0, 140.0));
    assert_eq!(judge(&facts, under.id, under_it), covered(&over, true));
    let beside_it = beneath(&under, &over, Rect::new(250.0, 20.0, 270.0, 40.0));
    assert_eq!(judge(&facts, under.id, beside_it), covered(&over, false));

    session.stop((desk, queue));
}
