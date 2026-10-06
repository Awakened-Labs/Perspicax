//! What a chord means.
//!
//! Resolution only. Which chords exist is config's decision (slice 5), and the
//! two escape hatches (Ctrl+Alt+Backspace, Ctrl+Alt+F1–F12) are not here at
//! all. The compositor takes those before a binding is ever consulted, which is
//! what keeps a binding from shadowing them.

use xkeysym::Keysym;

use crate::Direction;

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
    /// Send the focused window to the neighbouring output. See
    /// [`crate::neighbour`] and [`crate::carry`].
    MoveToOutput(Towards),
    /// Show the workspace one step across the grid. See
    /// [`crate::Workspaces::switch`].
    Workspace(Direction),
    /// Show the workspace a person calls this number, counting from 1.
    GoToWorkspace(u16),
    /// Move the focused window one workspace across the grid, and stay.
    SendToWorkspace(Direction),
    /// Move the focused window one workspace across the grid, and go with it.
    CarryToWorkspace(Direction),
    /// Put the focused window on every workspace, or back on one.
    ToggleSticky,
    /// Snap the focused window towards a side: halves, quarters, maximized,
    /// stepping as Windows does. See [`crate::keyed`].
    Snap(Direction),
    /// Start a program, as a program and its arguments. The person's own
    /// program: it is never granted agent consent.
    Spawn(Vec<String>),
    /// Maximize the focused window, or put a maximized one back.
    ToggleMaximize,
    /// Take the focused window off the screen until it is cycled back to.
    Minimize,
    /// Bring the next tab of the focused window's group to the front, or the
    /// previous one. See [`crate::Groups`].
    CycleTab { forward: bool },
    /// Make the focused window a tab of the window focused before it.
    TabWithPrevious,
    /// Take the focused window out of its tab group.
    DetachTab,
    /// Read the config file again and apply it.
    Reload,
    /// Ask the desktop shell for its start menu, on the monitor under the
    /// pointer. What a tap of the Logo key does in the classic profile.
    StartMenu,
    /// Ask the desktop shell for the root menu, at the pointer: the menu a
    /// right-click on the wallpaper opens.
    RootMenu,
    /// Switch the keyboard to its next layout, or its previous one, wrapping
    /// round. With one layout there is nothing to switch to.
    CycleLayout { forward: bool },
    /// Switch the keyboard to the layout a person calls this number,
    /// counting from 1 in the order the config lists them. A number past
    /// the last layout does nothing.
    Layout(u8),
}

/// Which output, from the one a window is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Towards {
    /// The next in reading order (left to right, then top to bottom),
    /// wrapping to the first.
    Next,
    /// The previous in reading order, wrapping to the last.
    Previous,
    /// The nearest on that side, as the monitors sit on the desk. No
    /// wrapping: there is nothing left of the leftmost monitor.
    Side(Direction),
}

/// A pointer button, as far as a binding cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// What dragging a window with the drag modifier held does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drag {
    /// Move the window, grabbed anywhere, not only by its titlebar.
    Move,
    /// Resize it from the edges nearest the pointer. See
    /// [`crate::edges_near`].
    Resize,
}

/// A table of chords and what each one does, plus what a tap of the Logo
/// key does, and the modifier that turns a press anywhere on a window into a
/// drag.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    keys: Vec<(Chord, Action)>,
    /// What tapping the Logo key alone does. See [`crate::LogoTap`].
    tap: Option<Action>,
    /// `None`: no modifier-drag at all, and every click is the client's.
    drag: Option<Mods>,
}

impl Bindings {
    /// The two bindings anyone coming from Windows or Plasma presses without
    /// thinking: Alt+F4 closes, Alt+Tab moves to the next window.
    ///
    /// Alt also drags: left to move a window, right to resize it, from
    /// anywhere on it. That is what Plasma does, and every X window manager
    /// before it.
    ///
    /// No terminal or launcher binding. Starting programs arrives with config
    /// (slice 5), and which program is the person's choice, not a guess made
    /// here.
    #[must_use]
    pub fn classic() -> Self {
        Self::default()
            .drag_with(Mods::alt())
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
        self.keys.retain(|(bound, _)| *bound != chord);
        self.keys.push((chord, action));
        self
    }

    /// Remove whatever a chord meant, so the key goes to the client. A user's
    /// config uses this to take back a profile's binding.
    #[must_use]
    pub fn unbind(mut self, chord: Chord) -> Self {
        self.keys.retain(|(bound, _)| *bound != chord);
        self
    }

    /// Make a tap of the Logo key, pressed and released with nothing else,
    /// do `action`, replacing whatever it did before.
    #[must_use]
    pub fn bind_tap(mut self, action: Action) -> Self {
        self.tap = Some(action);
        self
    }

    /// Make a tap of the Logo key do nothing.
    #[must_use]
    pub fn unbind_tap(mut self) -> Self {
        self.tap = None;
        self
    }

    /// What a tap of the Logo key does, if anything.
    #[must_use]
    pub fn tap(&self) -> Option<&Action> {
        self.tap.as_ref()
    }

    /// No modifier-drag: every click is the client's.
    #[must_use]
    pub fn no_drag(mut self) -> Self {
        self.drag = None;
        self
    }

    /// Every binding, in the order they were made.
    pub fn iter(&self) -> impl Iterator<Item = (&Chord, &Action)> {
        self.keys.iter().map(|(chord, action)| (chord, action))
    }

    /// Make `mods` the modifier that turns a press on a window into a drag.
    #[must_use]
    pub fn drag_with(mut self, mods: Mods) -> Self {
        self.drag = Some(mods);
        self
    }

    /// Whether pressing `button` with `mods` held starts a drag, and which.
    /// The modifiers must match exactly, as for keys, so Alt+Shift+click is
    /// still the client's.
    #[must_use]
    pub fn drag(&self, mods: Mods, button: Button) -> Option<Drag> {
        if self.drag != Some(mods) {
            return None;
        }
        match button {
            Button::Left => Some(Drag::Move),
            Button::Right => Some(Drag::Resize),
            Button::Middle => None,
        }
    }

    /// What this key press means, if anything.
    ///
    /// `syms` are the key's [`candidates`], the most particular first, and
    /// the first of them bound with these modifiers wins. Modifiers must
    /// match exactly, so Alt+Tab and Alt+Shift+Tab can mean different
    /// things.
    #[must_use]
    pub fn resolve(&self, mods: Mods, syms: &[Keysym]) -> Option<&Action> {
        syms.iter().find_map(|&sym| {
            self.keys
                .iter()
                .find(|(chord, _)| chord.mods == mods && chord.key == sym)
                .map(|(_, action)| action)
        })
    }
}

/// Every keysym a key press can be bound by, the most particular first.
///
/// First what the key made with the modifiers held, then what it makes
/// alone: with Shift held the layout turns `a` into `A`, and a binding
/// written `Shift+a` has to match either. Then two that matter only away
/// from a US keyboard:
///
/// - **`latin`**, the key's keysym in the first layout where it makes a
///   Latin character (smithay's `raw_latin_sym_or_raw_current_sym`). Under a
///   Cyrillic or Greek layout, `Logo+q` is pressed on the key that makes
///   `й`, and without this every letter chord dies the moment the person
///   switches layout. It needs a Latin layout in the keymap: "us,ru" binds
///   as "us" does, while "ru" alone does not.
/// - **`shifted`**, what the key makes with Shift in the active layout,
///   counted only when that is a digit. AZERTY puts `&é"'(` on the number
///   row and the digits above them, so `Logo+1`, pressed as the person
///   presses it on any other keyboard, would otherwise be `Logo+&` and match
///   nothing.
///
/// Repeats are dropped. Order matters to [`Bindings::resolve`]: a chord
/// written for what the layout itself makes beats one borrowed from another
/// layout.
#[must_use]
pub fn candidates(
    made: Keysym,
    raw: &[Keysym],
    latin: Option<Keysym>,
    shifted: Option<Keysym>,
) -> Vec<Keysym> {
    let digit = shifted.filter(|sym| (Keysym::_0.raw()..=Keysym::_9.raw()).contains(&sym.raw()));
    let mut candidates = Vec::with_capacity(raw.len() + 3);
    for sym in std::iter::once(made)
        .chain(raw.iter().copied())
        .chain(latin)
        .chain(digit)
    {
        if sym != Keysym::NoSymbol && !candidates.contains(&sym) {
            candidates.push(sym);
        }
    }
    candidates
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

    #[test]
    fn alt_left_drag_moves_and_alt_right_drag_resizes_in_the_classic_table() {
        let classic = Bindings::classic();
        assert_eq!(classic.drag(Mods::alt(), Button::Left), Some(Drag::Move));
        assert_eq!(classic.drag(Mods::alt(), Button::Right), Some(Drag::Resize));
        assert_eq!(classic.drag(Mods::alt(), Button::Middle), None);
    }

    #[test]
    fn a_plain_click_is_never_a_drag() {
        assert_eq!(
            Bindings::classic().drag(Mods::default(), Button::Left),
            None
        );
    }

    #[test]
    fn with_no_drag_modifier_nothing_drags() {
        assert_eq!(Bindings::default().drag(Mods::alt(), Button::Left), None);
    }

    fn logo(key: Keysym) -> Chord {
        Chord {
            mods: Mods {
                logo: true,
                ..Mods::default()
            },
            key,
        }
    }

    #[test]
    fn under_a_cyrillic_layout_a_letter_chord_is_found_on_the_latin_layout() {
        let syms = candidates(
            Keysym::Cyrillic_shorti,
            &[Keysym::Cyrillic_shorti],
            Some(Keysym::q),
            Some(Keysym::Cyrillic_SHORTI),
        );
        assert_eq!(syms, [Keysym::Cyrillic_shorti, Keysym::q]);
        let bindings = Bindings::default().bind(logo(Keysym::q), Action::Close);
        assert_eq!(
            bindings.resolve(logo(Keysym::q).mods, &syms),
            Some(&Action::Close)
        );
    }

    #[test]
    fn on_azerty_the_number_row_counts_as_its_digits() {
        let syms = candidates(
            Keysym::ampersand,
            &[Keysym::ampersand],
            Some(Keysym::ampersand),
            Some(Keysym::_1),
        );
        assert_eq!(syms, [Keysym::ampersand, Keysym::_1]);
        let bindings = Bindings::default().bind(logo(Keysym::_1), Action::GoToWorkspace(1));
        assert_eq!(
            bindings.resolve(logo(Keysym::_1).mods, &syms),
            Some(&Action::GoToWorkspace(1))
        );
    }

    #[test]
    fn a_shifted_symbol_that_is_not_a_digit_is_no_candidate() {
        let syms = candidates(
            Keysym::_1,
            &[Keysym::_1],
            Some(Keysym::_1),
            Some(Keysym::exclam),
        );
        assert_eq!(syms, [Keysym::_1], "a US number row stays its digits");
    }

    #[test]
    fn the_layouts_own_meaning_beats_one_borrowed_from_another_layout() {
        // Bound in the other order, so it is the candidates' order that
        // decides, not the bindings'.
        let bindings = Bindings::default()
            .bind(logo(Keysym::q), Action::Close)
            .bind(logo(Keysym::Cyrillic_shorti), Action::Minimize);
        let syms = candidates(
            Keysym::Cyrillic_shorti,
            &[Keysym::Cyrillic_shorti],
            Some(Keysym::q),
            None,
        );
        assert_eq!(
            bindings.resolve(logo(Keysym::q).mods, &syms),
            Some(&Action::Minimize)
        );
    }

    #[test]
    fn a_key_that_makes_nothing_is_no_candidate() {
        assert!(candidates(Keysym::NoSymbol, &[], None, None).is_empty());
    }

    #[test]
    fn unbinding_hands_the_chord_back_to_the_client() {
        let chord = Chord {
            mods: Mods::alt(),
            key: Keysym::F4,
        };
        let bindings = Bindings::classic().unbind(chord);
        assert_eq!(bindings.resolve(Mods::alt(), &[Keysym::F4]), None);
        assert!(bindings.resolve(Mods::alt(), &[Keysym::Tab]).is_some());
    }
}
