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
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, refused, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, SurfaceFacts};
use perspicax_node::Rect;
use smithay_client_toolkit::shell::{WaylandSurface, xdg::XdgSurface};
use wayland_client::{EventQueue, QueueHandle};

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
