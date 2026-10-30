//! A person's hand with a button down, as an agent meets it, by a
//! compositor with no screen.
//!
//! A drag held still is still a drag. Until the button comes up, whatever
//! the press started has the pointer -- a window being moved or resized, a
//! selection in a client, a file being dragged -- and an agent's click
//! would go to it: the window carried off to where the agent aimed, and the
//! drag ended under the person's hand by a release they never made. So
//! every act an agent makes is refused while the person has a button down,
//! however long they have held it still, as it is while they move: the
//! person is using the seat.
//!
//! What is not a hand is a menu. A menu open holds the pointer too, with no
//! button down, and an agent may still click in it: that is how it chooses
//! an item from a menu it opened.
//!
//! The person's hand is [`Command::Press`] and [`Command::Release`], which
//! find what is under the pointer from the clients' buffers, so this needs
//! the `capture` feature. Like the other live tests it binds a real Wayland
//! socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Session, menu_with_the_keyboard, until};
use perspicax_compositor::{ActError, Backend, Command, Host};
use perspicax_index::{Action as Verb, HostFacts, PointerButton};
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::{Button, Mods};
use wayland_client::{EventQueue, QueueHandle};

const LEFT: u32 = Button::Left.code();

/// A spot inside any of our windows, in window space, and in the corner of
/// "back" that "front", cascaded over it from (32, 32), leaves uncovered.
const SPOT: Rect = Rect {
    x0: 10.0,
    y0: 10.0,
    x1: 20.0,
    y1: 20.0,
};

/// One monitor, with a person at it: only a person's press may start a move.
fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)).with_person())
}

/// Two windows, "back" and, cascaded over it with the keyboard, "front",
/// drawn and described, with our pointer bound.
fn with_windows(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    for (n, title) in ["back", "front"].into_iter().enumerate() {
        desk.open_window(&qh, title, title);
        until(&mut queue, &mut desk, |desk| desk.has_drawn(n));
    }
    session.wait_for(|facts| {
        ["back", "front"]
            .iter()
            .all(|title| geometry(facts, title).is_some())
    });
    (desk, queue, qh)
}

/// The window titled `title`: its id and where it is on the desk.
fn window(facts: &HostFacts, title: &str) -> (SurfaceId, Rect) {
    let surface = facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .unwrap_or_else(|| panic!("no window titled {title}"));
    (surface.id, surface.geometry)
}

fn geometry(facts: &HostFacts, title: &str) -> Option<Rect> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| surface.geometry)
        .filter(|geometry| geometry.x1 > geometry.x0)
}

/// A rect's centre, in whole pixels, for a person's hand to aim at.
fn centre(at: Rect) -> (i32, i32) {
    (
        ((at.x0 + at.x1) / 2.0).round() as i32,
        ((at.y0 + at.y1) / 2.0).round() as i32,
    )
}

/// The person's left button going down at `at`, and staying down.
fn press(session: &Session, at: (i32, i32)) {
    session.command(Command::Press {
        at,
        button: Button::Left,
        mods: Mods::default(),
    });
}

/// The person letting go of it.
fn release(session: &Session) {
    session.command(Command::Release {
        button: Button::Left,
    });
}

fn click() -> Verb {
    Verb::Click {
        at: SPOT,
        button: PointerButton::Left,
    }
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_drag_held_still_keeps_an_agent_off_the_seat() {
    let session = session("held-drag");
    let (mut desk, mut queue, _qh) = with_windows(&session);
    let facts = session.facts.read();
    let ((back, _), (_, from)) = (window(&facts, "back"), window(&facts, "front"));
    let host = Host::new(&session.facts, &session.requests);

    // The person takes "front" by its client, which asks to be moved, as a
    // toolkit drawing its own titlebar does.
    press(&session, centre(from));
    until(&mut queue, &mut desk, |desk| desk.pressed.is_some());
    let seat = desk.seat.clone().expect("our pointer's seat");
    desk.windows[1].move_(&seat, desk.pressed.expect("waited for"));
    // A move takes the pointer from every client.
    until(&mut queue, &mut desk, |desk| desk.pointed.is_none());

    // Held still: the person has not touched the seat since. Every act is
    // refused, on a window the drag never touched.
    let refused = Err(ActError::PersonActive);
    let scroll = Verb::Scroll {
        at: SPOT,
        dx: 0.0,
        dy: 15.0,
    };
    let typing = Verb::Type {
        text: "x".to_owned(),
    };
    for verb in [
        click(),
        scroll,
        typing,
        Verb::Focus,
        Verb::Close,
        Verb::Raise,
    ] {
        assert_eq!(host.act(back, &verb).map(|_| ()), refused, "{verb:?}");
    }
    queue.roundtrip(&mut desk).expect("a roundtrip");
    assert_eq!(desk.buttons, [(LEFT, true)], "only the person's press");
    assert_eq!(desk.asked_to_close, 0);

    // The drag is the person's still: it goes where their hand takes it,
    // and no further. The window lands, and says so, as the button comes up.
    session.command(Command::Motion { by: (40, 30) });
    release(&session);
    let landed = session
        .wait_for(|facts| geometry(facts, "front").is_some_and(|at| at.x0 == from.x0 + 40.0));
    let (_, to) = window(&landed, "front");
    assert_eq!((to.x0, to.y0), (from.x0 + 40.0, from.y0 + 30.0));

    // Let go, the agent may act again.
    assert!(host.act(back, &click()).is_ok());
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 3);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_button_held_down_in_a_window_keeps_an_agent_off_the_seat() {
    let session = session("held-in-window");
    let (mut desk, mut queue, _qh) = with_windows(&session);
    let facts = session.facts.read();
    let ((back, _), (front, at)) = (window(&facts, "back"), window(&facts, "front"));
    let host = Host::new(&session.facts, &session.requests);

    // A selection being dragged out in "front", which has the keyboard.
    press(&session, centre(at));
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 1);

    let refused = Err(ActError::PersonActive);
    assert_eq!(host.act(back, &click()).map(|_| ()), refused);
    // Typing would replace the very selection being made.
    let typing = Verb::Type {
        text: "x".to_owned(),
    };
    assert_eq!(host.act(front, &typing).map(|_| ()), refused);
    queue.roundtrip(&mut desk).expect("a roundtrip");
    assert_eq!(desk.buttons, [(LEFT, true)], "only the person's press");

    release(&session);
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);
    assert!(host.act(back, &click()).is_ok());
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 4);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_holding_the_pointer_is_no_hand_on_it() {
    let session = session("held-menu");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_window(&qh, "page", "page");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| {
        facts.surfaces().iter().any(|surface| {
            surface.title.as_deref() == Some("page")
                && surface.mapped
                && surface.damage_generation >= 1
        })
    });
    let (page, _) = window(&facts, "page");
    // Open, with the keyboard and the pointer, and no button down.
    let _menu = menu_with_the_keyboard(&session, &mut desk, &mut queue, &qh, &ear, page);

    // An item chosen from it, as an agent chooses one.
    let menu = session
        .facts
        .read()
        .surfaces()
        .iter()
        .find(|surface| surface.hangs_from() == Some(page))
        .expect("described, once it drew")
        .id;
    let host = Host::new(&session.facts, &session.requests);
    assert!(host.act(menu, &click()).is_ok());
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);

    session.stop((desk, queue));
}
