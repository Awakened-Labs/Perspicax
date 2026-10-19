//! Who a key press is for: an escape hatch, a capture, a binding, or the
//! focused client.
//!
//! The order of authority, decided in one place:
//!
//! 1. **An escape hatch** ([`crate::hatch`]). Nothing takes one away, a
//!    capture included: the way out is still the way out while the settings
//!    app listens.
//! 2. **A capture.** The settings app asked for the next chord pressed, to
//!    bind it. The chord is told to the app and done by nothing, so that
//!    `Logo+Return` can be bound while `Logo+Return` opens a terminal. A
//!    modifier on its own is not a chord yet: it goes to the client as
//!    ever, which can show what is held. Nothing is captured while the
//!    session is locked.
//! 3. **A binding** ([`Bindings::resolve`]). While locked, only one that
//!    still works at the lock screen ([`Action::while_locked`]).
//! 4. Otherwise the focused client, which is `None` here.
//!
//! A tap of the Logo key is known only once the key comes back up
//! ([`crate::LogoTap`]), so [`take_tap`] asks the same of it at the release,
//! from step 2: no tap is a hatch.

use xkeysym::Keysym;

use crate::{Action, Bindings, Hatch, Mods, candidates, hatch};

/// A key pressed, as xkb read it: the modifiers held, and every keysym a
/// binding could find it by. What the compositor matches bindings against,
/// and what it tells the settings app for a capture, so that the app binds
/// exactly what will be matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    /// The modifiers held with it.
    pub mods: Mods,
    /// What the key made with them held.
    pub made: Keysym,
    /// What it makes with none, in the layout in use.
    pub raw: Vec<Keysym>,
    /// What it makes in the keymap's first layout that has a Latin letter
    /// on it. See [`candidates`].
    pub latin: Option<Keysym>,
    /// What it makes with Shift in the layout in use. Counted only when it
    /// is a digit: see [`candidates`].
    pub shifted: Option<Keysym>,
}

impl KeyPress {
    /// Every keysym a binding can find this press by, the most particular
    /// first.
    #[must_use]
    pub fn candidates(&self) -> Vec<Keysym> {
        candidates(self.made, &self.raw, self.latin, self.shifted)
    }

    /// What an escape hatch is matched against: what the key made, and what
    /// it makes alone. See [`hatch`].
    fn hatch_syms(&self) -> impl Iterator<Item = Keysym> + '_ {
        std::iter::once(self.made).chain(self.raw.iter().copied())
    }

    /// Whether this is a modifier, a lock key or a layout switch pressed
    /// on its own: Shift, Ctrl, Alt, Logo, AltGr, Caps Lock.
    fn is_modifier(&self) -> bool {
        self.hatch_syms().any(Keysym::is_modifier_key)
    }
}

/// What took a key press from the focused client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken<'a> {
    /// A way out, done whatever else is going on.
    Hatch(Hatch),
    /// The chord the settings app waits for: told to it, done by nothing.
    Captured,
    /// What a binding does.
    Bound(&'a Action),
}

/// Who takes `press`, if not the focused client. `capturing`: the settings
/// app waits for a chord. `locked`: the session is.
#[must_use]
pub fn take<'a>(
    press: &KeyPress,
    capturing: bool,
    locked: bool,
    bindings: &'a Bindings,
) -> Option<Taken<'a>> {
    if let Some(hatch) = hatch(press.mods, press.hatch_syms()) {
        return Some(Taken::Hatch(hatch));
    }
    if capturing && !locked && !press.is_modifier() {
        return Some(Taken::Captured);
    }
    bound(bindings.resolve(press.mods, &press.candidates()), locked)
}

/// Who takes a finished tap of the Logo key, as [`take`] asks of a press.
#[must_use]
pub fn take_tap(capturing: bool, locked: bool, bindings: &Bindings) -> Option<Taken<'_>> {
    if capturing && !locked {
        return Some(Taken::Captured);
    }
    bound(bindings.tap(), locked)
}

/// A binding's action, if it may be done: while locked, only one that works
/// at the lock screen.
fn bound(action: Option<&Action>, locked: bool) -> Option<Taken<'_>> {
    action
        .filter(|action| !locked || action.while_locked())
        .map(Taken::Bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Chord;

    /// Modifiers, as a test writes them: `held(&["Logo", "Shift"])`.
    fn held(names: &[&str]) -> Mods {
        let mut mods = Mods::default();
        for name in names {
            match *name {
                "Ctrl" => mods.ctrl = true,
                "Alt" => mods.alt = true,
                "Shift" => mods.shift = true,
                "Logo" => mods.logo = true,
                other => panic!("{other} is no modifier"),
            }
        }
        mods
    }

    /// A key on a US keyboard: it makes `sym`, with or without what is held.
    fn pressed(mods: &[&str], sym: Keysym) -> KeyPress {
        KeyPress {
            mods: held(mods),
            made: sym,
            raw: vec![sym],
            latin: Some(sym),
            shifted: None,
        }
    }

    fn terminal() -> Action {
        Action::Spawn {
            command: vec!["foot".to_owned()],
            agent: false,
        }
    }

    fn bound_to(mods: &[&str], key: Keysym, action: Action) -> Bindings {
        Bindings::default().bind(
            Chord {
                mods: held(mods),
                key,
            },
            action,
        )
    }

    #[test]
    fn logo_return_while_capturing_is_captured_not_bound() {
        let bindings = bound_to(&["Logo"], Keysym::Return, terminal());
        let press = pressed(&["Logo"], Keysym::Return);
        assert_eq!(take(&press, true, false, &bindings), Some(Taken::Captured));
        assert_eq!(
            take(&press, false, false, &bindings),
            Some(Taken::Bound(&terminal())),
            "and bound again once nothing waits"
        );
    }

    #[test]
    fn an_escape_hatch_while_capturing_is_still_the_hatch() {
        let bindings = Bindings::default();
        let stock = KeyPress {
            made: Keysym::XF86_Switch_VT_2,
            ..pressed(&["Ctrl", "Alt"], Keysym::F2)
        };
        assert_eq!(
            take(&stock, true, false, &bindings),
            Some(Taken::Hatch(Hatch::Vt(2)))
        );
        let exit = pressed(&["Ctrl", "Alt"], Keysym::BackSpace);
        assert_eq!(
            take(&exit, true, false, &bindings),
            Some(Taken::Hatch(Hatch::Exit))
        );
    }

    #[test]
    fn a_modifier_alone_while_capturing_is_the_clients() {
        let bindings = Bindings::default();
        for (mods, sym) in [
            (&["Logo"][..], Keysym::Super_L),
            (&["Shift"], Keysym::Shift_R),
            (&["Ctrl"], Keysym::Control_L),
            (&["Alt"], Keysym::Alt_L),
            (&["Logo", "Ctrl"], Keysym::Control_R),
            (&[], Keysym::ISO_Level3_Shift),
            (&[], Keysym::Caps_Lock),
            (&[], Keysym::ISO_Next_Group),
        ] {
            assert_eq!(
                take(&pressed(mods, sym), true, false, &bindings),
                None,
                "{sym:?}"
            );
        }
        assert_eq!(
            take(&pressed(&["Logo"], Keysym::a), true, false, &bindings),
            Some(Taken::Captured),
            "the key the modifiers were held for"
        );
    }

    #[test]
    fn nothing_is_captured_while_locked() {
        let bindings = bound_to(&["Logo"], Keysym::a, Action::Close).bind(
            Chord {
                mods: held(&["Logo"]),
                key: Keysym::space,
            },
            Action::CycleLayout { forward: true },
        );
        assert_eq!(
            take(&pressed(&["Logo"], Keysym::a), true, true, &bindings),
            None
        );
        assert_eq!(
            take(&pressed(&["Logo"], Keysym::space), true, true, &bindings),
            Some(Taken::Bound(&Action::CycleLayout { forward: true })),
            "a layout key works at the lock screen, captured or not"
        );
        assert_eq!(take_tap(true, true, &bindings), None);
    }

    #[test]
    fn a_tap_while_capturing_resolves_no_binding() {
        let bindings = Bindings::default().bind_tap(Action::StartMenu);
        assert_eq!(take_tap(true, false, &bindings), Some(Taken::Captured));
        assert_eq!(
            take_tap(false, false, &bindings),
            Some(Taken::Bound(&Action::StartMenu))
        );
        assert_eq!(
            take_tap(false, true, &bindings),
            None,
            "no menu over the lock"
        );
    }

    #[test]
    fn a_press_is_found_by_its_latin_key_as_a_binding_finds_it() {
        let press = KeyPress {
            mods: held(&["Logo"]),
            made: Keysym::Cyrillic_shorti,
            raw: vec![Keysym::Cyrillic_shorti],
            latin: Some(Keysym::q),
            shifted: Some(Keysym::Cyrillic_SHORTI),
        };
        assert_eq!(press.candidates(), [Keysym::Cyrillic_shorti, Keysym::q]);
        let bindings = bound_to(&["Logo"], Keysym::q, Action::Close);
        assert_eq!(
            take(&press, false, false, &bindings),
            Some(Taken::Bound(&Action::Close))
        );
    }
}
