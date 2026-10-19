//! A chord pressed while the app waits for one, as the app reads it.
//!
//! The compositor reports the press with the keysyms it matches a binding
//! by ([`KeyPress`]), and the entry that binds it is spelled from those, as
//! the compositor would match it. Escape on its own is the one press read
//! otherwise: it gives up, so a bare Escape is bound by typing it.

use perspicax_config::Trigger;
use perspicax_policy::{KeyPress, Keysym, Mods};

/// What a press means to a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pressed {
    /// A chord a config can bind, and the `[keys]` entry that writes it.
    Chord { trigger: Trigger, written: String },
    /// Escape on its own: the person gave up, and nothing is bound.
    GaveUp,
    /// A key no config can write yet, such as one on the keypad: the key it
    /// made.
    NotBindable(Keysym),
}

/// What `press` means to a capture.
#[must_use]
pub fn pressed(press: &KeyPress) -> Pressed {
    if press.mods == Mods::default() && press.made == Keysym::Escape {
        return Pressed::GaveUp;
    }
    perspicax_config::spell_press(press)
        .and_then(|written| {
            let trigger = perspicax_config::trigger(&written).ok()?;
            Some(Pressed::Chord { trigger, written })
        })
        .unwrap_or(Pressed::NotBindable(press.made))
}

/// A key's name, as xkb has it, for a key no config can write: `KP_Enter`.
#[must_use]
pub fn key_name(key: Keysym) -> String {
    match key.name() {
        Some(name) => name.strip_prefix("XK_").unwrap_or(name).to_owned(),
        None => format!("{:#x}", key.raw()),
    }
}

#[cfg(test)]
mod tests {
    use perspicax_policy::Chord;

    use super::*;

    fn press(mods: Mods, made: Keysym) -> KeyPress {
        KeyPress {
            mods,
            made,
            raw: vec![made],
            latin: None,
            shifted: None,
        }
    }

    #[test]
    fn escape_alone_gives_up_and_escape_with_a_modifier_is_a_chord() {
        assert_eq!(
            pressed(&press(Mods::default(), Keysym::Escape)),
            Pressed::GaveUp
        );
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            pressed(&press(logo, Keysym::Escape)),
            Pressed::Chord {
                trigger: Trigger::Chord(Chord {
                    mods: logo,
                    key: Keysym::Escape,
                }),
                written: "Logo+Escape".to_owned(),
            }
        );
    }

    #[test]
    fn an_unspellable_key_says_not_bindable_yet() {
        let keypad = press(Mods::default(), Keysym::KP_Enter);
        assert_eq!(pressed(&keypad), Pressed::NotBindable(Keysym::KP_Enter));
        assert_eq!(key_name(Keysym::KP_Enter), "KP_Enter");
        assert_eq!(key_name(Keysym::new(0x0eee_eeee)), "0xeeeeeee");
    }

    #[test]
    fn a_pressed_chord_is_written_as_the_compositor_matches_it() {
        // Shift+a makes `A`, which no file can write: it is written from the
        // key as it is with no modifier.
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        let capital = KeyPress {
            mods: shift,
            made: Keysym::A,
            raw: vec![Keysym::a],
            latin: None,
            shifted: None,
        };
        assert_eq!(
            pressed(&capital),
            Pressed::Chord {
                trigger: Trigger::Chord(Chord {
                    mods: shift,
                    key: Keysym::a,
                }),
                written: "Shift+a".to_owned(),
            }
        );
    }
}
