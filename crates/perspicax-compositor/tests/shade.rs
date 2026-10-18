//! A window rolled up to its titlebar, by a compositor with no screen.
//!
//! Headless nothing is drawn, but the facts say what would be: a window
//! rolled up keeps its place and its size, its frame is the titlebar and
//! the border round it, its own nodes are refused, and what was under its
//! client is no longer covered. The bindings that roll it up are carried out
//! with [`Session::perform`]; the titlebar's button and the wheel over it
//! need a seat.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Said, Session, configured, heard, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, SurfaceFacts, SurfaceKind, judge};
use perspicax_node::{Rect, Visibility};
use perspicax_policy::Action;
use smithay_client_toolkit::shell::{
    WaylandSurface as _,
    xdg::{XdgSurface as _, window::DecorationMode},
};
use wayland_client::{EventQueue, Proxy as _, QueueHandle};

/// A node of a window, in its window's own space.
const NODE: Rect = Rect {
    x0: 10.0,
    y0: 10.0,
    x1: 50.0,
    y1: 50.0,
};

fn surface<'a>(facts: &'a HostFacts, title: &str) -> &'a SurfaceFacts {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .expect("a window of that title")
}

fn has(facts: &HostFacts, title: &str) -> bool {
    facts
        .surfaces()
        .iter()
        .any(|surface| surface.title.as_deref() == Some(title) && surface.mapped)
}

/// How many strips of frame the window of this title is published with:
/// four for a frame of ours round a window that is not rolled up.
fn strips(facts: &HostFacts, title: &str) -> usize {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map_or(0, |surface| surface.frame.len())
}

fn configures(desk: &Desk, nth: usize) -> usize {
    desk.said_to(desk.windows[nth].wl_surface())
        .iter()
        .filter(|said| matches!(said, Said::Configure(_)))
        .count()
}

/// A window, white, that asked for a frame of ours, and has it.
fn framed(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
) -> HostFacts {
    let nth = desk.windows.len();
    desk.open_framed(qh, title);
    until(queue, desk, |desk| desk.has_drawn(nth));
    let facts = session.wait_for(|facts| strips(facts, title) == 4);
    queue.roundtrip(desk).expect("the compositor answers");
    facts
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_rolled_up_keeps_its_place_and_size_and_shows_only_its_titlebar() {
    let session = Session::start("shade-place", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, _) = session.client();
    let facts = framed(&session, &mut desk, &mut queue, &qh, "framed");
    let before = surface(&facts, "framed").clone();
    let told = configures(&desk, 0);

    session.perform(Action::Shade);
    let facts = session.wait_for(|facts| surface(facts, "framed").shaded);
    let rolled = surface(&facts, "framed");
    assert_eq!(
        rolled.geometry, before.geometry,
        "in its place, at its size"
    );
    assert!(rolled.mapped, "still on screen: its titlebar is");
    let bar = before.frame[0];
    assert_eq!(
        rolled.frame,
        vec![
            bar,
            Rect::new(bar.x0, before.geometry.y0, bar.x1, before.geometry.y0 + 2.0),
        ],
        "the titlebar, and the border under it"
    );
    assert_eq!(
        judge(&facts, rolled.id, NODE).visibility,
        Visibility::RolledUp
    );
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert_eq!(configures(&desk, 0), told, "its client is not told");

    // Rolled up already, it stays so: a second notch of the wheel is
    // nothing more. The opacity is only a mark that both were carried out.
    session.perform(Action::Shade);
    session.perform(Action::OpacityDown);
    let facts = session.wait_for(|facts| surface(facts, "framed").drawn_opacity.is_some());
    assert!(surface(&facts, "framed").shaded);

    session.perform(Action::Unshade);
    let facts = session.wait_for(|facts| !surface(facts, "framed").shaded);
    let unrolled = surface(&facts, "framed");
    assert_eq!(unrolled.geometry, before.geometry, "as it was");
    assert_eq!(unrolled.frame, before.frame);
    assert_eq!(
        judge(&facts, unrolled.id, NODE).visibility,
        Visibility::Visible
    );

    session.stop((desk, queue));
}

/// A window rolled up over another covers it by its titlebar alone: under
/// the bar, on the host's word, and nowhere under its client, which is not
/// drawn.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn what_was_under_a_rolled_up_window_shows_but_for_its_titlebar() {
    let session = Session::start("shade-under", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "lower", "lower");
    until(&mut queue, &mut desk, |desk| desk.has_drawn(0));
    session.wait_for(|facts| has(facts, "lower"));
    let facts = framed(&session, &mut desk, &mut queue, &qh, "upper");
    let (lower, upper) = (surface(&facts, "lower"), surface(&facts, "upper"));
    let (lower_id, upper_id) = (lower.id, upper.id);
    let origin = lower.geometry.origin().to_vec2();

    // A node of the lower window under the upper one's client, and one
    // under its titlebar, where the two overlap.
    let client = Rect::new(
        upper.geometry.x0 + 10.0,
        upper.geometry.y0 + 10.0,
        upper.geometry.x0 + 30.0,
        upper.geometry.y0 + 30.0,
    ) - origin;
    let bar = upper.frame[0];
    let under_bar = Rect::new(
        bar.x0 + 10.0,
        bar.y0.max(lower.geometry.y0) + 1.0,
        bar.x0 + 30.0,
        bar.y1 - 1.0,
    ) - origin;
    assert!(under_bar.y1 > under_bar.y0, "cascaded: {lower:?} {upper:?}");
    assert_eq!(
        judge(&facts, lower_id, client).visibility,
        Visibility::Occluded { by: upper_id },
        "covered while the upper one is not rolled up"
    );

    session.perform(Action::Shade);
    let facts = session.wait_for(|facts| surface(facts, "upper").shaded);
    assert_eq!(
        judge(&facts, lower_id, client).visibility,
        Visibility::Visible,
        "where its client was"
    );
    let covered = judge(&facts, lower_id, under_bar);
    assert_eq!(covered.visibility, Visibility::Occluded { by: upper_id });
    assert!(!covered.unproven, "the titlebar is ours, and proves it");

    session.stop((desk, queue));
}

/// A window that draws its own frame has no titlebar of ours to roll up
/// to, and is left as it is. Nor does one that takes its frame back stay
/// rolled up.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_draws_its_own_frame_does_not_roll_up() {
    let session = Session::start("shade-own-frame", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "own", "own");
    until(&mut queue, &mut desk, |desk| desk.has_drawn(0));
    session.wait_for(|facts| has(facts, "own"));

    session.perform(Action::ToggleShade);
    session.perform(Action::OpacityDown);
    let facts = session.wait_for(|facts| surface(facts, "own").drawn_opacity.is_some());
    let own = surface(&facts, "own");
    assert!(!own.shaded);
    assert!(own.frame.is_empty());
    assert_eq!(judge(&facts, own.id, NODE).visibility, Visibility::Visible);

    // One rolled up that takes its frame back, to draw its own, is unrolled:
    // nothing of it would be drawn at all.
    framed(&session, &mut desk, &mut queue, &qh, "framed");
    session.perform(Action::Shade);
    session.wait_for(|facts| surface(facts, "framed").shaded);
    desk.windows[1].request_decoration_mode(Some(DecorationMode::Client));
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| !surface(facts, "framed").shaded);
    assert!(surface(&facts, "framed").frame.is_empty());

    session.stop((desk, queue));
}

/// A fullscreen window has no titlebar, so going fullscreen unrolls one,
/// whoever asks: a taskbar, which needs no person at the seat, or the
/// window's own client, which does.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn going_fullscreen_unrolls_a_window_however_it_is_asked() {
    let session = Session::start("shade-fullscreen", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    framed(&session, &mut desk, &mut queue, &qh, "framed");
    until(&mut queue, &mut desk, |desk| desk.task("framed").is_some());
    session.perform(Action::Shade);
    session.wait_for(|facts| surface(facts, "framed").shaded);

    let task = desk.task("framed").expect("waited for").handle.clone();
    task.set_fullscreen(None);
    queue.flush().expect("sent");
    session.wait_for(|facts| {
        let window = surface(facts, "framed");
        !window.shaded && window.frame.is_empty()
    });
    task.unset_fullscreen();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| strips(facts, "framed") == 4);
    assert!(!surface(&facts, "framed").shaded, "and stays unrolled");
    session.stop((desk, queue));

    let session = Session::start(
        "shade-fullscreen-seated",
        Backend::headless((800, 600)).with_person(),
    );
    let (mut desk, mut queue, qh, _) = session.client();
    framed(&session, &mut desk, &mut queue, &qh, "framed");
    session.perform(Action::Shade);
    session.wait_for(|facts| surface(facts, "framed").shaded);
    desk.windows[0].set_fullscreen(None);
    queue.flush().expect("sent");
    session.wait_for(|facts| {
        let window = surface(facts, "framed");
        !window.shaded && window.frame.is_empty()
    });
    session.stop((desk, queue));
}

/// Maximized, a window rolls up to its titlebar across the top of the
/// monitor, and stays maximized; put back, it stays rolled up.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_maximized_window_rolls_up_to_its_titlebar_across_the_top_and_stays_maximized() {
    let session = Session::start("shade-maximized", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, _) = session.client();
    framed(&session, &mut desk, &mut queue, &qh, "framed");
    // Maximized, under its titlebar at the top of the monitor. The test's
    // client keeps its own size, and the bar is as wide as it.
    session.perform(Action::ToggleMaximize);
    let facts = session.wait_for(|facts| {
        let window = surface(facts, "framed");
        window.frame.len() == 1 && window.geometry.y0 == 24.0
    });
    let width = surface(&facts, "framed").geometry.x1;

    session.perform(Action::Shade);
    let facts = session.wait_for(|facts| surface(facts, "framed").shaded);
    assert_eq!(
        surface(&facts, "framed").frame,
        vec![Rect::new(0.0, 0.0, width, 24.0)],
        "the titlebar alone, at the top of the monitor"
    );

    session.perform(Action::ToggleMaximize);
    let facts = session.wait_for(|facts| strips(facts, "framed") == 2);
    assert!(
        surface(&facts, "framed").shaded,
        "put back, still rolled up"
    );

    // And maximized while rolled up, it stays rolled up.
    session.perform(Action::ToggleMaximize);
    let facts = session.wait_for(|facts| {
        let window = surface(facts, "framed");
        window.frame.len() == 1 && window.geometry.y0 == 24.0
    });
    assert!(
        surface(&facts, "framed").shaded,
        "maximized, still rolled up"
    );
    session.perform(Action::Unshade);
    session.wait_for(|facts| !surface(facts, "framed").shaded && strips(facts, "framed") == 1);
    session.perform(Action::ToggleMaximize);
    session.wait_for(|facts| strips(facts, "framed") == 4);

    session.stop((desk, queue));
}

/// A window rolled up has nothing drawn to hang a menu from: its menus
/// close as it rolls up, and one it opens while rolled up, as a key it
/// still has could ask for, is closed at once.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn rolling_a_window_up_closes_its_menus_and_refuses_a_new_one() {
    let session = Session::start("shade-menus", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, _) = session.client();
    framed(&session, &mut desk, &mut queue, &qh, "framed");
    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (10, 10), (60, 40));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (60, 40), 0xff22_cc44);
    menu.wl_surface().commit();
    queue.flush().expect("sent");
    session.wait_for(|facts| popups(facts) == 1);

    session.perform(Action::Shade);
    until(&mut queue, &mut desk, |desk| desk.popups_done == 1);
    session.wait_for(|facts| popups(facts) == 0 && surface(facts, "framed").shaded);

    let _again = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (10, 10), (60, 40));
    queue.flush().expect("sent");
    until(&mut queue, &mut desk, |desk| desk.popups_done == 2);

    session.stop((desk, queue));
}

fn popups(facts: &HostFacts) -> usize {
    facts
        .surfaces()
        .iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::Popup { .. }))
        .count()
}

/// As in Fluxbox and Openbox, a window rolled up keeps the keyboard: keys
/// still go to its client.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_rolled_up_window_keeps_the_keyboard() {
    let session = Session::start("shade-keyboard", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    framed(&session, &mut desk, &mut queue, &qh, "framed");
    let window = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });

    session.perform(Action::Shade);
    session.wait_for(|facts| surface(facts, "framed").shaded);
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert_eq!(heard(&ear).entered.as_ref(), Some(&window));

    session.stop((desk, queue));
}

/// The person's hand, and pictures, which need the `capture` feature
/// headless: both find what is drawn from the clients' buffers.
#[cfg(feature = "capture")]
mod drawn {
    use perspicax_compositor::{Command, Host};
    use perspicax_index::{Shot, ShotTarget};
    use perspicax_policy::{Button, Mods};

    use super::*;

    const LOWER: u32 = 0xffee_8822;
    const UPPER: u32 = 0xff33_6699;

    fn rgba(argb: u32) -> [u8; 4] {
        let [b, g, r, a] = argb.to_le_bytes();
        [r, g, b, a]
    }

    fn pixel(shot: &Shot, x: f64, y: f64) -> [u8; 4] {
        let (x, y) = ((x * shot.scale) as usize, (y * shot.scale) as usize);
        let at = (y * shot.width as usize + x) * 4;
        shot.rgba[at..at + 4].try_into().unwrap()
    }

    /// An orange window that draws its own frame, and over it, cascaded, a
    /// blue one in a frame of ours, which has the keyboard.
    fn two(name: &str) -> (Session, Desk, EventQueue<Desk>) {
        let session = Session::start(name, Backend::headless((800, 600)));
        let (mut desk, mut queue, qh, globals) = session.client();
        desk.bind_pointer(&globals, &qh);
        desk.open_coloured(&qh, "lower", "lower", LOWER);
        until(&mut queue, &mut desk, |desk| desk.has_drawn(0));
        desk.open_framed_coloured(&qh, "upper", UPPER);
        until(&mut queue, &mut desk, |desk| desk.has_drawn(1));
        session.wait_for(|facts| has(facts, "lower") && strips(facts, "upper") == 4);
        queue.roundtrip(&mut desk).expect("the compositor answers");
        (session, desk, queue)
    }

    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn a_click_where_a_rolled_up_window_was_lands_on_the_window_beneath() {
        let (session, mut desk, mut queue) = two("shade-click");
        let facts = session.wait_for(|_| true);
        let upper = surface(&facts, "upper").geometry;
        let at = ((upper.x0 + 10.0) as i32, (upper.y0 + 10.0) as i32);
        let click = Command::Click {
            at,
            button: Button::Left,
            mods: Mods::default(),
        };
        let (lower, upper) = (
            desk.windows[0].wl_surface().clone(),
            desk.windows[1].wl_surface().clone(),
        );

        session.command(click.clone());
        until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);
        assert_eq!(desk.pointed.as_ref(), Some(&upper));

        session.perform(Action::Shade);
        session.wait_for(|facts| surface(facts, "upper").shaded);
        session.command(click);
        until(&mut queue, &mut desk, |desk| desk.buttons.len() == 4);
        assert_eq!(desk.pointed.as_ref(), Some(&lower), "the window beneath");

        session.stop((desk, queue));
    }

    /// The monitor shows what was under the client, and nothing says the
    /// client is in the picture; a picture of the window alone shows all of
    /// it, as one of a minimized window does.
    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn a_picture_of_a_rolled_up_window_shows_it_whole() {
        let (session, desk, queue) = two("shade-picture");
        let host = Host::new(&session.facts, &session.requests);
        session.perform(Action::Shade);
        let facts = session.wait_for(|facts| surface(facts, "upper").shaded);
        let upper = surface(&facts, "upper");
        let (id, area) = (upper.id, upper.geometry);

        let monitor = host.capture(ShotTarget::Output(None)).expect("a picture");
        assert_eq!(
            pixel(&monitor, area.x0 + 10.0, area.y0 + 10.0),
            rgba(LOWER),
            "where its client was"
        );
        assert!(
            monitor.drawn.iter().all(|drawn| drawn.surface != id),
            "its client is not in the picture: {:?}",
            monitor.drawn
        );

        let shot = host.capture(ShotTarget::Window(id)).expect("a picture");
        assert_eq!((shot.width, shot.height), (400, 300));
        assert_eq!(pixel(&shot, 10.0, 10.0), rgba(UPPER));
        assert_eq!(pixel(&shot, 390.0, 290.0), rgba(UPPER));

        session.stop((desk, queue));
    }
}
