//! A window kept on top of the others, by a compositor with no screen.
//!
//! Headless nothing is drawn, but the facts list the surfaces bottom to top
//! as they would be drawn: a window kept on top is in front of every other
//! window, whichever is raised or has the keyboard, under the panels, and
//! under the fullscreen window in use. What it covers is judged covered by
//! it, a dialog of it is kept on top with it, and a tab group is kept as
//! one. Cycling focus walks the windows as it did before any was kept on
//! top, and the keyboard goes back where it went then. A menu of the window
//! in use is in front of a window kept on top, though its window is not,
//! and under the panels (#234): in the facts, under the pointer and in a
//! picture. A dialog another program opens for a window kept on top, as the
//! desktop portal opens its file chooser, is kept on top with it too (#235),
//! and let go once the window is no longer named to it. The binding is
//! carried out with [`Session::perform`]; the titlebar's button needs a
//! seat. X11's dialogs and menus are in `xwayland.rs`.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, configured, hand_over, menu_with_the_keyboard_on, until};
use perspicax_compositor::{Backend, Virtual};
use perspicax_index::{HostFacts, SurfaceFacts, SurfaceKind, judge};
use perspicax_node::{Rect, SurfaceId, Visibility};
use perspicax_policy::{Access, Action, Direction, Grid, Mode, Shape};
use smithay_client_toolkit::shell::{
    wlr_layer::Layer,
    xdg::{XdgSurface as _, popup::Popup},
};
use wayland_client::{EventQueue, QueueHandle, globals::GlobalList};
use wayland_protocols::xdg::foreign::zv2::client::{
    zxdg_exported_v2::ZxdgExportedV2, zxdg_imported_v2::ZxdgImportedV2,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::State;

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

/// The titles of the windows on screen, back to front.
fn stacked(facts: &HostFacts) -> Vec<String> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.mapped && matches!(surface.kind, SurfaceKind::Window))
        .filter_map(|surface| surface.title.clone())
        .collect()
}

/// The title of the window that took the keyboard last.
fn focused_last(facts: &HostFacts) -> Option<String> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.focused_at.is_some())
        .max_by_key(|surface| surface.focused_at)?
        .title
        .clone()
}

/// Where `over` overlaps `under`, in `under`'s own space, as a node of it is
/// given.
fn overlap(facts: &HostFacts, under: &str, over: &str) -> Rect {
    let (below, above) = (
        surface(facts, under).geometry,
        surface(facts, over).geometry,
    );
    let shared = Rect {
        x0: below.x0.max(above.x0),
        y0: below.y0.max(above.y0),
        x1: below.x1.min(above.x1),
        y1: below.y1.min(above.y1),
    };
    assert!(
        shared.x1 - shared.x0 > 20.0 && shared.y1 - shared.y0 > 20.0,
        "cascaded, they overlap: {below:?} and {above:?}"
    );
    Rect {
        x0: shared.x0 - below.x0 + 5.0,
        y0: shared.y0 - below.y0 + 5.0,
        x1: shared.x0 - below.x0 + 15.0,
        y1: shared.y0 - below.y0 + 15.0,
    }
}

/// Give the window of this title the keyboard and the top of the stack, as
/// a taskbar does, and wait until it has them.
fn activate(session: &Session, desk: &mut Desk, queue: &mut EventQueue<Desk>, title: &str) {
    desk.activate(title);
    until(queue, desk, |desk| {
        desk.task(title)
            .is_some_and(|task| task.is(State::Activated))
    });
    session.wait_for(|facts| focused_last(facts).as_deref() == Some(title));
}

/// Keep the focused window on top, or let it go, and wait until it is.
fn toggle(session: &Session, title: &str, kept: bool) -> HostFacts {
    session.perform(Action::ToggleOnTop);
    session.wait_for(|facts| surface(facts, title).on_top == kept)
}

/// One monitor, with two workspaces side by side.
fn two_workspaces() -> Backend {
    Backend::Headless {
        outputs: vec![Virtual::numbered(1, (800, 600))],
        workspaces: Shape {
            mode: Mode::Spanning,
            grid: Grid {
                columns: 2,
                rows: 1,
                wrap: false,
            },
        },
        access: Access::open(),
        person: false,
        opacity: Default::default(),
        agent: Default::default(),
    }
}

/// Windows of these titles, opened in turn, with a taskbar to raise them
/// by. The last has the keyboard and the top of the stack.
fn windows(name: &str, titles: &[&str]) -> (Session, Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (session, desk, queue, qh, _) = windows_on(name, titles, Backend::headless((800, 600)));
    (session, desk, queue, qh)
}

/// [`windows`], on `backend`, with the globals for binding more.
fn windows_on(
    name: &str,
    titles: &[&str],
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
    desk.bind_taskbar(&globals, &qh);
    for (nth, title) in titles.iter().enumerate() {
        desk.open_window(&qh, title, title);
        until(&mut queue, &mut desk, |desk| desk.has_drawn(nth));
        until(&mut queue, &mut desk, |desk| desk.task(title).is_some());
    }
    let last = titles.last().expect("a window at least");
    session.wait_for(|facts| {
        stacked(facts).len() == titles.len() && focused_last(facts).as_deref() == Some(*last)
    });
    (session, desk, queue, qh, globals)
}

/// "kept", kept on top, under "other", raised since.
fn kept_under_another(name: &str) -> (Session, Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (session, desk, queue, qh, _) = kept_under_another_with_globals(name);
    (session, desk, queue, qh)
}

/// [`kept_under_another`], with the globals for binding more.
fn kept_under_another_with_globals(
    name: &str,
) -> (
    Session,
    Desk,
    EventQueue<Desk>,
    QueueHandle<Desk>,
    GlobalList,
) {
    let (session, mut desk, mut queue, qh, globals) =
        windows_on(name, &["kept", "other"], Backend::headless((800, 600)));
    activate(&session, &mut desk, &mut queue, "kept");
    toggle(&session, "kept", true);
    activate(&session, &mut desk, &mut queue, "other");
    (session, desk, queue, qh, globals)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_kept_on_top_stays_in_front_when_another_is_raised() {
    let (session, desk, queue, _qh) = kept_under_another("on-top-raised");

    let facts = session.wait_for(|_| true);
    assert_eq!(
        stacked(&facts),
        ["other", "kept"],
        "raised and with the keyboard, the other is still behind it"
    );
    assert!(surface(&facts, "kept").on_top);
    assert!(!surface(&facts, "other").on_top);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_control_under_a_window_kept_on_top_is_occluded_by_it() {
    let (session, desk, queue, _qh) = kept_under_another("on-top-occludes");

    let facts = session.wait_for(|_| true);
    let node = overlap(&facts, "other", "kept");
    assert_eq!(
        judge(&facts, surface(&facts, "other").id, node).visibility,
        Visibility::Occluded {
            by: surface(&facts, "kept").id
        }
    );
    let node = overlap(&facts, "kept", "other");
    assert_eq!(
        judge(&facts, surface(&facts, "kept").id, node).visibility,
        Visibility::Visible,
        "nothing covers the window kept on top"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn of_two_windows_kept_on_top_the_one_raised_last_is_in_front() {
    let (session, mut desk, mut queue, _qh) = windows("on-top-two", &["first", "second", "other"]);
    activate(&session, &mut desk, &mut queue, "first");
    toggle(&session, "first", true);
    activate(&session, &mut desk, &mut queue, "second");
    toggle(&session, "second", true);
    activate(&session, &mut desk, &mut queue, "other");
    assert_eq!(
        stacked(&session.wait_for(|_| true)),
        ["other", "first", "second"]
    );

    activate(&session, &mut desk, &mut queue, "first");
    assert_eq!(
        stacked(&session.wait_for(|_| true)),
        ["other", "second", "first"],
        "raised, it comes in front of the other kept on top"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_let_go_is_in_front_of_the_others_until_another_is_raised() {
    let (session, mut desk, mut queue, _qh) = windows("on-top-let-go", &["kept", "other"]);
    activate(&session, &mut desk, &mut queue, "kept");
    toggle(&session, "kept", true);
    activate(&session, &mut desk, &mut queue, "other");
    activate(&session, &mut desk, &mut queue, "kept");

    let facts = toggle(&session, "kept", false);
    assert_eq!(
        stacked(&facts),
        ["other", "kept"],
        "let go, it is not lost behind the window raised over it while it was kept"
    );
    activate(&session, &mut desk, &mut queue, "other");
    assert_eq!(stacked(&session.wait_for(|_| true)), ["kept", "other"]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_fullscreen_window_in_use_covers_a_window_kept_on_top_and_goes_back_under_it_when_left() {
    let (session, mut desk, mut queue, _qh) = kept_under_another("on-top-fullscreen");

    desk.task("other")
        .expect("a known window")
        .handle
        .set_fullscreen(None);
    until(&mut queue, &mut desk, |desk| {
        desk.offered == Some((800, 600))
    });
    let facts = session.wait_for(|facts| height(facts, "other") > height(facts, "kept"));
    assert!(surface(&facts, "kept").on_top, "{:?}", stacked(&facts));

    activate(&session, &mut desk, &mut queue, "kept");
    let facts = session.wait_for(|facts| height(facts, "kept") > height(facts, "other"));
    assert_eq!(stacked(&facts), ["other", "kept"]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_stays_over_a_window_kept_on_top() {
    let (session, mut desk, mut queue, qh) = kept_under_another("on-top-panel");

    desk.open_strip(&qh, Layer::Top, "panel", 40, 0xffee_8822);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let facts = session.wait_for(|facts| {
        facts.surfaces().iter().any(|surface| {
            matches!(&surface.kind, SurfaceKind::Layer { namespace, .. } if namespace == "panel")
        })
    });
    let panel = facts
        .surfaces()
        .iter()
        .position(|surface| matches!(surface.kind, SurfaceKind::Layer { .. }))
        .expect("waited for");
    assert!(
        panel > height(&facts, "kept"),
        "the panel is over the window kept on top"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_kept_on_top_is_in_front_again_once_restored_and_once_its_workspace_is_back() {
    let (session, mut desk, mut queue, _qh, _) =
        windows_on("on-top-parked", &["kept", "other"], two_workspaces());
    activate(&session, &mut desk, &mut queue, "kept");
    toggle(&session, "kept", true);
    activate(&session, &mut desk, &mut queue, "other");

    desk.task("kept")
        .expect("a known window")
        .handle
        .set_minimized();
    queue.flush().expect("sent");
    session.wait_for(|facts| !surface(facts, "kept").mapped);
    activate(&session, &mut desk, &mut queue, "kept");
    activate(&session, &mut desk, &mut queue, "other");
    let facts = session.wait_for(|_| true);
    assert_eq!(stacked(&facts), ["other", "kept"], "restored");
    assert!(surface(&facts, "kept").on_top);

    session.perform(Action::Workspace(Direction::Right));
    session.wait_for(|facts| stacked(facts).is_empty());
    session.perform(Action::Workspace(Direction::Left));
    session.wait_for(|facts| stacked(facts).len() == 2);
    activate(&session, &mut desk, &mut queue, "other");
    assert_eq!(
        stacked(&session.wait_for(|_| true)),
        ["other", "kept"],
        "its workspace back"
    );

    session.stop((desk, queue));
}

/// The windows cycling focus gives the keyboard to, press by press, with
/// "b" kept on top or not.
fn cycled(name: &str, keep: bool) -> Vec<String> {
    let (session, mut desk, mut queue, _qh) = windows(name, &["a", "b", "c"]);
    activate(&session, &mut desk, &mut queue, "b");
    if keep {
        toggle(&session, "b", true);
    }
    let mut focused = Vec::new();
    for _ in 0..4 {
        // Three windows, and each press gives the keyboard to another.
        let before = focused_last(&session.wait_for(|_| true));
        session.perform(Action::CycleFocus);
        let facts = session.wait_for(|facts| focused_last(facts) != before);
        if keep {
            assert_eq!(
                stacked(&facts).last().map(String::as_str),
                Some("b"),
                "the window kept on top stays in front: {:?}",
                stacked(&facts)
            );
        }
        focused.push(focused_last(&facts).expect("a window has the keyboard"));
    }
    session.stop((desk, queue));
    focused
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn cycling_focus_walks_every_window_as_it_would_with_none_kept_on_top() {
    let walked = cycled("on-top-cycle-kept", true);
    assert_eq!(walked, ["a", "c", "b", "a"], "every window in turn");
    assert_eq!(walked, cycled("on-top-cycle-none", false));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_keyboard_goes_back_to_the_window_raised_last_as_before() {
    let (session, mut desk, mut queue, qh) = kept_under_another("on-top-keyboard");
    desk.open_window(&qh, "passing", "passing");
    until(&mut queue, &mut desk, |desk| desk.task("passing").is_some());
    session.wait_for(|facts| focused_last(facts).as_deref() == Some("passing"));

    desk.windows.remove(2);
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| {
        !facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("passing"))
            && focused_last(facts).as_deref() != Some("passing")
    });
    assert_eq!(
        focused_last(&facts).as_deref(),
        Some("other"),
        "to the window raised last, not the one kept on top in front of it"
    );
    assert_eq!(stacked(&facts), ["other", "kept"]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_dialog_of_a_window_kept_on_top_opens_in_front_of_it_and_is_kept_on_top_with_it() {
    let (session, mut desk, mut queue, qh) = kept_under_another("on-top-dialog");
    desk.open_dialog(&qh, "dialog", "kept", 0);
    until(&mut queue, &mut desk, |desk| desk.drawn == 3);
    let facts = session.wait_for(|facts| focused_last(facts).as_deref() == Some("dialog"));

    assert_eq!(stacked(&facts), ["other", "kept", "dialog"]);
    assert!(surface(&facts, "dialog").on_top, "with its window");
    activate(&session, &mut desk, &mut queue, "other");
    assert_eq!(
        stacked(&session.wait_for(|_| true)),
        ["other", "kept", "dialog"],
        "the other raised, both are still in front of it"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_given_a_parent_kept_on_top_after_it_shows_is_kept_on_top_with_it() {
    let (session, mut desk, mut queue, qh) = kept_under_another("on-top-parent-later");
    desk.open_window(&qh, "dialog", "kept");
    until(&mut queue, &mut desk, |desk| desk.drawn == 3);
    let facts = session.wait_for(|facts| focused_last(facts).as_deref() == Some("dialog"));
    assert_eq!(stacked(&facts), ["other", "dialog", "kept"], "not yet");

    let parent = desk.windows[0].clone();
    desk.windows[2].set_parent(Some(&parent));
    queue.flush().expect("sent");

    // Nothing is drawn after: the parent alone has to be heard.
    let facts = session.wait_for(|facts| surface(facts, "dialog").on_top);
    assert_eq!(stacked(&facts), ["other", "kept", "dialog"]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_dialog_is_kept_on_top_with_its_window_and_let_go_with_it() {
    let (session, mut desk, mut queue, qh) = windows("on-top-dialog-follows", &["window", "other"]);
    activate(&session, &mut desk, &mut queue, "window");
    desk.open_dialog(&qh, "dialog", "window", 0);
    until(&mut queue, &mut desk, |desk| desk.drawn == 3);
    session.wait_for(|facts| focused_last(facts).as_deref() == Some("dialog"));
    // Raised over its own dialog, as ever.
    activate(&session, &mut desk, &mut queue, "window");

    toggle(&session, "window", true);
    session.wait_for(|facts| surface(facts, "dialog").on_top);
    activate(&session, &mut desk, &mut queue, "other");
    assert_eq!(
        stacked(&session.wait_for(|_| true)),
        ["other", "dialog", "window"],
        "both in front of the other, as they were stacked"
    );

    activate(&session, &mut desk, &mut queue, "window");
    let facts = toggle(&session, "window", false);
    assert!(!surface(&facts, "dialog").on_top, "let go with it");

    session.stop((desk, queue));
}

/// A second program, the portal, with the window its dialog is for: the
/// export by that window's program, and the portal's import of it.
struct Portal {
    desk: Desk,
    queue: EventQueue<Desk>,
    exported: ZxdgExportedV2,
    imported: ZxdgImportedV2,
}

/// The portal opening "dialog" for `desk`'s first window, "kept", which its
/// program exported and handed it, as Firefox hands the portal the window a
/// file chooser is for; and "dialog" on screen.
fn portal_dialog_for_kept(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    globals: &GlobalList,
) -> Portal {
    desk.bind_foreign(globals, qh);
    let (exported, handle) = hand_over(queue, desk, qh, 0);
    let (mut portal, mut portal_queue, portal_qh, portal_globals) = session.client();
    portal.bind_foreign(&portal_globals, &portal_qh);
    let imported = portal.import(&portal_qh, &handle);
    portal.open_dialog_for(&portal_qh, "dialog", "portal", &imported);
    until(&mut portal_queue, &mut portal, |portal| portal.has_drawn(0));
    Portal {
        desk: portal,
        queue: portal_queue,
        exported,
        imported,
    }
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_dialog_another_program_opens_for_a_window_kept_on_top_opens_in_front_of_it_and_is_kept_on_top_with_it()
 {
    let (session, mut desk, mut queue, qh, globals) =
        kept_under_another_with_globals("on-top-foreign-dialog");
    let portal = portal_dialog_for_kept(&session, &mut desk, &mut queue, &qh, &globals);
    let facts = session.wait_for(|facts| focused_last(facts).as_deref() == Some("dialog"));

    assert_eq!(stacked(&facts), ["other", "kept", "dialog"]);
    assert!(surface(&facts, "dialog").on_top, "with its window");
    activate(&session, &mut desk, &mut queue, "other");
    assert_eq!(
        stacked(&session.wait_for(|_| true)),
        ["other", "kept", "dialog"],
        "the other raised, both are still in front of it"
    );

    session.stop(((desk, queue), (portal.desk, portal.queue)));
}

/// The issue's case, with a person at the seat: the portal, another program
/// the person's press never reached, opens its file chooser for the window
/// kept on top, which is in use. It takes the keys as it opens, so it is in
/// front of that window, not just beneath it (#235).
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_dialog_another_program_opens_for_a_window_kept_on_top_in_use_opens_in_front_of_it_with_a_person_at_the_seat()
 {
    let (session, mut desk, mut queue, qh, globals) = windows_on(
        "on-top-foreign-seated",
        &["kept", "other"],
        Backend::headless((800, 600)).with_person(),
    );
    activate(&session, &mut desk, &mut queue, "kept");
    toggle(&session, "kept", true);
    let portal = portal_dialog_for_kept(&session, &mut desk, &mut queue, &qh, &globals);

    let facts = session.wait_for(|facts| focused_last(facts).as_deref() == Some("dialog"));
    assert_eq!(stacked(&facts), ["other", "kept", "dialog"]);
    assert!(surface(&facts, "dialog").on_top, "with its window");

    session.stop(((desk, queue), (portal.desk, portal.queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_dialog_another_program_opened_for_a_window_kept_on_top_is_let_go_once_the_window_is_no_longer_exported()
 {
    let (session, mut desk, mut queue, qh, globals) =
        kept_under_another_with_globals("on-top-foreign-unexported");
    let portal = portal_dialog_for_kept(&session, &mut desk, &mut queue, &qh, &globals);
    session.wait_for(|facts| surface(facts, "dialog").on_top);

    portal.exported.destroy();
    queue.flush().expect("sent");

    let facts = session.wait_for(|facts| !surface(facts, "dialog").on_top);
    assert_eq!(
        stacked(&facts),
        ["other", "dialog", "kept"],
        "among the others, behind the window it no longer hangs from"
    );

    session.stop(((desk, queue), (portal.desk, portal.queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_dialog_another_program_opened_for_a_window_kept_on_top_is_let_go_once_its_program_lets_the_window_go()
 {
    let (session, mut desk, mut queue, qh, globals) =
        kept_under_another_with_globals("on-top-foreign-unimported");
    let portal = portal_dialog_for_kept(&session, &mut desk, &mut queue, &qh, &globals);
    session.wait_for(|facts| surface(facts, "dialog").on_top);

    portal.imported.destroy();
    portal.queue.flush().expect("sent");

    let facts = session.wait_for(|facts| !surface(facts, "dialog").on_top);
    assert_eq!(stacked(&facts), ["other", "dialog", "kept"]);

    session.stop(((desk, queue), (portal.desk, portal.queue)));
}

/// "first" kept on top, with "second" made a tab of it and in front, under
/// "other", raised since.
fn a_group_kept_on_top(name: &str) -> (Session, Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (session, mut desk, mut queue, qh) = windows(name, &["first", "second", "other"]);
    activate(&session, &mut desk, &mut queue, "first");
    toggle(&session, "first", true);
    activate(&session, &mut desk, &mut queue, "second");
    session.perform(Action::TabWithPrevious);
    session.wait_for(|facts| !surface(facts, "first").mapped);
    activate(&session, &mut desk, &mut queue, "other");
    (session, desk, queue, qh)
}

/// A group is kept on top as one, as it rolls up as one (#60): the tab
/// coming forward takes it, and none behind keeps it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tab_group_kept_on_top_stays_in_front_and_a_tab_coming_forward_comes_kept_on_top() {
    let (session, mut desk, mut queue, _qh) = a_group_kept_on_top("on-top-tabs");

    let facts = session.wait_for(|_| true);
    assert_eq!(
        stacked(&facts),
        ["other", "second"],
        "joining it, kept on top"
    );
    assert!(surface(&facts, "second").on_top);
    assert!(!surface(&facts, "first").on_top, "no tab behind keeps it");

    activate(&session, &mut desk, &mut queue, "second");
    session.perform(Action::CycleTab { forward: true });
    session.wait_for(|facts| surface(facts, "first").mapped);
    activate(&session, &mut desk, &mut queue, "other");
    let facts = session.wait_for(|_| true);
    assert_eq!(
        stacked(&facts),
        ["other", "first"],
        "coming forward, kept on top"
    );
    assert!(surface(&facts, "first").on_top);
    assert!(!surface(&facts, "second").on_top);

    session.stop((desk, queue));
}

/// As a tab taken out of a group rolled up comes out unrolled (#60), one
/// taken out of a group kept on top comes out among the other windows, and
/// the group is still kept.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tab_taken_out_of_a_group_kept_on_top_comes_out_among_the_others() {
    let (session, mut desk, mut queue, _qh) = a_group_kept_on_top("on-top-detach");
    activate(&session, &mut desk, &mut queue, "second");

    session.perform(Action::DetachTab);

    let facts = session.wait_for(|facts| surface(facts, "first").mapped);
    assert!(!surface(&facts, "second").on_top, "taken out, it is let go");
    assert!(surface(&facts, "first").on_top, "the group is still kept");
    assert_eq!(
        stacked(&facts).last().map(String::as_str),
        Some("first"),
        "with the keyboard, the one taken out is behind the group: {:?}",
        stacked(&facts)
    );

    session.stop((desk, queue));
}

/// Under click-to-focus, a click lands on what is drawn, not on what was
/// raised last: the stand-in hand finds windows from the clients' buffers,
/// so this needs the `capture` feature.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_where_a_window_kept_on_top_overlaps_another_lands_on_it() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Focus, FocusModel, Mods};

    let (session, mut desk, mut queue, _qh) = windows("on-top-click", &["kept", "other"]);
    activate(&session, &mut desk, &mut queue, "kept");
    toggle(&session, "kept", true);
    activate(&session, &mut desk, &mut queue, "other");
    let facts = session.wait_for(|_| true);
    let kept = surface(&facts, "kept").geometry;
    let other = surface(&facts, "other").geometry;
    let at = (
        ((kept.x1.min(other.x1) + other.x0.max(kept.x0)) / 2.0) as i32,
        ((kept.y1.min(other.y1) + other.y0.max(kept.y0)) / 2.0) as i32,
    );
    session.command(Command::FocusPolicy(Focus {
        model: FocusModel::Click,
        autoraise: false,
    }));

    session.command(Command::Click {
        at,
        button: Button::Left,
        mods: Mods::default(),
    });

    session.wait_for(|facts| focused_last(facts).as_deref() == Some("kept"));

    session.stop((desk, queue));
}

/// How big a menu here is.
const MENU: (u32, u32) = (120, 80);

/// Where a surface is in the facts, counted from the bottom.
fn place(facts: &HostFacts, id: SurfaceId) -> usize {
    facts
        .surfaces()
        .iter()
        .position(|surface| surface.id == id)
        .expect("a surface of that id")
}

/// The popups the facts list.
fn popups(facts: &HostFacts) -> Vec<&SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::Popup { .. }))
        .collect()
}

/// A popup of "other", this client's second window, drawn in `colour` over
/// the middle of "kept", once the facts describe it: where "kept" is in
/// front of "other", so the popup is either over "kept" or under it. A
/// popup without the grab, as a tooltip is. Placed in "other"'s window
/// geometry, as xdg-shell places a popup.
fn popup_over_kept(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    colour: u32,
) -> (Popup, SurfaceId) {
    let facts = session.wait_for(|_| true);
    let (kept, other) = (
        surface(&facts, "kept").geometry,
        surface(&facts, "other").geometry,
    );
    let at = (
        (kept.x0 + 40.0 - other.x0) as i32,
        (kept.y0 + 40.0 - other.y0) as i32,
    );
    let popup = desk.open_popup(qh, desk.windows[1].xdg_surface(), at, MENU);
    configured(desk, queue, 1);
    desk.paint(popup.wl_surface(), MENU, colour);
    popup.wl_surface().commit();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| popups(facts).iter().any(|popup| popup.mapped));
    let shown = popups(&facts)[0];
    let (area, kept) = (shown.geometry, surface(&facts, "kept").geometry);
    assert!(
        area.x0 > kept.x0 && area.y0 > kept.y0 && area.x1 < kept.x1 && area.y1 < kept.y1,
        "the popup is not over the window kept on top: {area:?} and {kept:?}"
    );
    (popup, shown.id)
}

/// The middle of the popup `id`, in its own space and on the desk.
fn middle_of(facts: &HostFacts, id: SurfaceId) -> (Rect, (i32, i32)) {
    let area = facts.surface(id).expect("the popup").geometry;
    let (x, y) = ((area.x0 + area.x1) / 2.0, (area.y0 + area.y1) / 2.0);
    (
        Rect::new(x - 5.0, y - 5.0, x + 5.0, y + 5.0),
        (x as i32, y as i32),
    )
}

/// Where the popup `id` is over "kept", as a node of "kept" is given: in
/// "kept"'s own space.
fn under_the_popup(facts: &HostFacts, id: SurfaceId) -> Rect {
    let (node, _) = middle_of(facts, id);
    let kept = surface(facts, "kept").geometry;
    Rect::new(
        node.x0 - kept.x0,
        node.y0 - kept.y0,
        node.x1 - kept.x0,
        node.y1 - kept.y0,
    )
}

/// Issue #234: a tooltip of the window in use, a popup that takes no grab,
/// opens in front of a window kept on top, where the window it hangs from is
/// behind that one, and covers what is under it there. Once the window kept
/// on top is in use, the tooltip goes back with its own window, behind it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tooltip_of_the_window_in_use_opens_in_front_of_a_window_kept_on_top() {
    let (session, mut desk, mut queue, qh) = kept_under_another("on-top-tooltip");

    let (_popup, tooltip) = popup_over_kept(&session, &mut desk, &mut queue, &qh, 0xffee_8822);

    let facts = session.wait_for(|facts| place(facts, tooltip) > height(facts, "kept"));
    assert_eq!(popups(&facts).len(), 1, "listed once: {facts:?}");
    assert_eq!(
        judge(
            &facts,
            surface(&facts, "kept").id,
            under_the_popup(&facts, tooltip)
        )
        .visibility,
        Visibility::Occluded { by: tooltip },
        "the window kept on top is covered where the tooltip is"
    );

    activate(&session, &mut desk, &mut queue, "kept");
    session.wait_for(|facts| place(facts, tooltip) < height(facts, "kept"));

    session.stop((desk, queue));
}

/// Issue #234: a right-click menu of the window in use, which takes the
/// keyboard, opens in front of a window kept on top. Its keys are its
/// window's, so its window is still the one in use (#97).
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_of_the_window_in_use_opens_in_front_of_a_window_kept_on_top() {
    // A person at the seat, for a menu to take the keyboard.
    let session = Session::start("on-top-menu", Backend::headless((800, 600)).with_person());
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    let with_the_keys = |title: &'static str| {
        let facts = &session.facts;
        move |_: &Desk| focused_last(&facts.read()).as_deref() == Some(title)
    };
    desk.open_window(&qh, "kept", "kept");
    until(&mut queue, &mut desk, with_the_keys("kept"));
    session.perform(Action::ToggleOnTop);
    session.wait_for(|facts| surface(facts, "kept").on_top);
    // Its client has the keys, so its next window takes them.
    desk.open_window(&qh, "other", "other");
    until(&mut queue, &mut desk, with_the_keys("other"));
    let facts = session.facts.read();
    let other = surface(&facts, "other").id;

    let _menu = menu_with_the_keyboard_on(&session, &mut desk, &mut queue, &qh, &ear, other, 1);

    let facts = session.wait_for(|facts| popups(facts).iter().any(|popup| popup.mapped));
    let menu = popups(&facts)[0].id;
    let (area, kept) = (
        facts.surface(menu).expect("the menu").geometry,
        surface(&facts, "kept").geometry,
    );
    assert!(
        area.x1 < kept.x1 && area.y1 < kept.y1,
        "the menu is not over the window kept on top: {area:?} and {kept:?}"
    );
    let facts = session.wait_for(|facts| place(facts, menu) > height(facts, "kept"));
    assert_eq!(
        judge(
            &facts,
            surface(&facts, "kept").id,
            under_the_popup(&facts, menu)
        )
        .visibility,
        Visibility::Occluded { by: menu },
        "the window kept on top is covered where the menu is"
    );

    session.stop((desk, queue));
}

/// Issue #234: in front of every window, but under the panels, as every
/// window but the fullscreen one in use is.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_of_the_window_in_use_stays_under_the_panels() {
    let (session, mut desk, mut queue, qh) = kept_under_another("on-top-menu-panel");
    desk.open_strip(&qh, Layer::Top, "panel", 40, 0xff00_0000);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let (_popup, tooltip) = popup_over_kept(&session, &mut desk, &mut queue, &qh, 0xffee_8822);

    let facts = session.wait_for(|facts| place(facts, tooltip) > height(facts, "kept"));
    let panel = facts
        .surfaces()
        .iter()
        .position(|surface| matches!(surface.kind, SurfaceKind::Layer { .. }))
        .expect("the panel");
    assert!(
        place(&facts, tooltip) < panel,
        "the popup is over the panel: {facts:?}"
    );

    session.stop((desk, queue));
}

/// Issue #234: what is drawn is what is pointed at. A person's click where
/// the menu of the window in use is over a window kept on top lands on the
/// menu.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_on_a_menu_of_the_window_in_use_over_a_window_kept_on_top_reaches_it() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Mods};

    let (session, mut desk, mut queue, qh, globals) =
        kept_under_another_with_globals("on-top-menu-click");
    desk.bind_pointer(&globals, &qh);
    let (popup, tooltip) = popup_over_kept(&session, &mut desk, &mut queue, &qh, 0xffee_8822);
    let facts = session.wait_for(|facts| place(facts, tooltip) > height(facts, "kept"));
    let (_, at) = middle_of(&facts, tooltip);

    session.command(Command::Click {
        at,
        button: Button::Left,
        mods: Mods::default(),
    });

    until(&mut queue, &mut desk, |desk| !desk.buttons.is_empty());
    assert_eq!(
        desk.pointed.as_ref(),
        Some(popup.wl_surface()),
        "the click landed elsewhere"
    );

    session.stop((desk, queue));
}

/// Issue #234: a picture shows the menu of the window in use over a window
/// kept on top, as the screen does.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_picture_shows_the_menu_of_the_window_in_use_over_a_window_kept_on_top() {
    use perspicax_compositor::Host;
    use perspicax_index::ShotTarget;

    const BLUE: u32 = 0xff33_6699;
    const ORANGE: u32 = 0xffee_8822;
    let session = Session::start("on-top-menu-picture", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_coloured(&qh, "kept", "kept", BLUE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "other", "other");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    until(&mut queue, &mut desk, |desk| desk.task("kept").is_some());
    session.wait_for(|facts| focused_last(facts).as_deref() == Some("other"));
    activate(&session, &mut desk, &mut queue, "kept");
    toggle(&session, "kept", true);
    activate(&session, &mut desk, &mut queue, "other");
    let (_popup, tooltip) = popup_over_kept(&session, &mut desk, &mut queue, &qh, ORANGE);
    let facts = session.wait_for(|facts| place(facts, tooltip) > height(facts, "kept"));
    let area = facts.surface(tooltip).expect("the popup").geometry;

    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");

    let pixel = |x: f64, y: f64| {
        let at = ((y as usize) * shot.width as usize + x as usize) * 4;
        <[u8; 4]>::try_from(&shot.rgba[at..at + 4]).expect("a pixel")
    };
    let (x, y) = ((area.x0 + area.x1) / 2.0, (area.y0 + area.y1) / 2.0);
    assert_eq!(pixel(x, y), common::rgba(ORANGE), "the menu, over kept");
    assert_eq!(
        pixel(area.x1 + 5.0, y),
        common::rgba(BLUE),
        "the window kept on top, beside the menu"
    );

    session.stop((desk, queue));
}
