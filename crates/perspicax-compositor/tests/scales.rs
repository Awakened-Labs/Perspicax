//! Which scale a surface is told to draw at: `wp_fractional_scale_v1`, as
//! GTK 4, Qt 6, Chromium and Firefox speak it.
//!
//! A window is told the scale of the monitor it is on as soon as it asks,
//! and again whenever that changes: when a display tool gives the monitor
//! another scale, or when the window moves onto another monitor. A surface
//! asking before it is a window, as GTK's does, is told the scale of the
//! monitor a new window opens on. A panel is told its own monitor's.
//!
//! And the issue it was offered for (#135). Firefox on a monitor at 0.75 is
//! told 0.75, draws its buffer at that, and says through a viewport that the
//! buffer is shown at its window's size. Its pixels then fill its window on
//! the monitor, one for one, where they used to fill 0.75 of it.
//!
//! A scale is told in 120ths, and a display tool sets one in 256ths: 0.9
//! arrives as 230/256, which is 107.8 120ths, told as 108.
//!
//! The picture needs the `capture` feature. Like the other live tests it
//! binds a real Wayland socket, so it needs `XDG_RUNTIME_DIR`, and is
//! `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Session, advertised, rescale, until};
use perspicax_compositor::{Backend, Virtual};
use perspicax_policy::{Access, Action, Direction, Place, Shape, Side, Towards};
use smithay_client_toolkit::shell::WaylandSurface;

/// Two monitors side by side, at a scale of 1 until a test says otherwise.
fn two_monitors() -> Backend {
    Backend::Headless {
        outputs: vec![
            Virtual::numbered(1, (1280, 800)),
            Virtual {
                place: Place::Beside {
                    side: Side::RightOf,
                    of: "HEADLESS-1".to_owned(),
                    offset: 0,
                },
                ..Virtual::numbered(2, (1280, 800))
            },
        ],
        workspaces: Shape::default(),
        access: Access::open(),
        person: false,
        opacity: Default::default(),
    }
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_client_finds_the_fractional_scale_manager() {
    let session = Session::start("scales-offered", Backend::headless((800, 600)));
    let (desk, queue, _qh, globals) = session.client();
    assert!(advertised(&globals, "wp_fractional_scale_manager_v1"));
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_is_told_its_monitors_scale_and_told_again_when_it_changes() {
    let session = Session::start("scales-rescaled", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_fractional_scale(&globals, &qh);
    desk.open_window(&qh, "page", "page");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let page = desk.windows[0].wl_surface().clone();
    let _scale = desk.fractional(&qh, &page);
    until(&mut queue, &mut desk, |desk| {
        !desk.scales_told(&page).is_empty()
    });
    assert_eq!(desk.scales_told(&page), [120]);

    rescale(&mut desk, &mut queue, &qh, &globals, "HEADLESS-1", 0.9);
    until(&mut queue, &mut desk, |desk| {
        desk.scales_told(&page).len() == 2
    });
    rescale(&mut desk, &mut queue, &qh, &globals, "HEADLESS-1", 1.5);
    until(&mut queue, &mut desk, |desk| {
        desk.scales_told(&page).len() == 3
    });
    assert_eq!(desk.scales_told(&page), [120, 108, 180]);

    session.stop((desk, queue));
}

/// GTK asks a surface's scale as it makes the surface, before the surface
/// is a window or anywhere: it is told at once, and the scale is the one
/// the window will open at.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_surface_asking_before_it_is_a_window_is_told_the_scale_it_will_open_at() {
    let session = Session::start("scales-early", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    rescale(&mut desk, &mut queue, &qh, &globals, "HEADLESS-1", 1.5);
    desk.bind_fractional_scale(&globals, &qh);

    let surface = desk.surface(&qh);
    let _scale = desk.fractional(&qh, &surface);
    queue.roundtrip(&mut desk).expect("round trip");
    assert_eq!(desk.scales_told(&surface), [180]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_moved_to_another_monitor_is_told_that_monitors_scale() {
    let session = Session::start("scales-moved", two_monitors());
    let (mut desk, mut queue, qh, globals) = session.client();
    rescale(&mut desk, &mut queue, &qh, &globals, "HEADLESS-2", 1.5);
    desk.bind_fractional_scale(&globals, &qh);
    desk.open_window(&qh, "page", "page");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let page = desk.windows[0].wl_surface().clone();
    let _scale = desk.fractional(&qh, &page);
    until(&mut queue, &mut desk, |desk| {
        !desk.scales_told(&page).is_empty()
    });
    assert_eq!(desk.scales_told(&page), [120], "on the left-hand monitor");

    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    until(&mut queue, &mut desk, |desk| {
        desk.scales_told(&page).len() == 2
    });
    assert_eq!(desk.scales_told(&page), [120, 180]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_is_told_its_monitors_scale() {
    let session = Session::start("scales-panel", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_fractional_scale(&globals, &qh);
    desk.open_panel(&qh, "panel", 30, 0xff20_2020);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let panel = desk.layers[0].0.wl_surface().clone();
    let _scale = desk.fractional(&qh, &panel);
    until(&mut queue, &mut desk, |desk| {
        !desk.scales_told(&panel).is_empty()
    });

    rescale(&mut desk, &mut queue, &qh, &globals, "HEADLESS-1", 1.25);
    until(&mut queue, &mut desk, |desk| {
        desk.scales_told(&panel).len() == 2
    });
    assert_eq!(desk.scales_told(&panel), [120, 150]);

    session.stop((desk, queue));
}

#[cfg(feature = "capture")]
mod firefox {
    use super::*;
    use crate::common::drawn_in;
    use perspicax_compositor::Host;
    use perspicax_index::ShotTarget;

    /// ARGB, as the client writes it.
    const BLUE: u32 = 0xff33_6699;

    /// The reproduction from #135, at 0.75 rather than 0.9 so each of the
    /// buffer's pixels is one of the picture's.
    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn a_window_drawing_as_firefox_does_fills_its_window_on_a_monitor_at_three_quarters() {
        let session = Session::start("scales-firefox", Backend::headless((800, 600)));
        let host = Host::new(&session.facts, &session.requests);
        let (mut desk, mut queue, qh, globals) = session.client();
        rescale(&mut desk, &mut queue, &qh, &globals, "HEADLESS-1", 0.75);
        desk.bind_viewporter(&globals, &qh);
        desk.bind_fractional_scale(&globals, &qh);

        // Told 90 120ths, it draws its 400 by 300 window in 300 by 225.
        desk.open_viewed(&qh, "firefox", BLUE, (300, 225), (400, 300));
        let firefox = desk.windows[0].wl_surface().clone();
        let _scale = desk.fractional(&qh, &firefox);
        until(&mut queue, &mut desk, |desk| {
            desk.drawn == 1 && !desk.scales_told(&firefox).is_empty()
        });
        assert_eq!(desk.scales_told(&firefox), [90]);

        let facts = session.wait_for(|facts| {
            facts
                .surfaces()
                .iter()
                .any(|surface| surface.mapped && surface.title.as_deref() == Some("firefox"))
        });
        let window = facts
            .surfaces()
            .iter()
            .find(|surface| surface.title.as_deref() == Some("firefox"))
            .expect("the window");
        let area = window.geometry;
        assert_eq!((area.width(), area.height()), (400.0, 300.0));

        let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
        assert_eq!(shot.scale, 0.75);
        assert_eq!(drawn_in(&shot, BLUE), [area], "its window, all of it");
        let drawn = shot
            .drawn
            .iter()
            .find(|drawn| drawn.surface == window.id)
            .expect("the window is accounted for");
        assert_eq!(drawn.rect, area);

        session.stop((desk, queue));
    }
}
