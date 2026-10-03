//! Chords as a person writes them: `"Logo+Shift+Return"`. And `"Logo"` on
//! its own, which is a tap of that key rather than a chord.
//!
//! The key is either a single character (`a`, `1`, `/`) or a name from
//! [`NAMED`]. The table is written out here rather than looked up in xkb's
//! full keysym list, for two reasons. xkb's names come from a C library this
//! crate does not link. And a short, explicit list of the keys people actually
//! bind is a stable interface, where the full list is thousands of entries a
//! typo could land in by accident.

use perspicax_policy::{Chord, Keysym, Mods};

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

    #[test]
    fn drag_none_means_no_drag() {
        assert_eq!(modifiers("none").unwrap(), None);
        assert_eq!(modifiers("Alt").unwrap(), Some(Mods::alt()));
    }
}
