//! The two chords no configuration can take away.
//!
//! A session on a real seat owns the keyboard, the screen and the VT. If it
//! wedges -- a client grabs everything, a frame never completes, a keybind the
//! person wrote shadows every other key -- these are how the person at the
//! keyboard gets out without a second machine:
//!
//! - **Ctrl+Alt+Backspace** ends the session.
//! - **Ctrl+Alt+F1 … F12** switches to that VT.
//!
//! The compositor asks [`hatch`] before any binding table or client sees the
//! key, which is what "cannot be rebound" means in practice. The config asks
//! it too, and refuses a `[keys]` entry for one of these chords: a binding
//! that could never fire is one more setting quietly ignored. One definition
//! for both, so the two can never disagree about which chords are taken.

use xkeysym::Keysym;

use crate::Mods;

/// A way out, as asked for at the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hatch {
    /// End the session and give the VT back.
    Exit,
    /// Switch to this VT, counted from 1.
    Vt(u8),
}

/// Whether a key pressed with `mods` is an escape hatch.
///
/// `syms` are every keysym the key goes by: what it made with the modifiers
/// held, and what it makes with none. Both count because layouts disagree
/// about Ctrl+Alt: xkb's stock keymap turns Ctrl+Alt+F3 into
/// `XF86Switch_VT_3` and Ctrl+Alt+Backspace into `Terminate_Server` only under
/// an option, while a layout without those types leaves the plain `F3` and
/// `BackSpace` behind. Either spelling means the same thing to the person
/// pressing it.
///
/// Ctrl and Alt must both be held. Shift or Logo held as well changes
/// nothing: the way out is still the way out.
#[must_use]
pub fn hatch(mods: Mods, syms: impl IntoIterator<Item = Keysym>) -> Option<Hatch> {
    if !(mods.ctrl && mods.alt) {
        return None;
    }
    syms.into_iter().find_map(hatch_for)
}

fn hatch_for(sym: Keysym) -> Option<Hatch> {
    if sym == Keysym::BackSpace || sym == Keysym::Terminate_Server {
        return Some(Hatch::Exit);
    }
    offset(sym, Keysym::XF86_Switch_VT_1, Keysym::XF86_Switch_VT_12)
        .or_else(|| offset(sym, Keysym::F1, Keysym::F12))
        .map(Hatch::Vt)
}

/// `sym`'s position in `first..=last`, counted from 1, if it is in the range.
fn offset(sym: Keysym, first: Keysym, last: Keysym) -> Option<u8> {
    (first.raw()..=last.raw())
        .contains(&sym.raw())
        .then(|| u8::try_from(sym.raw() - first.raw() + 1).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(ctrl: bool, alt: bool) -> Mods {
        Mods {
            ctrl,
            alt,
            ..Mods::default()
        }
    }

    #[test]
    fn ctrl_alt_backspace_ends_the_session() {
        let way_out = hatch(held(true, true), [Keysym::BackSpace]);
        assert_eq!(way_out, Some(Hatch::Exit));
    }

    #[test]
    fn terminate_server_is_the_same_exit_under_another_name() {
        let way_out = hatch(
            held(true, true),
            [Keysym::Terminate_Server, Keysym::BackSpace],
        );
        assert_eq!(way_out, Some(Hatch::Exit));
    }

    #[test]
    fn ctrl_alt_f3_switches_to_vt_3_whichever_way_the_layout_spells_it() {
        let stock = hatch(held(true, true), [Keysym::XF86_Switch_VT_3, Keysym::F3]);
        let plain = hatch(held(true, true), [Keysym::F3]);
        assert_eq!(stock, Some(Hatch::Vt(3)));
        assert_eq!(plain, Some(Hatch::Vt(3)));
    }

    #[test]
    fn f12_is_vt_12_and_f13_is_nothing() {
        assert_eq!(hatch(held(true, true), [Keysym::F12]), Some(Hatch::Vt(12)));
        assert_eq!(hatch(held(true, true), [Keysym::F13]), None);
    }

    #[test]
    fn one_modifier_short_is_the_clients_key() {
        assert_eq!(hatch(held(true, false), [Keysym::BackSpace]), None);
        assert_eq!(hatch(held(false, true), [Keysym::F2]), None);
    }

    #[test]
    fn a_hatch_with_more_modifiers_held_is_still_a_hatch() {
        let all = Mods {
            ctrl: true,
            alt: true,
            shift: true,
            logo: true,
        };
        assert_eq!(hatch(all, [Keysym::F1]), Some(Hatch::Vt(1)));
        assert_eq!(hatch(all, [Keysym::BackSpace]), Some(Hatch::Exit));
    }

    #[test]
    fn an_ordinary_chord_is_not_a_hatch() {
        assert_eq!(hatch(held(true, true), [Keysym::t]), None);
    }
}
