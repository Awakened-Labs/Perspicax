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
//! Both are decided here, before any binding table or client sees the key, and
//! both are pure functions of the modifiers and the keysyms so the decision can
//! be tested without a seat. Config (slice 5) resolves its bindings *after*
//! this, which is what "cannot be rebound" means in practice.

use smithay::input::keyboard::{Keysym, ModifiersState};

/// A way out, as asked for at the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hatch {
    /// End the session and give the VT back.
    Exit,
    /// Switch to this VT, counted from 1.
    Vt(i32),
}

/// Whether this key press is an escape hatch.
///
/// `modified` is the keysym after the layout applied the modifiers; `raw` is
/// what the key produces with none. Both are consulted because layouts
/// disagree about Ctrl+Alt: xkb's stock keymap turns Ctrl+Alt+F3 into
/// `XF86Switch_VT_3` and Ctrl+Alt+Backspace into `Terminate_Server` only under
/// an option, while a layout without those types leaves the plain `F3` and
/// `BackSpace` behind. Either spelling means the same thing to the person
/// pressing it.
pub(crate) fn classify(
    modifiers: &ModifiersState,
    modified: Keysym,
    raw: &[Keysym],
) -> Option<Hatch> {
    if !(modifiers.ctrl && modifiers.alt) {
        return None;
    }
    std::iter::once(modified)
        .chain(raw.iter().copied())
        .find_map(hatch_for)
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
fn offset(sym: Keysym, first: Keysym, last: Keysym) -> Option<i32> {
    (first.raw()..=last.raw())
        .contains(&sym.raw())
        .then(|| i32::try_from(sym.raw() - first.raw() + 1).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(ctrl: bool, alt: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            alt,
            ..ModifiersState::default()
        }
    }

    #[test]
    fn ctrl_alt_backspace_ends_the_session() {
        let hatch = classify(&held(true, true), Keysym::BackSpace, &[Keysym::BackSpace]);
        assert_eq!(hatch, Some(Hatch::Exit));
    }

    #[test]
    fn terminate_server_is_the_same_exit_under_another_name() {
        let hatch = classify(
            &held(true, true),
            Keysym::Terminate_Server,
            &[Keysym::BackSpace],
        );
        assert_eq!(hatch, Some(Hatch::Exit));
    }

    #[test]
    fn ctrl_alt_f3_switches_to_vt_3_whichever_way_the_layout_spells_it() {
        let stock = classify(&held(true, true), Keysym::XF86_Switch_VT_3, &[Keysym::F3]);
        let plain = classify(&held(true, true), Keysym::F3, &[Keysym::F3]);
        assert_eq!(stock, Some(Hatch::Vt(3)));
        assert_eq!(plain, Some(Hatch::Vt(3)));
    }

    #[test]
    fn f12_is_vt_12_and_f13_is_nothing() {
        assert_eq!(
            classify(&held(true, true), Keysym::F12, &[Keysym::F12]),
            Some(Hatch::Vt(12))
        );
        assert_eq!(
            classify(&held(true, true), Keysym::F13, &[Keysym::F13]),
            None
        );
    }

    #[test]
    fn one_modifier_short_is_the_clients_key() {
        assert_eq!(
            classify(&held(true, false), Keysym::BackSpace, &[Keysym::BackSpace]),
            None
        );
        assert_eq!(
            classify(&held(false, true), Keysym::F2, &[Keysym::F2]),
            None
        );
    }

    #[test]
    fn an_ordinary_chord_is_not_a_hatch() {
        assert_eq!(classify(&held(true, true), Keysym::t, &[Keysym::t]), None);
    }
}
