//! What a menu draws is its window's damage, where the menu is drawn.
//!
//! A menu, a combo list or a tooltip is an `xdg_popup`: a surface of its own,
//! tied to its window by xdg-shell and not by `wl_subsurface`. A compositor
//! that followed only subsurfaces to a window counted nothing a popup drew,
//! so a menu could highlight an item while its window stood still, and the
//! receipt read `quiet` (issue #42). These tests open popups on a window, on
//! a menu, with shadows, past the window's edge and on a panel, and check
//! that what each draws is counted for the window or panel it hangs from,
//! where it shows, once per frame -- and that the menu's picture is never
//! taken for its window's.
//!
//! Headless, a popup is placed exactly where its positioner says: only a
//! person's seat keeps one on screen. So each one here sits where the test
//! asked, and the damage is expected there.
//!
//! A menu that takes the keyboard, as a right-click opens one, does so only
//! with a person at the seat. Its keys are its window's, so the window is
//! still the one in use while it is open (issue #97): those tests have a
//! person, and a keyboard of their own to see where the keys went.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, WINDOW, configured, heard, landed, menu_with_the_keyboard, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, Layer as Level, SurfaceFacts, SurfaceKind};
use perspicax_node::{Rect, SurfaceId};
use smithay_client_toolkit::shell::{WaylandSurface as _, wlr_layer::Layer, xdg::XdgSurface as _};
use wayland_client::{EventQueue, Proxy as _, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::State;

const MENU: u32 = 0xff33_6699;
const HIGHLIT: u32 = 0xff99_6633;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

/// A session with a person at the seat, where a menu takes the keyboard.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).with_person())
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

/// Wait until the window called `title` has drawn and the facts say so, and
/// return its id with the damage it had taken by then.
fn settled(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    title: &str,
) -> (SurfaceId, u64) {
    until(queue, desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| {
        titled(facts, title).is_some_and(|window| window.mapped && window.damage_generation >= 1)
    });
    let window = titled(&facts, title).expect("waited for");
    (window.id, window.damage_generation)
}

/// Open a window called `title`, and wait as [`settled`] does.
fn settle(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
) -> (SurfaceId, u64) {
    desk.open_window(qh, title, title);
    settled(session, desk, queue, title)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn damage_a_menu_draws_counts_for_its_window_where_it_shows() {
    let session = session("popup-lands");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, before) = settle(&session, &mut desk, &mut queue, &qh, "page");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    let area = [Rect::new(40.0, 30.0, 160.0, 110.0)];
    let window = landed(&session, &mut desk, &mut queue, page, &area);
    assert_eq!(
        window.damage_generation,
        before + 1,
        "one frame, the menu's: opening it drew nothing"
    );
    // The two questions a receipt asks, for a node under the menu and for
    // one away from it: `on_target`, and not `quiet`.
    assert!(window.damage_touches(before, Rect::new(60.0, 50.0, 80.0, 70.0)));
    assert!(!window.damage_touches(before, Rect::new(300.0, 200.0, 320.0, 220.0)));

    // An item lit as the pointer passes, while the window draws nothing.
    desk.paint(menu.wl_surface(), (120, 80), HIGHLIT);
    menu.wl_surface().commit();
    let window = landed(&session, &mut desk, &mut queue, page, &area);
    assert_eq!(window.damage_generation, before + 2);
    assert!(
        window.mapped,
        "the menu's picture is not taken for its window's"
    );
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_hanging_past_its_window_is_counted_where_it_hangs() {
    let session = session("popup-past-the-edge");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, before) = settle(&session, &mut desk, &mut queue, &qh, "page");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (350, 250), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    // Past the window's 400 by 300, and not clipped to it: those are pixels
    // the menu changed, wherever its window ends.
    let window = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(350.0, 250.0, 470.0, 330.0)],
    );
    assert_eq!(window.damage_generation, before + 1);
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_with_a_shadow_hangs_its_menu_from_its_geometry() {
    let session = session("popup-window-shadow");
    let (mut desk, mut queue, qh, _) = session.client();
    // A frame inside a shadow 26 by 23 deep, as Firefox draws one.
    desk.open_declaring(&qh, "shadowed", Some((26, 23, 300, 200)));
    let (page, before) = settled(&session, &mut desk, &mut queue, "shadowed");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    // A popup is placed from the window's geometry, and its window's damage
    // is kept from the surface's own corner, outside the shadow.
    let window = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(66.0, 53.0, 186.0, 133.0)],
    );
    assert_eq!(window.damage_generation, before + 1);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_with_a_shadow_lands_by_its_own_geometry() {
    let session = session("popup-own-shadow");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, before) = settle(&session, &mut desk, &mut queue, &qh, "page");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    // The menu is 120 by 80 inside a shadow 8 deep all round, so its surface
    // begins 8 above and to the left of where it was placed.
    menu.xdg_surface().set_window_geometry(8, 8, 120, 80);
    desk.paint(menu.wl_surface(), (136, 96), MENU);
    menu.wl_surface().commit();
    let window = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(32.0, 22.0, 168.0, 118.0)],
    );
    assert_eq!(window.damage_generation, before + 1);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_submenu_lands_at_the_sum_of_its_menus() {
    let session = session("popup-submenu");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    let before = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(40.0, 30.0, 160.0, 110.0)],
    )
    .damage_generation;

    // Opened from the menu's item at (100, 10): placed from the menu, which
    // is placed from the window.
    let submenu = desk.open_popup(&qh, menu.xdg_surface(), (100, 10), (80, 60));
    configured(&mut desk, &mut queue, 2);
    desk.paint(submenu.wl_surface(), (80, 60), HIGHLIT);
    submenu.wl_surface().commit();
    let window = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(140.0, 40.0, 220.0, 100.0)],
    );
    assert_eq!(window.damage_generation, before + 1);
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn damage_drawn_into_a_subsurface_of_a_menu_counts_for_its_window() {
    let session = session("popup-subsurface");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    let (_placed, child) = desk.open_subsurface(&qh, menu.wl_surface(), (10, 20), false);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    let before = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(40.0, 30.0, 160.0, 110.0)],
    )
    .damage_generation;

    desk.paint(&child, (30, 20), HIGHLIT);
    child.commit();
    let window = landed(
        &session,
        &mut desk,
        &mut queue,
        page,
        &[Rect::new(50.0, 50.0, 80.0, 70.0)],
    );
    assert_eq!(window.damage_generation, before + 1);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_of_a_panel_counts_for_the_panel() {
    let session = session("popup-panel");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_strip(&qh, Layer::Top, "strip", 40, MENU);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let strip = |facts: &HostFacts| {
        facts
            .surfaces()
            .iter()
            .find(|surface| {
                surface.mapped
                    && matches!(
                        &surface.kind,
                        SurfaceKind::Layer { layer: Level::Top, namespace } if namespace == "strip"
                    )
            })
            .cloned()
    };
    let facts = session.wait_for(|facts| strip(facts).is_some_and(|s| s.damage_generation >= 1));
    let panel = strip(&facts).expect("waited for");

    // A tray menu, opened down from the strip. A layer surface has no window
    // geometry, so it is placed from the panel's own corner.
    let menu = desk.open_panel_popup(&qh, &desk.layers[0].0, (20, 40), (100, 150));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (100, 150), HIGHLIT);
    menu.wl_surface().commit();
    let shown = landed(
        &session,
        &mut desk,
        &mut queue,
        panel.id,
        &[Rect::new(20.0, 40.0, 120.0, 190.0)],
    );
    assert_eq!(shown.damage_generation, panel.damage_generation + 1);
    assert!(
        shown.mapped,
        "the menu's picture is not taken for its panel's"
    );
    assert_eq!(
        desk.layers_drawn, 1,
        "no configure redrew the panel meanwhile"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_closing_is_damage_where_it_showed() {
    let session = session("popup-closed");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    let area = [Rect::new(40.0, 30.0, 160.0, 110.0)];
    let before = landed(&session, &mut desk, &mut queue, page, &area).damage_generation;

    // An item chosen: the menu goes, as a toolkit closes one, by destroying
    // it with its picture still in it.
    drop(menu);
    let window = landed(&session, &mut desk, &mut queue, page, &area);
    assert_eq!(
        window.damage_generation,
        before + 1,
        "what the menu covered shows again, where it was"
    );
    assert!(window.mapped, "the window is on screen without its menu");
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_taking_its_picture_away_and_then_closing_counts_once() {
    let session = session("popup-removed-then-closed");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    // Declared, as a toolkit declares it, so the menu still has a geometry
    // once its picture is gone.
    menu.xdg_surface().set_window_geometry(0, 0, 120, 80);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    let area = [Rect::new(40.0, 30.0, 160.0, 110.0)];
    let before = landed(&session, &mut desk, &mut queue, page, &area).damage_generation;

    menu.wl_surface().attach(None, 0, 0);
    menu.wl_surface().commit();
    let hidden = landed(&session, &mut desk, &mut queue, page, &area).damage_generation;
    assert_eq!(hidden, before + 1, "taking its picture away is one frame");

    drop(menu);
    queue.roundtrip(&mut desk).expect("round trip");
    let facts = session.wait_for(|_| true);
    let window = facts.surface(page).expect("the window");
    assert_eq!(
        window.damage_generation, hidden,
        "a menu that already showed nothing changes nothing as it goes"
    );
    assert!(window.mapped);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_taking_its_picture_away_leaves_its_window_mapped() {
    let session = session("popup-removed");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");

    let menu = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    let area = [Rect::new(40.0, 30.0, 160.0, 110.0)];
    let before = landed(&session, &mut desk, &mut queue, page, &area).damage_generation;

    menu.wl_surface().attach(None, 0, 0);
    menu.wl_surface().commit();
    // All of the menu, and only the menu: what was under it shows again.
    let window = landed(&session, &mut desk, &mut queue, page, &area);
    assert!(window.mapped, "the window still has its own picture");
    assert_eq!(window.damage_generation, before + 1);
    assert_eq!(desk.drawn, 1, "no configure redrew the window meanwhile");
    // The window is the size it always was: its menu never measured it.
    let whole = Rect::new(0.0, 0.0, f64::from(WINDOW.0), f64::from(WINDOW.1));
    assert_eq!(window.geometry.size(), whole.size());

    session.stop((desk, queue));
}

/// Issue #97: a menu a window opens takes the keyboard, and smithay names
/// the menu, not the window, as holding it. The window was told it was no
/// longer active for as long as its menu was open -- every toolkit drew
/// itself in the background -- and active again once it closed. Its keys
/// are its own still, so it stays active, as on sway and Plasma.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_whose_own_menu_holds_the_keyboard_is_still_told_it_is_active() {
    let session = seated("popup-keeps-active");
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    assert_eq!(desk.activations_of(0), [true], "active from the start");

    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);
    let told = desk.activations_of(0);
    assert!(
        told.iter().all(|&active| active),
        "told it was not active while its menu was open: {told:?}"
    );

    session.stop((desk, queue));
}

/// Issue #97: a menu's client closes it, as it does when an item is chosen,
/// and smithay leaves the keyboard on the menu that is gone until the next
/// key or motion. No window was in use meanwhile: a fullscreen video went
/// under the panel, its frame grey and its task out, until the mouse moved.
/// The window has the keyboard back at once.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_its_client_closes_gives_its_window_the_keyboard_back_at_once() {
    let session = seated("popup-closed-hands-back");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    let ear = desk.bind_keyboard(&globals, &qh);
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);
    let before = session
        .facts
        .read()
        .surface(page)
        .expect("the window")
        .damage_generation;

    // An item chosen, and nothing more: no key, no motion.
    drop(menu);
    let window = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });
    session.wait_for(|facts| {
        facts
            .surface(page)
            .is_some_and(|window| window.damage_generation > before)
    });
    queue.roundtrip(&mut desk).expect("the taskbar told");
    assert!(
        desk.task("page")
            .is_some_and(|task| task.is(State::Activated)),
        "the window's task went out as its menu closed"
    );
    let told = desk.activations_of(0);
    assert!(
        told.iter().all(|&active| active),
        "told it was not active as its menu closed: {told:?}"
    );

    session.stop((desk, queue));
}

/// Issue #97: a submenu its client closes gives the keyboard back to the
/// menu it opened from, which is still open, and not past it to the window.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_submenu_its_client_closes_gives_the_keyboard_back_to_its_menu() {
    let session = seated("popup-submenu-hands-back");
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    // A submenu opens on an event of its own, the hover over its item, and
    // smithay tells nested grabs apart by their serials: not the menu's.
    let serial = desk.popup_serial.expect("the menu was configured");
    let submenu = desk.open_menu(&qh, menu.xdg_surface(), (100, 10), (80, 60), serial);
    let sub = submenu.wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&sub)
    });

    drop(submenu);
    let held = menu.wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&held)
    });
    let told = desk.activations_of(0);
    assert!(
        told.iter().all(|&active| active),
        "told it was not active while its menus were open: {told:?}"
    );

    session.stop((desk, queue));
}

/// Issue #97: a menu that grabs, opened from a popup that does not -- a
/// list, say -- gives the keyboard back to the window when its client closes
/// it, where smithay's grab would give it at the next input. A popup without
/// the grab never had the keyboard.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_opened_from_a_popup_without_the_grab_gives_the_keyboard_back_to_its_window() {
    let session = seated("popup-ungrabbed-parent-hands-back");
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    settle(&session, &mut desk, &mut queue, &qh, "page");
    let window = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });

    let list = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    let serial = desk.popup_serial.expect("the list was configured");
    let menu = desk.open_menu(&qh, list.xdg_surface(), (100, 10), (80, 60), serial);
    let held = menu.wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&held)
    });

    drop(menu);
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });

    session.stop((desk, queue));
}
