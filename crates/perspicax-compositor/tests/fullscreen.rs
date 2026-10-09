//! A fullscreen window and a panel, by a compositor with no screen.
//!
//! A panel on the `top` layer sits over the windows, but not over the
//! fullscreen window the person is using: that covers it, as on Plasma and
//! Windows. Left for another window, or taken out of fullscreen, it goes
//! back under the panel. A surface on `overlay` stays over everything. The
//! facts say so, and so does a picture.
//!
//! Fullscreen is asked for through the taskbar protocol, which a headless
//! compositor honours where it ignores a client's own request. Like the
//! other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, SurfaceKind, judge};
use perspicax_node::{Rect, SurfaceId, Visibility};
use smithay_client_toolkit::shell::wlr_layer::Layer;
use wayland_client::{EventQueue, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::State;

/// ARGB, as the client writes it.
const VIDEO: u32 = 0xff33_6699;
const PANEL: u32 = 0xffee_8822;
const MENU: u32 = 0xff22_aa44;

/// A corner of the window that a strip across the top of the screen covers,
/// relative to the window.
const CORNER: Rect = Rect {
    x0: 5.0,
    y0: 5.0,
    x1: 25.0,
    y1: 25.0,
};

fn window(facts: &HostFacts, title: &str) -> SurfaceId {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| surface.id)
        .expect("a window of that title")
}

fn layer(facts: &HostFacts, name: &str) -> SurfaceId {
    facts
        .surfaces()
        .iter()
        .find(|surface| {
            matches!(&surface.kind, SurfaceKind::Layer { namespace, .. } if namespace == name)
        })
        .map(|surface| surface.id)
        .expect("a layer surface of that namespace")
}

/// What covers the window's corner, if anything.
fn corner(facts: &HostFacts, title: &str) -> Visibility {
    judge(facts, window(facts, title), CORNER).visibility
}

/// A window, "video", with the keyboard, under a panel across the top.
fn video_under_a_panel(name: &str) -> (Session, Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let session = Session::start(name, Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_coloured(&qh, "video", "video", VIDEO);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_strip(&qh, Layer::Top, "panel", 40, PANEL);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    until(&mut queue, &mut desk, |desk| desk.task("video").is_some());
    desk.activate("video");
    until(&mut queue, &mut desk, |desk| {
        desk.task("video")
            .is_some_and(|task| task.is(State::Activated))
    });
    let facts = session.wait_for(|facts| facts.surfaces().iter().filter(|s| s.mapped).count() == 2);
    assert_eq!(
        corner(&facts, "video"),
        Visibility::Occluded {
            by: layer(&facts, "panel")
        },
        "a window is under the panel"
    );
    (session, desk, queue, qh)
}

fn fullscreen(desk: &Desk, title: &str) {
    desk.task(title)
        .expect("a known window")
        .handle
        .set_fullscreen(None);
}

/// The top left of a window, as the facts place it.
fn top_left(facts: &HostFacts, title: &str) -> (f64, f64) {
    let surface = facts
        .surface(window(facts, title))
        .expect("a window of that title");
    (surface.geometry.x0, surface.geometry.y0)
}

/// Whether a window is on screen, as the facts say.
fn mapped(facts: &HostFacts, title: &str) -> bool {
    facts
        .surface(window(facts, title))
        .is_some_and(|surface| surface.mapped)
}

/// Issue #90: a panel that reserves room moves the windows out from under
/// it, but not a fullscreen one, which covers the panel and keeps the corner
/// of its monitor. It used to be fitted below the panel with the rest.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_fullscreen_window_keeps_the_corner_of_its_monitor_when_a_panel_reserves_room() {
    let session = Session::start("fullscreen-reserved", Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_coloured(&qh, "plain", "plain", 0xffff_ffff);
    desk.open_coloured(&qh, "video", "video", VIDEO);
    until(&mut queue, &mut desk, |desk| {
        desk.task("plain").is_some() && desk.task("video").is_some()
    });
    fullscreen(&desk, "video");
    until(&mut queue, &mut desk, |desk| {
        desk.offered == Some((1280, 1024))
    });
    session.wait_for(|facts| top_left(facts, "video") == (0.0, 0.0));

    desk.open_panel(&qh, "panel", 40, PANEL);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let facts = session.wait_for(|facts| top_left(facts, "plain").1 >= 40.0);
    assert_eq!(
        top_left(&facts, "video"),
        (0.0, 0.0),
        "the fullscreen window was fitted below the panel"
    );

    session.stop((desk, queue));
}

/// Issue #90: a fullscreen window minimized and brought back comes back
/// covering its monitor, where it was, and not fitted below the panel as a
/// window that does not fill its monitor would be.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_fullscreen_window_comes_back_to_the_corner_of_its_monitor_after_it_was_minimized() {
    let session = Session::start("fullscreen-restored", Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_panel(&qh, "panel", 40, PANEL);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    desk.open_coloured(&qh, "video", "video", VIDEO);
    until(&mut queue, &mut desk, |desk| desk.task("video").is_some());
    fullscreen(&desk, "video");
    queue.flush().expect("sent");
    session.wait_for(|facts| top_left(facts, "video") == (0.0, 0.0));

    desk.task("video")
        .expect("a known window")
        .handle
        .set_minimized();
    queue.flush().expect("sent");
    session.wait_for(|facts| !mapped(facts, "video"));
    desk.activate("video");
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| mapped(facts, "video"));
    assert_eq!(
        top_left(&facts, "video"),
        (0.0, 0.0),
        "the fullscreen window came back fitted below the panel"
    );

    session.stop((desk, queue));
}

/// Issue #90: a window made fullscreen while it is minimized stays
/// minimized, and is fullscreen once it is brought back. It used to be drawn
/// over the screen while still counted as minimized, and then bringing it
/// back never showed it, since it was already there.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_minimized_window_made_fullscreen_stays_minimized_until_it_is_restored() {
    let session = Session::start("fullscreen-minimized", Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_coloured(&qh, "plain", "plain", 0xffff_ffff);
    desk.open_coloured(&qh, "video", "video", VIDEO);
    until(&mut queue, &mut desk, |desk| {
        desk.task("plain").is_some() && desk.task("video").is_some()
    });
    desk.task("video")
        .expect("a known window")
        .handle
        .set_minimized();
    queue.flush().expect("sent");
    session.wait_for(|facts| !mapped(facts, "video"));

    fullscreen(&desk, "video");
    queue.roundtrip(&mut desk).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(
        !mapped(&session.facts.read(), "video"),
        "made fullscreen, a minimized window came back on screen"
    );

    desk.activate("video");
    until(&mut queue, &mut desk, |desk| {
        desk.offered == Some((1280, 1024))
    });
    session.wait_for(|facts| mapped(facts, "video") && top_left(facts, "video") == (0.0, 0.0));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_fullscreen_window_in_use_covers_a_panel_on_the_top_layer() {
    let (session, desk, queue, _qh) = video_under_a_panel("fullscreen-covers");

    fullscreen(&desk, "video");
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| corner(facts, "video") == Visibility::Visible);
    let order: Vec<SurfaceId> = facts.surfaces().iter().map(|surface| surface.id).collect();
    let at = |id| order.iter().position(|known| *known == id);
    assert!(
        at(window(&facts, "video")) > at(layer(&facts, "panel")),
        "the window is above the panel in the stack: {order:?}"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_fullscreen_window_left_for_another_goes_back_under_the_panel() {
    let (session, mut desk, mut queue, qh) = video_under_a_panel("fullscreen-left");
    fullscreen(&desk, "video");
    queue.flush().expect("sent");
    session.wait_for(|facts| corner(facts, "video") == Visibility::Visible);

    desk.open_coloured(&qh, "other", "other", 0xffff_ffff);
    // Mapped, which the taskbar says: counting draws would count the
    // fullscreen window's own redraw at its new size too.
    until(&mut queue, &mut desk, |desk| desk.task("other").is_some());
    desk.activate("other");
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| corner(facts, "video") != Visibility::Visible);
    assert_eq!(
        corner(&facts, "video"),
        Visibility::Occluded {
            by: layer(&facts, "panel")
        }
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn leaving_fullscreen_puts_the_panel_back_on_top() {
    let (session, desk, queue, _qh) = video_under_a_panel("fullscreen-leave");
    fullscreen(&desk, "video");
    queue.flush().expect("sent");
    session.wait_for(|facts| corner(facts, "video") == Visibility::Visible);

    desk.task("video")
        .expect("a known window")
        .handle
        .unset_fullscreen();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| corner(facts, "video") != Visibility::Visible);
    assert_eq!(
        corner(&facts, "video"),
        Visibility::Occluded {
            by: layer(&facts, "panel")
        }
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_overlay_surface_stays_over_a_fullscreen_window() {
    let (session, mut desk, mut queue, qh) = video_under_a_panel("fullscreen-overlay");
    desk.open_strip(&qh, Layer::Overlay, "menu", 60, MENU);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 2);

    fullscreen(&desk, "video");
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .filter(|surface| surface.mapped)
            .count()
            == 3
            && matches!(
                corner(facts, "video"),
                Visibility::Occluded { by } if by == layer(facts, "menu")
            )
    });
    assert_eq!(
        corner(&facts, "video"),
        Visibility::Occluded {
            by: layer(&facts, "menu")
        }
    );

    session.stop((desk, queue));
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_picture_shows_the_fullscreen_window_where_the_panel_is() {
    use perspicax_compositor::Host;
    use perspicax_index::ShotTarget;

    fn rgba(argb: u32) -> [u8; 4] {
        let [b, g, r, a] = argb.to_le_bytes();
        [r, g, b, a]
    }

    let (session, desk, queue, _qh) = video_under_a_panel("fullscreen-picture");
    let host = Host::new(&session.facts, &session.requests);
    let at = |shot: &perspicax_index::Shot, x: usize, y: usize| -> [u8; 4] {
        let at = (y * shot.width as usize + x) * 4;
        shot.rgba[at..at + 4].try_into().unwrap()
    };
    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
    assert_eq!(at(&shot, 10, 10), rgba(PANEL), "the panel, over the window");

    fullscreen(&desk, "video");
    queue.flush().expect("sent");
    session.wait_for(|facts| corner(facts, "video") == Visibility::Visible);
    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
    assert_eq!(at(&shot, 10, 10), rgba(VIDEO), "the window, over the panel");

    session.stop((desk, queue));
}
