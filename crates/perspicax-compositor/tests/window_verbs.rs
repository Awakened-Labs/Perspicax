//! An agent's window verbs, as the compositor carries them out: `Close`
//! and `Forward` through `Host::act`, the path `window_close` and
//! `tab_forward` take, and `Focus` and `Type` addressed to a window by its
//! surface, the path `window_focus` and `window_type` take (#38).
//!
//! A tab behind another comes forward in its group's place; a close reaches
//! the client as a request; and while the session is locked neither is done.
//! A window this client draws has no accessibility tree, as foot and a game
//! have none, and it is focused and typed into by surface, without being
//! raised. The gate that decides whether an agent may ask is
//! `perspicax-index`'s and is tested there; what is tested of it here is
//! that the facts a real session publishes give it the answers it should:
//! a window hidden, rolled up, minimized or wholly covered is refused.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Session, heard, until};
use perspicax_compositor::{ActError, Backend, Host};
use perspicax_index::{Action as Verb, HostFacts, Refusal, WindowVerb, check_window};
use perspicax_node::SurfaceId;
use perspicax_policy::{Action, Grid, Mode, Shape};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::Proxy as _;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

fn id(facts: &HostFacts, title: &str) -> SurfaceId {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| surface.id)
        .expect("a window of that title")
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn forward_brings_a_tab_behind_to_the_front_of_its_group() {
    let session = session("verbs-forward");
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    session.perform(Action::TabWithPrevious);
    let facts = session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.behind_tab.is_some())
    });
    let (first, second) = (id(&facts, "first"), id(&facts, "second"));
    assert_eq!(facts.surface(first).unwrap().tabs, [first, second]);
    assert_eq!(facts.surface(first).unwrap().behind_tab, Some(second));

    let dispatched = host.act(first, &Verb::Forward).expect("dispatched");
    assert_eq!(dispatched.focus_after, Some(first));
    session.wait_for(|facts| {
        facts.surface(second).unwrap().behind_tab == Some(first)
            && facts.surface(first).unwrap().mapped
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn close_asks_the_client() {
    let session = session("verbs-close");
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "first", "org.example.First");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| !facts.surfaces().is_empty());
    let first = id(&facts, "first");
    assert_eq!(
        facts.surface(first).unwrap().app_id.as_deref(),
        Some("org.example.First")
    );
    assert_eq!(facts.surface(first).unwrap().workspace, Some(1));

    host.act(first, &Verb::Close).expect("dispatched");
    until(&mut queue, &mut desk, |desk| desk.asked_to_close == 1);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn neither_is_done_while_locked() {
    let session = session("verbs-locked");
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let first = id(
        &session.wait_for(|facts| !facts.surfaces().is_empty()),
        "first",
    );

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    assert_eq!(host.act(first, &Verb::Close), Err(ActError::Locked));
    assert_eq!(host.act(first, &Verb::Forward), Err(ActError::Locked));
    queue.roundtrip(&mut desk).expect("dispatch");
    assert_eq!(desk.asked_to_close, 0);

    desk.unlock();
    session.stop((desk, queue));
}

/// The keyboard's verbs, by surface, as the gate is asked them.
fn keys() -> [WindowVerb; 2] {
    [WindowVerb::Focus, WindowVerb::Type("x".to_owned())]
}

/// Where `surface` is in the stack, counted from the bottom.
fn height(facts: &HostFacts, surface: SurfaceId) -> usize {
    facts
        .surfaces()
        .iter()
        .position(|candidate| candidate.id == surface)
        .expect("in the facts")
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_with_no_tree_is_focused_and_typed_into_by_surface_and_not_raised() {
    let session = session("verbs-keys");
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_window(&qh, "lower", "lower");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "upper", "upper");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let facts = session.wait_for(|facts| facts.surfaces().len() == 2);
    let (lower, upper) = (id(&facts, "lower"), id(&facts, "upper"));
    assert!(height(&facts, lower) < height(&facts, upper));

    // Partly covered, it is in sight, and focusing it gives it the keyboard
    // and leaves it where it is in the stack.
    for verb in keys() {
        assert!(check_window(&facts, lower, &verb).is_ok());
    }
    let dispatched = host.act(lower, &Verb::Focus).expect("dispatched");
    assert_eq!(dispatched.focus_after, Some(lower));
    let facts = session.facts.read();
    assert!(
        height(&facts, lower) < height(&facts, upper),
        "focusing does not raise"
    );

    // Typed into, it hears the keys; the other window, without the
    // keyboard, is refused them.
    host.act(
        lower,
        &Verb::Type {
            text: "hi".to_owned(),
        },
    )
    .expect("typed");
    until(&mut queue, &mut desk, |_| heard(&ear).typed() == "hi");
    assert_eq!(heard(&ear).entered, Some(desk.windows[0].wl_surface().id()));
    assert_eq!(
        host.act(
            upper,
            &Verb::Type {
                text: "no".to_owned()
            }
        ),
        Err(ActError::FocusElsewhere {
            focused: Some(lower)
        })
    );

    // Covered wholly, by the other window maximized over it, it is refused
    // as occluded by that window, and can still be closed.
    host.act(upper, &Verb::Focus).expect("dispatched");
    session.perform(Action::ToggleMaximize);
    let facts = session.wait_for(|facts| {
        let (under, over) = (facts.surface(lower), facts.surface(upper));
        under
            .zip(over)
            .is_some_and(|(under, over)| over.geometry.intersect(under.geometry) == under.geometry)
    });
    for verb in keys() {
        assert_eq!(
            check_window(&facts, lower, &verb).unwrap_err(),
            Refusal::Occluded { by: upper }
        );
    }
    assert!(check_window(&facts, lower, &WindowVerb::Close).is_ok());

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_rolled_up_or_minimized_is_not_focused_or_typed_into_by_surface() {
    let session = session("verbs-keys-hidden");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_framed(&qh, "framed");
    until(&mut queue, &mut desk, |desk| desk.has_drawn(0));
    let facts = session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.frame.len() == 4)
    });
    let framed = id(&facts, "framed");

    session.perform(Action::Shade);
    let facts = session.wait_for(|facts| facts.surface(framed).is_some_and(|it| it.shaded));
    for verb in keys() {
        assert_eq!(
            check_window(&facts, framed, &verb).unwrap_err(),
            Refusal::RolledUp
        );
    }
    assert!(check_window(&facts, framed, &WindowVerb::Close).is_ok());

    session.perform(Action::Unshade);
    let facts = session.wait_for(|facts| facts.surface(framed).is_some_and(|it| !it.shaded));
    for verb in keys() {
        assert!(check_window(&facts, framed, &verb).is_ok());
    }

    session.perform(Action::Minimize);
    let facts = session.wait_for(|facts| facts.surface(framed).is_some_and(|it| !it.mapped));
    for verb in keys() {
        assert_eq!(
            check_window(&facts, framed, &verb).unwrap_err(),
            Refusal::Unmapped
        );
    }
    assert!(check_window(&facts, framed, &WindowVerb::Close).is_ok());

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_on_a_workspace_not_showing_is_not_focused_or_typed_into_by_surface() {
    let mut backend = Backend::headless((1280, 1024));
    if let Backend::Headless { workspaces, .. } = &mut backend {
        *workspaces = Shape {
            mode: Mode::Spanning,
            grid: Grid {
                columns: 2,
                rows: 1,
                wrap: false,
            },
        };
    }
    let session = Session::start("verbs-keys-workspace", backend);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let first = id(
        &session.wait_for(|facts| !facts.surfaces().is_empty()),
        "first",
    );

    session.perform(Action::GoToWorkspace(2));
    let facts = session.wait_for(|facts| {
        facts
            .surface(first)
            .is_some_and(|it| it.off_workspace.is_some())
    });
    for verb in keys() {
        assert_eq!(
            check_window(&facts, first, &verb).unwrap_err(),
            Refusal::OtherWorkspace { workspace: 1 }
        );
    }

    session.stop((desk, queue));
}
