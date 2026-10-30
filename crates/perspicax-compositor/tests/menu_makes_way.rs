//! A window's menu holding the keyboard, as the person meets it when the
//! keys go somewhere else, by a compositor with no screen.
//!
//! A right-click menu takes the pointer and keyboard until it closes, and
//! while it has them smithay's grab decides where input goes: the menu's own
//! application takes the pointer as it would without the menu, a press
//! anywhere else closes the menu, and the keyboard goes to the menu alone.
//! Issue #185: given to anything else, the keyboard was ignored. The hand
//! passing over another window under focus-follows-mouse is not giving it
//! the keys, and leaves the menu open, the window under the hand unraised.
//!
//! With a person at the seat, where alone a menu takes the grab. The hand
//! finds what is under it from the clients' buffers, so this needs the
//! `capture` feature. Like the other live tests it binds a real Wayland
//! socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Ear, Session, heard, menu_with_the_keyboard, until};
use perspicax_compositor::{Backend, Command};
use perspicax_index::HostFacts;
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::{Focus, FocusModel};
use smithay_client_toolkit::shell::{WaylandSurface as _, xdg::popup::Popup};
use wayland_client::{EventQueue, Proxy as _, QueueHandle};

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
fn geometry(facts: &HostFacts, title: &str) -> Rect {
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

/// Two applications at the seat.
///
/// "b" has a window, "b", a taskbar, a pointer and a keyboard. "a" has a
/// window, "a", opened after it and over it with the keys, and a pointer and
/// keyboard of its own; and once [`Pair::open_the_menu`] has run, a menu of
/// "a" holding the pointer and keyboard.
struct Pair {
    a: Desk,
    a_queue: EventQueue<Desk>,
    a_qh: QueueHandle<Desk>,
    a_ear: Ear,
    b: Desk,
    b_queue: EventQueue<Desk>,
    b_ear: Ear,
    a_id: SurfaceId,
    /// The menu of "a", once open: the client's own handle, kept so that it
    /// stays open.
    menu: Option<Popup>,
}

impl Pair {
    fn start(session: &Session) -> Self {
        let (mut b, mut b_queue, b_qh, b_globals) = session.client();
        b.bind_taskbar(&b_globals, &b_qh);
        b.bind_pointer(&b_globals, &b_qh);
        let b_ear = b.bind_keyboard(&b_globals, &b_qh);
        opened(session, &mut b, &mut b_queue, &b_qh, "b");
        // Nothing had the keys, so the first window has them (#259).
        let b_surface = b.windows[0].wl_surface().id();
        until(&mut b_queue, &mut b, |_| {
            heard(&b_ear).entered.as_ref() == Some(&b_surface)
        });

        let (mut a, mut a_queue, a_qh, a_globals) = session.client();
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
            b_ear,
            a_id,
            menu: None,
        }
    }

    /// Open the menu of "a" that holds the pointer and keyboard, as a
    /// right-click does.
    fn open_the_menu(&mut self, session: &Session) {
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

    fn stop(self, session: Session) {
        session.stop(((self.a, self.a_queue), (self.b, self.b_queue)));
    }
}

/// Under focus-follows-mouse with autoraise, the hand passing from the menu
/// over "b" leaves the menu open, with the keys, and "b" where it was:
/// while the menu holds the pointer, smithay tells "b" nothing, and the
/// hand over it is not pointing at it. Before, "b" was raised over the
/// menu, and handed keys the menu's grab ignored.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_hand_passing_over_another_window_leaves_the_menu_open() {
    let session = seated("menu-makes-way-hover");
    let mut pair = Pair::with_the_menu(&session);
    let over_b = Pair::b_alone(&session);
    session.command(Command::FocusPolicy(Focus {
        model: FocusModel::Sloppy,
        autoraise: true,
    }));

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
