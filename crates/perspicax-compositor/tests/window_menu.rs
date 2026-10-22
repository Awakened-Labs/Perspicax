//! A window acted on by the shell, as its menu does, by a compositor with no
//! screen.
//!
//! From version 5 the shell channel names a window by the handle the shell
//! holds on it from wlr-foreign-toplevel-management, and makes it sticky,
//! keeps it on top, rolls it up or sends it to a workspace, as the person's
//! keys do. A tab behind another stands for its group, in front. Nothing is
//! done while the session is locked, nor for a client no longer admitted to
//! the taskbar's protocol; and where each monitor has workspaces of its
//! own, a window stays on its monitor, whichever is named.
//!
//! The shell is told, of each window it holds a taskbar's handle on, what it
//! is made, what can be made of it and the workspace it is on, by its
//! pager's handle: once it holds the handles, whichever it bound first, and
//! again as any of it changes, whatever changed it, and only then. A tab
//! behind another is told its group's. A shell bound at version 4 is told
//! none of it, and none is told while the session is locked.
//!
//! Each request is carried out as it is read, and the facts are published
//! before the next is, so a roundtrip after one is as far as there is to
//! wait to see that it did nothing.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, WindowTold, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{HostFacts, SurfaceFacts};
use perspicax_policy::{
    Access, Action, Direction, Grid, Mode, Place, Program, Protocol, Rule, Shape, Side,
};
use perspicax_protocols::shell::v1::client::perspicax_shell_v1::{
    PerspicaxShellV1, WindowCapabilities, WindowState,
};
use wayland_client::{EventQueue, QueueHandle, globals::GlobalList};
use wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1::ExtWorkspaceHandleV1;
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1;

fn surface<'a>(facts: &'a HostFacts, title: &str) -> &'a SurfaceFacts {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .expect("a window of that title")
}

/// Where the window of this title is in the facts, counted from the bottom.
fn height(facts: &HostFacts, title: &str) -> usize {
    facts
        .surfaces()
        .iter()
        .position(|surface| surface.title.as_deref() == Some(title))
        .expect("a window of that title")
}

/// Workspaces `columns` by `rows`, across every monitor or each monitor's
/// own (`mode`), on these monitors.
fn backend(mode: Mode, columns: u16, rows: u16, outputs: Vec<Virtual>) -> Backend {
    Backend::Headless {
        outputs,
        workspaces: Shape {
            mode,
            grid: Grid {
                columns,
                rows,
                wrap: false,
            },
        },
        access: Access::open(),
        person: false,
        opacity: Default::default(),
        agent: Default::default(),
    }
}

/// One monitor, with workspaces across it `columns` by `rows`.
fn one_monitor(columns: u16, rows: u16) -> Backend {
    backend(
        Mode::Spanning,
        columns,
        rows,
        vec![Virtual::numbered(1, (800, 600))],
    )
}

/// A shell, its taskbar and its pager, bound as perspicax-shell binds them.
fn shell_on(
    name: &str,
    backend: Backend,
) -> (
    Session,
    Desk,
    EventQueue<Desk>,
    QueueHandle<Desk>,
    GlobalList,
) {
    let session = Session::start(name, backend);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_shell(&globals, &qh);
    desk.bind_taskbar(&globals, &qh);
    desk.bind_pager(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.pager_done > 0);
    (session, desk, queue, qh, globals)
}

/// Open a window of this title, its frame its own or (`framed`) ours, and
/// wait until it is drawn and told to the taskbar.
fn open(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
    framed: bool,
) {
    let nth = desk.windows.len();
    if framed {
        desk.open_framed(qh, title);
    } else {
        desk.open_window(qh, title, title);
    }
    until(queue, desk, |desk| {
        desk.has_drawn(nth) && desk.task(title).is_some()
    });
    session.wait_for(|facts| surface(facts, title).mapped);
}

/// A shell with windows of these titles, opened in turn.
fn windows(
    name: &str,
    backend: Backend,
    titles: &[&str],
) -> (Session, Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (session, mut desk, mut queue, qh, _) = shell_on(name, backend);
    for title in titles {
        open(&session, &mut desk, &mut queue, &qh, title, false);
    }
    (session, desk, queue, qh)
}

fn shell(desk: &Desk) -> PerspicaxShellV1 {
    desk.shell.clone().expect("bound")
}

/// The handle the taskbar holds on the window of this title.
fn handle(desk: &Desk, title: &str) -> ZwlrForeignToplevelHandleV1 {
    desk.task(title)
        .expect("told to the taskbar")
        .handle
        .clone()
}

/// The handle the pager holds on the workspace of this id.
fn workspace(desk: &Desk, id: &str) -> ExtWorkspaceHandleV1 {
    desk.paged
        .iter()
        .find(|paged| paged.id == id && !paged.removed)
        .unwrap_or_else(|| panic!("no workspace {id} among {:?}", desk.paged))
        .handle
        .clone()
}

/// Wait until what was asked has been carried out, or found to be nothing,
/// and say how the facts stand then.
fn carried_out(session: &Session, desk: &mut Desk, queue: &mut EventQueue<Desk>) -> HostFacts {
    queue.roundtrip(desk).expect("the compositor answers");
    session.wait_for(|_| true)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_shell_makes_a_window_sticky_and_puts_it_back_on_one_workspace() {
    let (session, mut desk, mut queue, _qh) =
        windows("menu-sticky", one_monitor(2, 1), &["window"]);
    let (shell, window) = (shell(&desk), handle(&desk, "window"));

    shell.set_sticky(&window, 1);
    queue.flush().expect("sent");
    session.wait_for(|facts| surface(facts, "window").workspace.is_none());
    session.perform(Action::Workspace(Direction::Right));
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["2"]
    });
    assert!(
        carried_out(&session, &mut desk, &mut queue)
            .surfaces()
            .iter()
            .any(|facts| facts.title.as_deref() == Some("window") && facts.mapped),
        "on the next workspace too"
    );

    shell.set_sticky(&window, 0);
    queue.flush().expect("sent");
    session.wait_for(|facts| surface(facts, "window").workspace == Some(2));
    session.perform(Action::Workspace(Direction::Left));
    session.wait_for(|facts| !surface(facts, "window").mapped);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_sent_to_a_workspace_goes_there_and_the_view_stays() {
    let (session, mut desk, mut queue, _qh) = windows("menu-send", one_monitor(2, 2), &["window"]);
    let (shell, window) = (shell(&desk), handle(&desk, "window"));

    shell.send_to_workspace(&window, &workspace(&desk, "perspicax:*:4"));
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| surface(facts, "window").workspace == Some(4));
    assert!(!surface(&facts, "window").mapped, "gone from this one");
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert_eq!(desk.active_workspaces(), ["1"], "the view stays");

    // A sticky window sent is on that one alone from then on.
    shell.set_sticky(&window, 1);
    shell.send_to_workspace(&window, &workspace(&desk, "perspicax:*:1"));
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| surface(facts, "window").workspace == Some(1));
    assert!(surface(&facts, "window").mapped);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_shell_keeps_a_window_on_top_and_lets_it_go() {
    let (session, mut desk, mut queue, _qh) =
        windows("menu-on-top", one_monitor(1, 1), &["kept", "other"]);
    let (shell, kept) = (shell(&desk), handle(&desk, "kept"));

    shell.set_on_top(&kept, 1);
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| surface(facts, "kept").on_top);
    assert!(
        height(&facts, "kept") > height(&facts, "other"),
        "raised as it is kept"
    );
    desk.activate("other");
    let facts = carried_out(&session, &mut desk, &mut queue);
    assert!(
        height(&facts, "kept") > height(&facts, "other"),
        "and in front of one raised since"
    );

    // Kept on top too, the other is raised in front of it; and the first,
    // asked again to be what it is, is left where it is.
    shell.set_on_top(&handle(&desk, "other"), 1);
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| surface(facts, "other").on_top);
    assert!(height(&facts, "other") > height(&facts, "kept"));
    shell.set_on_top(&kept, 1);
    let facts = carried_out(&session, &mut desk, &mut queue);
    assert!(
        height(&facts, "other") > height(&facts, "kept"),
        "kept on top already, so not raised"
    );

    shell.set_on_top(&kept, 0);
    queue.flush().expect("sent");
    session.wait_for(|facts| !surface(facts, "kept").on_top);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_shell_rolls_up_a_window_framed_here_and_not_one_that_draws_its_own_frame() {
    let (session, mut desk, mut queue, qh, _) = shell_on("menu-shade", one_monitor(1, 1));
    open(&session, &mut desk, &mut queue, &qh, "own", false);
    open(&session, &mut desk, &mut queue, &qh, "framed", true);
    let shell = shell(&desk);

    shell.set_shaded(&handle(&desk, "own"), 1);
    shell.set_shaded(&handle(&desk, "framed"), 1);
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| surface(facts, "framed").shaded);
    assert!(
        !surface(&facts, "own").shaded,
        "no titlebar of ours to roll up to"
    );

    shell.set_shaded(&handle(&desk, "framed"), 0);
    queue.flush().expect("sent");
    session.wait_for(|facts| !surface(facts, "framed").shaded);

    session.stop((desk, queue));
}

/// Where each monitor has workspaces of its own, a window is sent among its
/// monitor's: moving it to another monitor is not what a workspace asks.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn per_output_a_window_stays_on_its_monitor_whichever_workspace_is_named() {
    let second = Virtual {
        place: Place::Beside {
            side: Side::RightOf,
            of: "HEADLESS-1".to_owned(),
            offset: 0,
        },
        ..Virtual::numbered(2, (800, 600))
    };
    let monitors = vec![Virtual::numbered(1, (800, 600)), second];
    let (session, mut desk, mut queue, _qh) = windows(
        "menu-per-output",
        backend(Mode::PerOutput, 2, 2, monitors),
        &["window"],
    );
    let (shell, window) = (shell(&desk), handle(&desk, "window"));
    assert_eq!(
        surface(&session.wait_for(|_| true), "window").workspace,
        Some(1)
    );

    shell.send_to_workspace(&window, &workspace(&desk, "perspicax:HEADLESS-2:3"));
    let facts = carried_out(&session, &mut desk, &mut queue);
    assert_eq!(
        surface(&facts, "window").workspace,
        Some(1),
        "another monitor's workspace"
    );

    shell.send_to_workspace(&window, &workspace(&desk, "perspicax:HEADLESS-1:3"));
    queue.flush().expect("sent");
    session.wait_for(|facts| surface(facts, "window").workspace == Some(3));

    session.stop((desk, queue));
}

/// What is asked of a tab behind another is done to its group, through the
/// tab in front, which holds the group's workspace and the rest: set on the
/// tab behind, it would be lost the moment that tab came forward.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tab_behind_another_stands_for_its_group() {
    let (session, desk, queue, _qh) = windows("menu-tabs", one_monitor(2, 1), &["first", "second"]);
    session.perform(Action::TabWithPrevious);
    session.wait_for(|facts| !surface(facts, "first").mapped);
    let (shell, behind) = (shell(&desk), handle(&desk, "first"));

    shell.set_sticky(&behind, 1);
    shell.set_on_top(&behind, 1);
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| {
        let front = surface(facts, "second");
        front.workspace.is_none() && front.on_top
    });
    assert!(!surface(&facts, "first").on_top, "not the tab behind");

    session.perform(Action::CycleTab { forward: true });
    session.wait_for(|facts| {
        let front = surface(facts, "first");
        front.mapped && front.workspace.is_none() && front.on_top
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn nothing_is_done_while_the_session_is_locked_nor_for_a_shell_the_taskbar_or_pager_was_taken_from()
{
    let (session, mut desk, mut queue, _qh) =
        windows("menu-refused", one_monitor(2, 1), &["window"]);
    let (shell, window) = (shell(&desk), handle(&desk, "window"));

    // Locked by a program of its own, as swaylock locks.
    let (mut locker, mut locker_queue, locker_qh, locker_globals) = session.client();
    locker.lock(&locker_globals, &locker_qh);
    until(&mut locker_queue, &mut locker, |locker| locker.locked);
    shell.set_sticky(&window, 1);
    let facts = carried_out(&session, &mut desk, &mut queue);
    assert_eq!(surface(&facts, "window").workspace, Some(1), "locked");
    locker.unlock();
    locker_queue.roundtrip(&mut locker).expect("unlocked");
    let facts = carried_out(&session, &mut desk, &mut queue);
    assert_eq!(
        surface(&facts, "window").workspace,
        Some(1),
        "nor done once unlocked"
    );

    // A workspace is named only to a pager: taken back, its handles name
    // none.
    let second = workspace(&desk, "perspicax:*:2");
    session.command(Command::Protocols(Access::open().with(
        Protocol::Workspace,
        Rule::Only(vec![Program::parse("waybar")]),
    )));
    until(&mut queue, &mut desk, |desk| desk.pager_finished);
    shell.send_to_workspace(&window, &second);
    let facts = carried_out(&session, &mut desk, &mut queue);
    assert_eq!(
        surface(&facts, "window").workspace,
        Some(1),
        "the pager taken back"
    );

    session.command(Command::Protocols(Access::open().with(
        Protocol::ForeignToplevelManagement,
        Rule::Only(vec![Program::parse("waybar")]),
    )));
    until(&mut queue, &mut desk, |desk| desk.taskbar_finished);
    shell.set_sticky(&window, 1);
    let facts = carried_out(&session, &mut desk, &mut queue);
    assert_eq!(
        surface(&facts, "window").workspace,
        Some(1),
        "the taskbar taken back"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// What the shell was last told of the window of this title, once it has
/// been told and `ready` holds of it.
fn told(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    title: &str,
    ready: impl Fn(&WindowTold) -> bool,
) -> WindowTold {
    until(queue, desk, |desk| {
        desk.told_of(title).0.is_some_and(&ready)
    });
    desk.told_of(title).0.expect("told").clone()
}

/// The name of the workspace the shell was told a window is on.
fn named<'a>(desk: &'a Desk, told: &WindowTold) -> Option<&'a str> {
    told.workspace
        .as_ref()
        .and_then(|workspace| desk.workspace_named(workspace))
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_shell_is_told_of_each_window_once_it_holds_its_handles_whichever_it_bound_first() {
    let (session, mut desk, mut queue, _qh) = windows("told-bound", one_monitor(2, 1), &["window"]);
    let window = told(&mut desk, &mut queue, "window", |_| true);
    assert_eq!(window.state, WindowState::empty());
    assert_eq!(
        window.capabilities,
        WindowCapabilities::Workspace,
        "its frame its own: nothing to roll up"
    );
    assert_eq!(named(&desk, &window), Some("1"));

    // A shell coming later is told too, whichever of the three it binds
    // last: perspicax-shell's own order, the channel, the pager, the
    // taskbar; the pager last; and the channel last.
    let orders = [
        ["shell", "pager", "taskbar"],
        ["shell", "taskbar", "pager"],
        ["taskbar", "pager", "shell"],
    ];
    let mut late = Vec::new();
    for order in orders {
        let (mut other, mut other_queue, other_qh, other_globals) = session.client();
        for global in order {
            match global {
                "shell" => other.bind_shell(&other_globals, &other_qh),
                "pager" => other.bind_pager(&other_globals, &other_qh),
                _ => other.bind_taskbar(&other_globals, &other_qh),
            }
            other_queue.roundtrip(&mut other).expect("bound");
        }
        let seen = told(&mut other, &mut other_queue, "window", |told| {
            told.workspace.is_some()
        });
        assert_eq!(
            (seen.state, seen.capabilities, named(&other, &seen)),
            (window.state, window.capabilities, Some("1")),
            "bound in the order {order:?}"
        );
        late.push((other, other_queue));
    }

    // Told again only of a change: one the facts carry, that it is not
    // told, is none.
    session.perform(Action::OpacityDown);
    session.wait_for(|facts| surface(facts, "window").drawn_opacity.is_some());
    carried_out(&session, &mut desk, &mut queue);
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert_eq!(desk.told_of("window").1, 1, "told once");

    session.stop(((desk, queue), late));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn what_a_shell_is_told_of_a_window_follows_the_keys_and_the_window() {
    let (session, mut desk, mut queue, qh, _) = shell_on("told-keys", one_monitor(2, 1));
    open(&session, &mut desk, &mut queue, &qh, "framed", true);
    let all = WindowCapabilities::Workspace | WindowCapabilities::Shade;
    let first = told(&mut desk, &mut queue, "framed", |_| true);
    assert_eq!(first.capabilities, all, "framed here: it rolls up");

    session.perform(Action::ToggleSticky);
    let sticky = told(&mut desk, &mut queue, "framed", |told| {
        told.state.contains(WindowState::Sticky)
    });
    assert_eq!(sticky.workspace, None, "on every workspace, so on none");
    session.perform(Action::ToggleOnTop);
    told(&mut desk, &mut queue, "framed", |told| {
        told.state == WindowState::Sticky | WindowState::OnTop
    });
    session.perform(Action::ToggleShade);
    told(&mut desk, &mut queue, "framed", |told| {
        told.state.contains(WindowState::Shaded)
    });
    session.perform(Action::ToggleSticky);
    let back = told(&mut desk, &mut queue, "framed", |told| {
        !told.state.contains(WindowState::Sticky)
    });
    assert_eq!(named(&desk, &back), Some("1"), "on the one showing");

    // Fullscreen, it unrolls and cannot roll up.
    handle(&desk, "framed").set_fullscreen(None);
    queue.flush().expect("sent");
    let full = told(&mut desk, &mut queue, "framed", |told| {
        !told.capabilities.contains(WindowCapabilities::Shade)
    });
    assert!(!full.state.contains(WindowState::Shaded));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tab_behind_another_is_told_its_groups_state() {
    let (session, mut desk, mut queue, _qh) =
        windows("told-tabs", one_monitor(2, 1), &["first", "second"]);
    session.perform(Action::TabWithPrevious);
    session.wait_for(|facts| !surface(facts, "first").mapped);
    session.perform(Action::ToggleSticky);
    for title in ["first", "second"] {
        told(&mut desk, &mut queue, title, |told| {
            told.state.contains(WindowState::Sticky)
        });
    }

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_shell_bound_at_version_4_is_told_nothing_of_windows() {
    let session = Session::start("told-v4", one_monitor(2, 1));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_shell_v4(&globals, &qh);
    desk.bind_taskbar(&globals, &qh);
    desk.bind_pager(&globals, &qh);
    open(&session, &mut desk, &mut queue, &qh, "window", false);
    session.perform(Action::ToggleSticky);
    session.wait_for(|facts| surface(facts, "window").workspace.is_none());
    carried_out(&session, &mut desk, &mut queue);
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert!(desk.windows_told.is_empty(), "{:?}", desk.windows_told);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_shell_is_told_nothing_while_locked_and_what_changed_once_unlocked() {
    // A person at the desk, so the window's own fullscreen is done while
    // the taskbar's requests, like the shell's, wait for the lock to lift.
    let (session, mut desk, mut queue, qh, _) =
        shell_on("told-locked", one_monitor(2, 1).with_person());
    open(&session, &mut desk, &mut queue, &qh, "framed", true);
    told(&mut desk, &mut queue, "framed", |told| {
        told.capabilities.contains(WindowCapabilities::Shade)
    });

    let (mut locker, mut locker_queue, locker_qh, locker_globals) = session.client();
    locker.lock(&locker_globals, &locker_qh);
    until(&mut locker_queue, &mut locker, |locker| locker.locked);
    desk.windows[0].set_fullscreen(None);
    queue.flush().expect("sent");
    session.wait_for(|facts| surface(facts, "framed").frame.is_empty());
    carried_out(&session, &mut desk, &mut queue);
    queue.roundtrip(&mut desk).expect("the compositor answers");
    assert_eq!(desk.told_of("framed").1, 1, "nothing while locked");

    locker.unlock();
    locker_queue.flush().expect("sent");
    told(&mut desk, &mut queue, "framed", |told| {
        !told.capabilities.contains(WindowCapabilities::Shade)
    });

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// A workspace is named by the pager's handle on it: with the pager taken
/// back there is none to name, and bound again, the new one is named.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_is_told_on_no_workspace_while_the_pager_is_taken_back() {
    let (session, mut desk, mut queue, qh, globals) = shell_on("told-pager", one_monitor(2, 1));
    open(&session, &mut desk, &mut queue, &qh, "window", false);
    let first = told(&mut desk, &mut queue, "window", |told| {
        told.workspace.is_some()
    });

    session.command(Command::Protocols(Access::open().with(
        Protocol::Workspace,
        Rule::Only(vec![Program::parse("waybar")]),
    )));
    until(&mut queue, &mut desk, |desk| desk.pager_finished);
    told(&mut desk, &mut queue, "window", |told| {
        told.workspace.is_none()
    });

    session.command(Command::Protocols(Access::open()));
    queue.roundtrip(&mut desk).expect("the compositor answers");
    desk.bind_pager(&globals, &qh);
    let again = told(&mut desk, &mut queue, "window", |told| {
        told.workspace.is_some()
    });
    assert_ne!(again.workspace, first.workspace, "the new pager's handle");
    assert_eq!(named(&desk, &again), Some("1"));

    session.stop((desk, queue));
}
