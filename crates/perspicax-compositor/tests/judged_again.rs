//! An agent's click or scroll, judged again as the compositor sends it, by a
//! compositor with no screen.
//!
//! The gate judges a node against the facts as the agent read them, and the
//! compositor's loop carries the act out later. In between a window can be
//! raised over the node or a menu open over it, and with the pointer's focus
//! given explicitly a click aimed at the node's surface still reached it, at
//! a point the person saw something else on (issue #158). Now a click or a
//! scroll is judged again, against facts built in the turn that sends it,
//! and refused as the gate would refuse it, naming what covers the node
//! now. Nothing is sent. Focus and typing aim at no pixel, and are not. A
//! raise changes what the person is looking at, and is judged again whole.
//!
//! Each act here is sent to the compositor directly, past the gate, as one
//! the gate judged before the cover opened arrives. Like the other live
//! tests these bind a real Wayland socket, so they need `XDG_RUNTIME_DIR`,
//! and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, configured, until};
use perspicax_compositor::{ActError, Backend, Host};
use perspicax_index::{Action as Verb, HostFacts, PointerButton, Refusal, SurfaceKind, judge};
use perspicax_node::{Rect, SurfaceId, Visibility};
use smithay_client_toolkit::shell::{WaylandSurface as _, wlr_layer::Layer, xdg::XdgSurface as _};
use wayland_client::{EventQueue, QueueHandle};

/// Linux's code for the left button.
const BTN_LEFT: u32 = 0x110;

/// A spot of the first window that a second, cascaded over it at (32, 32),
/// covers, and that a menu opened at (30, 30) covers: in its window space.
const UNDER: Rect = Rect {
    x0: 40.0,
    y0: 40.0,
    x1: 60.0,
    y1: 60.0,
};

/// A spot clear of anything opened after the first window: its corner, in
/// its window space, or anywhere on what opened last.
const CLEAR: Rect = Rect {
    x0: 10.0,
    y0: 10.0,
    x1: 20.0,
    y1: 20.0,
};

/// A spot by a 400x300 window's far corner, in its window space: past its
/// edge if it were taken for global space on a window away from the corner
/// of the desk.
const FAR: Rect = Rect {
    x0: 370.0,
    y0: 270.0,
    x1: 380.0,
    y1: 280.0,
};

const MENU: u32 = 0xff33_6699;

/// One monitor and nobody at it, so nothing but a cover refuses.
fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
}

fn click(at: Rect) -> Verb {
    Verb::Click {
        at,
        button: PointerButton::Left,
    }
}

fn scroll(at: Rect) -> Verb {
    Verb::Scroll {
        at,
        dx: 0.0,
        dy: 15.0,
    }
}

/// The mapped window titled `title`.
fn titled(facts: &HostFacts, title: &str) -> Option<SurfaceId> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title) && surface.mapped)
        .map(|surface| surface.id)
}

/// The mapped popups hanging from `from`, bottom to top.
fn popups_of(facts: &HostFacts, from: SurfaceId) -> Vec<SurfaceId> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.mapped && surface.hangs_from() == Some(from))
        .map(|surface| surface.id)
        .collect()
}

/// The mapped layer surface of `namespace`.
fn layer(facts: &HostFacts, namespace: &str) -> Option<SurfaceId> {
    facts
        .surfaces()
        .iter()
        .find(|surface| {
            surface.mapped
                && matches!(&surface.kind, SurfaceKind::Layer { namespace: named, .. }
                    if named == namespace)
        })
        .map(|surface| surface.id)
}

/// Open a window called `title`, wait until the facts say it is on screen,
/// and return its id.
fn opened(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
) -> SurfaceId {
    desk.open_window(qh, title, title);
    until(queue, desk, |_| {
        titled(&session.facts.read(), title).is_some()
    });
    titled(&session.facts.read(), title).expect("waited for")
}

/// A window raised over a spot of another since the gate judged it: a click
/// or a scroll there is refused, naming the window over it, and reaches
/// neither window. Beside it, and anywhere on the window in front, out to
/// its far corner, they land. Before, both went to the window underneath,
/// through the one over it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_or_scroll_on_a_window_covered_since_is_refused_naming_what_covers_it() {
    let session = session("judged-again-window");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    let under = opened(&session, &mut desk, &mut queue, &qh, "under");
    assert_eq!(
        judge(&session.facts.read(), under, UNDER).visibility,
        Visibility::Visible,
        "what the gate judged"
    );

    let over = opened(&session, &mut desk, &mut queue, &qh, "over");
    let host = Host::new(&session.facts, &session.requests);
    let refused = Err(ActError::OutOfSight(Refusal::Occluded { by: over }));
    assert_eq!(host.act(under, &click(UNDER)).map(|_| ()), refused);
    assert_eq!(host.act(under, &scroll(UNDER)).map(|_| ()), refused);
    queue.roundtrip(&mut desk).expect("a round trip");
    assert!(desk.buttons.is_empty(), "no button reached either window");
    assert_eq!(desk.scrolls, 0, "nor any scroll");

    host.act(under, &click(CLEAR))
        .expect("beside the window over it");
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);
    assert_eq!(desk.pointed.as_ref(), Some(desk.windows[0].wl_surface()));
    host.act(over, &scroll(FAR))
        .expect("on the window in front");
    until(&mut queue, &mut desk, |desk| desk.scrolls == 1);
    assert_eq!(desk.pointed.as_ref(), Some(desk.windows[1].wl_surface()));
    assert_eq!(desk.buttons, [(BTN_LEFT, true), (BTN_LEFT, false)]);

    session.stop((desk, queue));
}

/// Something a raise does not pass, up since the gate judged it: the window
/// stays where it is, and the keyboard does too. Before it came up, the raise
/// went ahead.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_raise_under_something_up_since_the_gate_is_refused_naming_it() {
    let session = session("judged-again-raise");
    let (mut desk, mut queue, qh, _) = session.client();
    let under = opened(&session, &mut desk, &mut queue, &qh, "under");
    let over = opened(&session, &mut desk, &mut queue, &qh, "over");
    let order = |facts: &HostFacts| {
        facts
            .surfaces()
            .iter()
            .filter(|surface| surface.title.is_some())
            .map(|surface| surface.id)
            .collect::<Vec<_>>()
    };

    desk.open_strip(&qh, Layer::Overlay, "cover", 1024, 0xff44_4444);
    until(&mut queue, &mut desk, |_| {
        layer(&session.facts.read(), "cover").is_some()
    });
    let cover = layer(&session.facts.read(), "cover").expect("waited for");
    let host = Host::new(&session.facts, &session.requests);
    let before = host
        .act(over, &Verb::Focus)
        .expect("the keyboard is over's");
    assert_eq!(before.focus_after, Some(over));
    assert_eq!(
        host.act(under, &Verb::Raise).map(|_| ()),
        Err(ActError::OutOfSight(Refusal::Occluded { by: cover }))
    );
    let facts = session.facts.read();
    assert_eq!(order(&facts), [under, over], "nothing was raised");
    assert_eq!(
        host.act(over, &Verb::Focus).expect("still").focus_before,
        Some(over),
        "the keyboard did not move"
    );

    desk.layers.clear();
    queue.roundtrip(&mut desk).expect("destroyed");
    session.wait_for(|facts| layer(facts, "cover").is_none());
    let raised = host
        .act(under, &Verb::Raise)
        .expect("nothing in the way now");
    assert_eq!(raised.focus_after, Some(under));
    assert_eq!(order(&session.facts.read()), [over, under]);

    session.stop((desk, queue));
}

/// A menu opened over a spot of its own window since the gate judged it: a
/// click on the window there is refused, naming the menu, which is what the
/// person sees there. An item of the menu is chosen.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_under_a_menu_opened_since_is_refused_naming_the_menu() {
    let session = session("judged-again-menu");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    let page = opened(&session, &mut desk, &mut queue, &qh, "page");
    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (30, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| !popups_of(facts, page).is_empty());
    let shown = popups_of(&facts, page)[0];
    let host = Host::new(&session.facts, &session.requests);

    assert_eq!(
        host.act(page, &click(UNDER)).map(|_| ()),
        Err(ActError::OutOfSight(Refusal::Occluded { by: shown }))
    );
    queue.roundtrip(&mut desk).expect("a round trip");
    assert!(desk.buttons.is_empty(), "nothing reached the window");

    host.act(shown, &click(CLEAR)).expect("an item chosen");
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);
    assert_eq!(desk.pointed.as_ref(), Some(menu.wl_surface()));

    session.stop((desk, queue));
}

/// A spot reaching past its window's edge is refused as clipped, as the
/// gate refuses a node there: the part of it outside is something else's.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_beyond_its_window_is_refused_as_clipped() {
    let session = session("judged-again-clipped");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    let page = opened(&session, &mut desk, &mut queue, &qh, "page");
    let host = Host::new(&session.facts, &session.requests);

    // Across the 400x300 window's far corner.
    let corner = Rect::new(390.0, 290.0, 410.0, 310.0);
    let refused = Err(ActError::OutOfSight(Refusal::Clipped));
    assert_eq!(host.act(page, &click(corner)).map(|_| ()), refused);
    assert_eq!(host.act(page, &scroll(corner)).map(|_| ()), refused);
    queue.roundtrip(&mut desk).expect("a round trip");
    assert!(desk.buttons.is_empty() && desk.scrolls == 0);

    session.stop((desk, queue));
}

/// Focus and typing aim at no pixel, and are not judged again (dave, #158):
/// a window wholly covered since is still given the keyboard, and typed
/// into, where a click on it is refused. Typing checks where the keyboard
/// is instead.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn focus_and_typing_are_not_judged_again() {
    let session = session("judged-again-focus");
    let (mut desk, mut queue, qh, _) = session.client();
    let page = opened(&session, &mut desk, &mut queue, &qh, "page");
    // Over the whole of the 400x300 window, and taking no keys.
    desk.open_strip(&qh, Layer::Overlay, "cover", 400, 0xff20_2020);
    until(&mut queue, &mut desk, |_| {
        layer(&session.facts.read(), "cover").is_some()
    });
    let cover = layer(&session.facts.read(), "cover").expect("waited for");
    let host = Host::new(&session.facts, &session.requests);

    assert_eq!(
        host.act(page, &click(CLEAR)).map(|_| ()),
        Err(ActError::OutOfSight(Refusal::Occluded { by: cover }))
    );
    host.act(page, &Verb::Focus)
        .expect("focus is not judged again");
    host.act(
        page,
        &Verb::Type {
            text: "x".to_owned(),
        },
    )
    .expect("nor is typing");

    session.stop((desk, queue));
}
