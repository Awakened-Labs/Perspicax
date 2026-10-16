//! Where a window opens, with a person at the seat (#88).
//!
//! A new window is cascaded from the top-left of the work area under the
//! pointer, before anything knows how big it is. A big one, or one deep in
//! the cascade, would hang off the right or the bottom of its monitor; one
//! wider than its monitor could not be dragged back at all. So at its first
//! picture, the first time its size is known, it is put wholly inside the
//! work area, frame and all: slid on at its own size, and offered a size
//! that fits if it is still too big. Once. After that, where it goes is the
//! person's, and a window dragged partly off the screen stays there.
//!
//! With nobody at the seat it stays where the cascade put it, at the size
//! it chose, as the occlusion tests rely on.
//!
//! The windows here draw their own frame, so their frame is their window.
//! Big ones are drawn small and shown big through a viewport, which they
//! keep showing whatever size they are offered: what a test reads is the
//! place they were given and the size they were offered. Like the other
//! live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Configured, Desk, Said, Session, until};
use perspicax_compositor::{Backend, Virtual};
use perspicax_index::HostFacts;
use perspicax_node::Rect;
use perspicax_policy::{Access, Shape};
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::{EventQueue, QueueHandle};

/// ARGB, as the client writes it.
const BLUE: u32 = 0xff33_6699;
const PANEL: u32 = 0xff20_2020;

/// One 1280 by 1024 monitor, with a person at it.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).with_person())
}

/// `n` plain windows, each drawn before the next opens, so the cascade
/// has moved `n` steps in.
fn open_plain(desk: &mut Desk, queue: &mut EventQueue<Desk>, qh: &QueueHandle<Desk>, n: usize) {
    for nth in 0..n {
        desk.open_window(qh, &format!("plain {nth}"), "plain");
        until(queue, desk, |desk| desk.has_drawn(nth));
    }
}

/// Where the window titled `title` is, once it is on screen.
fn geometry(facts: &HostFacts, title: &str) -> Option<Rect> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.mapped && surface.title.as_deref() == Some(title))
        .map(|surface| surface.geometry)
        .filter(|geometry| geometry.x1 > geometry.x0)
}

/// The configures the `nth` window opened was sent.
fn configures(desk: &Desk, nth: usize) -> Vec<Configured> {
    desk.said_to(desk.windows[nth].wl_surface())
        .into_iter()
        .filter_map(|said| match said {
            Said::Configure(configured) => Some(configured),
            _ => None,
        })
        .collect()
}

/// The configures the last window opened was sent that offered it a size.
fn sized(desk: &Desk) -> Vec<Configured> {
    let window = desk.windows.last().expect("a window");
    desk.said_to(window.wl_surface())
        .into_iter()
        .filter_map(|said| match said {
            Said::Configure(configured) if configured.size.is_some() => Some(configured),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_new_window_wider_than_the_work_area_goes_to_its_left_edge_and_is_offered_its_width() {
    let session = seated("opening-wide");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_viewporter(&globals, &qh);
    desk.open_panel(&qh, "panel", 40, PANEL);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    open_plain(&mut desk, &mut queue, &qh, 2);

    // Third in the cascade under the panel: put at (64, 104), and 1500 wide
    // on a monitor 1280 wide.
    desk.open_viewed(&qh, "wide", BLUE, (750, 300), (1500, 600));
    until(&mut queue, &mut desk, |desk| {
        sized(desk).last().and_then(|configured| configured.size) == Some((1280, 600))
    });
    let facts = session.wait_for(|facts| geometry(facts, "wide").is_some_and(|at| at.x0 == 0.0));
    let at = geometry(&facts, "wide").expect("waited for");
    assert_eq!(
        (at.x0, at.y0),
        (0.0, 104.0),
        "at the left edge, where the cascade put its top"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_new_window_running_off_the_bottom_slides_up_at_its_own_size() {
    let session = seated("opening-tall");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_viewporter(&globals, &qh);
    open_plain(&mut desk, &mut queue, &qh, 1);

    // Second in the cascade, at (32, 32), and 1000 tall on a monitor 1024
    // tall: it fits, 8 px higher up.
    desk.open_viewed(&qh, "tall", BLUE, (200, 500), (400, 1000));
    until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
    let facts = session.wait_for(|facts| geometry(facts, "tall").is_some_and(|at| at.y0 < 32.0));
    assert_eq!(
        geometry(&facts, "tall"),
        Some(Rect::new(32.0, 24.0, 432.0, 1024.0))
    );
    queue.roundtrip(&mut desk).expect("a roundtrip");
    assert_eq!(sized(&desk), [], "it fits as it is, so it is not resized");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_new_window_wider_than_its_monitor_settles_on_the_monitor_it_opened_on() {
    let session = Session::start(
        "opening-two-monitors",
        Backend::Headless {
            outputs: vec![
                Virtual::numbered(1, (1280, 1024)),
                Virtual::numbered(2, (1280, 1024)),
            ],
            workspaces: Shape::default(),
            access: Access::open(),
            person: true,
            opacity: Default::default(),
            agent: Default::default(),
        },
    );
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_viewporter(&globals, &qh);
    open_plain(&mut desk, &mut queue, &qh, 2);

    // Put at (64, 64) on the first monitor, it reaches further onto the
    // second one than it is on the first.
    desk.open_viewed(&qh, "wide", BLUE, (1250, 150), (2500, 300));
    until(&mut queue, &mut desk, |desk| {
        sized(desk).last().and_then(|configured| configured.size) == Some((1280, 300))
    });
    let facts = session.wait_for(|facts| geometry(facts, "wide").is_some_and(|at| at.y0 == 64.0));
    let at = geometry(&facts, "wide").expect("waited for");
    assert_eq!(at.x0, 0.0, "on the monitor it opened on, not the next one");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn with_nobody_at_the_seat_a_new_window_wider_than_the_screen_stays_where_the_cascade_put_it() {
    let session = Session::start("opening-nobody", Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_viewporter(&globals, &qh);
    open_plain(&mut desk, &mut queue, &qh, 1);

    desk.open_viewed(&qh, "wide", BLUE, (750, 300), (1500, 600));
    let n = desk.windows.len() - 1;
    until(&mut queue, &mut desk, |desk| desk.has_drawn(n));
    let facts = session.wait_for(|facts| geometry(facts, "wide").is_some());
    assert_eq!(
        geometry(&facts, "wide"),
        Some(Rect::new(32.0, 32.0, 1532.0, 632.0))
    );
    queue.roundtrip(&mut desk).expect("a roundtrip");
    assert_eq!(sized(&desk), [], "nobody asked for it to be resized");
    assert!(
        configures(&desk, 1)
            .iter()
            .all(|configured| configured.bounds.is_none()),
        "told no room, so it sizes itself as it always has"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_new_window_is_told_the_room_the_panels_leave_less_any_frame_drawn_for_it() {
    let session = seated("opening-bounds");
    let (mut desk, mut queue, qh, _globals) = session.client();
    desk.open_panel(&qh, "panel", 40, PANEL);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);

    // It draws its own frame: all the room under the panel is its.
    desk.open_window(&qh, "own frame", "own");
    until(&mut queue, &mut desk, |desk| desk.has_drawn(0));
    let told = configures(&desk, 0);
    assert_eq!(
        told.first().and_then(|configured| configured.bounds),
        Some((1280, 984)),
        "told before it first draws"
    );

    // Ours is drawn around it: a 2 px border, and a 24 px titlebar above.
    desk.open_framed(&qh, "framed");
    until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
    let told = configures(&desk, 1);
    assert_eq!(
        told.last().and_then(|configured| configured.bounds),
        Some((1276, 956))
    );

    session.stop((desk, queue));
}

/// The person's hand, which needs the `capture` feature headless.
#[cfg(feature = "capture")]
mod hand {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Mods};

    use super::*;

    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn a_window_dragged_partly_off_the_screen_stays_there_when_it_draws_again() {
        let session = seated("opening-dragged-off");
        let (mut desk, mut queue, qh, globals) = session.client();
        desk.bind_pointer(&globals, &qh);
        open_plain(&mut desk, &mut queue, &qh, 1);
        let from = session
            .wait_for(|facts| geometry(facts, "plain 0").is_some())
            .surfaces()
            .iter()
            .find(|surface| surface.title.as_deref() == Some("plain 0"))
            .map(|surface| surface.geometry)
            .expect("waited for");

        // The person takes it by its client, which asks to be moved, and
        // drags it half off the left of the screen.
        let centre = (
            ((from.x0 + from.x1) / 2.0) as i32,
            ((from.y0 + from.y1) / 2.0) as i32,
        );
        session.command(Command::Press {
            at: centre,
            button: Button::Left,
            mods: Mods::default(),
        });
        until(&mut queue, &mut desk, |desk| desk.pressed.is_some());
        let seat = desk.seat.clone().expect("our pointer's seat");
        desk.windows[0].move_(&seat, desk.pressed.expect("waited for"));
        until(&mut queue, &mut desk, |desk| desk.pointed.is_none());
        session.command(Command::Motion { by: (-200, 0) });
        session.command(Command::Release {
            button: Button::Left,
        });
        let off = from.x0 - 200.0;
        session.wait_for(|facts| geometry(facts, "plain 0").is_some_and(|at| at.x0 == off));

        // Another window opens and takes the keyboard: the first is told it
        // is no longer active, and draws again.
        let drawn = desk.drawn;
        desk.open_window(&qh, "next", "next");
        until(&mut queue, &mut desk, |desk| desk.drawn >= drawn + 2);

        let facts = session.wait_for(|facts| geometry(facts, "next").is_some());
        assert_eq!(
            geometry(&facts, "plain 0").map(|at| at.x0),
            Some(off),
            "where the person put it"
        );

        session.stop((desk, queue));
    }
}
