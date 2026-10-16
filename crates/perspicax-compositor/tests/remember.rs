//! A program's next window opens where its last one closed (#89).
//!
//! A Wayland window cannot ask where to open, so the compositor remembers
//! for it, by app id: the monitor its program's last window closed on, and
//! where on it. The next window of that program opens there at its first
//! picture, the first time it can be known by its program, rather than where
//! the cascade put it as it was made. Where a filled or snapped window is
//! put back is the place remembered, and a minimized one is remembered where
//! it was. A second window, opened while the first is still there, steps
//! aside. A dialog is never remembered, nor put where its program was.
//!
//! With nobody at the seat a window stays where the cascade puts it, as the
//! occlusion tests rely on. The windows here are 400 by 300 and draw their
//! own frame, so their frame is their window, and the cascade puts the nth
//! at (32n, 32n). Like the other live tests these bind a real Wayland socket,
//! so they need `XDG_RUNTIME_DIR`, and are `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{HostFacts, SurfaceFacts};
use perspicax_policy::{Access, Action, Direction, Grid, Mode, Shape, Towards};
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::{EventQueue, QueueHandle};

/// One 1280 by 1024 monitor, with a person at it.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).with_person())
}

/// Two 1280 by 1024 monitors side by side, with a person at them, and four
/// workspaces shared as `mode` says.
fn two_monitors(name: &str, mode: Mode) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![
                Virtual::numbered(1, (1280, 1024)),
                Virtual::numbered(2, (1280, 1024)),
            ],
            workspaces: Shape {
                mode,
                grid: Grid {
                    columns: 2,
                    rows: 2,
                    wrap: false,
                },
            },
            access: Access::open(),
            person: true,
            opacity: Default::default(),
            agent: Default::default(),
        },
    )
}

/// The window titled `title`, if the facts have it.
fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

/// Where the window titled `title`'s corner is, once it is on screen and
/// has drawn.
fn corner(facts: &HostFacts, title: &str) -> Option<(f64, f64)> {
    titled(facts, title)
        .filter(|surface| surface.mapped && surface.geometry.x1 > surface.geometry.x0)
        .map(|surface| (surface.geometry.x0, surface.geometry.y0))
}

/// Open a window of `app`, and wait for it to be on screen: where it is.
fn open(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
    app: &str,
) -> (f64, f64) {
    desk.open_window(qh, title, app);
    until(queue, desk, |_| {
        corner(&session.facts.read(), title).is_some()
    });
    corner(&session.facts.read(), title).expect("waited for")
}

/// Close the `nth` window, titled `title`, as its client does, and wait for
/// it to be gone.
fn close(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    nth: usize,
    title: &str,
) {
    desk.windows.remove(nth);
    queue.roundtrip(desk).expect("the close taken in");
    session.wait_for(|facts| titled(facts, title).is_none());
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_opens_its_next_window_where_its_last_one_closed() {
    let session = seated("remember-reopens");
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    let b = open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    assert_eq!(b, (32.0, 32.0));
    close(&session, &mut desk, &mut queue, 1, "b");
    open(&session, &mut desk, &mut queue, &qh, "c", "app.c");

    let again = open(&session, &mut desk, &mut queue, &qh, "b again", "app.b");
    assert_eq!(again, (32.0, 32.0), "where b closed, not the cascade's 96");
    let d = open(&session, &mut desk, &mut queue, &qh, "d", "app.d");
    assert_eq!(
        d,
        (96.0, 96.0),
        "the cascade did not move on for the window that went where it was"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_programs_second_window_steps_aside_from_its_first() {
    let session = seated("remember-second");
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    close(&session, &mut desk, &mut queue, 1, "b");

    let first = open(&session, &mut desk, &mut queue, &qh, "b one", "app.b");
    assert_eq!(first, (32.0, 32.0));
    let second = open(&session, &mut desk, &mut queue, &qh, "b two", "app.b");
    assert_eq!(second, (64.0, 64.0), "a cascade step on from the first");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_closed_maximized_or_snapped_is_remembered_where_it_would_be_put_back() {
    let session = seated("remember-filled");
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    session.perform(Action::ToggleMaximize);
    session.wait_for(|facts| corner(facts, "b") == Some((0.0, 0.0)));
    open(&session, &mut desk, &mut queue, &qh, "c", "app.c");
    session.perform(Action::Snap(Direction::Right));
    session.wait_for(|facts| corner(facts, "c") == Some((640.0, 0.0)));
    close(&session, &mut desk, &mut queue, 1, "b");
    close(&session, &mut desk, &mut queue, 1, "c");

    let b = open(&session, &mut desk, &mut queue, &qh, "b again", "app.b");
    assert_eq!(b, (32.0, 32.0), "where b unmaximizes to");
    let c = open(&session, &mut desk, &mut queue, &qh, "c again", "app.c");
    assert_eq!(c, (64.0, 64.0), "where c unsnaps to");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_opens_maximized_keeps_its_monitor_rather_than_its_programs_place() {
    let session = seated("remember-opens-filled");
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    close(&session, &mut desk, &mut queue, 1, "b");

    // Asked for before its first picture, as a program that remembers it
    // was maximized asks.
    desk.open_window(&qh, "b again", "app.b");
    desk.windows.last().expect("just opened").set_maximized();
    until(&mut queue, &mut desk, |_| {
        corner(&session.facts.read(), "b again").is_some()
    });
    assert_eq!(
        corner(&session.facts.read(), "b again"),
        Some((0.0, 0.0)),
        "filling its monitor, not moved to where b closed"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_closed_while_minimized_is_remembered_where_it_was() {
    let session = seated("remember-minimized");
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    session.perform(Action::Minimize);
    session.wait_for(|facts| titled(facts, "b").is_some_and(|b| !b.mapped));
    close(&session, &mut desk, &mut queue, 1, "b");

    let again = open(&session, &mut desk, &mut queue, &qh, "b again", "app.b");
    assert_eq!(again, (32.0, 32.0), "not the cascade's 64");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_reopens_on_the_monitor_it_closed_on_not_the_one_under_the_pointer() {
    let session = two_monitors("remember-monitor", Mode::Spanning);
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    let moved = session.wait_for(|facts| corner(facts, "b").is_some_and(|(x, _)| x >= 1280.0));
    let there = corner(&moved, "b").expect("waited for");
    close(&session, &mut desk, &mut queue, 1, "b");

    // The pointer is still at the origin, on the left monitor.
    let again = open(&session, &mut desk, &mut queue, &qh, "b again", "app.b");
    assert_eq!(again, there);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_whose_monitor_was_unplugged_reopens_at_the_same_corner_of_the_pointers() {
    let session = two_monitors("remember-unplugged", Mode::Spanning);
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    let moved = session.wait_for(|facts| corner(facts, "b").is_some_and(|(x, _)| x >= 1280.0));
    let (x, y) = corner(&moved, "b").expect("waited for");
    close(&session, &mut desk, &mut queue, 1, "b");
    session.command(Command::Unplug("HEADLESS-2".to_owned()));
    session.wait_for(|facts| facts.outputs().len() == 1);

    let again = open(&session, &mut desk, &mut queue, &qh, "b again", "app.b");
    assert_eq!(again, (x - 1280.0, y), "not the cascade's 64");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn per_output_a_program_reopens_on_the_workspace_its_monitor_shows_now() {
    let session = two_monitors("remember-per-output", Mode::PerOutput);
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    let moved = session.wait_for(|facts| corner(facts, "b").is_some_and(|(x, _)| x >= 1280.0));
    let there = corner(&moved, "b").expect("waited for");
    // The right monitor, where b is, goes to workspace 2, and b is put away
    // with workspace 1. Closed there, it is remembered where it was put.
    session.perform(Action::GoToWorkspace(2));
    session.wait_for(|facts| titled(facts, "b").is_some_and(|b| b.off_workspace == Some(1)));
    close(&session, &mut desk, &mut queue, 1, "b");

    let again = open(&session, &mut desk, &mut queue, &qh, "b again", "app.b");
    assert_eq!(again, there);
    let facts = session.facts.read();
    let b = titled(&facts, "b again").expect("open");
    assert_eq!(
        (b.workspace, b.off_workspace),
        (Some(2), None),
        "on the workspace the right monitor shows, not the left one's"
    );
    assert_eq!(
        titled(&facts, "a").map(|a| (a.workspace, a.off_workspace)),
        Some((Some(1), None)),
        "the left monitor still shows workspace 1"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_dialog_is_not_remembered_nor_put_where_its_program_was() {
    let session = seated("remember-dialog");
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    close(&session, &mut desk, &mut queue, 1, "b");

    // b's program opens a dialog over a, which is cascaded as ever.
    desk.open_dialog(&qh, "b's dialog", "app.b", 0);
    until(&mut queue, &mut desk, |_| {
        corner(&session.facts.read(), "b's dialog").is_some()
    });
    assert_eq!(
        corner(&session.facts.read(), "b's dialog"),
        Some((64.0, 64.0)),
        "the cascade's, not where b closed"
    );
    close(&session, &mut desk, &mut queue, 1, "b's dialog");

    let again = open(&session, &mut desk, &mut queue, &qh, "b again", "app.b");
    assert_eq!(
        again,
        (32.0, 32.0),
        "where b closed, not where its dialog did"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_its_client_unmaps_comes_back_where_it_was() {
    let session = seated("remember-unmapped");
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    let surface = desk.windows[1].wl_surface().clone();
    surface.attach(None, 0, 0);
    surface.commit();
    queue.roundtrip(&mut desk).expect("the unmap taken in");
    session.wait_for(|facts| titled(facts, "b").is_none());

    // Asked for again, as a client asks for a new window.
    desk.windows[1].commit();
    until(&mut queue, &mut desk, |_| {
        corner(&session.facts.read(), "b").is_some()
    });
    assert_eq!(
        corner(&session.facts.read(), "b"),
        Some((32.0, 32.0)),
        "not the cascade's 64"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tab_closed_behind_another_is_remembered_where_its_group_is() {
    let session = two_monitors("remember-tab", Mode::Spanning);
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    // b joins a's group, in front, where a was; a waits behind it there.
    session.perform(Action::TabWithPrevious);
    session.wait_for(|facts| corner(facts, "b") == Some((0.0, 0.0)));
    // The group goes to the right monitor, with a left behind where the
    // group was when it went behind.
    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    let moved = session.wait_for(|facts| corner(facts, "b").is_some_and(|(x, _)| x >= 1280.0));
    let group = corner(&moved, "b").expect("waited for");
    close(&session, &mut desk, &mut queue, 0, "a");

    let again = open(&session, &mut desk, &mut queue, &qh, "a again", "app.a");
    assert_eq!(again, group, "where its group is, not where it went behind");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn with_nobody_at_the_seat_a_program_reopens_where_the_cascade_puts_it() {
    let session = Session::start("remember-nobody", Backend::headless((1280, 1024)));
    let (mut desk, mut queue, qh, _) = session.client();
    open(&session, &mut desk, &mut queue, &qh, "a", "app.a");
    open(&session, &mut desk, &mut queue, &qh, "b", "app.b");
    close(&session, &mut desk, &mut queue, 1, "b");
    open(&session, &mut desk, &mut queue, &qh, "c", "app.c");

    let again = open(&session, &mut desk, &mut queue, &qh, "b again", "app.b");
    assert_eq!(again, (96.0, 96.0));

    session.stop((desk, queue));
}

#[cfg(feature = "capture")]
mod hand {
    use perspicax_policy::{Button, Mods};

    use super::*;

    /// The issue's own: dragged to the right half and closed, a program
    /// opens there again.
    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn a_window_dragged_somewhere_and_closed_reopens_there() {
        let session = seated("remember-dragged");
        let (mut desk, mut queue, qh, globals) = session.client();
        desk.bind_pointer(&globals, &qh);
        let (x, y) = open(&session, &mut desk, &mut queue, &qh, "a", "app.a");

        // The person takes it by its client, which asks to be moved, and
        // drags it onto the right half.
        session.command(Command::Press {
            at: (x as i32 + 200, y as i32 + 150),
            button: Button::Left,
            mods: Mods::default(),
        });
        until(&mut queue, &mut desk, |desk| desk.pressed.is_some());
        let seat = desk.seat.clone().expect("our pointer's seat");
        desk.windows[0].move_(&seat, desk.pressed.expect("waited for"));
        until(&mut queue, &mut desk, |desk| desk.pointed.is_none());
        session.command(Command::Motion { by: (700, 300) });
        session.command(Command::Release {
            button: Button::Left,
        });
        let put = (x + 700.0, y + 300.0);
        session.wait_for(|facts| corner(facts, "a") == Some(put));
        close(&session, &mut desk, &mut queue, 0, "a");

        let again = open(&session, &mut desk, &mut queue, &qh, "a again", "app.a");
        assert_eq!(again, put);

        session.stop((desk, queue));
    }

    /// A window left hanging off the left of the second monitor, mostly on
    /// it with its corner on the first, is remembered on the second, and its
    /// program's next window is slid onto that one, not the first.
    #[test]
    #[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
    fn a_window_left_hanging_off_its_monitor_reopens_slid_onto_that_monitor() {
        let session = two_monitors("remember-hanging", Mode::Spanning);
        let (mut desk, mut queue, qh, globals) = session.client();
        desk.bind_pointer(&globals, &qh);
        open(&session, &mut desk, &mut queue, &qh, "a", "app.a");

        session.command(Command::Press {
            at: (200, 150),
            button: Button::Left,
            mods: Mods::default(),
        });
        until(&mut queue, &mut desk, |desk| desk.pressed.is_some());
        let seat = desk.seat.clone().expect("our pointer's seat");
        desk.windows[0].move_(&seat, desk.pressed.expect("waited for"));
        until(&mut queue, &mut desk, |desk| desk.pointed.is_none());
        // 90 of its 400 px on the first monitor, the rest on the second.
        session.command(Command::Motion { by: (1190, 0) });
        session.command(Command::Release {
            button: Button::Left,
        });
        session.wait_for(|facts| corner(facts, "a") == Some((1190.0, 0.0)));
        close(&session, &mut desk, &mut queue, 0, "a");

        let again = open(&session, &mut desk, &mut queue, &qh, "a again", "app.a");
        assert_eq!(
            again,
            (1280.0, 0.0),
            "onto the second, not back onto the first"
        );

        session.stop((desk, queue));
    }
}
