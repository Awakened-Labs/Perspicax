//! Key bindings on a window whose own menu is open, by a compositor with no
//! screen.
//!
//! A right-click menu takes the keyboard, and smithay names the menu as
//! holding it, not the window. Issue #114: every binding on "the focused
//! window" asked for a window with the keyboard, found none while the menu
//! was open, and did nothing. Alt+F4 left the window open; Alt+Tab focused
//! the window it came from, which the menu's grab ignored. Now a binding on
//! the window closes the menu first, as a click outside it would, and acts
//! on the window it was opened from. A binding that is not about the window
//! leaves the menu open.
//!
//! Driven by `Command::Perform`, the same code a person's key bindings run,
//! with a person at the seat, where alone a menu takes the keyboard. Like
//! the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Ear, Said, Session, heard, menu_with_the_keyboard, until};
use perspicax_compositor::{Backend, Command, Keymap, Virtual};
use perspicax_index::{HostFacts, SurfaceFacts};
use perspicax_node::SurfaceId;
use perspicax_policy::{Access, Action, Direction, Grid, Mode, Shape};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{EventQueue, Proxy as _, QueueHandle};

/// One monitor, two workspaces side by side, and a person at it.
fn seated(name: &str) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![Virtual::numbered(1, (1280, 1024))],
            workspaces: Shape {
                mode: Mode::Spanning,
                grid: Grid {
                    columns: 2,
                    rows: 1,
                    wrap: false,
                },
            },
            access: Access::open(),
            person: true,
            opacity: Default::default(),
            agent: Default::default(),
        },
    )
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
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
        titled(&session.facts.read(), title).is_some_and(|window| window.mapped)
    });
    titled(&session.facts.read(), title).expect("waited for").id
}

/// A client with the keyboard bound and one window, "page", open with it.
fn page(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, Ear, SurfaceId) {
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    let page = opened(session, &mut desk, &mut queue, &qh, "page");
    (desk, queue, qh, ear, page)
}

/// The same, with a second window, "other", opened after it, then "page"
/// brought forward and given the keyboard again, for its menu to take, as
/// Alt+Tab brings it.
fn page_and_other(
    session: &Session,
) -> (
    Desk,
    EventQueue<Desk>,
    QueueHandle<Desk>,
    Ear,
    SurfaceId,
    SurfaceId,
) {
    let (mut desk, mut queue, qh, ear, page) = page(session);
    let other = opened(session, &mut desk, &mut queue, &qh, "other");
    until(&mut queue, &mut desk, |desk| on_window(desk, &ear, 1));
    session.perform(Action::CycleFocus);
    until(&mut queue, &mut desk, |desk| on_window(desk, &ear, 0));
    (desk, queue, qh, ear, page, other)
}

/// Whether the keyboard is on the `nth` window's own surface, as its client
/// last heard.
fn on_window(desk: &Desk, ear: &Ear, nth: usize) -> bool {
    heard(ear).entered.as_ref() == Some(&desk.windows[nth].wl_surface().id())
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn close_closes_the_windows_menu_and_asks_it_to_close() {
    let session = seated("menu-open-close");
    let (mut desk, mut queue, qh, ear, page) = page(&session);
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    session.perform(Action::Close);
    until(&mut queue, &mut desk, |desk| {
        desk.asked_to_close == 1 && desk.popups_done == 1 && on_window(desk, &ear, 0)
    });
    let told = desk.activations_of(0);
    assert!(
        told.iter().all(|&active| active),
        "told it was not active as its menu closed: {told:?}"
    );

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn maximize_closes_the_menu_first() {
    let session = seated("menu-open-maximize");
    let (mut desk, mut queue, qh, ear, page) = page(&session);
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    session.perform(Action::ToggleMaximize);
    until(&mut queue, &mut desk, |desk| {
        desk.popups_done == 1
            && desk
                .said_to(desk.windows[0].wl_surface())
                .iter()
                .any(|said| matches!(said, Said::Configure(configure) if configure.maximized))
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn minimize_closes_the_menu_and_the_keyboard_moves_on() {
    let session = seated("menu-open-minimize");
    let (mut desk, mut queue, qh, ear, page, other) = page_and_other(&session);
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    session.perform(Action::Minimize);
    session.wait_for(|facts| {
        facts
            .surface(page)
            .is_some_and(|page| !page.mapped && page.off_workspace.is_none())
            && facts.surface(other).is_some_and(|other| other.mapped)
    });
    until(&mut queue, &mut desk, |desk| {
        desk.popups_done == 1 && on_window(desk, &ear, 1)
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn sending_the_window_away_closes_its_menu_and_the_keyboard_moves_on() {
    let session = seated("menu-open-send");
    let (mut desk, mut queue, qh, ear, page, other) = page_and_other(&session);
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    session.perform(Action::SendToWorkspace(Direction::Right));
    session.wait_for(|facts| {
        facts
            .surface(page)
            .is_some_and(|page| page.off_workspace == Some(2))
            && facts.surface(other).is_some_and(|other| other.mapped)
    });
    until(&mut queue, &mut desk, |desk| {
        desk.popups_done == 1 && on_window(desk, &ear, 1)
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn cycling_focus_closes_the_menu_and_goes_to_the_next_window() {
    let session = seated("menu-open-cycle");
    let (mut desk, mut queue, qh, ear, page, _) = page_and_other(&session);
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    session.perform(Action::CycleFocus);
    until(&mut queue, &mut desk, |desk| {
        desk.popups_done == 1 && on_window(desk, &ear, 1)
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn tabbing_with_the_previous_window_closes_the_menu() {
    let session = seated("menu-open-tab");
    let (mut desk, mut queue, qh, ear, page, other) = page_and_other(&session);
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    // "other" was focused before "page": "page" joins it, in front.
    session.perform(Action::TabWithPrevious);
    session.wait_for(|facts| {
        facts
            .surface(other)
            .is_some_and(|other| other.behind_tab == Some(page))
    });
    until(&mut queue, &mut desk, |desk| desk.popups_done == 1);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_step_of_opacity_closes_the_menu() {
    let session = seated("menu-open-opacity");
    let (mut desk, mut queue, qh, ear, page) = page(&session);
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    session.perform(Action::OpacityDown);
    session.wait_for(|facts| {
        facts
            .surface(page)
            .is_some_and(|page| page.drawn_opacity == Some(90))
    });
    until(&mut queue, &mut desk, |desk| {
        desk.popups_done == 1 && on_window(desk, &ear, 0)
    });

    session.stop((desk, queue));
}

/// A submenu holding the keyboard closes with the menu it opened from, and
/// the window gets the keyboard, not the menu below.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn close_from_a_submenu_closes_both_menus() {
    let session = seated("menu-open-submenu");
    let (mut desk, mut queue, qh, ear, page) = page(&session);
    let menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);
    // A submenu opens on an event of its own, the hover over its item, and
    // smithay tells nested grabs apart by their serials: not the menu's.
    let serial = desk.popup_serial.expect("the menu was configured");
    let submenu = desk.open_menu(&qh, menu.xdg_surface(), (100, 10), (80, 60), serial);
    let sub = submenu.wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&sub)
    });

    session.perform(Action::Close);
    until(&mut queue, &mut desk, |desk| {
        desk.asked_to_close == 1 && desk.popups_done == 2 && on_window(desk, &ear, 0)
    });

    session.stop((desk, queue));
}

/// A binding that is not about the window leaves its menu open: switched
/// with the menu open, the layout is the menu's window's, as #97 has it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_layout_switched_with_a_menu_open_leaves_it_open() {
    let session = seated("menu-open-layout");
    session.command(Command::Keymap(Keymap {
        layout: "us,ru".to_owned(),
        ..Keymap::default()
    }));
    let (mut desk, mut queue, qh, ear, page) = page(&session);
    until(&mut queue, &mut desk, |_| heard(&ear).layouts == 2);
    let menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    session.perform(Action::Layout(2));
    until(&mut queue, &mut desk, |_| heard(&ear).group() == 1);
    queue.roundtrip(&mut desk).expect("flush");
    queue
        .roundtrip(&mut desk)
        .expect("and again, past an idle turn");
    assert_eq!(desk.popups_done, 0, "the menu was closed");
    assert_eq!(
        heard(&ear).entered.as_ref(),
        Some(&menu.wl_surface().id()),
        "the keyboard left the menu"
    );

    session.stop((desk, queue));
}

/// A menu dismissed by a click outside it, which its client has yet to
/// destroy, holds nothing: the keyboard went back to its window, and from
/// there to another. Smithay counts the menu's grab as held until its
/// client destroys it, which this client never does. Asked that, a binding
/// gave the keyboard back to the menu's window and acted on it, not on the
/// window that has the keyboard.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_dismissed_but_not_destroyed_leaves_bindings_to_the_window_with_the_keyboard() {
    use perspicax_compositor::Host;
    use perspicax_index::Action as Verb;
    use perspicax_policy::{Button, Mods};

    let session = seated("menu-open-dismissed");
    let (mut desk, mut queue, qh, ear, page, other) = page_and_other(&session);
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    // Dismissed by a click on the empty desk, and left undestroyed.
    let desktop = (1270, 1014);
    let facts = session.facts.read();
    for window in [page, other] {
        let at = facts.surface(window).expect("open").geometry;
        assert!(f64::from(desktop.0) > at.x1 && f64::from(desktop.1) > at.y1);
    }
    session.command(Command::Click {
        at: desktop,
        button: Button::Left,
        mods: Mods::default(),
    });
    until(&mut queue, &mut desk, |desk| {
        desk.popups_done == 1 && on_window(desk, &ear, 0)
    });
    Host::new(&session.facts, &session.requests)
        .act(other, &Verb::Focus)
        .expect("focused");
    until(&mut queue, &mut desk, |desk| on_window(desk, &ear, 1));

    session.perform(Action::Minimize);
    session.wait_for(|facts| {
        facts.surface(other).is_some_and(|other| !other.mapped)
            && facts.surface(page).is_some_and(|page| page.mapped)
    });

    session.stop((desk, queue));
}
