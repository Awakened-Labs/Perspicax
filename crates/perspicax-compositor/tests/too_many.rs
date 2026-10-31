//! More of something than smithay can add up, from a client that asks for
//! it, by a compositor with no screen.
//!
//! #154 holds each number a client sends to the edge of the world. Smithay
//! 0.7.0 still adds them up across surfaces: the room each exclusive layer on
//! a monitor leaves is carried to the next, raw, in `i32`. Issue #226: enough
//! layers overflowed that, so the compositor keeps a monitor to
//! [`MOST_LAYERS_ON_A_MONITOR`], and tells the next one it is closed.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Said, Session, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{HostFacts, SurfaceKind};
use perspicax_policy::{MOST_LAYERS_ON_A_MONITOR, Place, Side};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{EventQueue, QueueHandle};

const PANEL: u32 = 0xff22_2222;

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
