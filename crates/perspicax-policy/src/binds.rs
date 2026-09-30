//! What a chord means.
//!
//! Resolution only. Which chords exist is config's decision (slice 5), and the
//! two escape hatches (Ctrl+Alt+Backspace, Ctrl+Alt+F1–F12) are not here at
//! all. The compositor takes those before a binding is ever consulted, which is
//! what keeps a binding from shadowing them.

use xkeysym::Keysym;

/// Modifiers, as far as a binding cares. Caps Lock and Num Lock are left out
/// on purpose: a binding that stopped working because Num Lock was on is a
/// bug every window manager has shipped at least once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl Mods {
    /// Just Alt.
    #[must_use]
    pub fn alt() -> Self {
        Self {
            alt: true,
            ..Self::default()
        }
    }
}

/// A key and the modifiers held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub mods: Mods,
    pub key: Keysym,
}

/// What a binding does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Ask the focused window to close. A request the client may decline
    /// (with an unsaved-changes dialog, say), never a kill.
    Close,
    /// Bring the next window forward and focus it. See [`crate::cycle`].
    CycleFocus,
}

/// A table of chords and what each one does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings(Vec<(Chord, Action)>);

impl Bindings {
    /// The two bindings anyone coming from Windows or Plasma presses without
    /// thinking: Alt+F4 closes, Alt+Tab moves to the next window.
    ///
    /// No terminal or launcher binding. Starting programs arrives with config
    /// (slice 5), and which program is the person's choice, not a guess made
    /// here.
    #[must_use]
    pub fn classic() -> Self {
        Self::default()
            .bind(
                Chord {
                    mods: Mods::alt(),
                    key: Keysym::F4,
                },
                Action::Close,
            )
            .bind(
                Chord {
                    mods: Mods::alt(),
                    key: Keysym::Tab,
                },
                Action::CycleFocus,
            )
    }

    /// Bind a chord, replacing whatever it meant before. A later layer (a
    /// user's config over a profile) overrides an earlier one this way.
    #[must_use]
    pub fn bind(mut self, chord: Chord, action: Action) -> Self {
        self.0.retain(|(bound, _)| *bound != chord);
        self.0.push((chord, action));
        self
    }

    /// What this key press means, if anything.
    ///
    /// `syms` should hold every keysym the key produces: the one after the
    /// layout applied the modifiers, and the unmodified one. With Shift held,
    /// the layout turns `a` into `A`, and a binding written `Shift+a` has to
    /// match either. Modifiers must match exactly, so Alt+Tab and
    /// Alt+Shift+Tab can mean different things.
    #[must_use]
    pub fn resolve(&self, mods: Mods, syms: &[Keysym]) -> Option<&Action> {
        self.0
            .iter()
            .find(|(chord, _)| chord.mods == mods && syms.contains(&chord.key))
            .map(|(_, action)| action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_f4_closes_and_alt_tab_cycles_in_the_classic_table() {
        let classic = Bindings::classic();
        assert_eq!(
            classic.resolve(Mods::alt(), &[Keysym::F4]),
            Some(&Action::Close)
        );
        assert_eq!(
            classic.resolve(Mods::alt(), &[Keysym::Tab]),
            Some(&Action::CycleFocus)
        );
    }

    #[test]
    fn an_extra_modifier_is_a_different_chord() {
        let shifted = Mods {
            shift: true,
            ..Mods::alt()
        };
        assert_eq!(Bindings::classic().resolve(shifted, &[Keysym::Tab]), None);
    }

    #[test]
    fn a_key_without_its_modifier_is_the_clients() {
        assert_eq!(
            Bindings::classic().resolve(Mods::default(), &[Keysym::F4]),
            None
        );
    }

    #[test]
    fn a_binding_matches_the_unmodified_keysym_too() {
        let bindings = Bindings::default().bind(
            Chord {
                mods: Mods {
                    shift: true,
                    logo: true,
                    ..Mods::default()
                },
                key: Keysym::a,
            },
            Action::Close,
        );
        let held = Mods {
            shift: true,
            logo: true,
            ..Mods::default()
        };
        assert!(bindings.resolve(held, &[Keysym::A, Keysym::a]).is_some());
    }

    #[test]
    fn a_later_binding_replaces_an_earlier_one_for_the_same_chord() {
        let chord = Chord {
            mods: Mods::alt(),
            key: Keysym::F4,
        };
        let bindings = Bindings::classic().bind(chord, Action::CycleFocus);
        assert_eq!(
            bindings.resolve(Mods::alt(), &[Keysym::F4]),
            Some(&Action::CycleFocus)
        );
    }
}
