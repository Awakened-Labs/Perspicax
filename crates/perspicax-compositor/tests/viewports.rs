//! Windows drawn at a size of their own and shown at another:
//! `wp_viewporter`, as a client drawing at a fractional scale speaks it.
//!
//! Firefox on a monitor at 0.9 draws a buffer 0.9 times its window and says,
//! through a viewport, that it is shown at the window's size; offered no
//! viewporter it was drawn at 0.81 of its window, with its clicks landing
//! where it laid things out rather than where it drew them (issue #135). A
//! window so drawn is described at the size it is shown at, and a viewport
//! changed with no new buffer is a frame where the window was and is. On a
//! monitor, the window's pixels fill it; damage named in its buffer is where
//! its pixels changed, and none of what the viewport crops away; and the
//! pointer reaches all of it.
//!
//! Pictures are taken on a monitor at a scale that undoes the viewport's, so
//! that each of the buffer's pixels is one of the picture's and the
//! comparison is exact. Drawn any larger or smaller, smithay blends the
//! picture's edges with what is beside them.
//!
//! The pictures and the pointer need the `capture` feature. Like the other
//! live tests it binds a real Wayland socket, so it needs `XDG_RUNTIME_DIR`,
//! and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Session, advertised, until};
use perspicax_compositor::Backend;
use perspicax_index::HostFacts;
use perspicax_node::{Rect, SurfaceId};
use smithay_client_toolkit::shell::WaylandSurface;

/// ARGB, as the client writes it.
const BLUE: u32 = 0xff33_6699;

/// The window titled `title`, once it is on screen: its id and where it is.
fn shown(session: &Session, title: &str) -> (SurfaceId, Rect) {
    let facts = session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.mapped && surface.title.as_deref() == Some(title))
    });
    window(&facts, title)
}

fn window(facts: &HostFacts, title: &str) -> (SurfaceId, Rect) {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| (surface.id, surface.geometry))
        .expect("a window of that title")
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_client_finds_the_viewporter() {
    let session = Session::start("viewports-offered", Backend::headless((800, 600)));
    let (desk, queue, _qh, globals) = session.client();
    assert!(advertised(&globals, "wp_viewporter"));
    session.stop((desk, queue));
}

/// Firefox at 0.9, halved for round numbers: a buffer half the window's
/// size, shown at all of it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_drawn_at_half_its_size_is_described_at_the_size_it_is_shown() {
    let session = Session::start("viewports-described", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_viewporter(&globals, &qh);
    desk.open_viewed(&qh, "viewed", BLUE, (200, 150), (400, 300));
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);

    let (_, area) = shown(&session, "viewed");
    assert_eq!((area.width(), area.height()), (400.0, 300.0));

    session.stop((desk, queue));
}

/// A client may change only its viewport, with the buffer it already
/// showed: the window is resized all the same, and what it covered and
/// covers now changed.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_viewport_changed_with_no_new_buffer_is_a_frame_where_the_window_was_and_is() {
    let session = Session::start("viewports-changed", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_viewporter(&globals, &qh);
    desk.open_viewed(&qh, "viewed", BLUE, (200, 150), (400, 300));
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let (id, was) = shown(&session, "viewed");
    let seen = session
        .wait_for(|facts| facts.surface(id).is_some_and(|s| s.damage_generation >= 1))
        .surface(id)
        .expect("the window")
        .damage_generation;

    let viewed = &mut desk.viewed[0];
    viewed.shown = (200, 150);
    viewed.viewport.set_destination(200, 150);
    viewed.window.wl_surface().commit();
    queue.roundtrip(&mut desk).expect("round trip");

    let facts = session.wait_for(|facts| {
        facts
            .surface(id)
            .is_some_and(|window| window.damage_generation > seen)
    });
    let window = facts.surface(id).expect("the window");
    let now = Rect::new(was.x0, was.y0, was.x0 + 200.0, was.y0 + 150.0);
    assert_eq!(window.geometry, now);
    assert_eq!(window.damage_generation, seen + 1, "one frame");
    assert_eq!(window.damage_since(seen), Some(vec![was, now]));

    session.stop((desk, queue));
}

#[cfg(feature = "capture")]
mod pictures {
    use super::*;
    use crate::common::{drawn_in, rescale};
    use perspicax_compositor::{Command, Host};
    use perspicax_index::ShotTarget;
    use wayland_client::protocol::wl_output;

    const ORANGE: u32 = 0xffee_8822;
    const MARKED: u32 = 0xff22_cc44;

    /// The window fills its frame on the monitor, the band beyond its buffer
    /// that Firefox left see-through gone, and a picture says so.
    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn a_window_drawn_at_half_its_size_fills_its_frame_on_the_monitor() {
        let session = Session::start("viewports-filled", Backend::headless((800, 600)));
        let host = Host::new(&session.facts, &session.requests);
        let (mut desk, mut queue, qh, globals) = session.client();
        rescale(&mut desk, &mut queue, &qh, &globals, "HEADLESS-1", 0.5);
        desk.bind_viewporter(&globals, &qh);
        desk.open_viewed(&qh, "viewed", BLUE, (200, 150), (400, 300));
        until(&mut queue, &mut desk, |desk| desk.drawn == 1);
        let (id, area) = shown(&session, "viewed");
        assert_eq!((area.width(), area.height()), (400.0, 300.0));

        let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
        assert_eq!(shot.scale, 0.5);
        assert_eq!(drawn_in(&shot, BLUE), [area], "all of it, and no more");
        let drawn = shot
            .drawn
            .iter()
            .find(|drawn| drawn.surface == id)
            .expect("the window is accounted for");
        assert_eq!(drawn.rect, area);

        session.stop((desk, queue));
    }

    /// What a receipt reads, against what is drawn, through a viewport that
    /// crops a buffer and shows the rest at twice its size: damage named in
    /// the buffer's pixels is where they changed on the monitor, and damage
    /// named outside what the viewport shows changed nothing.
    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn damage_named_in_a_viewed_buffer_is_where_its_pixels_changed() {
        let session = Session::start("viewports-damage", Backend::headless((800, 600)));
        let host = Host::new(&session.facts, &session.requests);
        let (mut desk, mut queue, qh, globals) = session.client();
        rescale(&mut desk, &mut queue, &qh, &globals, "HEADLESS-1", 0.5);
        desk.bind_viewporter(&globals, &qh);
        desk.open_coloured(&qh, "page", "page", BLUE);
        until(&mut queue, &mut desk, |desk| desk.drawn == 1);
        let (id, _) = shown(&session, "page");
        let root = desk.windows[0].wl_surface().clone();
        let (_placed, child) = desk.open_subsurface(&qh, &root, (40, 30), false);
        root.commit();

        // A picture 120 by 80, of which the part from (10, 5), 60 by 40, is
        // shown at 120 by 80: orange, but for a corner of its pixels, which
        // is all it names and which the source cuts in half each way.
        let viewport = desk.viewport(&qh, &child);
        viewport.set_source(10.0, 5.0, 60.0, 40.0);
        viewport.set_destination(120, 80);
        let seen = session
            .wait_for(|facts| facts.surface(id).is_some_and(|s| s.damage_generation >= 1))
            .surface(id)
            .expect("the window")
            .damage_generation;
        desk.paint_turned(
            &child,
            (120, 80),
            wl_output::Transform::Normal,
            ORANGE,
            (20, 10),
            MARKED,
        );
        child.commit();
        queue.roundtrip(&mut desk).expect("round trip");
        let facts = session.wait_for(|facts| {
            facts
                .surface(id)
                .is_some_and(|window| window.damage_generation > seen)
        });
        let window = facts.surface(id).expect("the window");
        let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
        let drawn = drawn_in(&shot, MARKED);
        assert_eq!(drawn, [Rect::new(40.0, 30.0, 60.0, 40.0)]);
        assert_eq!(window.damage_since(seen), Some(drawn), "named, then drawn");

        // Named where the viewport shows nothing, with the same picture
        // again and then with none: neither is a frame. Then where it shows
        // something, which is.
        let seen = window.damage_generation;
        desk.paint_naming(
            &child,
            (120, 80),
            ORANGE,
            (20, 10),
            MARKED,
            (100, 60, 10, 10),
        );
        child.commit();
        child.damage_buffer(100, 60, 10, 10);
        child.commit();
        child.damage_buffer(10, 5, 10, 5);
        child.commit();
        queue.roundtrip(&mut desk).expect("round trip");
        let facts = session.wait_for(|facts| {
            facts
                .surface(id)
                .is_some_and(|window| window.damage_generation > seen)
        });
        let window = facts.surface(id).expect("the window");
        assert_eq!(window.damage_generation, seen + 1, "one frame");
        assert_eq!(
            window.damage_since(seen),
            Some(vec![Rect::new(40.0, 30.0, 60.0, 40.0)])
        );

        session.stop((desk, queue));
    }

    /// Where the person points on the window is where its client hears it,
    /// as far as its bottom-right corner.
    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn the_pointer_reaches_all_of_a_window_drawn_at_half_its_size() {
        let session = Session::start(
            "viewports-pointer",
            Backend::headless((800, 600)).with_person(),
        );
        let (mut desk, mut queue, qh, globals) = session.client();
        desk.bind_pointer(&globals, &qh);
        desk.bind_viewporter(&globals, &qh);
        desk.open_viewed(&qh, "viewed", BLUE, (200, 150), (400, 300));
        until(&mut queue, &mut desk, |desk| desk.drawn == 1);
        let (_, area) = shown(&session, "viewed");
        assert_eq!((area.x0, area.y0), (0.0, 0.0), "at the pointer's origin");
        queue.roundtrip(&mut desk).expect("round trip");

        // Onto the window, from the origin, where the pointer starts: an
        // arrival, not yet a motion.
        session.command(Command::Motion { by: (100, 80) });
        until(&mut queue, &mut desk, |desk| desk.entered.is_some());

        // On, past the part of the window its buffer would cover unviewed.
        session.command(Command::Motion { by: (290, 210) });
        until(&mut queue, &mut desk, |desk| !desk.motions.is_empty());
        assert_eq!(desk.motions.last().copied(), Some((390.0, 290.0)));
        assert_eq!(
            desk.pointed.as_ref(),
            Some(desk.viewed[0].window.wl_surface())
        );

        session.stop((desk, queue));
    }
}
