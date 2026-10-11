//! A keyboard with more than one layout, as a client hears it: an agent's
//! text arrives as typed, in whichever layout has each character, a new
//! keymap keeps the layout in use, and under `switching = "window"` each
//! window keeps its own. And an agent's text goes only into the window it
//! named, never to whichever holds the keyboard instead.
//!
//! The client reads its keys as a toolkit does. It keeps the keymap it is
//! sent and the modifiers and group in force at each key, and turns them into
//! text with xkb, so what it read is what an application would have.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Ear, Session, heard, until};
use perspicax_compositor::{ActError, Backend, Command, Host, Keymap};
use perspicax_index::{Action as Verb, HostFacts};
use perspicax_node::SurfaceId;
use perspicax_policy::{Action, Switching};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{EventQueue, Proxy as _};

fn keymap(layout: &str) -> Keymap {
    Keymap {
        layout: layout.to_owned(),
        ..Keymap::default()
    }
}

/// A headless session with these layouts, one window of a client that
/// listens to its keyboard, and the keyboard on that window.
fn typist(name: &str, layout: &str) -> (Session, Desk, EventQueue<Desk>, Ear, SurfaceId) {
    let session = Session::start(name, Backend::headless((1280, 1024)));
    session.command(Command::Keymap(keymap(layout)));
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_window(&qh, "typist", "typist");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let window = id(&session.wait_for(|facts| !facts.surfaces().is_empty()));
    Host::new(&session.facts, &session.requests)
        .act(window, &Verb::Focus)
        .expect("focused");
    let layouts = layout.split(',').count();
    until(&mut queue, &mut desk, |_| {
        usize::try_from(heard(&ear).layouts) == Ok(layouts)
    });
    (session, desk, queue, ear, window)
}

fn id(facts: &HostFacts) -> SurfaceId {
    facts.surfaces()[0].id
}

fn titled(facts: &HostFacts, title: &str) -> Option<SurfaceId> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| surface.id)
}

fn focus(session: &Session, window: SurfaceId) {
    Host::new(&session.facts, &session.requests)
        .act(window, &Verb::Focus)
        .expect("focused");
}

fn type_text(session: &Session, window: SurfaceId, text: &str) {
    Host::new(&session.facts, &session.requests)
        .act(
            window,
            &Verb::Type {
                text: text.to_owned(),
            },
        )
        .expect("typed");
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn latin_and_cyrillic_arrive_as_typed_and_the_layout_is_put_back() {
    let (session, mut desk, mut queue, ear, window) = typist("keyboard-type", "us,ru");

    // From the first layout: the Cyrillic is typed in the second, and the
    // first put back after.
    type_text(&session, window, "Hello, Мир");
    until(&mut queue, &mut desk, |_| {
        heard(&ear).typed() == "Hello, Мир"
    });
    assert_eq!(heard(&ear).group(), 0, "back in the first layout");

    // From the second: the Latin is typed in the first, and the second put
    // back after.
    session.perform(Action::Layout(2));
    until(&mut queue, &mut desk, |_| heard(&ear).group() == 1);
    heard(&ear).keys.clear();
    type_text(&session, window, "й q");
    until(&mut queue, &mut desk, |_| heard(&ear).typed() == "й q");
    assert_eq!(heard(&ear).group(), 1, "back in the second layout");

    // Nothing on either layout: refused, and nothing typed.
    assert_eq!(
        Host::new(&session.facts, &session.requests).act(
            window,
            &Verb::Type {
                text: "q中".to_owned()
            }
        ),
        Err(ActError::Untypeable('中'))
    );
    queue.roundtrip(&mut desk).expect("flush");
    assert_eq!(heard(&ear).typed(), "й q", "not even the q");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_new_keymap_keeps_the_layout_in_use_and_is_typed_from() {
    let (session, mut desk, mut queue, ear, window) = typist("keyboard-keep", "us,ru");
    session.perform(Action::CycleLayout { forward: true });
    until(&mut queue, &mut desk, |_| heard(&ear).group() == 1);

    // A third layout added: still Russian, and the client is told so after
    // the keymap, which on its own would have put it back in English.
    session.command(Command::Keymap(keymap("us,ru,de")));
    until(&mut queue, &mut desk, |_| heard(&ear).layouts == 3);
    assert_eq!(heard(&ear).group(), 1, "the layout in use, kept");

    // And typed from: ß is only on the new third layout.
    type_text(&session, window, "йß");
    until(&mut queue, &mut desk, |_| heard(&ear).typed() == "йß");
    assert_eq!(heard(&ear).group(), 1);

    // Down to one layout: the last there is.
    session.command(Command::Keymap(keymap("us")));
    until(&mut queue, &mut desk, |_| heard(&ear).layouts == 1);
    assert_eq!(heard(&ear).group(), 0);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn under_window_switching_each_window_keeps_its_own_layout() {
    let (session, mut desk, mut queue, ear, first) = typist("keyboard-window", "us,ru");
    session.command(Command::LayoutSwitching(Switching::Window));
    desk.open_window(&queue.handle(), "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let facts = session.wait_for(|facts| titled(facts, "second").is_some());
    let second = titled(&facts, "second").expect("waited for");

    // The first window in Russian; the second, new, starts in English.
    focus(&session, first);
    session.perform(Action::Layout(2));
    until(&mut queue, &mut desk, |_| heard(&ear).group() == 1);
    focus(&session, second);
    until(&mut queue, &mut desk, |_| heard(&ear).group() == 0);
    // Each goes back to its own.
    focus(&session, first);
    until(&mut queue, &mut desk, |_| heard(&ear).group() == 1);
    focus(&session, second);
    until(&mut queue, &mut desk, |_| heard(&ear).group() == 0);

    // The session's again: the layout stays as it is wherever the keyboard
    // goes.
    session.command(Command::LayoutSwitching(Switching::Global));
    session.perform(Action::Layout(2));
    until(&mut queue, &mut desk, |_| heard(&ear).group() == 1);
    focus(&session, first);
    focus(&session, second);
    queue.roundtrip(&mut desk).expect("flush");
    queue
        .roundtrip(&mut desk)
        .expect("and again, past an idle turn");
    assert_eq!(heard(&ear).group(), 1);

    session.stop((desk, queue));
}

/// #35: an agent with consent for one window typed into whichever held the
/// keyboard -- the person's terminal, say. Now the keys go only into the
/// window the act names, while it holds the keyboard, and otherwise nothing is
/// typed and the refusal names the window that does hold it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_without_the_keyboard_is_not_typed_into_and_the_refusal_names_the_one_with_it() {
    let (session, mut desk, mut queue, ear, first) = typist("keyboard-elsewhere", "us");
    desk.open_window(&queue.handle(), "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let facts = session.wait_for(|facts| titled(facts, "second").is_some());
    let second = titled(&facts, "second").expect("waited for");
    focus(&session, second);

    assert_eq!(
        Host::new(&session.facts, &session.requests).act(
            first,
            &Verb::Type {
                text: "nope".to_owned()
            }
        ),
        Err(ActError::FocusElsewhere {
            focused: Some(second)
        })
    );
    queue.roundtrip(&mut desk).expect("flush");
    assert!(heard(&ear).keys.is_empty(), "nothing typed, into either");

    // Given the keyboard, it is typed into, and it is the one that hears it.
    focus(&session, first);
    type_text(&session, first, "yes");
    until(&mut queue, &mut desk, |_| heard(&ear).typed() == "yes");
    assert_eq!(
        heard(&ear).entered,
        Some(desk.windows[0].wl_surface().id()),
        "into the first window"
    );

    session.stop((desk, queue));
}
