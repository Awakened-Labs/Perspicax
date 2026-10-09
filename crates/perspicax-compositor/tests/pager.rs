//! The workspaces, as a pager is told them, by a compositor with no screen:
//! `ext-workspace-v1`, as waybar's `ext/workspaces` speaks it.
//!
//! Spanning, one group of the grid's workspaces over both monitors; per
//! output, a group each. A pager that activates a workspace and commits
//! switches to it, and a switch made any other way -- a person's key -- is
//! what the pager sees next. While the session is locked it can switch
//! nothing.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Session, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_policy::{
    Access, Action, Direction, Grid, Mode, Place, Program, Protocol, Rule, Shape, Side,
};

fn session(name: &str, mode: Mode) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![
                Virtual::numbered(1, (1280, 1024)),
                Virtual {
                    place: Place::Beside {
                        side: Side::RightOf,
                        of: "HEADLESS-1".to_owned(),
                        offset: 0,
                    },
                    ..Virtual::numbered(2, (1920, 1080))
                },
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
            person: false,
            opacity: Default::default(),
        },
    )
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_pager_sees_one_group_of_four_spanning_both_monitors() {
    let session = session("pager-spanning", Mode::Spanning);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pager(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.pager_done > 0);

    let groups = desk.live_groups();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].outputs, 2);
    assert_eq!(groups[0].workspaces.len(), 4);
    assert_eq!(desk.active_workspaces(), ["1"]);
    let fourth = desk.paged.iter().find(|paged| paged.name == "4").unwrap();
    assert_eq!(fourth.coordinates, [1, 1]);
    assert!(!fourth.id.is_empty());

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn activating_and_committing_switches_and_a_key_is_seen_too() {
    let session = session("pager-switch", Mode::Spanning);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.bind_pager(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.pager_done > 0);

    desk.switch_to("4");
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["4"]
    });
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.off_workspace == Some(1))
    });

    // A person's key, and the pager follows.
    session.perform(Action::Workspace(Direction::Left));
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["3"]
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn per_output_each_monitor_is_its_own_group() {
    let session = session("pager-per-output", Mode::PerOutput);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pager(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.pager_done > 0);

    let groups = desk.live_groups();
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|group| group.outputs == 1));
    assert!(groups.iter().all(|group| group.workspaces.len() == 4));
    assert_eq!(desk.active_workspaces(), ["1", "1"]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_pager_switches_nothing_while_locked() {
    let session = session("pager-locked", Mode::Spanning);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.bind_pager(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.pager_done > 0);

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    desk.switch_to("2");
    queue.roundtrip(&mut desk).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    let facts = session.wait_for(|_| true);
    assert!(
        facts
            .surfaces()
            .iter()
            .all(|surface| surface.off_workspace.is_none()),
        "still on workspace 1: {facts:?}"
    );
    assert_eq!(desk.active_workspaces(), ["1"]);

    desk.unlock();
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_narrowed_rule_takes_the_pager_back() {
    let session = session("pager-narrowed", Mode::Spanning);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pager(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.pager_done > 0);

    session.command(Command::Protocols(Access::open().with(
        Protocol::Workspace,
        Rule::Only(vec![Program::parse("waybar")]),
    )));
    until(&mut queue, &mut desk, |desk| desk.pager_finished);
    assert!(desk.live_groups().is_empty());

    session.stop((desk, queue));
}
