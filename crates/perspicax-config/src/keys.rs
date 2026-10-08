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

use perspicax_policy::{Button, Chord, Gesture, Keysym, Mods, MouseChord, Wheel};

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
pub(crate) fn modifiers(text: &str) -> Result<Option<Mods>, String> {
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
pub(crate) enum Trigger {
    Chord(Chord),
    /// `"Logo"` on its own: pressed and let go with nothing else in between.
    /// See [`perspicax_policy::LogoTap`].
    LogoTap,
}

/// Parse a `[keys]` entry: `"Mod+Mod+Key"`, or a lone `"Logo"` for a tap.
/// Other modifiers cannot be tapped: Ctrl, Alt and Shift on their own are
/// pressed far too often on the way to a chord.
pub(crate) fn trigger(text: &str) -> Result<Trigger, String> {
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

/// Modifiers as a person writes them, in a fixed order: `Ctrl+Alt`.
pub(crate) fn spelled(mods: Mods) -> String {
    [
        (mods.ctrl, "Ctrl"),
        (mods.alt, "Alt"),
        (mods.shift, "Shift"),
        (mods.logo, "Logo"),
    ]
    .into_iter()
    .filter(|&(held, _)| held)
    .map(|(_, name)| name)
    .collect::<Vec<_>>()
    .join("+")
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
    fn drag_none_means_no_drag() {
        assert_eq!(modifiers("none").unwrap(), None);
        assert_eq!(modifiers("Alt").unwrap(), Some(Mods::alt()));
    }
}
