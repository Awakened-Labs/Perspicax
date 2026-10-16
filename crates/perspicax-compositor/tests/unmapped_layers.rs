//! Layer surfaces their clients unmap, by attaching no buffer to them.
//!
//! wlr-layer-shell says a layer surface with a null buffer attached is
//! unmapped: it cannot be shown, and it returns to the state it had right
//! after `get_layer_surface`. Its client still holds it, and may map it
//! again. Issue #144: perspicax kept such a panel in its monitor's layers,
//! so it held its room and the keyboard, an agent could still reach it, and
//! it could never be shown again. These tests unmap panels and launchers and
//! check that each goes as one destroyed does, and that its client is told
//! nothing while it is away. #102 is the same for windows
//! (`tests/unmapped.rs`).
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Said, Session, configured, heard, until};
#[cfg(feature = "capture")]
use perspicax_compositor::Command;
use perspicax_compositor::{ActError, Backend, Host, Virtual};
use perspicax_index::{Action as Verb, HostFacts, PointerButton, SurfaceFacts, SurfaceKind};
use perspicax_node::{Rect, SurfaceId};
#[cfg(feature = "capture")]
use perspicax_policy::{Button, Mods};
use perspicax_policy::{Place, Side};
use smithay_client_toolkit::shell::WaylandSurface as _;
#[cfg(feature = "capture")]
use smithay_client_toolkit::shell::wlr_layer::Layer;
use wayland_client::{EventQueue, Proxy as _, QueueHandle};

const PANEL: u32 = 0xff22_2222;
const LAUNCHER: u32 = 0xff44_4444;
const MENU: u32 = 0xff33_6699;
/// How tall each panel is, and how much room it keeps for itself.
const PANEL_HEIGHT: u32 = 40;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

/// A session with a person at the seat, where a window asking to be
/// maximized is.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).with_person())
}

/// The layer surface the client named `namespace`.
fn layer_named<'a>(facts: &'a HostFacts, namespace: &str) -> Option<&'a SurfaceFacts> {
    facts.surfaces().iter().find(|surface| {
        matches!(&surface.kind, SurfaceKind::Layer { namespace: named, .. } if named == namespace)
    })
}

/// Open a panel named `namespace`, wait until it has drawn and the facts
/// say it is on screen, and return its id.
fn panel_up(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    namespace: &str,
) -> SurfaceId {
    desk.open_panel(qh, namespace, PANEL_HEIGHT, PANEL);
    let drawn = desk.layers_drawn + 1;
    until(queue, desk, |desk| desk.layers_drawn == drawn);
    let facts =
        session.wait_for(|facts| layer_named(facts, namespace).is_some_and(|panel| panel.mapped));
    layer_id(&facts, namespace)
}

/// The id of the layer surface named `namespace`, which the facts know.
fn layer_id(facts: &HostFacts, namespace: &str) -> SurfaceId {
    layer_named(facts, namespace).expect("waited for").id
}

/// Unmap the `nth` layer as layer-shell has a client do it: attach no
/// buffer, and commit. Sent, and taken in, before this returns.
fn unmap_layer(desk: &mut Desk, queue: &mut EventQueue<Desk>, nth: usize) {
    let surface = desk.layers[nth].0.wl_surface();
    surface.attach(None, 0, 0);
    surface.commit();
    queue.roundtrip(desk).expect("the commit taken in");
}

/// How many configures the `nth` layer has been sent.
fn layer_configures(desk: &Desk, nth: usize) -> usize {
    desk.said_to(desk.layers[nth].0.wl_surface())
        .iter()
        .filter(|said| matches!(said, Said::LayerConfigure(_)))
        .count()
}

/// #144: a panel its client unmaps gives its room back. A maximized window,
/// fitted below the panel, fills the whole monitor again, and the panel's
/// client is told nothing while it is away.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_its_client_unmaps_gives_its_room_back() {
    let session = seated("unmap-layer-room");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "filling", "filling");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.windows[0].set_maximized();
    until(&mut queue, &mut desk, |desk| {
        desk.offered == Some((1280, 1024))
    });
    panel_up(&session, &mut desk, &mut queue, &qh, "panel");
    until(&mut queue, &mut desk, |desk| {
        desk.offered == Some((1280, 1024 - PANEL_HEIGHT))
    });
    let told = layer_configures(&desk, 0);

    unmap_layer(&mut desk, &mut queue, 0);
    until(&mut queue, &mut desk, |desk| {
        desk.offered == Some((1280, 1024))
    });
    assert_eq!(
        layer_configures(&desk, 0),
        told,
        "told something while unmapped"
    );

    session.stop((desk, queue));
}

/// #144: a panel its client unmaps leaves the facts, and an agent's click
/// aimed at it finds nothing there, as if it had gone.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_its_client_unmaps_leaves_the_facts_and_no_agent_reaches_it() {
    let session = session("unmap-layer-facts");
    let (mut desk, mut queue, qh, _) = session.client();
    let gone = panel_up(&session, &mut desk, &mut queue, &qh, "panel");

    unmap_layer(&mut desk, &mut queue, 0);
    session.wait_for(|facts| facts.surface(gone).is_none());
    let host = Host::new(&session.facts, &session.requests);
    let clicked = host.act(
        gone,
        &Verb::Click {
            at: Rect::new(10.0, 10.0, 20.0, 20.0),
            button: PointerButton::Left,
        },
    );
    assert_eq!(clicked.err(), Some(ActError::NoSuchSurface(gone.0)));

    session.stop((desk, queue));
}

/// #144: a launcher holding the keyboard, unmapped by its client, gives it
/// to the window beneath, as one that goes does. Headless too: the launcher
/// took the keyboard for itself, headless or not, and the keys typed next
/// would otherwise go to a surface nobody can see.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_its_client_unmaps_gives_the_keyboard_to_the_window_beneath() {
    let session = session("unmap-layer-keyboard");
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_window(&qh, "beneath", "beneath");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let beneath = desk.windows[0].wl_surface().id();
    desk.open_launcher(&qh, 200, LAUNCHER);
    let launcher = desk.layers[0].0.wl_surface().id();
    until(&mut queue, &mut desk, |desk| {
        desk.layers_drawn == 1 && heard(&ear).entered.as_ref() == Some(&launcher)
    });
    let told = layer_configures(&desk, 0);

    unmap_layer(&mut desk, &mut queue, 0);
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&beneath)
    });
    assert_eq!(
        layer_configures(&desk, 0),
        told,
        "told something while unmapped"
    );

    session.stop((desk, queue));
}

/// #144: a panel's open menu is dismissed when its client unmaps the panel,
/// and a menu given to the panel while it is away is dismissed at once: a
/// menu needs its panel on screen.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_menus_of_a_panel_its_client_unmaps_are_dismissed() {
    let session = session("unmap-layer-menus");
    let (mut desk, mut queue, qh, _) = session.client();
    let panel = panel_up(&session, &mut desk, &mut queue, &qh, "panel");
    let layer = desk.layers[0].0.clone();
    let menu = desk.open_panel_popup(&qh, &layer, (20, 40), (100, 150));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (100, 150), MENU);
    menu.wl_surface().commit();
    queue.flush().expect("sent");
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.kind == SurfaceKind::Popup { from: panel } && surface.mapped)
    });

    unmap_layer(&mut desk, &mut queue, 0);
    until(&mut queue, &mut desk, |desk| desk.popups_done == 1);
    let _late = desk.open_panel_popup(&qh, &layer, (20, 40), (100, 150));
    until(&mut queue, &mut desk, |desk| desk.popups_done == 2);

    session.stop((desk, queue));
}

/// A panel that never drew was never mapped, so a null buffer attached to it
/// unmaps nothing: it stays as it was, and its first picture shows it, under
/// the id it had.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_null_buffer_on_a_panel_that_never_drew_changes_nothing() {
    let session = session("unmap-layer-undrawn");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_panel(&qh, "panel", PANEL_HEIGHT, PANEL);
    desk.leave_undrawn(0);
    until(&mut queue, &mut desk, |desk| layer_configures(desk, 0) == 1);
    let facts = session.wait_for(|facts| layer_named(facts, "panel").is_some());
    let waiting = layer_id(&facts, "panel");

    unmap_layer(&mut desk, &mut queue, 0);
    desk.draw_layer(0);
    queue.flush().expect("sent");
    session.wait_for(|facts| facts.surface(waiting).is_some_and(|panel| panel.mapped));
    assert_eq!(
        layer_configures(&desk, 0),
        1,
        "configured again, as if it had been unmapped"
    );

    session.stop((desk, queue));
}

/// #144: a panel destroyed after its client unmapped it is forgotten
/// without fuss: the next panel is mapped as any is.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_destroyed_after_its_client_unmapped_it_is_forgotten_quietly() {
    let session = session("unmap-layer-destroyed");
    let (mut desk, mut queue, qh, _) = session.client();
    let gone = panel_up(&session, &mut desk, &mut queue, &qh, "panel");
    unmap_layer(&mut desk, &mut queue, 0);
    session.wait_for(|facts| facts.surface(gone).is_none());

    desk.layers.clear();
    queue.roundtrip(&mut desk).expect("destroyed");
    let next = panel_up(&session, &mut desk, &mut queue, &qh, "next");
    assert_ne!(next, gone);

    session.stop((desk, queue));
}

/// #144: a panel unmapped on a monitor that is unplugged while it is away
/// is told it is closed, as one on screen there is: there is nothing left to
/// show it on.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_unmapped_on_a_monitor_that_goes_is_told_it_is_closed() {
    let session = session("unmap-layer-unplugged");
    session.command(perspicax_compositor::Command::Plug(Virtual {
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
    let gone = panel_up(&session, &mut desk, &mut queue, &qh, "panel");
    let surface = desk.layers[0].0.wl_surface().clone();
    until(&mut queue, &mut desk, |desk| {
        desk.said_to(&surface)
            .iter()
            .any(|said| matches!(said, Said::Enter(_)))
    });
    let on = desk
        .said_to(&surface)
        .into_iter()
        .find_map(|said| match said {
            Said::Enter(name) => Some(name),
            _ => None,
        })
        .expect("waited for");
    unmap_layer(&mut desk, &mut queue, 0);
    session.wait_for(|facts| facts.surface(gone).is_none());

    session.command(perspicax_compositor::Command::Unplug(on));
    until(&mut queue, &mut desk, |desk| {
        desk.said_to(&surface).contains(&Said::Closed)
    });

    session.stop((desk, queue));
}

/// #144: an overlay its client unmaps under a still pointer gives the
/// pointer back to the window it uncovered, as one that goes does.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panel_its_client_unmaps_gives_the_pointer_to_what_it_uncovered() {
    let session = session("unmap-layer-pointer");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.open_window(&qh, "beneath", "beneath");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    // A person's click on the window leaves the pointer resting there.
    session.command(Command::Click {
        at: (200, 150),
        button: Button::Left,
        mods: Mods::default(),
    });
    until(&mut queue, &mut desk, |desk| {
        desk.entered.is_some() && desk.buttons.len() == 2
    });

    // An overlay comes up over the whole screen, from another client.
    let (mut menu, mut menu_queue, menu_qh, menu_globals) = session.client();
    menu.bind_pointer(&menu_globals, &menu_qh);
    menu.open_strip(&menu_qh, Layer::Overlay, "menu", 800, LAUNCHER);
    until(&mut menu_queue, &mut menu, |menu| menu.entered.is_some());
    until(&mut queue, &mut desk, |desk| desk.entered.is_none());

    // Its client unmaps it, and the window has the pointer back, still
    // without a move.
    unmap_layer(&mut menu, &mut menu_queue, 0);
    until(&mut queue, &mut desk, |desk| desk.entered.is_some());

    session.stop(((desk, queue), (menu, menu_queue)));
}
