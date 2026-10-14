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
//! A menu is also a surface of its own in the facts (issue #112), directly
//! above the window or panel it hangs from, as smithay draws it and finds
//! the pointer on it: it covers what is under it, that window's own nodes
//! included, and a node joined to it is judged inside it, past its window's
//! edge too. The tests from `a_menu_is_a_surface_of_its_own` on check that.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Session, WINDOW, configured, heard, landed, menu_with_the_keyboard, until};
use perspicax_compositor::{ActError, Backend, Host};
use perspicax_index::{
    Action as Verb, HostFacts, Layer as Level, PointerButton, SurfaceFacts, SurfaceKind, judge,
};
use perspicax_node::{Rect, SurfaceId, Vec2, Visibility};
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

// # What a menu covers (issue #112)

/// Every popup the facts describe, bottom to top.
fn popups(facts: &HostFacts) -> Vec<&SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::Popup { .. }))
        .collect()
}

/// Where a surface is in the stack, counting from the bottom.
fn place(facts: &HostFacts, id: SurfaceId) -> usize {
    facts
        .surfaces()
        .iter()
        .position(|surface| surface.id == id)
        .expect("described")
}

/// Open a menu of the first window, `size` big at `at` in its window
/// geometry, paint it, and wait for the facts to describe it on screen.
fn open_and_show(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    at: (i32, i32),
    size: (u32, u32),
) -> (
    smithay_client_toolkit::shell::xdg::popup::Popup,
    SurfaceFacts,
) {
    let menu = desk.open_popup(qh, desk.windows[0].xdg_surface(), at, size);
    let configured_before = desk.popups_configured;
    configured(desk, queue, configured_before + 1);
    desk.paint(menu.wl_surface(), size, MENU);
    menu.wl_surface().commit();
    queue.flush().expect("sent");
    let known: Vec<SurfaceId> = popups(&session.facts.read()).iter().map(|p| p.id).collect();
    let facts = session.wait_for(|facts| {
        popups(facts)
            .iter()
            .any(|popup| popup.mapped && !known.contains(&popup.id))
    });
    let shown = popups(&facts)
        .into_iter()
        .find(|popup| !known.contains(&popup.id))
        .cloned()
        .expect("waited for");
    (menu, shown)
}

/// A rect `size` big at `at` in a surface's window space.
fn rect(at: (f64, f64), size: (f64, f64)) -> Rect {
    Rect::new(at.0, at.1, at.0 + size.0, at.1 + size.1)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_is_a_surface_of_its_own_directly_above_its_window() {
    let session = session("popup-surface");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let (_menu, shown) = open_and_show(&session, &mut desk, &mut queue, &qh, (40, 30), (120, 80));

    let facts = session.facts.read();
    let window = facts.surface(page).expect("described");
    assert_eq!(shown.kind, SurfaceKind::Popup { from: page });
    assert_eq!(place(&facts, shown.id), place(&facts, page) + 1);
    assert_eq!(
        shown.geometry,
        rect((40.0, 30.0), (120.0, 80.0)) + window.geometry.origin().to_vec2(),
        "where its positioner put it in its window's geometry"
    );
    assert!(shown.mapped_at.is_some(), "said when it came on screen");
    assert_eq!(shown.origin, window.origin, "drawn by its window's client");
    // Its own frames, in its own space, beside its window's copy (#42).
    assert!(shown.damage_generation >= 1);
    assert!(shown.damage_touches(0, rect((10.0, 10.0), (20.0, 20.0))));

    session.stop((desk, queue));
}

/// Issue #112's first failure, on the menu's own window: a button under
/// it was judged visible, and the menu took the click.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_windows_own_node_under_its_menu_is_covered_by_it() {
    let session = session("popup-covers-own");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let (_menu, shown) = open_and_show(&session, &mut desk, &mut queue, &qh, (40, 30), (120, 80));

    let facts = session.facts.read();
    let under = rect((60.0, 50.0), (10.0, 10.0));
    assert_eq!(
        judge(&facts, page, under).visibility,
        Visibility::Occluded { by: shown.id }
    );
    assert_eq!(
        judge(&facts, page, rect((300.0, 200.0), (10.0, 10.0))).visibility,
        Visibility::Visible,
        "away from it, as ever"
    );

    session.stop((desk, queue));
}

/// Issue #112's first failure on another application's window: a menu
/// opened over a button of a window beneath its own covered nothing, and
/// took the click meant for the button.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_node_of_another_window_under_a_menu_is_covered_by_it() {
    let session = session("popup-covers-other");
    let (mut desk, mut queue, qh, _) = session.client();
    let (beneath, _) = settle(&session, &mut desk, &mut queue, &qh, "beneath");
    desk.open_window(&qh, "above", "above");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let facts = session.wait_for(|facts| titled(facts, "above").is_some_and(|w| w.mapped));
    let above = titled(&facts, "above").expect("waited for").clone();

    // A menu of the window above, hanging off its left edge over the strip
    // of the window beneath that nothing else covers.
    let menu = desk.open_popup(&qh, desk.windows[1].xdg_surface(), (-30, 100), (25, 50));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (25, 50), MENU);
    menu.wl_surface().commit();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| popups(facts).iter().any(|popup| popup.mapped));
    let shown = popups(&facts)[0].clone();
    assert_eq!(shown.kind, SurfaceKind::Popup { from: above.id });

    let window = facts.surface(beneath).expect("described");
    let global = rect(
        (shown.geometry.x0 + 5.0, shown.geometry.y0 + 5.0),
        (10.0, 10.0),
    );
    assert!(
        global.intersect(above.covers()).area() == 0.0
            && above
                .frame
                .iter()
                .all(|strip| strip.intersect(global).area() == 0.0),
        "nothing but the menu is over it"
    );
    assert_eq!(
        judge(&facts, beneath, global - window.geometry.origin().to_vec2()).visibility,
        Visibility::Occluded { by: shown.id }
    );

    session.stop((desk, queue));
}

/// Issue #112's second failure: an item of a menu hanging past its
/// window's edge was refused as clipped. Judged on its menu, it is inside.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_item_past_its_windows_edge_is_inside_its_menu() {
    let session = session("popup-past-edge");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let (_menu, shown) = open_and_show(&session, &mut desk, &mut queue, &qh, (350, 250), (120, 80));

    let facts = session.facts.read();
    let window = facts.surface(page).expect("described");
    // The menu's last item, wholly past the window's corner.
    let item = rect((60.0, 60.0), (40.0, 15.0));
    assert_eq!(
        judge(&facts, shown.id, item).visibility,
        Visibility::Visible
    );
    let on_the_window = shown.to_global(item) - window.geometry.origin().to_vec2();
    assert_eq!(
        judge(&facts, page, on_the_window).visibility,
        Visibility::Clipped,
        "on its window, where it used to be joined, it is past the edge"
    );

    session.stop((desk, queue));
}

/// A submenu hangs from the window its menu does, directly above the menu.
/// Of two menus opened from one window, the first is drawn in front: smithay
/// lists a window's menus in the order they opened, each after its
/// submenus, front to back.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn menus_stack_as_smithay_draws_them() {
    let session = session("popup-stack");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let (menu, first) = open_and_show(&session, &mut desk, &mut queue, &qh, (40, 30), (120, 80));

    let submenu = desk.open_popup(&qh, menu.xdg_surface(), (100, 10), (80, 60));
    configured(&mut desk, &mut queue, 2);
    desk.paint(submenu.wl_surface(), (80, 60), HIGHLIT);
    submenu.wl_surface().commit();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| popups(facts).iter().filter(|p| p.mapped).count() == 2);
    let sub = popups(&facts)
        .into_iter()
        .find(|popup| popup.id != first.id)
        .cloned()
        .expect("waited for");
    assert_eq!(sub.kind, SurfaceKind::Popup { from: page }, "its window's");
    assert_eq!(place(&facts, first.id), place(&facts, page) + 1);
    assert_eq!(place(&facts, sub.id), place(&facts, first.id) + 1);
    assert_eq!(
        sub.geometry.origin(),
        first.geometry.origin() + Vec2::new(100.0, 10.0),
        "where its menu put it"
    );

    // A second menu of the window itself, opened after the first.
    let (_later, second) =
        open_and_show(&session, &mut desk, &mut queue, &qh, (200, 100), (60, 60));
    let facts = session.facts.read();
    assert_eq!(
        [
            place(&facts, second.id),
            place(&facts, first.id),
            place(&facts, sub.id)
        ],
        [
            place(&facts, page) + 1,
            place(&facts, page) + 2,
            place(&facts, page) + 3
        ],
        "the later one behind the first and its submenu"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_panels_menu_is_a_surface_above_the_panel() {
    let session = session("popup-panel-surface");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_strip(&qh, Layer::Top, "strip", 40, MENU);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let facts = session.wait_for(|facts| {
        facts.surfaces().iter().any(|s| {
            matches!(&s.kind, SurfaceKind::Layer { namespace, .. } if namespace == "strip")
                && s.mapped
        })
    });
    let panel = facts
        .surfaces()
        .iter()
        .find(|s| matches!(&s.kind, SurfaceKind::Layer { namespace, .. } if namespace == "strip"))
        .cloned()
        .expect("waited for");

    let menu = desk.open_panel_popup(&qh, &desk.layers[0].0, (20, 40), (100, 150));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (100, 150), HIGHLIT);
    menu.wl_surface().commit();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| popups(facts).iter().any(|popup| popup.mapped));
    let shown = popups(&facts)[0].clone();
    assert_eq!(shown.kind, SurfaceKind::Popup { from: panel.id });
    assert_eq!(place(&facts, shown.id), place(&facts, panel.id) + 1);
    assert_eq!(
        shown.geometry,
        rect((20.0, 40.0), (100.0, 150.0)) + panel.geometry.origin().to_vec2(),
        "from the panel's own corner"
    );

    session.stop((desk, queue));
}

/// A minimized window's menus are not drawn, so they are not described.
/// Seated, since only a person's window is minimized at its client's asking.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_parked_windows_menu_is_not_described() {
    let session = seated("popup-parked");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let (_menu, shown) = open_and_show(&session, &mut desk, &mut queue, &qh, (40, 30), (120, 80));

    desk.windows[0].set_minimized();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| facts.surface(page).is_some_and(|w| !w.mapped));
    assert!(facts.surface(shown.id).is_none());

    session.stop((desk, queue));
}

/// A window that has drawn nothing is not described, but smithay draws its
/// menus all the same, over whatever is under them: an empty window and a
/// painted menu is issue #112's attack by another route. The menu is
/// described, and covers.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_of_a_window_that_has_drawn_nothing_covers_what_is_under_it() {
    let session = session("popup-empty-window");
    let (mut desk, mut queue, qh, _) = session.client();
    let (beneath, _) = settle(&session, &mut desk, &mut queue, &qh, "beneath");
    desk.open_bare(&qh, "empty");
    until(&mut queue, &mut desk, |desk| desk.blank_configured >= 1);

    let menu = desk.open_popup(&qh, desk.blank[0].xdg_surface(), (10, 10), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| popups(facts).iter().any(|popup| popup.mapped));
    let shown = popups(&facts)[0].clone();
    assert!(
        titled(&facts, "empty").is_none(),
        "the window itself is not described"
    );
    let window = facts.surface(beneath).expect("described");
    let under =
        shown.to_global(rect((20.0, 20.0), (10.0, 10.0))) - window.geometry.origin().to_vec2();
    assert_eq!(
        judge(&facts, beneath, under).visibility,
        Visibility::Occluded { by: shown.id }
    );

    session.stop((desk, queue));
}

/// A menu its client destroys leaves the facts, and its window's nodes
/// under it are uncovered again.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_its_client_destroys_leaves_the_facts() {
    let session = session("popup-destroyed");
    let (mut desk, mut queue, qh, _) = session.client();
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let (menu, shown) = open_and_show(&session, &mut desk, &mut queue, &qh, (40, 30), (120, 80));

    drop(menu);
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| facts.surface(shown.id).is_none());
    assert_eq!(
        judge(&facts, page, rect((60.0, 50.0), (10.0, 10.0))).visibility,
        Visibility::Visible
    );

    session.stop((desk, queue));
}

/// A person's click outside a menu that holds the pointer dismisses it, and
/// smithay stops drawing it at once, though its client has yet to destroy
/// it -- this one never does. The facts stop describing it at once too:
/// otherwise its items would be judged on a menu that is no longer drawn.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_a_click_outside_dismissed_leaves_the_facts_at_once() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Mods};

    let session = seated("popup-dismissed");
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);
    let facts = session.wait_for(|facts| popups(facts).iter().any(|popup| popup.mapped));
    let shown = popups(&facts)[0].clone();
    // On the empty desk, away from every window: smithay passes a press on
    // the menu's own client through to it, and dismisses the menu for any
    // other.
    let at = (900, 700);
    assert!(
        facts
            .surfaces()
            .iter()
            .all(|surface| !surface.covers().contains((900.0, 700.0).into()))
    );
    session.command(Command::Press {
        at,
        button: Button::Left,
        mods: Mods::default(),
    });
    session.command(Command::Release {
        button: Button::Left,
    });
    until(&mut queue, &mut desk, |desk| desk.popups_done == 1);
    let facts = session.wait_for(|facts| facts.surface(shown.id).is_none());
    assert!(popups(&facts).is_empty());
    assert!(
        menu.wl_surface().is_alive(),
        "its client has not destroyed it"
    );

    session.stop((desk, queue));
}

// # Acting on a menu (issue #112)

/// Linux's code for the left button.
const BTN_LEFT: u32 = 0x110;

/// Open a menu of the first window past its edge, drawn with a shadow of 8
/// around it as toolkits draw one, and wait for the facts to describe it.
fn shadowed_menu(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
) -> (
    smithay_client_toolkit::shell::xdg::popup::Popup,
    SurfaceFacts,
) {
    let menu = desk.open_popup(qh, desk.windows[0].xdg_surface(), (350, 250), (120, 80));
    configured(desk, queue, 1);
    menu.xdg_surface().set_window_geometry(8, 8, 120, 80);
    desk.paint(menu.wl_surface(), (136, 96), MENU);
    menu.wl_surface().commit();
    queue.flush().expect("sent");
    let facts = session.wait_for(|facts| popups(facts).iter().any(|popup| popup.mapped));
    let shown = popups(&facts)[0].clone();
    (menu, shown)
}

/// An agent's click on a menu's item lands on the menu: on its surface, at
/// the item's centre, measured from the menu's window geometry as the index
/// measures its nodes. Before, an act could only name a window, and a click
/// was delivered to the window's surface at the menu's position.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agents_click_on_a_menu_lands_on_the_menu() {
    let session = session("popup-click");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    settle(&session, &mut desk, &mut queue, &qh, "page");
    let (menu, shown) = shadowed_menu(&session, &mut desk, &mut queue, &qh);

    // The menu's third row, past the window's edge.
    let item = rect((10.0, 40.0), (100.0, 20.0));
    Host::new(&session.facts, &session.requests)
        .act(
            shown.id,
            &Verb::Click {
                at: item,
                button: PointerButton::Left,
            },
        )
        .expect("clicked");
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);
    assert_eq!(desk.pointed.as_ref(), Some(menu.wl_surface()));
    assert_eq!(
        desk.arrived,
        Some((68.0, 58.0)),
        "the item's centre, its window geometry 8 into its surface"
    );
    assert_eq!(desk.buttons, [(BTN_LEFT, true), (BTN_LEFT, false)]);

    session.stop((desk, queue));
}

/// A menu that has gone takes no act: there is nothing drawn to put it on.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_that_has_gone_takes_no_act() {
    let session = session("popup-gone-act");
    let (mut desk, mut queue, qh, _) = session.client();
    settle(&session, &mut desk, &mut queue, &qh, "page");
    let (menu, shown) = shadowed_menu(&session, &mut desk, &mut queue, &qh);
    drop(menu);
    queue.flush().expect("sent");
    session.wait_for(|facts| facts.surface(shown.id).is_none());

    assert_eq!(
        Host::new(&session.facts, &session.requests).act(
            shown.id,
            &Verb::Click {
                at: rect((10.0, 10.0), (10.0, 10.0)),
                button: PointerButton::Left,
            },
        ),
        Err(ActError::NoSuchSurface(shown.id.0))
    );

    session.stop((desk, queue));
}

/// A menu gets the keyboard only through its own grab: an agent cannot give
/// it one.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_without_the_keyboard_cannot_be_given_it() {
    let session = session("popup-focus");
    let (mut desk, mut queue, qh, _) = session.client();
    settle(&session, &mut desk, &mut queue, &qh, "page");
    let (_menu, shown) = shadowed_menu(&session, &mut desk, &mut queue, &qh);

    assert_eq!(
        Host::new(&session.facts, &session.requests).act(shown.id, &Verb::Focus),
        Err(ActError::TakesNoKeyboard(shown.id.0))
    );

    session.stop((desk, queue));
}

/// While a window's menu holds the keyboard, keys go to the menu, not the
/// window: typed at one of the window's fields they would land in the menu,
/// where a letter can choose an item. So typing at the window is refused,
/// naming the menu, and typing at the menu is typed (dave, #112). The
/// window is still the one in use (#97).
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn keys_typed_while_a_menu_holds_the_keyboard_go_only_to_the_menu() {
    let session = seated("popup-typing");
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    let (page, _) = settle(&session, &mut desk, &mut queue, &qh, "page");
    let menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);
    let facts = session.wait_for(|facts| popups(facts).iter().any(|popup| popup.mapped));
    let shown = popups(&facts)[0].id;
    let host = Host::new(&session.facts, &session.requests);

    let type_at = |surface: SurfaceId, text: &str| {
        host.act(
            surface,
            &Verb::Type {
                text: text.to_owned(),
            },
        )
    };
    assert_eq!(
        type_at(page, "x").map(|_| ()),
        Err(ActError::FocusElsewhere {
            focused: Some(shown)
        })
    );
    let dispatched = type_at(shown, "y").expect("typed");
    assert_eq!(
        dispatched.focus_before,
        Some(page),
        "the window is the one in use"
    );
    until(&mut queue, &mut desk, |_| heard(&ear).typed() == "y");
    assert_eq!(heard(&ear).entered, Some(menu.wl_surface().id()));
    assert!(host.act(shown, &Verb::Focus).is_ok(), "it has it already");

    session.stop((desk, queue));
}
