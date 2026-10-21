//! A window kept on top of the others, by a compositor with no screen.
//!
//! Headless nothing is drawn, but the facts list the surfaces bottom to top
//! as they would be drawn: a window kept on top is in front of every other
//! window, whichever is raised or has the keyboard, under the panels, and
//! under the fullscreen window in use. What it covers is judged covered by
//! it. Cycling focus walks the windows as it did before any was kept on top,
//! and the keyboard goes back where it went then. The binding is carried out
//! with [`Session::perform`]; the titlebar's button needs a seat.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, until};
use perspicax_compositor::{Backend, Virtual};
use perspicax_index::{HostFacts, SurfaceFacts, SurfaceKind, judge};
use perspicax_node::{Rect, Visibility};
use perspicax_policy::{Access, Action, Direction, Grid, Mode, Shape};
use smithay_client_toolkit::shell::wlr_layer::Layer;
use wayland_client::{EventQueue, QueueHandle};
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
    windows_on(name, titles, Backend::headless((800, 600)))
}

fn windows_on(
    name: &str,
    titles: &[&str],
    backend: Backend,
) -> (Session, Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let session = Session::start(name, backend);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    for (nth, title) in titles.iter().enumerate() {
        desk.open_window(&qh, title, title);
        until(&mut queue, &mut desk, |desk| desk.drawn == nth + 1);
        until(&mut queue, &mut desk, |desk| desk.task(title).is_some());
    }
    let last = titles.last().expect("a window at least");
    session.wait_for(|facts| {
        stacked(facts).len() == titles.len() && focused_last(facts).as_deref() == Some(*last)
    });
    (session, desk, queue, qh)
}

/// "kept", kept on top, under "other", raised since.
fn kept_under_another(name: &str) -> (Session, Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (session, mut desk, mut queue, qh) = windows(name, &["kept", "other"]);
    activate(&session, &mut desk, &mut queue, "kept");
    toggle(&session, "kept", true);
    activate(&session, &mut desk, &mut queue, "other");
    (session, desk, queue, qh)
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
    let (session, mut desk, mut queue, _qh) =
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
