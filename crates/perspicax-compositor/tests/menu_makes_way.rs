//! A window's menu holding the keyboard, as the person meets it when the
//! keys go somewhere else, by a compositor with no screen.
//!
//! A right-click menu takes the pointer and keyboard until it closes, and
//! while it has them smithay's grab decides where input goes: the menu's own
//! application takes the pointer as it would without the menu, a press
//! anywhere else closes the menu, and the keyboard goes to the menu alone.
//! Issue #185: given to anything else, the keyboard was ignored. A click on
//! another window raised it and left the keys behind, and a launcher, a
//! window the person asked for or one activated came up without them. Now
//! the keyboard going anywhere but the menu's own window and menus closes
//! the menu first, as a click outside it would. The hand passing over
//! another window under focus-follows-mouse is not giving it the keys, and
//! leaves the menu open, the window under the hand unraised.
//!
//! With a person at the seat, where alone a menu takes the grab. The hand
//! finds what is under it from the clients' buffers, so its tests need the
//! `capture` feature. Like the other live tests these bind a real Wayland
//! socket, so they need `XDG_RUNTIME_DIR`, and are `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Ear, Session, Told, heard, menu_with_the_keyboard, until};
use perspicax_compositor::Backend;
use perspicax_index::HostFacts;
use perspicax_node::SurfaceId;
use perspicax_policy::Action;
use smithay_client_toolkit::shell::{WaylandSurface as _, xdg::popup::Popup};
use wayland_client::{EventQueue, Proxy as _, QueueHandle, backend::ObjectId};

/// Linux's code for the left button.
#[cfg(feature = "capture")]
const BTN_LEFT: u32 = 0x110;

/// One monitor, with a person at it: only a person's menu takes the grab.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).with_person())
}

/// The mapped window titled `title`.
fn titled(facts: &HostFacts, title: &str) -> Option<SurfaceId> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title) && surface.mapped)
        .map(|surface| surface.id)
}

/// Where the window titled `title` is, on the desk.
#[cfg(feature = "capture")]
fn geometry(facts: &HostFacts, title: &str) -> perspicax_node::Rect {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title) && surface.mapped)
        .expect("a window of that title")
        .geometry
}

/// The title of the window on top, the facts giving them bottom to top.
fn on_top(facts: &HostFacts) -> Option<String> {
    facts
        .surfaces()
        .iter()
        .rfind(|surface| surface.mapped && surface.title.is_some())?
        .title
        .clone()
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

/// Put a focus policy in force, as a seat does from `[focus]`.
#[cfg(feature = "capture")]
fn policy(session: &Session, model: perspicax_policy::FocusModel, autoraise: bool) {
    session.command(perspicax_compositor::Command::FocusPolicy(
        perspicax_policy::Focus { model, autoraise },
    ));
}

/// A person's click at `at`.
#[cfg(feature = "capture")]
fn click(session: &Session, at: (i32, i32), button: perspicax_policy::Button) {
    session.command(perspicax_compositor::Command::Click {
        at,
        button,
        mods: perspicax_policy::Mods::default(),
    });
}

/// Two applications at the seat.
///
/// "b" has a window, "b", a taskbar, the shell's channel, a pointer and a
/// keyboard: it stands in for the shell too. "a" has a window, "a", opened
/// after it and over it with the keys, and a pointer and keyboard of its
/// own; and once [`Pair::open_the_menu`] has run, a menu of "a" holding the
/// pointer and keyboard.
struct Pair {
    a: Desk,
    a_queue: EventQueue<Desk>,
    a_qh: QueueHandle<Desk>,
    a_ear: Ear,
    b: Desk,
    b_queue: EventQueue<Desk>,
    b_qh: QueueHandle<Desk>,
    b_ear: Ear,
    a_id: SurfaceId,
    /// The menu of "a", once open: the client's own handle, kept so that it
    /// stays open.
    menu: Option<Popup>,
    /// The serial the menu grabbed with: the window's keyboard enter.
    menu_serial: Option<u32>,
}

impl Pair {
    fn start(session: &Session) -> Self {
        let (mut b, mut b_queue, b_qh, b_globals) = session.client();
        b.bind_taskbar(&b_globals, &b_qh);
        b.bind_shell(&b_globals, &b_qh);
        b.bind_activation(&b_globals, &b_qh);
        b.bind_pointer(&b_globals, &b_qh);
        let b_ear = b.bind_keyboard(&b_globals, &b_qh);
        opened(session, &mut b, &mut b_queue, &b_qh, "b");
        // Nothing had the keys, so the first window has them (#259).
        let b_surface = b.windows[0].wl_surface().id();
        until(&mut b_queue, &mut b, |_| {
            heard(&b_ear).entered.as_ref() == Some(&b_surface)
        });

        let (mut a, mut a_queue, a_qh, a_globals) = session.client();
        a.bind_activation(&a_globals, &a_qh);
        a.bind_pointer(&a_globals, &a_qh);
        let a_ear = a.bind_keyboard(&a_globals, &a_qh);
        let a_id = opened(session, &mut a, &mut a_queue, &a_qh, "a");
        // Picked in the taskbar of "b", the program in use (#270): it opened
        // without the keys, "b" having them (#259), and has them, and the
        // top of the stack, once the person gives them.
        until(&mut b_queue, &mut b, |b| b.task("a").is_some());
        b.activate("a");
        b_queue.roundtrip(&mut b).expect("the pick sent");
        let a_surface = a.windows[0].wl_surface().id();
        until(&mut a_queue, &mut a, |_| {
            heard(&a_ear).entered.as_ref() == Some(&a_surface)
        });
        Self {
            a,
            a_queue,
            a_qh,
            a_ear,
            b,
            b_queue,
            b_qh,
            b_ear,
            a_id,
            menu: None,
            menu_serial: None,
        }
    }

    /// Open the menu of "a" that holds the pointer and keyboard, as a
    /// right-click does.
    fn open_the_menu(&mut self, session: &Session) {
        // What `menu_with_the_keyboard` grabs with.
        self.menu_serial = heard(&self.a_ear).enter_serial;
        self.menu = Some(menu_with_the_keyboard(
            session,
            &mut self.a,
            &mut self.a_queue,
            &self.a_qh,
            &self.a_ear,
            self.a_id,
        ));
    }

    /// Both of them, with the menu open.
    fn with_the_menu(session: &Session) -> Self {
        let mut pair = Self::start(session);
        pair.open_the_menu(session);
        pair
    }

    /// A point of "b" that "a", and so its menu, does not cover: a corner.
    #[cfg(feature = "capture")]
    fn b_alone(session: &Session) -> (i32, i32) {
        let facts = session.wait_for(|_| true);
        let (a, b) = (geometry(&facts, "a"), geometry(&facts, "b"));
        let inside = |(x, y): (f64, f64)| x >= a.x0 && x < a.x1 && y >= a.y0 && y < a.y1;
        [
            (b.x0 + 5.0, b.y0 + 5.0),
            (b.x1 - 5.0, b.y0 + 5.0),
            (b.x0 + 5.0, b.y1 - 5.0),
            (b.x1 - 5.0, b.y1 - 5.0),
        ]
        .into_iter()
        .find(|&corner| !inside(corner))
        .map(|(x, y)| (x as i32, y as i32))
        .unwrap_or_else(|| panic!("{a:?} covers every corner of {b:?}"))
    }

    /// Show a launcher on "b", as the shell shows its start menu or fuzzel
    /// shows itself: a strip over everything that asks for every key,
    /// drawn. Its surface.
    fn show_launcher(&mut self) -> ObjectId {
        let before = self.b.layers_drawn;
        self.b.open_launcher(&self.b_qh, 40, 0xff20_4060);
        let surface = self
            .b
            .layers
            .last()
            .expect("just opened")
            .0
            .wl_surface()
            .id();
        until(&mut self.b_queue, &mut self.b, |b| b.layers_drawn > before);
        surface
    }

    /// Let both clients hear whatever the compositor sent them.
    fn settle(&mut self) {
        for _ in 0..3 {
            self.b_queue.roundtrip(&mut self.b).expect("b's round trip");
            self.a_queue.roundtrip(&mut self.a).expect("a's round trip");
        }
    }

    /// Whether the keyboard is in the menu, as "a" last heard.
    fn keys_in_the_menu(&self) -> bool {
        let menu = self.menu.as_ref().expect("the menu is open").wl_surface();
        heard(&self.a_ear).entered.as_ref() == Some(&menu.id())
    }

    /// Whether the keyboard is on the `nth` window of "a", as it last heard.
    fn keys_on_a(&self, nth: usize) -> bool {
        heard(&self.a_ear).entered.as_ref() == Some(&self.a.windows[nth].wl_surface().id())
    }

    /// Whether the keyboard is on the window of "b", as it last heard.
    fn keys_on_b(&self) -> bool {
        heard(&self.b_ear).entered.as_ref() == Some(&self.b.windows[0].wl_surface().id())
    }

    fn stop(self, session: Session) {
        session.stop(((self.a, self.a_queue), (self.b, self.b_queue)));
    }
}

/// The issue's own case: a menu of "a" is open, and the person clicks "b".
/// The menu closes, "b" comes to the top with the keys, and the click
/// reaches it. Before, "b" came to the top and the keys went back to "a",
/// so what the person typed next went to the window behind.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_on_another_window_closes_the_menu_and_gives_that_window_the_keys() {
    use perspicax_policy::{Button, FocusModel};

    let session = seated("menu-makes-way-click");
    let mut pair = Pair::with_the_menu(&session);
    let at = Pair::b_alone(&session);
    policy(&session, FocusModel::Click, false);

    click(&session, at, Button::Left);
    until(&mut pair.a_queue, &mut pair.a, |a| a.popups_done == 1);
    until(&mut pair.b_queue, &mut pair.b, |b| b.buttons.len() == 2);
    pair.settle();

    assert!(pair.keys_on_b(), "the keys went with the click");
    assert!(heard(&pair.a_ear).entered.is_none(), "and left a");
    assert_eq!(pair.b.buttons, [(BTN_LEFT, true), (BTN_LEFT, false)]);
    let facts = session.wait_for(|_| true);
    assert_eq!(on_top(&facts).as_deref(), Some("b"));

    pair.stop(session);
}

/// A right-click on "b" while a menu of "a" is open opens the menu of "b":
/// "b" has the keys by the time its click reaches it, so its menu may take
/// them (#187). Before, the keys were still with "a", and the menu of "b"
/// was dismissed as one asked for by a program without them.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_right_click_on_another_window_opens_that_windows_menu() {
    use perspicax_policy::{Button, FocusModel};
    use smithay_client_toolkit::shell::xdg::XdgSurface as _;

    let session = seated("menu-makes-way-right-click");
    let mut pair = Pair::with_the_menu(&session);
    let at = Pair::b_alone(&session);
    policy(&session, FocusModel::Click, false);

    click(&session, at, Button::Right);
    until(&mut pair.b_queue, &mut pair.b, |b| b.pressed.is_some());
    let serial = pair.b.pressed.expect("waited for");
    let menu = pair.b.open_menu(
        &pair.b_qh,
        pair.b.windows[0].xdg_surface(),
        (10, 10),
        (120, 80),
        serial,
    );
    pair.settle();

    assert_eq!(pair.b.popups_done, 0, "the menu of b was not dismissed");
    assert_eq!(
        heard(&pair.b_ear).entered.as_ref(),
        Some(&menu.wl_surface().id()),
        "it has the keys"
    );
    assert_eq!(pair.a.popups_done, 1, "and the menu of a closed");

    pair.stop(session);
}

/// A click on the window the menu was opened from is that application's
/// own: it reaches the window, and the menu stays open, as it does under
/// smithay's grab, for the application to close it if it means to.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_on_the_menus_own_window_goes_to_it() {
    use perspicax_policy::{Button, FocusModel};

    let session = seated("menu-makes-way-own");
    let mut pair = Pair::with_the_menu(&session);
    let facts = session.wait_for(|_| true);
    let a = geometry(&facts, "a");
    // Its bottom right, past the menu.
    let at = ((a.x1 - 10.0) as i32, (a.y1 - 10.0) as i32);
    policy(&session, FocusModel::Click, false);

    click(&session, at, Button::Left);
    until(&mut pair.a_queue, &mut pair.a, |a| a.buttons.len() == 2);
    pair.settle();

    assert_eq!(pair.a.popups_done, 0, "the menu is still open");
    assert!(pair.keys_in_the_menu(), "with the keys");
    assert!(pair.b.buttons.is_empty());

    pair.stop(session);
}

/// The person's key asks the shell for its start menu while a window's
/// menu is open. The start menu takes the keys as it shows, the person
/// having asked for it (#267), and the window's menu closes. Once the start
/// menu goes, the keys go back to the window. Before, the start menu showed
/// and the keys stayed in the window's menu.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_the_person_asks_for_closes_the_menu_and_takes_the_keys() {
    let session = seated("menu-makes-way-launcher");
    let mut pair = Pair::with_the_menu(&session);

    // The binding hands the person's press on to the shell, "b".
    session.press_bound(Action::StartMenu);
    until(&mut pair.b_queue, &mut pair.b, |b| {
        b.told.iter().any(|told| matches!(told, Told::StartMenu(_)))
    });
    let launcher = pair.show_launcher();
    pair.settle();

    assert_eq!(
        heard(&pair.b_ear).entered.as_ref(),
        Some(&launcher),
        "the start menu has the keys"
    );
    assert_eq!(pair.a.popups_done, 1, "and the window's menu closed");

    pair.b.layers.clear();
    pair.b_queue.roundtrip(&mut pair.b).expect("closed");
    until(&mut pair.a_queue, &mut pair.a, |pair_a| {
        heard(&pair.a_ear).entered.as_ref() == Some(&pair_a.windows[0].wl_surface().id())
    });

    pair.stop(session);
}

/// A launcher nobody asked for takes no keys as it shows (#267), so it
/// closes no menu either: a program cannot close the person's menu by
/// showing one.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_nobody_asked_for_leaves_the_menu_open() {
    let session = seated("menu-makes-way-launcher-unasked");
    let mut pair = Pair::with_the_menu(&session);

    pair.show_launcher();
    pair.settle();

    assert_eq!(pair.a.popups_done, 0, "the menu is still open");
    assert!(pair.keys_in_the_menu(), "with the keys");
    assert!(heard(&pair.b_ear).entered.is_none());

    pair.stop(session);
}

/// The application whose menu is open opens another window: it has the
/// keys, so the window is its, asked for (#259), and takes them as it
/// opens; the menu closes. Another window of the same application is not
/// the menu's: had the application closed the menu itself, the keys would
/// have gone back to the window it was opened from. Before, the window
/// opened and the keys stayed in the menu.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_the_menus_application_opens_takes_the_keys_and_closes_the_menu() {
    let session = seated("menu-makes-way-opens");
    let mut pair = Pair::with_the_menu(&session);

    opened(
        &session,
        &mut pair.a,
        &mut pair.a_queue,
        &pair.a_qh,
        "another",
    );
    until(&mut pair.a_queue, &mut pair.a, |a| a.has_drawn(1));
    pair.settle();

    assert!(pair.keys_on_a(1), "the new window has the keys");
    assert_eq!(pair.a.popups_done, 1, "and the menu closed");

    pair.stop(session);
}

/// A token made by "a" while its menu has the keys, as "Open link" picked
/// from it makes one, and spent by "b", as the browser handed the link
/// does. "b" comes forward with the keys, and the menu closes, whether or
/// not "a" got round to closing it. Before, "b" came forward without them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_activated_while_a_menu_is_open_takes_the_keys_and_closes_it() {
    let session = seated("menu-makes-way-activated");
    let mut pair = Pair::with_the_menu(&session);

    let serial = heard(&pair.a_ear).enter_serial.expect("the menu's");
    pair.a.make_token(&pair.a_qh, 0, serial);
    until(&mut pair.a_queue, &mut pair.a, |a| !a.tokens.is_empty());
    let token = pair.a.tokens.remove(0);
    pair.b.spend_token(&token, 0);
    pair.b_queue.roundtrip(&mut pair.b).expect("spent");
    session.wait_for(|facts| on_top(facts).as_deref() == Some("b"));
    pair.settle();

    assert!(pair.keys_on_b(), "b has the keys");
    assert_eq!(pair.a.popups_done, 1, "and the menu closed");

    pair.stop(session);
}

/// A `[mouse]` binding on the desktop acts on the click that closes the
/// menu: here, the thumb button asking the shell for its start menu. The
/// press is past the menu's application, so the menu closes first, as
/// smithay's grab would close it, and the press is then the compositor's to
/// look a binding up for. Before, no binding was looked for while the menu
/// held the pointer, and the click did nothing but close it.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_desktop_binding_acts_on_the_click_that_closes_the_menu() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Context, Gesture, Mods, MouseBindings, MouseChord};

    let session = seated("menu-makes-way-binding");
    let mut pair = Pair::with_the_menu(&session);
    session.command(Command::MouseBindings(MouseBindings::default().bind(
        Context::Desktop,
        MouseChord {
            mods: Mods::default(),
            gesture: Gesture::Press(Button::Side),
        },
        Action::StartMenu,
    )));

    // The empty desk's far corner, past every window.
    click(&session, (1270, 1014), Button::Side);
    until(&mut pair.a_queue, &mut pair.a, |a| a.popups_done == 1);
    until(&mut pair.b_queue, &mut pair.b, |b| {
        b.told.iter().any(|told| matches!(told, Told::StartMenu(_)))
    });

    pair.stop(session);
}

/// A task picked in a panel while a window's menu is open: the press on
/// the panel closes the menu and reaches the panel, so its pick is one the
/// person asked for (#270), and the window picked comes forward with the
/// keys.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_task_picked_in_a_panel_while_a_menu_is_open_comes_forward_with_the_keys() {
    use perspicax_policy::Button;

    let session = seated("menu-makes-way-panel");
    let mut pair = Pair::with_the_menu(&session);
    let before = pair.b.layers_drawn;
    pair.b.open_panel(&pair.b_qh, "panel", 30, 0xff20_2020);
    until(&mut pair.b_queue, &mut pair.b, |b| b.layers_drawn > before);

    click(&session, (640, 10), Button::Left);
    until(&mut pair.b_queue, &mut pair.b, |b| b.buttons.len() == 2);
    until(&mut pair.a_queue, &mut pair.a, |a| a.popups_done == 1);
    pair.b.activate("b");
    pair.b_queue.roundtrip(&mut pair.b).expect("the pick sent");
    until(&mut pair.b_queue, &mut pair.b, |_| {
        heard(&pair.b_ear).entered.is_some()
    });
    pair.settle();

    assert!(pair.keys_on_b(), "the window picked has the keys");
    let facts = session.wait_for(|_| true);
    assert_eq!(on_top(&facts).as_deref(), Some("b"));

    pair.stop(session);
}

/// The application asks to move its window with its menu's own serial,
/// while the menu holds the pointer. That serial names the menu's grab, not
/// a press holding the pointer, and the move is refused: the menu stays
/// open, with the keys. Before, a move was started from the grab's made-up
/// start, the desk's origin with no button down. Its grab ended the menu's,
/// taking the keys back to the window with the menu still drawn, and the
/// window would have followed the hand from where nobody pressed.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_move_asked_for_with_the_menus_serial_is_refused() {
    let session = seated("menu-makes-way-move");
    let mut pair = Pair::with_the_menu(&session);

    let seat = pair.a.seat.clone().expect("a's seat");
    let serial = pair.menu_serial.expect("the menu's");
    pair.a.windows[0].move_(&seat, serial);
    pair.settle();

    assert_eq!(pair.a.popups_done, 0, "the menu is still open");
    assert!(pair.keys_in_the_menu(), "with the keys");

    pair.stop(session);
}

/// Under focus-follows-mouse with autoraise, the hand passing from the menu
/// over "b" leaves the menu open, with the keys, and "b" where it was:
/// while the menu holds the pointer, smithay tells "b" nothing, and the
/// hand over it is not pointing at it. Before, "b" was raised over the
/// menu, and handed keys the menu's grab ignored.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_hand_passing_over_another_window_leaves_the_menu_open() {
    use perspicax_compositor::Command;
    use perspicax_policy::FocusModel;

    let session = seated("menu-makes-way-hover");
    let mut pair = Pair::with_the_menu(&session);
    let over_b = Pair::b_alone(&session);
    policy(&session, FocusModel::Sloppy, true);

    // The hand starts at the origin.
    session.command(Command::Motion { by: over_b });
    // A key after it, which the menu hears: by then the motion is done with.
    session.command(Command::Type("x".to_owned()));
    until(&mut pair.a_queue, &mut pair.a, |_| {
        !heard(&pair.a_ear).keys.is_empty()
    });
    pair.settle();

    let facts = session.wait_for(|_| true);
    assert_eq!(on_top(&facts).as_deref(), Some("a"), "nothing was raised");
    assert_eq!(pair.a.popups_done, 0, "the menu is still open");
    assert!(pair.keys_in_the_menu(), "with the keys");
    assert!(heard(&pair.b_ear).entered.is_none(), "none of them in b");

    pair.stop(session);
}
