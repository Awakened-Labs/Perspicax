//! Chords as a person writes them: `"Logo+Shift+Return"`. And `"Logo"` on
//! its own, which is a tap of that key rather than a chord.
//!
//! The key is either a single character (`a`, `1`, `/`) or a name from
//! [`NAMED`]. The table is written out here rather than looked up in xkb's
//! full keysym list, for two reasons. xkb's names come from a C library this
//! crate does not link. And a short, explicit list of the keys people actually
//! bind is a stable interface, where the full list is thousands of entries a
//! typo could land in by accident.
//!
//! A `[mouse]` entry is written the same way, ending in a button or a turn of
//! the wheel instead of a key, with `Double` among the modifiers for a
//! double-click: `"Mouse8"`, `"Alt+WheelUp"`, `"Double+left"`. Buttons have
//! two spellings, a name ([`BUTTONS`]) and X's number (`Mouse8`), so that a
//! Fluxbox `keys` line carries over, and a button with neither can be named
//! by its Linux code (`Button275`).
//!
//! Both ways, as `actions` does for actions: [`trigger`] reads an entry, and
//! [`spell_trigger`] writes one back, for the settings app. A spelling is
//! only ever given when reading it back is the same chord. A key pressed is
//! written by [`spell_press`], as the chord the compositor would match it by.

use perspicax_policy::{Button, Chord, Gesture, KeyPress, Keysym, Mods, MouseChord, Wheel};

/// The named keys a binding may use, matched without regard to case. Where a
/// key has two common spellings, both are listed.
const NAMED: &[(&str, Keysym)] = &[
    ("Return", Keysym::Return),
    ("Enter", Keysym::Return),
    ("Tab", Keysym::Tab),
    ("Escape", Keysym::Escape),
    ("Esc", Keysym::Escape),
    ("Space", Keysym::space),
    ("BackSpace", Keysym::BackSpace),
    ("Delete", Keysym::Delete),
    ("Insert", Keysym::Insert),
    ("Home", Keysym::Home),
    ("End", Keysym::End),
    ("Page_Up", Keysym::Page_Up),
    ("PageUp", Keysym::Page_Up),
    ("Page_Down", Keysym::Page_Down),
    ("PageDown", Keysym::Page_Down),
    ("Left", Keysym::Left),
    ("Right", Keysym::Right),
    ("Up", Keysym::Up),
    ("Down", Keysym::Down),
    ("Print", Keysym::Print),
    ("Pause", Keysym::Pause),
    ("Menu", Keysym::Menu),
    ("F1", Keysym::F1),
    ("F2", Keysym::F2),
    ("F3", Keysym::F3),
    ("F4", Keysym::F4),
    ("F5", Keysym::F5),
    ("F6", Keysym::F6),
    ("F7", Keysym::F7),
    ("F8", Keysym::F8),
    ("F9", Keysym::F9),
    ("F10", Keysym::F10),
    ("F11", Keysym::F11),
    ("F12", Keysym::F12),
    ("XF86AudioRaiseVolume", Keysym::XF86_AudioRaiseVolume),
    ("XF86AudioLowerVolume", Keysym::XF86_AudioLowerVolume),
    ("XF86AudioMute", Keysym::XF86_AudioMute),
    ("XF86AudioMicMute", Keysym::XF86_AudioMicMute),
    ("XF86AudioPlay", Keysym::XF86_AudioPlay),
    ("XF86AudioPause", Keysym::XF86_AudioPause),
    ("XF86AudioStop", Keysym::XF86_AudioStop),
    ("XF86AudioNext", Keysym::XF86_AudioNext),
    ("XF86AudioPrev", Keysym::XF86_AudioPrev),
    ("XF86MonBrightnessUp", Keysym::XF86_MonBrightnessUp),
    ("XF86MonBrightnessDown", Keysym::XF86_MonBrightnessDown),
];

/// The buttons a `[mouse]` entry may name, matched without regard to case.
/// `side` and `extra` are the two thumb buttons, which browsers take as Back
/// and Forward; `forward` and `back` are what a few mice send instead.
const BUTTONS: &[(&str, Button)] = &[
    ("left", Button::Left),
    ("middle", Button::Middle),
    ("right", Button::Right),
    ("side", Button::Side),
    ("extra", Button::Extra),
    ("forward", Button::Forward),
    ("back", Button::Back),
    ("task", Button::Task),
];

/// The wheel's turns, matched without regard to case. X counts them as
/// buttons 4 to 7, in this order.
const WHEEL: &[(&str, Wheel)] = &[
    ("WheelUp", Wheel::Up),
    ("WheelDown", Wheel::Down),
    ("WheelLeft", Wheel::Left),
    ("WheelRight", Wheel::Right),
];

/// The Linux codes a button can have: from `BTN_MISC` to the last of the
/// `BTN_TRIGGER_HAPPY` range. Everything libinput reports as a pointer's
/// button is in here.
const BUTTON_CODES: std::ops::RangeInclusive<u32> = 0x100..=0x2ff;

/// Parse modifiers alone, as `drag = "Alt"` writes them. `"none"` is no
/// modifier-drag at all, returned as `None`.
///
/// # Errors
///
/// Why `text` is no set of modifiers, naming the part that is not one.
pub fn modifiers(text: &str) -> Result<Option<Mods>, String> {
    if text.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let mut mods = Mods::default();
    for part in text.split('+') {
        modifier(part.trim(), &mut mods)?;
    }
    Ok(Some(mods))
}

/// What a `[keys]` entry binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Chord(Chord),
    /// `"Logo"` on its own: pressed and let go with nothing else in between.
    /// See [`perspicax_policy::LogoTap`].
    LogoTap,
}

/// Parse a `[keys]` entry: `"Mod+Mod+Key"`, or a lone `"Logo"` for a tap.
/// Other modifiers cannot be tapped: Ctrl, Alt and Shift on their own are
/// pressed far too often on the way to a chord.
///
/// # Errors
///
/// Why `text` binds nothing: an unknown modifier or key name, a modifier
/// other than Logo on its own, or no key at all.
pub fn trigger(text: &str) -> Result<Trigger, String> {
    let lone = text.trim();
    let mut mods = Mods::default();
    if modifier(lone, &mut mods).is_ok() {
        return if mods.logo {
            Ok(Trigger::LogoTap)
        } else {
            Err(format!(
                "`{lone}` on its own cannot be bound; only Logo can be tapped, so add a \
                 key, as in `{lone}+F1`"
            ))
        };
    }
    chord(text).map(Trigger::Chord)
}

/// Parse `"Mod+Mod+Key"`.
pub(crate) fn chord(text: &str) -> Result<Chord, String> {
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    // A chord ending in `+` means the plus key itself: "Ctrl++".
    if text.ends_with("++") {
        parts.pop();
        parts.pop();
        parts.push("+");
    }
    let Some(key) = parts.pop().filter(|key| !key.is_empty()) else {
        return Err(format!("`{text}` names no key"));
    };
    let mut mods = Mods::default();
    for part in parts {
        modifier(part, &mut mods)?;
    }
    Ok(Chord {
        mods,
        key: keysym(key)?,
    })
}

/// Parse a `[mouse]` entry: `"Mod+Mod+Button"`, with `Double` among the
/// modifiers for a double-click.
pub(crate) fn mouse(text: &str) -> Result<MouseChord, String> {
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let pointed = parts.pop().unwrap_or_default();
    let mut mods = Mods::default();
    let mut double = false;
    for part in parts {
        if part.eq_ignore_ascii_case("double") {
            double = true;
        } else {
            modifier(part, &mut mods)?;
        }
    }
    let lone = pointed.eq_ignore_ascii_case("double") || modifier(pointed, &mut mods).is_ok();
    if pointed.is_empty() || lone {
        return Err(format!(
            "`{text}` names no button; end it with one, as in `Alt+Mouse1`"
        ));
    }
    let gesture = match (gesture(pointed)?, double) {
        (Gesture::Press(button), true) => Gesture::Double(button),
        (Gesture::Wheel(_), true) => {
            return Err(format!(
                "`{pointed}` is a turn of the wheel, which has no double-click"
            ));
        }
        (gesture, _) => gesture,
    };
    Ok(MouseChord { mods, gesture })
}

/// One button or turn of the wheel, by name, by X's number, or by Linux
/// code.
fn gesture(name: &str) -> Result<Gesture, String> {
    let named = BUTTONS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|&(_, button)| Gesture::Press(button))
        .or_else(|| {
            WHEEL
                .iter()
                .find(|(known, _)| known.eq_ignore_ascii_case(name))
                .map(|&(_, wheel)| Gesture::Wheel(wheel))
        });
    if let Some(named) = named {
        return Ok(named);
    }
    if let Some(number) = numbered(name, "mouse") {
        return x_button(number).ok_or_else(|| {
            let last = BUTTON_CODES.end() - Button::Side.code() + 8;
            format!("`{name}` is no button; X numbers them from Mouse1 to Mouse{last}")
        });
    }
    if let Some(code) = numbered(name, "button") {
        return BUTTON_CODES
            .contains(&code)
            .then(|| Gesture::Press(Button::from_code(code)))
            .ok_or_else(|| {
                format!(
                    "`{name}` is no button's code; libinput's are {} to {}, and X's button \
                     {code} is written `Mouse{code}`",
                    BUTTON_CODES.start(),
                    BUTTON_CODES.end()
                )
            });
    }
    Err(format!(
        "`{name}` is not a button this config knows; use left, middle, right, side, extra, \
         forward, back, task, WheelUp, WheelDown, WheelLeft, WheelRight, X's Mouse1 to Mouse12, \
         or Button followed by the code `libinput debug-events` shows for it"
    ))
}

/// The number after `prefix`, which is matched without regard to case:
/// `Mouse8` is 8 under `mouse`.
fn numbered(name: &str, prefix: &str) -> Option<u32> {
    let (head, tail) = (name.get(..prefix.len())?, name.get(prefix.len()..)?);
    head.eq_ignore_ascii_case(prefix)
        .then(|| tail.parse().ok())
        .flatten()
}

/// X's button `number`, as xf86-input-libinput numbers Linux's: 1 to 3 are
/// left, middle and right, 4 to 7 the wheel, and from 8 on the codes from
/// `BTN_SIDE` up. `None` for 0, and past the last button code.
fn x_button(number: u32) -> Option<Gesture> {
    let wheel = |at: u32| Gesture::Wheel(WHEEL[usize::try_from(at).unwrap_or_default()].1);
    match number {
        1 => Some(Gesture::Press(Button::Left)),
        2 => Some(Gesture::Press(Button::Middle)),
        3 => Some(Gesture::Press(Button::Right)),
        4..=7 => Some(wheel(number - 4)),
        8.. => {
            let code = Button::Side.code().checked_add(number - 8)?;
            BUTTON_CODES
                .contains(&code)
                .then(|| Gesture::Press(Button::from_code(code)))
        }
        0 => None,
    }
}

/// Modifiers as a person writes them, in a fixed order with Shift last, as
/// the README writes them: `Ctrl+Alt`, `Logo+Shift`.
pub(crate) fn spelled(mods: Mods) -> String {
    [
        (mods.ctrl, "Ctrl"),
        (mods.alt, "Alt"),
        (mods.logo, "Logo"),
        (mods.shift, "Shift"),
    ]
    .into_iter()
    .filter(|&(held, _)| held)
    .map(|(_, name)| name)
    .collect::<Vec<_>>()
    .join("+")
}

/// A `[keys]` entry as a person writes it: `"Logo+Shift+Return"`, or
/// `"Logo"` for a tap. `None` where the key has no spelling (see
/// [`spell_chord`]).
#[must_use]
pub fn spell_trigger(written: Trigger) -> Option<String> {
    let text = match written {
        Trigger::LogoTap => "Logo".to_owned(),
        Trigger::Chord(chord) => {
            let key = spell_key(chord.key)?;
            match spelled(chord.mods) {
                mods if mods.is_empty() => key,
                mods => format!("{mods}+{key}"),
            }
        }
    };
    trigger(&text)
        .is_ok_and(|read| read == written)
        .then_some(text)
}

/// `drag` as a person writes it: `"Alt"`, `"Ctrl+Logo"`, or `"none"` for no
/// modifier-drag. `None` where no file can write it: a drag with no
/// modifier held.
#[must_use]
pub fn spell_modifiers(drag: Option<Mods>) -> Option<String> {
    let text = drag.map_or_else(|| "none".to_owned(), spelled);
    modifiers(&text)
        .is_ok_and(|read| read == drag)
        .then_some(text)
}

/// A chord as a person writes it, the modifiers in their fixed order:
/// `"Logo+Shift+Return"`.
///
/// `None` for a key this config has no way to write yet -- a keypad key,
/// `F13`, a capital letter, the plus key with nothing held -- which is as
/// much as to say that chord cannot be bound yet.
#[must_use]
pub fn spell_chord(chord: Chord) -> Option<String> {
    spell_trigger(Trigger::Chord(chord))
}

/// A key pressed, as the `[keys]` entry that binds it: what the settings
/// app writes for a chord the person pressed.
///
/// The key is one of those a binding finds the press by
/// ([`KeyPress::candidates`]), so the entry written is the one the
/// compositor will match. Of those, the one a person would write: the digit
/// an AZERTY number row makes with Shift, then the key in the keymap's Latin
/// layout, then what it makes alone, then what it made. So under `us,ru`
/// with Russian in use the key that makes `й` is written `q`, as it is under
/// `us`, and Shift+a is `Shift+a` rather than a capital no file can write.
///
/// `None` when none of them has a spelling: a keypad key, say, which no
/// config can bind yet.
#[must_use]
pub fn spell_press(press: &KeyPress) -> Option<String> {
    let found = press.candidates();
    let digit = press
        .shifted
        .filter(|sym| sym.key_char().is_some_and(|made| made.is_ascii_digit()));
    digit
        .into_iter()
        .chain(press.latin)
        .chain(press.raw.iter().copied())
        .chain([press.made])
        .filter(|key| found.contains(key))
        .find_map(|key| {
            spell_chord(Chord {
                mods: press.mods,
                key,
            })
        })
}

/// A key as a person writes it: the first of its names in [`NAMED`], or else
/// the character it makes, when that is one a person can see.
///
/// The spelling is kept only when [`keysym`] reads it back as the same key.
/// xkb gives the keypad's `1` the character `1`, and a binding written `1` is
/// the number row's: without the check, capturing the one would bind the
/// other.
fn spell_key(sym: Keysym) -> Option<String> {
    let text = NAMED
        .iter()
        .find(|&&(_, named)| named == sym)
        .map(|&(name, _)| name.to_owned())
        .or_else(|| {
            sym.key_char()
                .filter(|made| !made.is_control() && !made.is_whitespace())
                .map(String::from)
        })?;
    keysym(&text).is_ok_and(|read| read == sym).then_some(text)
}

fn modifier(name: &str, mods: &mut Mods) -> Result<(), String> {
    let slot = match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => &mut mods.ctrl,
        "alt" => &mut mods.alt,
        "shift" => &mut mods.shift,
        "logo" | "super" | "meta" | "win" => &mut mods.logo,
        _ => {
            return Err(format!(
                "`{name}` is not a modifier; use Ctrl, Alt, Shift or Logo"
            ));
        }
    };
    *slot = true;
    Ok(())
}

/// One key. A letter is taken as lower case: that is the key's unshifted
/// symbol, which is what a binding with Shift in its modifiers still matches.
fn keysym(name: &str) -> Result<Keysym, String> {
    let mut chars = name.chars();
    if let (Some(single), None) = (chars.next(), chars.next()) {
        return Ok(Keysym::from_char(single.to_ascii_lowercase()));
    }
    NAMED
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|&(_, sym)| sym)
        .ok_or_else(|| {
            format!(
                "`{name}` is not a key name this config knows; see `perspicax-config`'s key table"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_parses_modifiers_in_any_case_and_any_order() {
        let parsed = chord("shift+LOGO+Return").unwrap();
        assert_eq!(
            parsed.mods,
            Mods {
                shift: true,
                logo: true,
                ..Mods::default()
            }
        );
        assert_eq!(parsed.key, Keysym::Return);
    }

    #[test]
    fn a_letter_binds_its_unshifted_symbol() {
        assert_eq!(chord("Logo+Shift+R").unwrap().key, Keysym::r);
    }

    #[test]
    fn super_and_logo_are_the_same_key() {
        assert_eq!(chord("Super+a").unwrap(), chord("Logo+a").unwrap());
    }

    #[test]
    fn the_plus_key_itself_can_be_bound() {
        assert_eq!(chord("Ctrl++").unwrap().key, Keysym::plus);
    }

    #[test]
    fn an_unknown_key_name_is_refused_by_name() {
        let error = chord("Alt+Frobnicate").unwrap_err();
        assert!(error.contains("`Frobnicate`"), "{error}");
    }

    #[test]
    fn an_unknown_modifier_is_refused_by_name() {
        let error = chord("Hyper+a").unwrap_err();
        assert!(error.contains("`Hyper`"), "{error}");
    }

    #[test]
    fn a_chord_with_no_key_is_refused() {
        assert!(chord("Alt+").is_err());
    }

    #[test]
    fn logo_super_meta_and_win_alone_are_a_tap_of_logo() {
        for name in ["Logo", "super", "Meta", "WIN"] {
            assert_eq!(trigger(name).unwrap(), Trigger::LogoTap, "{name}");
        }
        assert!(matches!(trigger("Logo+a").unwrap(), Trigger::Chord(_)));
    }

    #[test]
    fn another_modifier_alone_is_refused_by_name() {
        let error = trigger("Alt").unwrap_err();
        assert!(error.contains("`Alt` on its own"), "{error}");
        assert!(trigger("Ctrl").is_err());
    }

    fn pressed(text: &str) -> Gesture {
        mouse(text).unwrap().gesture
    }

    #[test]
    fn a_button_is_named_in_any_case_or_by_xs_number_or_by_its_code() {
        assert_eq!(pressed("left"), Gesture::Press(Button::Left));
        assert_eq!(pressed("MIDDLE"), Gesture::Press(Button::Middle));
        assert_eq!(pressed("Mouse1"), Gesture::Press(Button::Left));
        assert_eq!(pressed("mouse2"), Gesture::Press(Button::Middle));
        assert_eq!(pressed("Mouse3"), Gesture::Press(Button::Right));
        assert_eq!(pressed("Mouse8"), pressed("side"));
        assert_eq!(pressed("Button275"), pressed("side"));
        assert_eq!(pressed("Mouse9"), pressed("extra"));
        assert_eq!(pressed("Mouse10"), pressed("forward"));
        assert_eq!(pressed("Mouse11"), pressed("back"));
        assert_eq!(pressed("Mouse12"), pressed("task"));
        assert_eq!(pressed("Mouse13"), Gesture::Press(Button::Code(0x118)));
        assert_eq!(pressed("Button256"), Gesture::Press(Button::Code(0x100)));
    }

    #[test]
    fn xs_buttons_four_to_seven_are_the_wheel() {
        assert_eq!(pressed("Mouse4"), pressed("WheelUp"));
        assert_eq!(pressed("Mouse5"), pressed("wheeldown"));
        assert_eq!(pressed("Mouse6"), Gesture::Wheel(Wheel::Left));
        assert_eq!(pressed("Mouse7"), Gesture::Wheel(Wheel::Right));
    }

    #[test]
    fn modifiers_and_double_come_before_the_button_in_any_order() {
        let chord = mouse("Double+Alt+Mouse1").unwrap();
        assert_eq!(chord.mods, Mods::alt());
        assert_eq!(chord.gesture, Gesture::Double(Button::Left));
        assert_eq!(mouse("alt+double+left").unwrap(), chord);
        assert_eq!(mouse("Mouse8").unwrap().mods, Mods::default());
    }

    #[test]
    fn a_mouse_entry_without_a_button_is_refused() {
        for text in ["Alt", "Double", "Alt+", ""] {
            let error = mouse(text).unwrap_err();
            assert!(error.contains("names no button"), "{text}: {error}");
        }
    }

    #[test]
    fn the_wheel_has_no_double_click() {
        let error = mouse("Double+WheelUp").unwrap_err();
        assert!(error.contains("`WheelUp`"), "{error}");
    }

    #[test]
    fn an_unknown_button_or_number_is_refused_by_name() {
        for name in ["thumb", "Mouse0", "Button12", "Button9999", "Mouse999"] {
            let error = mouse(&format!("Alt+{name}")).unwrap_err();
            assert!(error.contains(&format!("`{name}`")), "{name}: {error}");
        }
        let error = mouse("Hyper+left").unwrap_err();
        assert!(error.contains("`Hyper`"), "{error}");
    }

    #[test]
    fn modifiers_are_spelled_as_a_person_writes_them() {
        assert_eq!(spelled(Mods::alt()), "Alt");
        let held = Mods {
            ctrl: true,
            logo: true,
            ..Mods::default()
        };
        assert_eq!(spelled(held), "Ctrl+Logo");
    }

    #[test]
    fn every_drag_modifier_set_spells_and_reads_back() {
        for mods in every_mods().filter(|&mods| mods != Mods::default()) {
            let written = spell_modifiers(Some(mods)).expect("a spelling");
            assert_eq!(modifiers(&written), Ok(Some(mods)), "{written}");
        }
        assert_eq!(spell_modifiers(None).as_deref(), Some("none"));
        assert_eq!(
            spell_modifiers(Some(Mods::default())),
            None,
            "a drag needs a modifier"
        );
    }

    /// Every set of modifiers, none to all four.
    fn every_mods() -> impl Iterator<Item = Mods> {
        (0..16).map(|bits: u8| Mods {
            ctrl: bits & 1 != 0,
            alt: bits & 2 != 0,
            shift: bits & 4 != 0,
            logo: bits & 8 != 0,
        })
    }

    /// The keys a person can write: every name, and every character on a US
    /// keyboard but the capitals, which the file reads as lower case.
    fn writable_keys() -> impl Iterator<Item = Keysym> {
        let named = NAMED.iter().map(|&(_, sym)| sym);
        let typed = ('!'..='~')
            .filter(|made| !made.is_ascii_uppercase())
            .map(Keysym::from_char);
        named.chain(typed)
    }

    #[test]
    fn every_named_key_is_spelled_its_first_way() {
        for &(name, sym) in NAMED {
            let first = NAMED.iter().find(|&&(_, named)| named == sym).unwrap().0;
            assert_eq!(spell_key(sym).as_deref(), Some(first), "{name}");
        }
        assert_eq!(spell_key(Keysym::Return).as_deref(), Some("Return"));
        assert_eq!(spell_key(Keysym::space).as_deref(), Some("Space"));
    }

    #[test]
    fn every_printable_ascii_key_but_a_capital_spells_as_itself_and_back() {
        for made in ('!'..='~').filter(|made| !made.is_ascii_uppercase()) {
            let sym = Keysym::from_char(made);
            assert_eq!(spell_key(sym), Some(made.to_string()), "{made}");
        }
    }

    #[test]
    fn a_capital_a_keypad_key_and_f13_have_no_spelling() {
        for sym in [Keysym::A, Keysym::KP_Enter, Keysym::KP_1, Keysym::F13] {
            assert_eq!(spell_key(sym), None, "{sym:?}");
        }
    }

    #[test]
    fn a_letter_of_another_alphabet_spells_as_itself() {
        assert_eq!(spell_key(Keysym::Cyrillic_shorti).as_deref(), Some("й"));
    }

    #[test]
    fn a_chord_spells_its_modifiers_in_the_readmes_order() {
        let spelled = |text| spell_chord(chord(text).unwrap()).unwrap();
        assert_eq!(spelled("shift+logo+return"), "Logo+Shift+Return");
        assert_eq!(spelled("alt+ctrl+Delete"), "Ctrl+Alt+Delete");
        assert_eq!(spelled("Super+A"), "Logo+a");
        assert_eq!(spelled("Ctrl++"), "Ctrl++");
        assert_eq!(spelled("F1"), "F1");
    }

    #[test]
    fn the_plus_key_alone_has_no_spelling() {
        let plus = Chord {
            mods: Mods::default(),
            key: Keysym::plus,
        };
        assert_eq!(spell_chord(plus), None);
    }

    #[test]
    fn every_spelled_chord_reads_back_as_itself() {
        for mods in every_mods() {
            for key in writable_keys() {
                let written = Chord { mods, key };
                match spell_chord(written) {
                    Some(text) => assert_eq!(chord(&text), Ok(written), "{text}"),
                    None => assert!(
                        mods == Mods::default() && key == Keysym::plus,
                        "{written:?} has no spelling"
                    ),
                }
            }
        }
    }

    /// A key pressed on a US keyboard, which makes `sym` whatever is held.
    fn plain(mods: Mods, sym: Keysym) -> KeyPress {
        KeyPress {
            mods,
            made: sym,
            raw: vec![sym],
            latin: Some(sym),
            shifted: None,
        }
    }

    fn logo() -> Mods {
        Mods {
            logo: true,
            ..Mods::default()
        }
    }

    /// The key that makes `й` on the Russian layout and `q` on the US one,
    /// pressed with Russian in use.
    fn shorti(mods: Mods) -> KeyPress {
        KeyPress {
            mods,
            made: if mods.shift {
                Keysym::Cyrillic_SHORTI
            } else {
                Keysym::Cyrillic_shorti
            },
            raw: vec![Keysym::Cyrillic_shorti],
            latin: Some(Keysym::q),
            shifted: Some(Keysym::Cyrillic_SHORTI),
        }
    }

    /// AZERTY's `&` key, which makes `1` with Shift.
    fn azerty_one(mods: Mods) -> KeyPress {
        KeyPress {
            mods,
            made: if mods.shift {
                Keysym::_1
            } else {
                Keysym::ampersand
            },
            raw: vec![Keysym::ampersand],
            latin: Some(Keysym::ampersand),
            shifted: Some(Keysym::_1),
        }
    }

    #[test]
    fn a_press_on_the_ru_layout_is_written_in_the_latin_one() {
        assert_eq!(spell_press(&shorti(logo())).as_deref(), Some("Logo+q"));
        let without_latin = KeyPress {
            latin: Some(Keysym::Cyrillic_shorti),
            ..shorti(logo())
        };
        assert_eq!(
            spell_press(&without_latin).as_deref(),
            Some("Logo+й"),
            "a keymap with no Latin layout binds the letter itself"
        );
    }

    #[test]
    fn a_shifted_letter_is_written_lower_case_with_shift() {
        let held = Mods {
            shift: true,
            ..logo()
        };
        let press = KeyPress {
            made: Keysym::A,
            shifted: Some(Keysym::A),
            ..plain(held, Keysym::a)
        };
        assert_eq!(spell_press(&press).as_deref(), Some("Logo+Shift+a"));
        let slash = KeyPress {
            made: Keysym::question,
            shifted: Some(Keysym::question),
            ..plain(held, Keysym::slash)
        };
        assert_eq!(spell_press(&slash).as_deref(), Some("Logo+Shift+/"));
    }

    #[test]
    fn azertys_number_row_is_written_as_the_digit() {
        assert_eq!(spell_press(&azerty_one(logo())).as_deref(), Some("Logo+1"));
        let held = Mods {
            shift: true,
            ..logo()
        };
        assert_eq!(
            spell_press(&azerty_one(held)).as_deref(),
            Some("Logo+Shift+1")
        );
    }

    #[test]
    fn a_keypad_enter_has_no_spelling_yet() {
        assert_eq!(spell_press(&plain(logo(), Keysym::KP_Enter)), None);
        assert_eq!(spell_press(&plain(Mods::default(), Keysym::KP_1)), None);
    }

    #[test]
    fn every_spelled_press_resolves_to_its_binding() {
        let unusual = [shorti(logo()), azerty_one(logo())];
        let presses = every_mods()
            .flat_map(|mods| {
                writable_keys()
                    .map(move |sym| plain(mods, sym))
                    .chain([shorti(mods), azerty_one(mods)])
            })
            .chain(unusual);
        let mut spelled = 0;
        for press in presses {
            let Some(text) = spell_press(&press) else {
                continue;
            };
            let Ok(Trigger::Chord(written)) = trigger(&text) else {
                panic!("{text} is no chord");
            };
            let bindings = perspicax_policy::Bindings::default()
                .bind(written, perspicax_policy::Action::Close);
            assert_eq!(
                bindings.resolve(press.mods, &press.candidates()),
                Some(&perspicax_policy::Action::Close),
                "{text} for {press:?}"
            );
            spelled += 1;
        }
        assert!(spelled > 16 * 100, "only {spelled} spelled");
    }

    #[test]
    fn the_logo_tap_is_spelled_logo() {
        assert_eq!(spell_trigger(Trigger::LogoTap).as_deref(), Some("Logo"));
        assert_eq!(trigger("Logo"), Ok(Trigger::LogoTap));
    }

    #[test]
    fn drag_none_means_no_drag() {
        assert_eq!(modifiers("none").unwrap(), None);
        assert_eq!(modifiers("Alt").unwrap(), Some(Mods::alt()));
    }
}
