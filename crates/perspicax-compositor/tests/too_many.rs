//! More of something than smithay can add up, from a client that asks for
//! it, by a compositor with no screen.
//!
//! #154 holds each number a client sends to the edge of the world. Smithay
//! 0.7.0 still adds them up across surfaces: the room each exclusive layer on
//! a monitor leaves is carried to the next, raw, in `i32`. Issue #226: enough
//! layers overflowed that, so the compositor keeps a monitor to
//! [`MOST_LAYERS_ON_A_MONITOR`], and tells the next one it is closed. And a
//! chain of popups adds up two ways, which part once it is long enough to
//! saturate, so it goes no deeper than [`DEEPEST_POPUP`], and the next one is
//! told it is done. A build with `capture` checks at every commit that the
//! two ways agree.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use std::sync::{Mutex, MutexGuard, PoisonError};

use common::{Desk, Said, Session, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{HostFacts, SurfaceKind};
use perspicax_policy::{DEEPEST_POPUP, MOST_LAYERS_ON_A_MONITOR, Place, Side};
use smithay_client_toolkit::shell::{
    WaylandSurface as _,
    xdg::{XdgSurface as _, popup::Popup},
};
use wayland_client::{EventQueue, QueueHandle, protocol::wl_surface::WlSurface};
use wayland_protocols::xdg::shell::client::{xdg_positioner, xdg_surface::XdgSurface};

const PANEL: u32 = 0xff22_2222;
const MENU: u32 = 0xff33_6699;

/// Taken for as long as a test fills a monitor. Filling one is most of what
/// these tests cost, some seconds of a compositor's time, and a session lasts
/// 30 s: several side by side, on a loaded machine, could run out of it.
static FILLING: Mutex<()> = Mutex::new(());

fn filling() -> MutexGuard<'static, ()> {
    FILLING.lock().unwrap_or_else(PoisonError::into_inner)
}

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

/// How many layers the facts know.
fn layers(facts: &HostFacts) -> usize {
    facts
        .surfaces()
        .iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::Layer { .. }))
        .count()
}

/// Whether the facts know a layer the client named `namespace`.
fn knows(facts: &HostFacts, namespace: &str) -> bool {
    facts.surfaces().iter().any(|surface| {
        matches!(&surface.kind, SurfaceKind::Layer { namespace: named, .. } if named == namespace)
    })
}

/// Have the newest layer ask for all the numbers it can, which is what adds
/// up fastest: the most room, past the widest margin, once its first commit
/// is taken in.
fn ask_for_the_most(desk: &mut Desk) {
    let nth = desk.layers.len() - 1;
    desk.leave_undrawn(nth);
    let panel = &desk.layers[nth].0;
    panel.set_exclusive_zone(i32::MAX);
    panel.set_margin(i32::MAX, 0, 0, 0);
    panel.commit();
}

/// Open `count` panels that ask for all the numbers they can, and wait until
/// the compositor has taken them in.
fn open_many(desk: &mut Desk, queue: &mut EventQueue<Desk>, qh: &QueueHandle<Desk>, count: usize) {
    for nth in 0..count {
        desk.open_panel(qh, "panel", 1, PANEL);
        ask_for_the_most(desk);
        // A client's socket holds only so much.
        if nth % 16 == 0 {
            queue.roundtrip(desk).expect("taken in");
        }
    }
    queue.roundtrip(desk).expect("taken in");
}

/// Open a panel named `namespace` past the others, and have it told whatever
/// it is told. Returns which layer of the client's it is.
fn open_one_more(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    namespace: &str,
) -> usize {
    desk.open_panel(qh, namespace, 1, PANEL);
    ask_for_the_most(desk);
    queue.roundtrip(desk).expect("taken in");
    desk.layers.len() - 1
}

/// Whether the `nth` layer has been told it is closed.
fn closed(desk: &Desk, nth: usize) -> bool {
    desk.said_to(desk.layers[nth].0.wl_surface())
        .contains(&Said::Closed)
}

/// #226: a monitor keeps as many layers as it can add up, each asking for
/// all it can, and the next is told it is closed, and is in no one's facts.
/// Those it kept are told nothing.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_layer_past_what_a_monitor_keeps_is_told_it_is_closed() {
    let _one_at_a_time = filling();
    let session = session("too-many-layers");
    let (mut desk, mut queue, qh, _) = session.client();
    open_many(&mut desk, &mut queue, &qh, MOST_LAYERS_ON_A_MONITOR);
    session.wait_for(|facts| layers(facts) == MOST_LAYERS_ON_A_MONITOR);
    assert!(
        !(0..MOST_LAYERS_ON_A_MONITOR).any(|nth| closed(&desk, nth)),
        "a monitor keeps this many"
    );

    let more = open_one_more(&mut desk, &mut queue, &qh, "one-too-many");

    assert!(closed(&desk, more), "told it will not be shown");
    assert!(
        !(0..MOST_LAYERS_ON_A_MONITOR).any(|nth| closed(&desk, nth)),
        "none is closed to make way"
    );
    let facts = session.wait_for(|_| true);
    assert_eq!(layers(&facts), MOST_LAYERS_ON_A_MONITOR);
    assert!(!knows(&facts, "one-too-many"));

    session.stop((desk, queue));
}

/// #226: one that goes makes room for the next.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_layer_that_goes_makes_room_for_the_next() {
    let _one_at_a_time = filling();
    let session = session("too-many-room");
    let (mut desk, mut queue, qh, _) = session.client();
    open_many(&mut desk, &mut queue, &qh, MOST_LAYERS_ON_A_MONITOR);
    session.wait_for(|facts| layers(facts) == MOST_LAYERS_ON_A_MONITOR);

    desk.layers.remove(0);
    queue.roundtrip(&mut desk).expect("taken in");
    session.wait_for(|facts| layers(facts) == MOST_LAYERS_ON_A_MONITOR - 1);

    let next = open_one_more(&mut desk, &mut queue, &qh, "next");
    assert!(!closed(&desk, next), "there was room for it");
    session.wait_for(|facts| knows(facts, "next"));

    session.stop((desk, queue));
}

/// #226: a panel its client unmapped has no place kept for it, and asked for
/// again on a monitor that filled while it was away, it is told it is closed.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_asked_for_again_on_a_full_monitor_is_told_it_is_closed() {
    let _one_at_a_time = filling();
    let session = session("too-many-again");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_panel(&qh, "away", 40, PANEL);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    session.wait_for(|facts| knows(facts, "away"));
    let away = desk.layers[0].0.wl_surface().clone();
    away.attach(None, 0, 0);
    away.commit();
    queue.roundtrip(&mut desk).expect("taken in");
    session.wait_for(|facts| !knows(facts, "away"));
    open_many(&mut desk, &mut queue, &qh, MOST_LAYERS_ON_A_MONITOR);
    session.wait_for(|facts| layers(facts) == MOST_LAYERS_ON_A_MONITOR);
    assert!(!closed(&desk, 0), "told nothing while it is away");

    desk.layers[0].0.commit();
    queue.roundtrip(&mut desk).expect("taken in");

    assert!(closed(&desk, 0), "no room to come back to");
    let facts = session.wait_for(|_| true);
    assert_eq!(layers(&facts), MOST_LAYERS_ON_A_MONITOR);
    assert!(!knows(&facts, "away"));

    session.stop((desk, queue));
}

/// #226: a client that takes no notice of being told, and draws in its closed
/// layer and opens a menu from it all the same, changes nothing for anyone.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_closed_layer_that_carries_on_changes_nothing() {
    let _one_at_a_time = filling();
    let session = session("too-many-carries-on");
    let (mut desk, mut queue, qh, _) = session.client();
    open_many(&mut desk, &mut queue, &qh, MOST_LAYERS_ON_A_MONITOR);
    session.wait_for(|facts| layers(facts) == MOST_LAYERS_ON_A_MONITOR);
    let more = open_one_more(&mut desk, &mut queue, &qh, "one-too-many");
    assert!(closed(&desk, more));

    let surface = desk.layers[more].0.wl_surface().clone();
    desk.layers[more].0.commit();
    desk.paint(&surface, (40, 1), PANEL);
    surface.commit();
    let panel = desk.layers[more].0.clone();
    let menu = desk.open_panel_popup(&qh, &panel, (0, 0), (40, 40));
    desk.paint(menu.wl_surface(), (40, 40), PANEL);
    menu.wl_surface().commit();
    queue.roundtrip(&mut desk).expect("taken in");

    let (witness, mut witness_queue, ..) = session.client();
    let mut witness = witness;
    witness_queue.roundtrip(&mut witness).expect("still up");
    let facts = session.wait_for(|_| true);
    assert_eq!(layers(&facts), MOST_LAYERS_ON_A_MONITOR);
    assert!(!knows(&facts, "one-too-many"));

    drop(menu);
    desk.layers.remove(more);
    queue.roundtrip(&mut desk).expect("taken in");
    session.stop(((desk, queue), (witness, witness_queue)));
}

/// #226: what a monitor keeps is its own: another, with room, takes a layer
/// the full one would not. The sum smithay carries is a monitor's alone.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn another_monitor_keeps_its_own() {
    let _one_at_a_time = filling();
    let session = session("too-many-another");
    session.command(Command::Plug(Virtual {
        name: "HEADLESS-2".to_owned(),
        size: (1280, 1024),
        place: Place::Beside {
            side: Side::RightOf,
            of: "HEADLESS-1".to_owned(),
            offset: 0,
        },
    }));
    session.wait_for(|facts| facts.outputs().len() == 2);
    let (mut desk, mut queue, qh, _) = session.client();
    until(&mut queue, &mut desk, |desk| {
        desk.has_monitor("HEADLESS-1") && desk.has_monitor("HEADLESS-2")
    });
    for nth in 0..MOST_LAYERS_ON_A_MONITOR {
        desk.open_panel_on(&qh, "panel", 1, PANEL, "HEADLESS-1");
        ask_for_the_most(&mut desk);
        if nth % 16 == 0 {
            queue.roundtrip(&mut desk).expect("taken in");
        }
    }
    queue.roundtrip(&mut desk).expect("taken in");
    session.wait_for(|facts| layers(facts) == MOST_LAYERS_ON_A_MONITOR);

    desk.open_panel_on(&qh, "elsewhere", 1, PANEL, "HEADLESS-2");
    queue.roundtrip(&mut desk).expect("taken in");
    let elsewhere = desk.layers.len() - 1;
    assert!(!closed(&desk, elsewhere), "this monitor has room");
    session.wait_for(|facts| knows(facts, "elsewhere"));

    desk.open_panel_on(&qh, "here", 1, PANEL, "HEADLESS-1");
    queue.roundtrip(&mut desk).expect("taken in");
    let here = desk.layers.len() - 1;
    assert!(closed(&desk, here), "this one has none");

    session.stop((desk, queue));
}

/// How many popups the facts know.
fn popups(facts: &HostFacts) -> usize {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.is_popup())
        .count()
}

/// Open a popup of `parent` as far from it as a client can ask, one way or
/// back the other, and draw it, as a client's next menu does once it is
/// answered.
fn hang(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    parent: &XdgSurface,
    back: bool,
) -> Popup {
    let (far, gravity) = if back {
        (i32::MAX, xdg_positioner::Gravity::BottomRight)
    } else {
        (i32::MIN, xdg_positioner::Gravity::BottomLeft)
    };
    let positioner = desk.positioner((far, 0), (65536, 1));
    positioner.set_gravity(gravity);
    positioner.set_offset(far, 0);
    let popup = desk.open_popup_by(qh, parent, &positioner);
    positioner.destroy();
    queue.roundtrip(desk).expect("taken in");
    draw(desk, queue, popup.wl_surface());
    popup
}

/// Draw in a popup that has been answered, or told it is done.
fn draw(desk: &mut Desk, queue: &mut EventQueue<Desk>, surface: &WlSurface) {
    desk.paint(surface, (1, 1), MENU);
    surface.commit();
    queue.roundtrip(desk).expect("taken in");
}

/// Hang a chain from `first`, as long as a chain goes, out as far as it can
/// go and back, so the sums down it and up it have the most reason to part.
/// Returns its popups, `first`'s child first.
fn chain_from(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    first: &XdgSurface,
    from: usize,
) -> Vec<Popup> {
    let mut chain: Vec<Popup> = Vec::new();
    for at in from..DEEPEST_POPUP {
        let parent = chain
            .last()
            .map_or_else(|| first.clone(), |popup| popup.xdg_surface().clone());
        chain.push(hang(desk, queue, qh, &parent, at % 2 == 1));
    }
    chain
}

/// A window, drawn, and a chain of popups from it as long as a chain goes.
fn window_and_chain(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
) -> Vec<Popup> {
    desk.open_window(qh, "page", "page");
    until(queue, desk, |desk| desk.drawn == 1);
    let window = desk.windows[0].xdg_surface().clone();
    let chain = chain_from(desk, queue, qh, &window, 0);
    session.wait_for(|facts| popups(facts) == DEEPEST_POPUP);
    assert_eq!(desk.popups_done, 0, "a chain goes this deep");
    assert_eq!(desk.popups_configured, DEEPEST_POPUP);
    chain
}

/// #226: a chain goes as deep as it can add up, each popup as far from the
/// last as a client can ask, and the next is told it is done, as it opens,
/// and is in no one's facts. A build with `capture` asks at every commit
/// whether the popup hangs where smithay would draw it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_popup_past_how_deep_a_chain_goes_is_told_it_is_done() {
    let session = session("too-deep-popups");
    let (mut desk, mut queue, qh, _) = session.client();
    let chain = window_and_chain(&session, &mut desk, &mut queue, &qh);

    let last = chain.last().expect("a chain").xdg_surface().clone();
    let more = hang(&mut desk, &mut queue, &qh, &last, false);

    assert_eq!(desk.popups_done, 1, "told as it opens");
    assert_eq!(desk.popups_configured, DEEPEST_POPUP, "and not answered");
    let facts = session.wait_for(|_| true);
    assert_eq!(popups(&facts), DEEPEST_POPUP);

    drop(more);
    session.stop((desk, queue));
}

/// #226: a popup of one refused is refused too, however far the client goes,
/// and a child of it never comes up at the top of its window's popups, which
/// is where smithay files a popup whose parent it cannot find.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_popup_of_one_refused_is_refused() {
    let session = session("too-deep-children");
    let (mut desk, mut queue, qh, _) = session.client();
    let chain = window_and_chain(&session, &mut desk, &mut queue, &qh);

    let mut refused: Vec<Popup> = Vec::new();
    for told in 1..=3 {
        let parent = refused.last().map_or_else(
            || chain.last().expect("a chain").xdg_surface().clone(),
            |popup| popup.xdg_surface().clone(),
        );
        refused.push(hang(&mut desk, &mut queue, &qh, &parent, false));
        assert_eq!(desk.popups_done, told);
    }

    assert_eq!(desk.popups_configured, DEEPEST_POPUP);
    let facts = session.wait_for(|_| true);
    assert_eq!(popups(&facts), DEEPEST_POPUP);

    drop(refused);
    session.stop((desk, queue));
}

/// #226: a chain from a panel is counted as a window's is, the panel's menu
/// being the first.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_chain_from_a_panel_goes_no_deeper() {
    let session = session("too-deep-panel");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_panel(&qh, "panel", 40, PANEL);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let panel = desk.layers[0].0.clone();
    let menu = desk.open_panel_popup(&qh, &panel, (0, 0), (40, 40));
    queue.roundtrip(&mut desk).expect("taken in");
    draw(&mut desk, &mut queue, menu.wl_surface());

    let chain = chain_from(&mut desk, &mut queue, &qh, &menu.xdg_surface().clone(), 1);
    session.wait_for(|facts| popups(facts) == DEEPEST_POPUP);
    assert_eq!(desk.popups_done, 0, "a chain goes this deep");

    let last = chain.last().expect("a chain").xdg_surface().clone();
    let more = hang(&mut desk, &mut queue, &qh, &last, false);
    assert_eq!(desk.popups_done, 1);
    assert_eq!(popups(&session.wait_for(|_| true)), DEEPEST_POPUP);

    drop((more, chain, menu));
    session.stop((desk, queue));
}

/// #226: a popup that goes makes room at the end of its chain for another.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_popup_that_goes_makes_room_at_the_end_of_its_chain() {
    let session = session("too-deep-room");
    let (mut desk, mut queue, qh, _) = session.client();
    let mut chain = window_and_chain(&session, &mut desk, &mut queue, &qh);

    drop(chain.pop());
    queue.roundtrip(&mut desk).expect("taken in");
    session.wait_for(|facts| popups(facts) == DEEPEST_POPUP - 1);

    let last = chain.last().expect("a chain").xdg_surface().clone();
    let next = hang(&mut desk, &mut queue, &qh, &last, false);
    assert_eq!(desk.popups_done, 0, "there was room for it");
    session.wait_for(|facts| popups(facts) == DEEPEST_POPUP);

    drop(next);
    session.stop((desk, queue));
}
