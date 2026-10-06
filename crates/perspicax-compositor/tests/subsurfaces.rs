//! What a window draws into its subsurfaces is its window's damage.
//!
//! Firefox draws every page into a subsurface of its toplevel and leaves the
//! toplevel itself alone, so a compositor that counted only a toplevel's own
//! commits saw one frame where Firefox drew dozens. Receipts then read
//! `quiet`, and a canvas page read as fully explained (issue #33). These
//! tests draw into subsurfaces, synchronized and not, nested, and under a
//! panel, and check that the damage is counted for the window, where in it
//! the pixels changed, once per frame.
//!
//! The rest is what a frame is. A commit that brings no new buffer and names
//! no damage, which a client sends to ask for a frame callback (Firefox sent
//! 173 of them beside 79 it drew in), changed nothing on screen. And a window
//! that has drawn nothing is not on screen at all.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, WINDOW, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, Layer as Level, SurfaceFacts, SurfaceKind};
use perspicax_node::{Rect, SurfaceId};
use smithay_client_toolkit::shell::{WaylandSurface, wlr_layer::Layer};
use wayland_client::{EventQueue, QueueHandle};

const PAGE: u32 = 0xff33_6699;
const PAGE_AGAIN: u32 = 0xff99_6633;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

/// The rectangles of the newest frame of damage, in the surface's own
/// coordinates.
fn newest(surface: &SurfaceFacts) -> Vec<Rect> {
    surface
        .damage
        .iter()
        .filter(|(generation, _)| *generation == surface.damage_generation)
        .map(|(_, region)| *region)
        .collect()
}

/// Open a window called `title`, wait until it has drawn and the facts say
/// so, and return its id with the damage it had taken by then.
fn settle(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
) -> (SurfaceId, u64) {
    desk.open_window(qh, title, title);
    until(queue, desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| {
        titled(facts, title).is_some_and(|window| window.mapped && window.damage_generation >= 1)
    });
    let window = titled(&facts, title).expect("waited for");
    (window.id, window.damage_generation)
}

/// Once the compositor has taken in everything sent so far, wait until
/// `id`'s newest frame of damage is exactly `regions`.
///
/// The round trip comes first because a test often sends two commits
/// together, and facts read between them would describe a frame the test is
/// not asking about. The compositor publishes as it handles each commit, so
/// once the round trip is answered the facts are final.
fn landed(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    id: SurfaceId,
    regions: &[Rect],
) -> SurfaceFacts {
    queue.roundtrip(desk).expect("round trip");
    let facts = session.wait_for(|facts| facts.surface(id).is_some_and(|s| newest(s) == regions));
    facts.surface(id).expect("waited for").clone()
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn damage_drawn_into_a_subsurface_counts_for_its_window_where_it_lands() {
    let session = session("subsurface-lands");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, before) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let root = desk.windows[0].wl_surface().clone();

    let (_placed, child) = desk.open_subsurface(&qh, &root, (40, 30), false);
    // A subsurface's position is its parent's state, made current by the
    // parent's commit -- which brings nothing new to the window itself.
    root.commit();
    desk.paint(&child, (120, 80), PAGE);
    child.commit();

    let window = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(40.0, 30.0, 160.0, 110.0)],
    );
    assert_eq!(
        window.damage_generation,
        before + 1,
        "one frame, the subsurface's: placing it drew nothing"
    );
    // The two questions a receipt asks, for a node over the subsurface and
    // for one away from it: `on_target`, and not `quiet`.
    assert!(window.damage_touches(before, Rect::new(60.0, 50.0, 80.0, 70.0)));
    assert!(!window.damage_touches(before, Rect::new(300.0, 200.0, 320.0, 220.0)));
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_synchronized_subsurface_is_counted_once_with_the_commit_that_shows_it() {
    let session = session("subsurface-sync");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, before) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let root = desk.windows[0].wl_surface().clone();

    let (_placed, child) = desk.open_subsurface(&qh, &root, (40, 30), true);
    desk.paint(&child, (120, 80), PAGE);
    child.commit();
    queue.roundtrip(&mut desk).expect("round trip");
    let facts = session.wait_for(|_| true);
    assert_eq!(
        facts.surface(page).expect("the window").damage_generation,
        before,
        "a synchronized subsurface's commit is held until its parent's"
    );

    root.commit();
    let window = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(40.0, 30.0, 160.0, 110.0)],
    );
    assert_eq!(
        window.damage_generation,
        before + 1,
        "the parent's commit that showed it is one frame, and drew nothing of its own"
    );
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_commit_that_draws_nothing_new_is_not_a_frame() {
    let session = session("subsurface-nothing-new");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, before) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let root = desk.windows[0].wl_surface().clone();
    let whole = Rect::new(0.0, 0.0, f64::from(WINDOW.0), f64::from(WINDOW.1));

    // Asking for a frame callback, and nothing else.
    root.frame(&qh, root.clone());
    root.commit();
    // A real frame after it, so one counted for the commit before would sit
    // between the two.
    desk.paint(&root, WINDOW, PAGE);
    root.commit();
    let window = landed(&session, &mut desk, &mut queue, page, &[whole]);
    assert_eq!(
        window.damage_generation,
        before + 1,
        "the window's commit that only asked for a frame callback is not a frame"
    );

    let (_placed, child) = desk.open_subsurface(&qh, &root, (40, 30), false);
    root.commit();
    desk.paint(&child, (120, 80), PAGE);
    child.commit();
    let child_area = [Rect::new(40.0, 30.0, 160.0, 110.0)];
    let drawn = landed(&session, &mut desk, &mut queue, page, &child_area).damage_generation;

    child.frame(&qh, child.clone());
    child.commit();
    desk.paint(&child, (120, 80), PAGE_AGAIN);
    child.commit();
    let window = landed(&session, &mut desk, &mut queue, page, &child_area);
    assert_eq!(
        window.damage_generation,
        drawn + 1,
        "the subsurface's commit that only asked for a frame callback is not a frame"
    );
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn nested_subsurfaces_land_at_the_sum_of_their_offsets() {
    let session = session("subsurface-nested");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let root = desk.windows[0].wl_surface().clone();

    let (_outer_placed, outer) = desk.open_subsurface(&qh, &root, (40, 30), false);
    root.commit();
    desk.paint(&outer, (200, 150), PAGE);
    outer.commit();
    let before = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(40.0, 30.0, 240.0, 180.0)],
    )
    .damage_generation;

    // Synchronized under one that is not: its parent's commit shows it.
    let (_inner_placed, inner) = desk.open_subsurface(&qh, &outer, (10, 20), true);
    desk.paint(&inner, (30, 20), PAGE_AGAIN);
    inner.commit();
    outer.commit();
    let window = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(50.0, 50.0, 80.0, 70.0)],
    );
    assert_eq!(window.damage_generation, before + 1);
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn damage_drawn_into_a_subsurface_of_a_panel_counts_for_the_panel() {
    let session = session("subsurface-panel");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_strip(&qh, Layer::Top, "strip", 40, PAGE);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let strip = |facts: &HostFacts| {
        facts
            .surfaces()
            .iter()
            .find(|surface| {
                surface.mapped
                    && matches!(
                        &surface.kind,
                        SurfaceKind::Layer { layer: Level::Top, namespace } if namespace == "strip"
                    )
            })
            .cloned()
    };
    let facts = session.wait_for(|facts| strip(facts).is_some_and(|s| s.damage_generation >= 1));
    let panel = strip(&facts).expect("waited for");
    let surface = desk.layers[0].0.wl_surface().clone();

    let (_placed, child) = desk.open_subsurface(&qh, &surface, (20, 5), false);
    surface.commit();
    desk.paint(&child, (30, 20), PAGE_AGAIN);
    child.commit();
    let shown = landed(
        &session,
        &mut desk,
        &mut queue,
        panel.id,
        &[Rect::new(20.0, 5.0, 50.0, 25.0)],
    );
    assert_eq!(shown.damage_generation, panel.damage_generation + 1);
    assert_eq!(
        desk.layers_drawn, 1,
        "no configure redrew the panel meanwhile"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_subsurface_taking_its_picture_away_leaves_its_window_mapped() {
    let session = session("subsurface-removed");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let root = desk.windows[0].wl_surface().clone();

    let (_placed, child) = desk.open_subsurface(&qh, &root, (40, 30), false);
    root.commit();
    desk.paint(&child, (120, 80), PAGE);
    child.commit();
    let before = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(40.0, 30.0, 160.0, 110.0)],
    )
    .damage_generation;

    child.attach(None, 0, 0);
    child.commit();
    let whole = Rect::new(0.0, 0.0, f64::from(WINDOW.0), f64::from(WINDOW.1));
    let window = landed(&session, &mut desk, &mut queue, page, &[whole]);
    assert!(window.mapped, "the window still has its own picture");
    assert_eq!(
        window.damage_generation,
        before + 1,
        "what was under the subsurface shows now, and where that was is no longer known, so \
         all of the window changed"
    );
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_has_drawn_nothing_is_not_mapped() {
    let session = session("subsurface-blank");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_blank(&qh, "blank");
    until(&mut queue, &mut desk, |desk| desk.blank_configured >= 1);
    let facts = session.wait_for(|facts| titled(facts, "blank").is_some());
    assert!(
        !titled(&facts, "blank").expect("waited for").mapped,
        "committed, with a geometry, and nothing to look at"
    );

    let surface = desk.blank[0].wl_surface().clone();
    desk.paint(&surface, WINDOW, PAGE);
    surface.commit();
    queue.roundtrip(&mut desk).expect("round trip");
    session.wait_for(|facts| titled(facts, "blank").is_some_and(|window| window.mapped));

    session.stop((desk, queue));
}
