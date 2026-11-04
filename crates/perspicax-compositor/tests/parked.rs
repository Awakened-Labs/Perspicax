//! What the workspaces park stays parked, whatever brings it forward (#220),
//! by a compositor with no screen.
//!
//! One rule decides what is on screen: a window is drawn when the person has
//! not minimized it, it is the front of its tab group, and its workspace is
//! the one its monitor shows. A tab coming forward into a group on a
//! workspace not showing -- the tab in front having closed, say -- waits
//! there with its group, in its place, rather than being drawn in front of
//! the person and given the keys while still parked on its own, to vanish at
//! the next switch.
//!
//! Every test counts the windows of a title the facts list: one both parked
//! and drawn is listed twice, and both entries say it is not drawn, so
//! whether it is drawn would not show it.
//!
//! The windows here draw their own frame, so their frame is their window.
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Configured, Desk, Said, Session, until};
use perspicax_compositor::{Backend, Virtual};
use perspicax_index::HostFacts;
use perspicax_policy::{Access, Action, Direction, Grid, Mode, Shape, Towards};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{EventQueue, QueueHandle, globals::GlobalList};

/// Where a window is, as the facts an agent reads tell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// On screen.
    Shown,
    /// On another workspace, counting from 1.
    Away(u16),
    /// A tab behind another.
    Behind,
    /// Off screen, on no other workspace and in front of its group:
    /// minimized.
    Hidden,
}

/// One 1280 by 1024 monitor with two workspaces side by side, and a person
/// at it.
fn seated(name: &str) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![Virtual::numbered(1, (1280, 1024))],
            workspaces: Shape {
                mode: Mode::Spanning,
                grid: Grid {
                    columns: 2,
                    rows: 1,
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

/// Two 1280 by 1024 monitors side by side, each with its own four
/// workspaces, and a person at them.
fn per_output(name: &str) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![
                Virtual::numbered(1, (1280, 1024)),
                Virtual::numbered(2, (1280, 1024)),
            ],
            workspaces: Shape {
                mode: Mode::PerOutput,
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

/// "first" and then "second", one program's, grouped: "second", which has
/// the keys, in front, in the place "first" had.
fn grouped(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList) {
    let (mut desk, mut queue, qh, globals) = session.client();
    for (n, title) in ["first", "second"].into_iter().enumerate() {
        desk.open_window(&qh, title, "app.tabbed");
        until(&mut queue, &mut desk, |desk| desk.has_drawn(n));
    }
    session.wait_for(|facts| {
        listed(facts, "first") == [Seen::Shown]
            && listed(facts, "second") == [Seen::Shown]
            && focused_at(facts, "second") > focused_at(facts, "first")
    });
    session.perform(Action::TabWithPrevious);
    session.wait_for(|facts| {
        listed(facts, "first") == [Seen::Behind] && listed(facts, "second") == [Seen::Shown]
    });
    (desk, queue, qh, globals)
}

/// Close the `nth` window, titled `title`, as its client does, and wait for
/// it to be gone.
fn close(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    nth: usize,
    title: &str,
) -> HostFacts {
    desk.windows.remove(nth);
    queue.roundtrip(desk).expect("the close taken in");
    session.wait_for(|facts| listed(facts, title).is_empty())
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tab_coming_forward_in_a_group_on_a_workspace_not_showing_waits_there_with_it() {
    let session = seated("parked-heir");
    let (mut desk, mut queue, _qh, _globals) = grouped(&session);
    let place = corner(&session.facts.read(), "second");
    session.perform(Action::Workspace(Direction::Right));
    let away = session.wait_for(|facts| listed(facts, "second") == [Seen::Away(1)]);
    let focused = focused_at(&away, "first");

    // The tab in front closes, as a tabbed terminal's does when its shell
    // exits: the one behind comes forward, on the group's workspace.
    let closed = close(&session, &mut desk, &mut queue, 1, "second");
    assert_eq!(
        listed(&closed, "first"),
        [Seen::Away(1)],
        "one window, waiting on workspace 1: {closed:?}"
    );
    assert_eq!(
        focused_at(&closed, "first"),
        focused,
        "given the keys unseen"
    );

    session.perform(Action::Workspace(Direction::Left));
    let back = session.wait_for(|facts| listed(facts, "first") == [Seen::Shown]);
    assert_eq!(corner(&back, "first"), place, "in the group's place");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tab_coming_forward_in_a_snapped_group_elsewhere_snaps_as_it_comes_back() {
    let session = seated("parked-heir-snapped");
    let (mut desk, mut queue, _qh, _globals) = grouped(&session);
    session.perform(Action::Snap(Direction::Right));
    let snapped = session.wait_for(|facts| corner(facts, "second").0 >= 640.0);
    let place = corner(&snapped, "second");
    session.perform(Action::Workspace(Direction::Right));
    session.wait_for(|facts| listed(facts, "second") == [Seen::Away(1)]);

    let closed = close(&session, &mut desk, &mut queue, 1, "second");
    assert_eq!(
        listed(&closed, "first"),
        [Seen::Away(1)],
        "one window, waiting on workspace 1: {closed:?}"
    );

    // Snapped to the right half as its group was, and alone now, so with no
    // strip of tabs above it.
    session.perform(Action::Workspace(Direction::Left));
    let back = session.wait_for(|facts| listed(facts, "first") == [Seen::Shown]);
    assert_eq!(corner(&back, "first"), (place.0, 0.0));
    until(&mut queue, &mut desk, |desk| {
        told(desk, 0).is_some_and(|told| told.tiled && told.size == Some((640, 1024)))
    });

    session.stop((desk, queue));
}

/// Per output, the monitor a window is on decides which workspace it is on.
/// A tab behind was parked where its group was when it went behind, so one
/// coming forward after its group moved monitor is judged where its group
/// is now, not where it was left.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn per_output_a_tab_coming_forward_is_judged_on_its_groups_monitor() {
    let session = per_output("parked-per-output");
    let (mut desk, mut queue, qh, _globals) = grouped(&session);
    session.perform(Action::MoveToOutput(Towards::Side(Direction::Right)));
    session.wait_for(|facts| corner(facts, "second").0 >= 1280.0);

    // The left monitor, where "first" went behind, flips to its workspace 2,
    // by a window there taking the keys.
    desk.open_window(&qh, "third", "app.third");
    until(&mut queue, &mut desk, |desk| desk.has_drawn(2));
    session.wait_for(|facts| {
        listed(facts, "third") == [Seen::Shown] && corner(facts, "third").0 < 1280.0
    });
    session.perform(Action::Workspace(Direction::Right));
    let flipped = session.wait_for(|facts| {
        listed(facts, "third") == [Seen::Away(1)]
            && focused_at(facts, "second") > focused_at(facts, "third")
    });
    assert_eq!(listed(&flipped, "second"), [Seen::Shown]);

    session.perform(Action::CycleTab { forward: true });
    let cycled = session.wait_for(|facts| listed(facts, "second") == [Seen::Behind]);
    assert_eq!(
        listed(&cycled, "first"),
        [Seen::Shown],
        "one window, on screen: {cycled:?}"
    );
    assert!(
        corner(&cycled, "first").0 >= 1280.0,
        "on the right monitor, with its group: {cycled:?}"
    );

    session.stop((desk, queue));
}

/// Each window titled `title` the facts list now, and where. A window both
/// parked and drawn is listed twice.
fn listed(facts: &HostFacts, title: &str) -> Vec<Seen> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.title.as_deref() == Some(title))
        .map(
            |surface| match (surface.mapped, surface.off_workspace, surface.behind_tab) {
                (true, ..) => Seen::Shown,
                (false, Some(workspace), _) => Seen::Away(workspace),
                (false, None, Some(_)) => Seen::Behind,
                (false, None, None) => Seen::Hidden,
            },
        )
        .collect()
}

/// The last configure the `nth` window was sent.
fn told(desk: &Desk, nth: usize) -> Option<Configured> {
    desk.said_to(desk.windows[nth].wl_surface())
        .into_iter()
        .rev()
        .find_map(|said| match said {
            Said::Configure(told) => Some(told),
            _ => None,
        })
}

/// When the first window titled `title` was last given the keyboard.
fn focused_at(facts: &HostFacts, title: &str) -> Option<std::time::Instant> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .and_then(|surface| surface.focused_at)
}

/// Where the first window titled `title`'s corner is.
fn corner(facts: &HostFacts, title: &str) -> (f64, f64) {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| (surface.geometry.x0, surface.geometry.y0))
        .unwrap_or_else(|| panic!("no window titled {title}: {facts:?}"))
}
