//! A window's menu holding the pointer and keyboard, as an agent meets it,
//! by a compositor with no screen.
//!
//! A right-click menu takes the pointer and keyboard until it closes, and
//! while it has them smithay's grab decides where input goes: the menu's
//! own application takes the pointer as it would without the menu, and a
//! press anywhere else closes the menu; the keyboard goes to the menu alone.
//! Issue #157: an agent's act on another application went through that
//! grab. Its click closed the person's menu, and only then reached the
//! window it aimed at; its scroll reached nobody, and the menu stayed open;
//! its focus was ignored. Each was reported as done. Now each is refused as
//! `MenuOpen`, naming the menu, and the menu stays open, as is bringing a
//! tab forward, which gives it the keyboard. Choosing from the menu,
//! clicking anywhere in its own application, and focusing its window, go
//! ahead as they do for the person. The menu is the person's, and closes
//! for them alone: their click on another window, its own application's
//! other window too, gives that window the keys and closes the menu
//! (#185), where an agent's click on the application's other window leaves
//! it open.
//!
//! With a person at the seat, where alone a menu takes the grab. Like the
//! other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Ear, Session, configured, heard, menu_with_the_keyboard_on, until};
use perspicax_compositor::{ActError, Backend, Host};
use perspicax_index::{Action as Verb, HostFacts, PointerButton, SurfaceKind, judge};
use perspicax_node::{Rect, SurfaceId, Visibility};
use perspicax_policy::Action;
use smithay_client_toolkit::shell::{
    WaylandSurface as _,
    xdg::{XdgSurface as _, popup::Popup},
};
use wayland_client::{EventQueue, Proxy as _, QueueHandle, protocol::wl_surface::WlSurface};

/// Linux's code for the left button.
const BTN_LEFT: u32 = 0x110;

/// A spot inside every surface here, the panel's strip included, in the
/// surface's own space.
const SPOT: Rect = Rect {
    x0: 10.0,
    y0: 10.0,
    x1: 20.0,
    y1: 20.0,
};

/// One monitor, with a person at it: only a person's menu takes the grab.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).with_person())
}

fn click() -> Verb {
    Verb::Click {
        at: SPOT,
        button: PointerButton::Left,
    }
}

fn scroll() -> Verb {
    Verb::Scroll {
        at: SPOT,
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

/// Two applications at the seat.
///
/// "b" has a panel across the top, a window, "elsewhere", and a popup of it
/// that holds nothing, as a tooltip does. "a" has two windows, "own" and
/// then "page", over it with the keyboard; and once [`Apps::open_the_menu`]
/// has run, a menu of "page" holding the pointer and keyboard, over both.
struct Apps {
    a: Desk,
    a_queue: EventQueue<Desk>,
    a_qh: QueueHandle<Desk>,
    ear: Ear,
    b: Desk,
    b_queue: EventQueue<Desk>,
    page: SurfaceId,
    own: SurfaceId,
    elsewhere: SurfaceId,
    tooltip: SurfaceId,
    panel: SurfaceId,
    /// The menu of "page", once open: its id, and the client's own handle,
    /// kept so that it stays open.
    menu: Option<(SurfaceId, Popup)>,
    /// "b"'s popup, kept so that it stays open.
    _tooltip: Popup,
}

impl Apps {
    fn start(session: &Session) -> Self {
        let (mut b, mut b_queue, b_qh, b_globals) = session.client();
        b.bind_taskbar(&b_globals, &b_qh);
        b.bind_pointer(&b_globals, &b_qh);
        b.open_panel(&b_qh, "panel", 30, 0xff20_2020);
        until(&mut b_queue, &mut b, |_| {
            layer(&session.facts.read(), "panel").is_some()
        });
        let panel = layer(&session.facts.read(), "panel").expect("waited for");
        let elsewhere = opened(session, &mut b, &mut b_queue, &b_qh, "elsewhere");
        let tooltip = b.open_popup(&b_qh, b.windows[0].xdg_surface(), (20, 20), (60, 40));
        configured(&mut b, &mut b_queue, 1);
        b.paint(tooltip.wl_surface(), (60, 40), 0xff66_9933);
        tooltip.wl_surface().commit();
        until(&mut b_queue, &mut b, |_| {
            !popups_of(&session.facts.read(), elsewhere).is_empty()
        });
        let tooltip_id = popups_of(&session.facts.read(), elsewhere)[0];

        let (mut a, mut a_queue, a_qh, a_globals) = session.client();
        a.bind_pointer(&a_globals, &a_qh);
        let ear = a.bind_keyboard(&a_globals, &a_qh);
        let own = opened(session, &mut a, &mut a_queue, &a_qh, "own");
        // Picked in the taskbar of "b", the program in use (#270): it opened
        // without the keys, "elsewhere" having them (#259), and has them once
        // the person gives them.
        until(&mut b_queue, &mut b, |b| b.task("own").is_some());
        b.activate("own");
        b_queue.roundtrip(&mut b).expect("the pick sent");
        let own_surface = a.windows[0].wl_surface().id();
        until(&mut a_queue, &mut a, |_| {
            heard(&ear).entered.as_ref() == Some(&own_surface)
        });
        // Over "own", with the keyboard as it opened, so that its menu is
        // over "own" too, where an agent's click on the menu lands.
        let page = opened(session, &mut a, &mut a_queue, &a_qh, "page");
        Self {
            a,
            a_queue,
            a_qh,
            ear,
            b,
            b_queue,
            page,
            own,
            elsewhere,
            tooltip: tooltip_id,
            panel,
            menu: None,
            _tooltip: tooltip,
        }
    }

    /// Open the menu of "page" that holds the pointer and keyboard, as a
    /// right-click does, and wait for the facts to describe it.
    fn open_the_menu(&mut self, session: &Session) {
        let menu = menu_with_the_keyboard_on(
            session,
            &mut self.a,
            &mut self.a_queue,
            &self.a_qh,
            &self.ear,
            self.page,
            1,
        );
        let page = self.page;
        let facts = session.wait_for(|facts| !popups_of(facts, page).is_empty());
        self.menu = Some((popups_of(&facts, page)[0], menu));
    }

    /// Both of them, with the menu open.
    fn with_the_menu(session: &Session) -> Self {
        let mut apps = Self::start(session);
        apps.open_the_menu(session);
        apps
    }

    fn menu(&self) -> SurfaceId {
        self.menu.as_ref().expect("the menu is open").0
    }

    fn menu_popup(&self) -> &Popup {
        &self.menu.as_ref().expect("the menu is open").1
    }

    fn menu_surface(&self) -> &WlSurface {
        self.menu_popup().wl_surface()
    }

    /// Let both clients hear whatever the compositor sent them.
    fn settle(&mut self) {
        for _ in 0..3 {
            self.b_queue.roundtrip(&mut self.b).expect("b's round trip");
            self.a_queue.roundtrip(&mut self.a).expect("a's round trip");
        }
    }

    /// Whether the keyboard is on `surface`, as "a" last heard.
    fn keys_on(&self, surface: &WlSurface) -> bool {
        heard(&self.ear).entered.as_ref() == Some(&surface.id())
    }

    fn stop(self, session: Session) {
        session.stop(((self.a, self.a_queue), (self.b, self.b_queue)));
    }
}

/// While a menu of "a" holds the pointer and keyboard, an agent's click or
/// scroll on "b" -- its window, its popup, its panel -- is refused, naming
/// the menu, and reaches nothing. The menu stays open, with the keys in it.
/// Before, the click closed the menu and then landed on "b", and the
/// scroll reached nobody, both reported as done.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agent_cannot_click_or_scroll_another_application_past_a_menu() {
    let session = seated("held-menu-past");
    let mut apps = Apps::with_the_menu(&session);
    let host = Host::new(&session.facts, &session.requests);

    let refused = Err(ActError::MenuOpen { popup: apps.menu() });
    for target in [apps.elsewhere, apps.tooltip, apps.panel] {
        assert_eq!(
            host.act(target, &click()).map(|_| ()),
            refused,
            "a click on {target:?}"
        );
        assert_eq!(
            host.act(target, &scroll()).map(|_| ()),
            refused,
            "a scroll on {target:?}"
        );
    }

    apps.settle();
    assert!(apps.b.buttons.is_empty(), "no button reached b");
    assert_eq!(apps.b.scrolls, 0, "no scroll reached b");
    assert_eq!(apps.a.popups_done, 0, "the menu is still open");
    assert!(apps.keys_on(apps.menu_surface()));

    apps.stop(session);
}

/// A menu holding the pointer is named before what covers the node, as it
/// is judged again as the click is sent (#158): told of the menu, an agent
/// can choose from it, and a click anywhere on "b" would close it under the
/// person. A spot of "elsewhere" under "own" is refused as the menu's.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_holding_the_pointer_is_named_before_what_covers_the_node() {
    let session = seated("held-menu-covered");
    let apps = Apps::with_the_menu(&session);
    // "own" is cascaded over "elsewhere", 32 further in each way.
    let covered = Rect::new(40.0, 40.0, 50.0, 50.0);
    assert_eq!(
        judge(&session.facts.read(), apps.elsewhere, covered).visibility,
        Visibility::Occluded { by: apps.own }
    );

    assert_eq!(
        Host::new(&session.facts, &session.requests)
            .act(
                apps.elsewhere,
                &Verb::Click {
                    at: covered,
                    button: PointerButton::Left,
                },
            )
            .map(|_| ()),
        Err(ActError::MenuOpen { popup: apps.menu() })
    );

    apps.stop(session);
}

/// The menu's own application takes the pointer as it does the person's:
/// an item chosen from the menu, a scroll on its window and a click on its
/// other window all land, and the menu stays open.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agent_may_click_and_scroll_the_menus_own_application() {
    let session = seated("held-menu-own");
    let mut apps = Apps::with_the_menu(&session);
    let host = Host::new(&session.facts, &session.requests);

    host.act(apps.menu(), &click()).expect("an item chosen");
    until(&mut apps.a_queue, &mut apps.a, |a| a.buttons.len() == 2);
    assert_eq!(apps.a.pointed.as_ref(), Some(apps.menu_surface()));

    host.act(apps.page, &scroll()).expect("its window scrolled");
    until(&mut apps.a_queue, &mut apps.a, |a| a.scrolls == 1);

    host.act(apps.own, &click())
        .expect("its other window clicked");
    until(&mut apps.a_queue, &mut apps.a, |a| a.buttons.len() == 4);
    assert_eq!(
        apps.a.buttons,
        [
            (BTN_LEFT, true),
            (BTN_LEFT, false),
            (BTN_LEFT, true),
            (BTN_LEFT, false)
        ]
    );

    apps.settle();
    assert_eq!(apps.a.popups_done, 0, "the menu is still open");
    assert!(apps.b.buttons.is_empty() && apps.b.scrolls == 0);

    apps.stop(session);
}

/// With a submenu open, the submenu holds the pointer and keyboard, and it
/// is the menu named: the same one typing at the window names.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_submenu_holding_the_pointer_is_the_menu_named() {
    let session = seated("held-menu-submenu");
    let mut apps = Apps::with_the_menu(&session);
    // A submenu opens on an event of its own, the hover over its item, and
    // smithay tells nested grabs apart by their serials: not the menu's.
    let serial = apps.a.popup_serial.expect("the menu was configured");
    let submenu = apps.a.open_menu(
        &apps.a_qh,
        apps.menu_popup().xdg_surface(),
        (100, 10),
        (80, 60),
        serial,
    );
    until(&mut apps.a_queue, &mut apps.a, |_| {
        heard(&apps.ear).entered.as_ref() == Some(&submenu.wl_surface().id())
    });
    configured(&mut apps.a, &mut apps.a_queue, 2);
    apps.a.paint(submenu.wl_surface(), (80, 60), 0xff99_6633);
    submenu.wl_surface().commit();
    apps.a_queue.flush().expect("sent");
    let (page, menu) = (apps.page, apps.menu());
    let facts = session.wait_for(|facts| popups_of(facts, page).len() == 2);
    let sub = *popups_of(&facts, page)
        .iter()
        .find(|popup| **popup != menu)
        .expect("the submenu");
    let host = Host::new(&session.facts, &session.requests);

    assert_eq!(
        host.act(apps.elsewhere, &click()).map(|_| ()),
        Err(ActError::MenuOpen { popup: sub })
    );
    assert_eq!(
        host.act(
            apps.page,
            &Verb::Type {
                text: "x".to_owned()
            }
        )
        .map(|_| ()),
        Err(ActError::FocusElsewhere { focused: Some(sub) })
    );

    apps.stop(session);
}

/// A menu that a click outside has dismissed holds nothing, though its
/// client has not destroyed it yet and smithay still counts its grab: an
/// agent's click on another application lands.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_a_click_outside_dismissed_holds_nothing() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Mods};

    let session = seated("held-menu-dismissed");
    let mut apps = Apps::with_the_menu(&session);
    // The person clicks the empty desk, past every window.
    session.command(Command::Click {
        at: (1270, 1014),
        button: Button::Left,
        mods: Mods::default(),
    });
    until(&mut apps.a_queue, &mut apps.a, |a| a.popups_done == 1);
    assert!(
        apps.menu_surface().is_alive(),
        "its client has not destroyed it"
    );

    Host::new(&session.facts, &session.requests)
        .act(apps.elsewhere, &click())
        .expect("clicked");
    until(&mut apps.b_queue, &mut apps.b, |b| b.buttons.len() == 2);

    apps.stop(session);
}

/// A person's button down is named before the menu: the person is using
/// the seat, and waiting for them is the answer until they let go. Then
/// the menu is.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_persons_hand_is_named_before_the_menu() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Mods};

    let session = seated("held-menu-hand");
    let mut apps = Apps::with_the_menu(&session);
    let menu = apps.menu();
    let on_menu = session
        .facts
        .read()
        .surface(menu)
        .expect("described")
        .geometry;
    // Down on the menu, the menu's own client: it stays open.
    session.command(Command::Press {
        at: (
            ((on_menu.x0 + on_menu.x1) / 2.0).round() as i32,
            ((on_menu.y0 + on_menu.y1) / 2.0).round() as i32,
        ),
        button: Button::Left,
        mods: Mods::default(),
    });
    until(&mut apps.a_queue, &mut apps.a, |a| a.buttons.len() == 1);
    let host = Host::new(&session.facts, &session.requests);
    assert_eq!(
        host.act(apps.elsewhere, &click()).map(|_| ()),
        Err(ActError::PersonActive)
    );

    session.command(Command::Release {
        button: Button::Left,
    });
    until(&mut apps.a_queue, &mut apps.a, |a| a.buttons.len() == 2);
    assert_eq!(
        host.act(apps.elsewhere, &click()).map(|_| ()),
        Err(ActError::MenuOpen { popup: menu })
    );
    assert_eq!(apps.a.popups_done, 0, "the menu is still open");

    apps.stop(session);
}

/// While a menu of "a" holds the keyboard, an agent's focus on anything but
/// its window and the menu is refused, naming the menu: "b"'s window, popup
/// and panel, and "a"'s other window. smithay's grab would have ignored it,
/// and the keys stay in the menu. Before, each was reported as done, and
/// the window recorded as focused, though it never was.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agent_cannot_focus_past_a_menu() {
    let session = seated("held-menu-focus");
    let mut apps = Apps::with_the_menu(&session);
    let host = Host::new(&session.facts, &session.requests);
    let before = session.facts.read();

    let refused = Err(ActError::MenuOpen { popup: apps.menu() });
    for target in [apps.elsewhere, apps.tooltip, apps.panel, apps.own] {
        assert_eq!(
            host.act(target, &Verb::Focus).map(|_| ()),
            refused,
            "focus on {target:?}"
        );
    }

    apps.settle();
    assert!(apps.keys_on(apps.menu_surface()));
    let after = session.facts.read();
    for window in [apps.elsewhere, apps.own] {
        let focused_at = |facts: &HostFacts| facts.surface(window).map(|w| w.focused_at);
        assert_eq!(
            focused_at(&after),
            focused_at(&before),
            "{window:?} recorded as focused"
        );
    }
    assert_eq!(apps.a.popups_done, 0, "the menu is still open");

    apps.stop(session);
}

/// The menu's own window may still be focused, and the menu itself: the
/// window is already the one in use, and the menu holds the keys already.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_menus_own_window_and_the_menu_may_still_be_focused() {
    let session = seated("held-menu-own-focus");
    let mut apps = Apps::with_the_menu(&session);
    let host = Host::new(&session.facts, &session.requests);

    let focused = host.act(apps.page, &Verb::Focus).expect("its window");
    assert_eq!(focused.focus_after, Some(apps.page));
    host.act(apps.menu(), &Verb::Focus).expect("the menu");

    apps.settle();
    assert!(apps.keys_on(apps.menu_surface()));
    assert_eq!(apps.a.popups_done, 0, "the menu is still open");

    apps.stop(session);
}

/// A tab brought forward is given the keyboard, so past a menu it is
/// refused as focus is: a tab behind the menu's window would put that
/// window away, and its menu with it, and another application's would be
/// raised over the menu. The menu's own window is the front of its group
/// already, and bringing it forward does nothing.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn tab_forward_past_a_menu_is_refused() {
    let session = seated("held-menu-forward");
    let mut apps = Apps::start(&session);
    // "page", focused, joins "own", the window focused before it, in front.
    session.perform(Action::TabWithPrevious);
    let (page, own) = (apps.page, apps.own);
    session.wait_for(|facts| {
        facts.surface(own).is_some_and(|own| !own.mapped)
            && facts.surface(page).is_some_and(|page| page.mapped)
    });
    apps.open_the_menu(&session);
    let host = Host::new(&session.facts, &session.requests);

    let refused = Err(ActError::MenuOpen { popup: apps.menu() });
    assert_eq!(host.act(own, &Verb::Forward).map(|_| ()), refused);
    assert_eq!(
        host.act(apps.elsewhere, &Verb::Forward).map(|_| ()),
        refused
    );
    host.act(page, &Verb::Forward).expect("in front already");

    apps.settle();
    let facts = session.facts.read();
    assert!(
        facts.surface(page).is_some_and(|page| page.mapped),
        "the menu's window is still in front"
    );
    assert_eq!(apps.a.popups_done, 0, "the menu is still open");

    apps.stop(session);
}

/// Closing a window asks its client to, and gives nothing the pointer or
/// the keyboard: past a menu it still goes ahead, and the menu stays open.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn window_close_still_reaches_past_a_menu() {
    let session = seated("held-menu-close");
    let mut apps = Apps::with_the_menu(&session);

    Host::new(&session.facts, &session.requests)
        .act(apps.elsewhere, &Verb::Close)
        .expect("asked to close");
    until(&mut apps.b_queue, &mut apps.b, |b| b.asked_to_close == 1);

    apps.settle();
    assert_eq!(apps.a.popups_done, 0, "the menu is still open");

    apps.stop(session);
}
