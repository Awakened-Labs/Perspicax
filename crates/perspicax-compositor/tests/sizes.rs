//! A size from a client that smithay 0.7 would take unchecked is checked
//! first, and refused as the protocol says, so no client can end the
//! session with one request.
//!
//! Smithay's `Size` asserts in a debug build that neither of its sides is
//! negative, and smithay 0.7.0 builds one straight from what a client sent
//! in several requests. Until issue #106 any client panicked a debug build
//! of perspicax that way -- every `cargo run`, and every one of these tests
//! -- and a release build took the values, turned inside out. Each test
//! sends one such request, sees it refused or dropped, and sees the
//! compositor still running: `Session::stop` fails a test whose compositor
//! panicked.
//!
//! The same audit found numbers each fine on their own whose sum is past
//! what an `i32` holds: a region's far edge, a surface's corner behind a
//! geometry at `i32::MIN`. Until #154 adding them panicked a debug build
//! too, and wrapped round in a release one. Those tests send such numbers,
//! and see them described where the client put them, or, where smithay
//! adds them up itself, taken into the world (`perspicax_policy::FAR`) first.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, refused, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, SurfaceFacts, SurfaceKind};
use perspicax_node::Rect;
use perspicax_policy::FAR;
use smithay_client_toolkit::shell::{
    WaylandSurface,
    wlr_layer::Layer,
    xdg::{XdgPositioner, XdgSurface},
};
use wayland_client::{EventQueue, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_positioner;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

/// Open a window called `title`, and wait until it has drawn and the facts
/// show it.
fn settle(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
) {
    let drawn = desk.drawn;
    desk.open_window(qh, title, title);
    until(queue, desk, |desk| desk.drawn == drawn + 1);
    session.wait_for(|facts| titled(facts, title).is_some_and(|window| window.mapped));
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
    queue.roundtrip(desk).expect("the compositor answers");
    let facts = session.wait_for(|facts| titled(facts, title).is_some_and(&ready));
    titled(&facts, title).expect("waited for").clone()
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_damage_turned_inside_out_is_dropped() {
    let session = session("sizes-damage");
    let (mut desk, mut queue, qh, _) = session.client();
    settle(&session, &mut desk, &mut queue, &qh, "w");
    let surface = desk.windows[0].wl_surface().clone();

    surface.damage(0, 0, -1, -1);
    surface.commit();
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert!(!session.ended(), "the compositor is still running");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_region_rectangle_without_area_is_dropped() {
    let session = session("sizes-region");
    let (mut desk, mut queue, qh, _) = session.client();
    settle(&session, &mut desk, &mut queue, &qh, "w");
    let surface = desk.windows[0].wl_surface().clone();
    let whole = Rect::new(0.0, 0.0, 400.0, 300.0);

    // Added, a rectangle turned inside out adds nothing.
    let region = desk.region();
    region.add(0, 0, -1, -1);
    surface.set_opaque_region(Some(region.wl_region()));
    surface.commit();
    becomes(&session, &mut desk, &mut queue, "w", |w| {
        w.opaque.is_empty()
    });

    // Taken away, it takes nothing away: nor does one with no area at all.
    let region = desk.region();
    region.add(0, 0, 400, 300);
    region.subtract(0, 0, -1, -1);
    region.subtract(100, 100, 0, 50);
    surface.set_opaque_region(Some(region.wl_region()));
    surface.commit();
    becomes(&session, &mut desk, &mut queue, "w", |w| {
        w.opaque == [whole]
    });
    assert!(!session.ended(), "the compositor is still running");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_geometry_without_area_is_a_protocol_error() {
    let session = session("sizes-geometry");
    // xdg-shell: "The width and height of the effective window geometry
    // must be greater than zero. Setting an invalid size will raise an
    // invalid_size error." Each on a client of its own, since the error
    // ends the client.
    for (x, y, width, height) in [(0, 0, -1, -1), (0, 0, 0, 0), (0, 0, 400, 0)] {
        let title = format!("{width}x{height}");
        let (mut desk, mut queue, qh, _) = session.client();
        settle(&session, &mut desk, &mut queue, &qh, &title);
        let window = desk.windows[0].clone();

        window
            .xdg_surface()
            .set_window_geometry(x, y, width, height);
        window.commit();
        assert_eq!(
            refused(&mut queue, &mut desk),
            ("xdg_surface".to_owned(), 5),
            "{title}: invalid_size"
        );
        assert!(!session.ended(), "the compositor is still running");
    }

    let (desk, queue, ..) = session.client();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_negative_min_or_max_size_is_a_protocol_error() {
    let session = session("sizes-limits");

    // Zero is "no limit", and is taken.
    let (mut desk, mut queue, qh, _) = session.client();
    settle(&session, &mut desk, &mut queue, &qh, "w");
    let toplevel = desk.windows[0].xdg_toplevel().clone();
    toplevel.set_min_size(0, 0);
    toplevel.set_max_size(0, 0);
    desk.windows[0].commit();
    queue.roundtrip(&mut desk).expect("the compositor answers");

    // xdg-shell: "Using strictly negative values for width or height will
    // result in a invalid_size error."
    for (which, width, height) in [
        ("min", -1, -1),
        ("min", 100, -1),
        ("max", -1, -1),
        ("max", 100, -1),
    ] {
        let title = format!("{which} {width}x{height}");
        let (mut desk, mut queue, qh, _) = session.client();
        settle(&session, &mut desk, &mut queue, &qh, &title);
        let toplevel = desk.windows[0].xdg_toplevel();
        if which == "min" {
            toplevel.set_min_size(width, height);
        } else {
            toplevel.set_max_size(width, height);
        }
        desk.windows[0].commit();
        assert_eq!(
            refused(&mut queue, &mut desk),
            ("xdg_toplevel".to_owned(), 2),
            "{title}: invalid_size"
        );
        assert!(!session.ended(), "the compositor is still running");
    }

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_negative_parent_size_is_a_protocol_error() {
    let session = session("sizes-parent");

    // A parent size of the window's own places a menu as ever.
    let (mut desk, mut queue, qh, _) = session.client();
    settle(&session, &mut desk, &mut queue, &qh, "w");
    let positioner = desk.positioner((10, 10), (100, 50));
    positioner.set_parent_size(400, 300);
    let menu = desk.open_popup_by(&qh, desk.windows[0].xdg_surface(), &positioner);
    until(&mut queue, &mut desk, |desk| desk.popups_configured == 1);

    // One turned inside out describes no parent: `invalid_input`, as the
    // positioner's other sizes get.
    for (width, height) in [(-1, -1), (400, -1)] {
        let title = format!("{width}x{height}");
        let (mut desk, mut queue, qh, _) = session.client();
        settle(&session, &mut desk, &mut queue, &qh, &title);
        let positioner = desk.positioner((10, 10), (100, 50));
        positioner.set_parent_size(width, height);
        assert_eq!(
            refused(&mut queue, &mut desk),
            ("xdg_positioner".to_owned(), 0),
            "{title}: invalid_input"
        );
        assert!(!session.ended(), "the compositor is still running");
    }

    session.stop((menu, desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_layer_size_past_i32_is_a_protocol_error() {
    let session = session("sizes-layer");
    // layer-shell's sizes are `u32`s; smithay casts them to `i32`s, so one
    // past `i32::MAX` would come out negative.
    let past = u32::try_from(i32::MAX).unwrap() + 1;
    for (width, height) in [(past, 1), (0, u32::MAX)] {
        let (mut desk, mut queue, qh, _) = session.client();
        desk.open_strip(&qh, Layer::Top, "strip", 30, 0xff33_6699);
        until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
        let (strip, ..) = &desk.layers[0];
        strip.set_size(width, height);
        strip.commit();
        assert_eq!(
            refused(&mut queue, &mut desk),
            ("zwlr_layer_surface_v1".to_owned(), 1),
            "{width}x{height}: invalid_size"
        );
        assert!(!session.ended(), "the compositor is still running");
    }

    let (desk, queue, ..) = session.client();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_pool_resized_to_nothing_is_a_protocol_error() {
    let session = session("sizes-pool");
    // A pool only grows. Smithay 0.7.0 posts `invalid_fd` for a size of 0
    // or below, then went on to resize the pool to it: at 0 that unwrapped
    // a `NonZeroUsize` of 0, and panicked a release build too.
    for size in [0, -1] {
        let (mut desk, mut queue, ..) = session.client();
        let pool = desk.raw_pool(4096);
        pool.pool().resize(size);
        assert_eq!(
            refused(&mut queue, &mut desk),
            ("wl_shm_pool".to_owned(), 2),
            "resized to {size}: invalid_fd"
        );
        assert!(!session.ended(), "the compositor is still running");
        drop(pool);
    }

    let (desk, queue, ..) = session.client();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_opaque_region_at_the_edge_of_i32_is_described_as_sent() {
    let session = session("sizes-far-region");
    let (mut desk, mut queue, qh, _) = session.client();
    settle(&session, &mut desk, &mut queue, &qh, "w");
    let surface = desk.windows[0].wl_surface().clone();
    let edge = f64::from(i32::MAX);

    // Every number in it is one a client may send, and its far edge is one
    // past what an `i32` holds (#154).
    let region = desk.region();
    region.add(i32::MAX, 0, 1, 1);
    surface.set_opaque_region(Some(region.wl_region()));
    surface.commit();
    becomes(&session, &mut desk, &mut queue, "w", |w| {
        w.opaque == [Rect::new(edge, 0.0, edge + 1.0, 1.0)]
    });

    // A subsurface's, which is measured into its window's at every commit.
    let root = desk.windows[0].wl_surface().clone();
    let (_placed, child) = desk.open_subsurface(&qh, &root, (40, 30), false);
    root.commit();
    desk.set_opaque(&child, (i32::MAX, i32::MAX, 1, 1));
    desk.paint(&child, (120, 80), 0xff00_00ff);
    child.commit();
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert!(!session.ended(), "the compositor is still running");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_geometry_declared_at_i32_min_before_drawing_is_described() {
    let session = session("sizes-far-geometry");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_bare(&qh, "far");
    queue.roundtrip(&mut desk).expect("the compositor answers");

    // Nothing is drawn, so the declared geometry is the window's as it is;
    // its surface's corner is then 2^31 to the right of it.
    let window = desk.blank[0].clone();
    window.xdg_surface().set_window_geometry(i32::MIN, 0, 1, 1);
    window.commit();
    let far = becomes(&session, &mut desk, &mut queue, "far", |w| {
        w.geometry.width() == 1.0
    });
    assert_eq!(
        far.buffer_origin.x - far.geometry.x0,
        2_147_483_648.0,
        "the surface's corner is where the client put it"
    );
    assert!(!session.ended(), "the compositor is still running");

    session.stop((desk, queue));
}

/// A positioner placing a popup of 100x50 from `anchor`, `(x, y, width,
/// height)`, on its `edge`, growing towards `gravity`.
fn positioned(
    desk: &Desk,
    anchor: (i32, i32, i32, i32),
    edge: xdg_positioner::Anchor,
    gravity: xdg_positioner::Gravity,
) -> XdgPositioner {
    let positioner = desk.positioner((0, 0), (100, 50));
    positioner.set_anchor_rect(anchor.0, anchor.1, anchor.2, anchor.3);
    positioner.set_anchor(edge);
    positioner.set_gravity(gravity);
    positioner
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_anchor_rectangle_at_the_edge_of_i32_is_taken_into_the_world() {
    use xdg_positioner::{Anchor, Gravity};
    let session = session("sizes-far-anchor");
    // Smithay 0.7.0 adds the anchor rectangle's width, or half of it, to its
    // corner in `i32` to find where a popup hangs from (#154).
    let shapes = [
        ((i32::MAX, 0, 1, 1), Anchor::Right, FAR + 1),
        ((i32::MAX, 0, 2, 1), Anchor::None, FAR + 1 - 50),
        ((1, 0, i32::MAX, 1), Anchor::Right, FAR + 1),
        ((i32::MIN, 0, 1, 1), Anchor::Left, -FAR),
    ];
    for (anchor, edge, x) in shapes {
        let title = format!("{anchor:?} {edge:?}");
        let (mut desk, mut queue, qh, _) = session.client();
        settle(&session, &mut desk, &mut queue, &qh, &title);
        let positioner = positioned(&desk, anchor, edge, Gravity::BottomRight);
        let gravity = if edge == Anchor::None {
            Gravity::None
        } else {
            Gravity::BottomRight
        };
        positioner.set_gravity(gravity);
        let menu = desk.open_popup_by(&qh, desk.windows[0].xdg_surface(), &positioner);
        until(&mut queue, &mut desk, |desk| desk.popups_configured == 1);
        assert_eq!(
            desk.popups_placed[0].0, x,
            "{title}: hung from the anchor where it meets the world's edge"
        );
        assert!(!session.ended(), "the compositor is still running");
        drop(menu);
    }

    let (desk, queue, ..) = session.client();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_offset_at_i32_min_is_taken_into_the_world() {
    use xdg_positioner::{Anchor, Gravity};
    let session = session("sizes-far-offset");
    // Smithay 0.7.0 adds the offset to the anchor's point saturating, then
    // takes the popup's width away from it in `i32`, to grow it leftwards.
    // The anchor is at (10, 10).
    for (offset, gravity, x) in [
        ((i32::MIN, 0), Gravity::Left, 10 - FAR - 100),
        ((i32::MIN, 0), Gravity::None, 10 - FAR - 50),
        ((0, i32::MIN), Gravity::Top, 10 - 50),
    ] {
        let title = format!("{offset:?} {gravity:?}");
        let (mut desk, mut queue, qh, _) = session.client();
        settle(&session, &mut desk, &mut queue, &qh, &title);
        let positioner = positioned(&desk, (10, 10, 1, 1), Anchor::TopLeft, gravity);
        positioner.set_offset(offset.0, offset.1);
        let menu = desk.open_popup_by(&qh, desk.windows[0].xdg_surface(), &positioner);
        until(&mut queue, &mut desk, |desk| desk.popups_configured == 1);
        assert_eq!(desk.popups_placed[0].0, x, "{title}");
        assert!(!session.ended(), "the compositor is still running");
        drop(menu);
    }

    let (desk, queue, ..) = session.client();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_popup_size_past_the_world_is_taken_into_it() {
    use xdg_positioner::{Anchor, Gravity};
    // With a person at the seat, a popup is kept on its monitor, and smithay
    // 0.7.0 measures how far it sticks out in `i32`: its corner plus its
    // width.
    let session = Session::start(
        "sizes-far-popup",
        Backend::headless((1280, 1024)).with_person(),
    );
    let (mut desk, mut queue, qh, _) = session.client();
    settle(&session, &mut desk, &mut queue, &qh, "w");
    let positioner = positioned(&desk, (10, 0, 1, 1), Anchor::TopLeft, Gravity::BottomRight);
    positioner.set_size(i32::MAX, 1);
    let menu = desk.open_popup_by(&qh, desk.windows[0].xdg_surface(), &positioner);
    until(&mut queue, &mut desk, |desk| desk.popups_configured == 1);
    let (.., width, _) = desk.popups_placed[0];
    assert!(width <= FAR, "as wide as the world at most: {width}");
    assert!(!session.ended(), "the compositor is still running");

    session.stop((menu, desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_layer_margin_at_i32_min_is_taken_into_the_world() {
    let session = session("sizes-far-margin");
    // Smithay 0.7.0 takes each anchored side's margin away from the room the
    // layer has, in `i32`. The strip is anchored top, left and right across
    // a monitor 1280 wide; a margin below 0 puts it past the monitor's edge,
    // here as far as the world's. A side margin widens it, and it draws
    // again as wide as it is told to be.
    let (far, wide) = (-f64::from(FAR), f64::from(1280 + FAR));
    for (top, right, left, at, redrawn) in [
        (0, 0, i32::MIN, (far, 0.0, 1280.0), true),
        (0, i32::MIN, 0, (0.0, 0.0, wide), true),
        (i32::MIN, 0, 0, (0.0, far, 1280.0), false),
    ] {
        let namespace = format!("{top} {right} {left}");
        let (mut desk, mut queue, qh, _) = session.client();
        desk.open_strip(&qh, Layer::Top, &namespace, 30, 0xff33_6699);
        until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
        let (strip, ..) = &desk.layers[0];
        strip.set_margin(top, right, 0, left);
        strip.commit();
        if redrawn {
            until(&mut queue, &mut desk, |desk| desk.layers_drawn == 2);
        }
        queue.roundtrip(&mut desk).expect("the compositor answers");
        session.wait_for(|facts| {
            layer_named(facts, &namespace).is_some_and(|strip| {
                (strip.geometry.x0, strip.geometry.y0, strip.geometry.x1) == at
            })
        });
        assert!(!session.ended(), "the compositor is still running");
    }

    let (desk, queue, ..) = session.client();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_exclusive_zone_past_the_world_is_taken_into_it() {
    let session = session("sizes-far-zone");
    // Smithay 0.7.0 moves the room left for windows past a panel's zone and
    // margin, adding the two in `i32`.
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_panel(&qh, "panel", 30, 0xff33_6699);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let (panel, ..) = &desk.layers[0];
    panel.set_exclusive_zone(i32::MAX);
    panel.set_margin(1, 0, 0, 0);
    panel.commit();
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert!(!session.ended(), "the compositor is still running");

    session.stop((desk, queue));
}

fn layer_named<'a>(facts: &'a HostFacts, namespace: &str) -> Option<&'a SurfaceFacts> {
    facts.surfaces().iter().find(|surface| {
        matches!(&surface.kind, SurfaceKind::Layer { namespace: named, .. } if named == namespace)
    })
}
