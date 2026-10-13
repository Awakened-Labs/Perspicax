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
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.
//! They mean the same in a plain headless build, where smithay measures
//! nothing, and in one with `capture`, where it does and the compositor
//! checks at every commit that the two agree.

mod common;

use common::{Desk, Geometry, Session, WINDOW, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, SurfaceFacts, SurfaceKind};
use perspicax_node::{Rect, Vec2};
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
