//! A keyboard with more than one layout, as a client hears it: an agent's
//! text arrives as typed, in whichever layout has each character, and a new
//! keymap keeps the layout in use.
//!
//! The client reads its keys as a toolkit does. It keeps the keymap it is
//! sent and the modifiers and group in force at each key, and turns them into
//! text with xkb, so what it read is what an application would have.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use std::{
    fs::File,
    os::unix::fs::FileExt as _,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use common::{Desk, Session, until};
use perspicax_compositor::{Backend, Command, Host, Keymap};
use perspicax_index::{Action as Verb, HostFacts};
use perspicax_node::SurfaceId;
use perspicax_policy::Action;
use smithay::input::keyboard::{Keycode, xkb};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    globals::GlobalList,
    protocol::{wl_keyboard, wl_seat},
};

/// What the client's keyboard was told, kept as it arrived. xkb's own types
/// stay on the test's side of the lock: they are not `Send`.
#[derive(Default)]
struct Heard {
    /// The last keymap, as text.
    keymap: Option<String>,
    /// How many layouts it has.
    layouts: u32,
    /// The modifiers and group as last sent: depressed, latched, locked,
    /// group.
    modifiers: [u32; 4],
    /// Each key pressed since the last keymap, with the modifiers in force.
    keys: Vec<(u32, [u32; 4])>,
}

type Ear = Arc<Mutex<Heard>>;

fn heard(ear: &Ear) -> MutexGuard<'_, Heard> {
    ear.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Heard {
    fn group(&self) -> u32 {
        self.modifiers[3]
    }

    /// The text the keys spell, read with the keymap and modifiers the
    /// client was sent.
    fn typed(&self) -> String {
        let Some(text) = self.keymap.clone() else {
            return String::new();
        };
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::COMPILE_NO_FLAGS,
        )
        .expect("the keymap the client was sent compiles");
        let mut state = xkb::State::new(&keymap);
        self.keys
            .iter()
            .map(|&(key, [depressed, latched, locked, group])| {
                state.update_mask(depressed, latched, locked, 0, 0, group);
                // Wayland's keycodes are evdev's; xkb's are 8 more.
                state.key_get_utf8(Keycode::new(key + 8))
            })
            .collect()
    }
}

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
    let ear = listen(&globals, &qh);
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

fn listen(globals: &GlobalList, qh: &QueueHandle<Desk>) -> Ear {
    let seat: wl_seat::WlSeat = globals.bind(qh, 1..=7, ()).expect("wl_seat");
    let ear = Ear::default();
    seat.get_keyboard(qh, ear.clone());
    ear
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
        Err(perspicax_compositor::ActError::Untypeable('中'))
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

impl Dispatch<wl_keyboard::WlKeyboard, Ear> for Desk {
    fn event(
        _: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        ear: &Ear,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut heard = heard(ear);
        match event {
            wl_keyboard::Event::Keymap { fd, size, .. } => {
                let mut bytes = vec![0; usize::try_from(size).expect("a size")];
                File::from(fd)
                    .read_exact_at(&mut bytes, 0)
                    .expect("the keymap reads");
                let text = String::from_utf8(bytes)
                    .expect("a keymap is text")
                    .trim_end_matches('\0')
                    .to_owned();
                let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                let keymap = xkb::Keymap::new_from_string(
                    &context,
                    text.clone(),
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::COMPILE_NO_FLAGS,
                )
                .expect("the keymap compiles");
                heard.layouts = keymap.num_layouts();
                heard.keymap = Some(text);
                heard.keys.clear();
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => heard.modifiers = [mods_depressed, mods_latched, mods_locked, group],
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                let modifiers = heard.modifiers;
                heard.keys.push((key, modifiers));
            }
            _ => {}
        }
    }
}
