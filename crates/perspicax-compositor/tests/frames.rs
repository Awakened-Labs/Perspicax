//! Telling a window it may draw again, on a compositor with no screen.
//!
//! A Wayland client draws, asks to be told when it may draw again, and
//! waits: GTK and Chromium wait for that frame callback with no limit. A
//! seat sends them to the windows smithay counts as on the monitor it just
//! drew, and smithay only counts again when asked to. Asked only when a
//! client committed, a window brought back onto the screen -- by a switch of
//! workspace, or rescued from a monitor that went -- was drawn there at once
//! but not counted, and so never told it may draw, until some other client
//! drew. While the shell ran, its panel always did. Once it had gone,
//! every window waited on every other, and all of them stayed frozen (#99).
//!
//! Headless, every window in the space is told on a timer, counted or not,
//! so what a test can see is the count itself: a window is told which
//! monitor it is on (`wl_surface.enter`) the moment smithay counts it there.
//! Brought back, it must be told that before it is told it may draw. On a
//! seat the first comes with the second, or neither comes.
//!
//! smithay can only count a window it knows the size of, and headless it
//! learns sizes from the buffers only the `capture` feature keeps, so this
//! needs `capture`. Like the other live tests it binds a real Wayland
//! socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Said, Session, Told, heard, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_policy::{Access, Action, Direction, Grid, Mode, Place, Shape, Side, Towards};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{Proxy as _, protocol::wl_surface::WlSurface};

const PANEL: u32 = 40;
const PANEL_COLOUR: u32 = 0xff20_2020;
const LAUNCHER: u32 = 200;
const LAUNCHER_COLOUR: u32 = 0xff40_4040;

/// One monitor, 1280 by 800, with two workspaces side by side.
fn two_workspaces(person: bool) -> Backend {
    Backend::Headless {
        outputs: vec![Virtual::numbered(1, (1280, 800))],
        workspaces: Shape {
            mode: Mode::Spanning,
            grid: Grid {
                columns: 2,
                rows: 1,
                wrap: false,
            },
        },
        access: Access::open(),
        person,
        opacity: Default::default(),
        agent: Default::default(),
    }
}

/// Two monitors side by side, HEADLESS-2 to the right of HEADLESS-1.
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
        agent: Default::default(),
    }
}

/// How many times `surface` was told it is on a monitor.
fn entered(desk: &Desk, surface: &WlSurface) -> usize {
    told(desk, surface, |said| matches!(said, Said::Enter(_)))
}

/// How many times `surface` was told it is no longer on a monitor.
fn left(desk: &Desk, surface: &WlSurface) -> usize {
    told(desk, surface, |said| matches!(said, Said::Leave(_)))
}

fn told(desk: &Desk, surface: &WlSurface, which: impl Fn(&Said) -> bool) -> usize {
    desk.said_to(surface)
        .iter()
        .filter(|said| which(said))
        .count()
}

/// Whether, since it last left a monitor, `surface` was told it is on one
/// before it was told it may draw: whatever told it to draw knew where it
/// was.
fn entered_before_it_may_draw(desk: &Desk, surface: &WlSurface) -> bool {
    let said = desk.said_to(surface);
    let since = said
        .iter()
        .rposition(|said| matches!(said, Said::Leave(_)))
        .map_or(0, |at| at + 1);
    let said = &said[since..];
    let enter = said.iter().position(|said| matches!(said, Said::Enter(_)));
    let done = said.iter().position(|said| *said == Said::Done);
    match (enter, done) {
        (Some(enter), Some(done)) => enter < done,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// The size of the last configure `surface` was sent that offered one.
fn offered(desk: &Desk, surface: &WlSurface) -> Option<(u32, u32)> {
    desk.said_to(surface)
        .iter()
        .rev()
        .find_map(|said| match said {
            Said::Configure(configured) => configured.size,
            _ => None,
        })
}

/// The window comes back with nothing drawn since: no client commits to
/// make smithay count it, and it used to wait for one.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_brought_back_by_a_switch_is_told_it_is_on_its_monitor_though_nothing_drew() {
    let session = Session::start("frames-switch", two_workspaces(false));
    let (mut desk, mut queue, qh, _globals) = session.client();
    desk.open_window(&qh, "page", "page");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let page = desk.windows[0].wl_surface().clone();
    until(&mut queue, &mut desk, |desk| entered(desk, &page) == 1);

    session.perform(Action::Workspace(Direction::Right));
    until(&mut queue, &mut desk, |desk| left(desk, &page) == 1);
    // Away, it drew, as a terminal does when its program writes, and now
    // waits to be told it may draw again. Nothing is told that off screen.
    desk.ask_frame(&qh, &page);
    queue.roundtrip(&mut desk).expect("round trip");

    // Back. A plain window fits where it was, so it is sent no configure,
    // and draws nothing.
    session.perform(Action::Workspace(Direction::Left));
    until(&mut queue, &mut desk, |desk| entered(desk, &page) == 2);
    assert!(
        entered_before_it_may_draw(&desk, &page),
        "told it may draw before it was told it is on the monitor: {:?}",
        desk.said_to(&page)
    );
    session.stop((desk, queue));
}

/// Issue #99, as it happened: a shell with a panel and a launcher open goes
/// with its client, over two windows that wait for frame callbacks as GTK's
/// do, snapped side by side. They are fitted to the room the panel gave
/// back, the keyboard goes back to the window on top, and after a switch
/// away and back each is told it is on the monitor before it is told it
/// may draw. On a seat they were never told either.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn windows_a_dead_shell_left_are_fitted_to_its_room_and_told_where_they_are_after_a_switch() {
    let session = Session::start("frames-shell-gone", two_workspaces(true));
    // The shell, on a client of its own: a panel holding room at the top.
    let (mut shell, mut shell_queue, shell_qh, shell_globals) = session.client();
    shell.bind_shell(&shell_globals, &shell_qh);
    shell.open_panel(&shell_qh, "panel", PANEL, PANEL_COLOUR);
    until(&mut shell_queue, &mut shell, |shell| {
        shell.layers_drawn == 1
    });

    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_patient(&qh, "left", "left");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let left_window = desk.windows[0].wl_surface().clone();
    session.perform(Action::Snap(Direction::Left));
    until(&mut queue, &mut desk, |desk| {
        offered(desk, &left_window) == Some((640, 800 - PANEL))
    });
    desk.open_patient(&qh, "right", "right");
    until(&mut queue, &mut desk, |desk| desk.drawn >= 2);
    let right_window = desk.windows[1].wl_surface().clone();
    session.perform(Action::Snap(Direction::Right));
    until(&mut queue, &mut desk, |desk| {
        offered(desk, &right_window) == Some((640, 800 - PANEL))
    });
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&right_window.id())
    });

    // The person presses their binding for a pie, and the shell shows it: a
    // launcher over everything, which takes the keyboard, the binding having
    // handed the shell their press (#267).
    session.press_bound(Action::Pie("launchers".to_owned()));
    until(&mut shell_queue, &mut shell, |shell| {
        shell
            .told
            .iter()
            .any(|told| matches!(told, Told::PieMenu(..)))
    });
    shell.open_launcher(&shell_qh, LAUNCHER, LAUNCHER_COLOUR);
    until(&mut shell_queue, &mut shell, |shell| {
        shell.layers_drawn == 2
    });
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    // The shell's client goes, as perspicax-shell's did (#101).
    drop((shell, shell_queue, shell_qh, shell_globals));
    until(&mut queue, &mut desk, |desk| {
        offered(desk, &left_window) == Some((640, 800))
            && offered(desk, &right_window) == Some((640, 800))
    });
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&right_window.id())
    });

    session.perform(Action::Workspace(Direction::Right));
    until(&mut queue, &mut desk, |desk| {
        left(desk, &left_window) == 1 && left(desk, &right_window) == 1
    });
    // Each draws while it is away, and waits.
    desk.ask_frame(&qh, &left_window);
    desk.ask_frame(&qh, &right_window);
    queue.roundtrip(&mut desk).expect("round trip");

    session.perform(Action::Workspace(Direction::Left));
    until(&mut queue, &mut desk, |desk| {
        entered(desk, &left_window) == 2 && entered(desk, &right_window) == 2
    });
    for window in [&left_window, &right_window] {
        assert!(
            entered_before_it_may_draw(&desk, window),
            "told it may draw before it was told it is on the monitor: {:?}",
            desk.said_to(window)
        );
    }
    session.stop((desk, queue));
}

/// A monitor unplugged under a window: the window is brought onto the one
/// left, which it is told it is on, though it draws nothing in the move.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_rescued_from_an_unplugged_monitor_is_told_it_is_on_the_one_it_landed_on() {
    let session = Session::start("frames-unplug", two_monitors());
    let (mut desk, mut queue, qh, _globals) = session.client();
    desk.open_window(&qh, "page", "page");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let page = desk.windows[0].wl_surface().clone();
    let on = |name: &str| Said::Enter(name.to_owned());
    until(&mut queue, &mut desk, |desk| {
        desk.said_to(&page).contains(&on("HEADLESS-1"))
    });

    // Across to the right-hand monitor, and drawn there.
    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    session.wait_for(|facts| {
        facts.surfaces().iter().any(|surface| {
            surface.title.as_deref() == Some("page") && surface.geometry.x0 >= 1280.0
        })
    });
    desk.ask_frame(&qh, &page);
    until(&mut queue, &mut desk, |desk| {
        desk.said_to(&page).contains(&on("HEADLESS-2"))
    });

    session.command(Command::Unplug("HEADLESS-2".to_owned()));
    until(&mut queue, &mut desk, |desk| {
        told(desk, &page, |said| *said == on("HEADLESS-1")) == 2
    });
    session.stop((desk, queue));
}
