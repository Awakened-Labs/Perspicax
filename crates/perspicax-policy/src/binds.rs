//! What a chord means.
//!
//! Resolution only. Which chords exist is config's decision (slice 5), and the
//! two escape hatches (Ctrl+Alt+Backspace, Ctrl+Alt+F1–F12) are not bindings
//! at all: they are [`crate::hatch`]'s. The compositor takes those before a
//! binding is ever consulted, which is what keeps a binding from shadowing
//! them.

use xkeysym::Keysym;

use crate::{Button, Context, Direction, Gesture, MouseChord};

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
    /// Show the next workspace in reading order, or the previous one, as a
    /// scroll over the desktop does. See [`crate::Grid::next`].
    CycleWorkspace { forward: bool },
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
    /// program, unless `agent`: then the person has said, in their own
    /// config, that an agent may act on it, and its process is granted until
    /// it exits. What it starts in turn is not.
    Spawn { command: Vec<String>, agent: bool },
    /// Maximize the focused window, or put a maximized one back.
    ToggleMaximize,
    /// Take the focused window off the screen until it is cycled back to.
    Minimize,
    /// Roll the focused window up to its titlebar: it keeps its place and
    /// its size, and only the bar is drawn. Rolled up already, it stays so,
    /// so a second notch of a wheel bound to this does nothing more. Only a
    /// window whose frame the compositor draws has a titlebar to roll up to.
    Shade,
    /// Unroll the focused window, as it was. Nothing for one not rolled up.
    Unshade,
    /// Roll the focused window up to its titlebar, or unroll it.
    ToggleShade,
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
    /// Ask the desktop shell for the pie of this name, centred at the
    /// pointer: one of `[shell.pie.menus]`. Asked again, it closes.
    Pie(String),
    /// Switch the keyboard to its next layout, or its previous one, wrapping
    /// round. With one layout there is nothing to switch to.
    CycleLayout { forward: bool },
    /// Switch the keyboard to the layout a person calls this number,
    /// counting from 1 in the order the config lists them. A number past
    /// the last layout does nothing.
    Layout(u8),
    /// Make the focused window more opaque by `[opacity] step`, as far as
    /// wholly opaque. For the person's eyes: a see-through window still
    /// covers what is behind it, as far as an agent is told.
    OpacityUp,
    /// Make the focused window less opaque by `[opacity] step`, as far as
    /// `[opacity] floor`.
    OpacityDown,
    /// Put the focused window back to its application's rule in
    /// `[opacity]`, or opaque, forgetting what the keys set.
    OpacityReset,
    /// Let an agent act on the focused window's program, or stop letting it:
    /// a grant to its process, which also ends when the process exits. Not
    /// an X11 window's, whose consent is Xwayland's and every X11 program's
    /// at once.
    ToggleAgent,
    /// Take back everything an agent was granted, whoever granted it.
    RevokeAgents,
    /// Ask the desktop shell to change the sound, as a volume key does:
    /// the output's volume a step up or down, its sound muted or back, or
    /// the microphone's. Nothing at the lock screen, where the shell is
    /// told nothing.
    Volume(VolumeKey),
}

/// What a volume key asks of the sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKey {
    /// The output's volume up a step.
    Up,
    /// Down a step.
    Down,
    /// The output muted, or its sound back.
    Mute,
    /// The microphone muted, or back.
    MicMute,
}

impl Action {
    /// Whether a binding to this still works at the lock screen. Only
    /// switching layout does: a password is typed in a layout too. Anything
    /// else a binding does over the lock screen -- a terminal opened, a
    /// window moved -- would be an unlock, or a look behind it.
    #[must_use]
    pub fn while_locked(&self) -> bool {
        matches!(self, Self::CycleLayout { .. } | Self::Layout(_))
    }

    /// Whether this acts on the focused window, or takes the keyboard from
    /// it: what a menu of that window's holding the keyboard is closed for
    /// first, so the binding acts on the window and not past it. Switching
    /// workspace is not, though it may take the window away: it acts on a
    /// monitor, whatever has the keyboard. Every action is named, so a new
    /// one is decided here when it is added.
    #[must_use]
    pub fn on_focused_window(&self) -> bool {
        match self {
            Self::Close
            | Self::CycleFocus
            | Self::MoveToOutput(_)
            | Self::SendToWorkspace(_)
            | Self::CarryToWorkspace(_)
            | Self::ToggleSticky
            | Self::Snap(_)
            | Self::ToggleMaximize
            | Self::Minimize
            | Self::Shade
            | Self::Unshade
            | Self::ToggleShade
            | Self::CycleTab { .. }
            | Self::TabWithPrevious
            | Self::DetachTab
            | Self::OpacityUp
            | Self::OpacityDown
            | Self::OpacityReset
            | Self::ToggleAgent => true,
            Self::Workspace(_)
            | Self::GoToWorkspace(_)
            | Self::CycleWorkspace { .. }
            | Self::Spawn { .. }
            | Self::Reload
            | Self::StartMenu
            | Self::RootMenu
            | Self::Pie(_)
            | Self::CycleLayout { .. }
            | Self::Layout(_)
            | Self::RevokeAgents
            | Self::Volume(_) => false,
        }
    }
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

    /// Remove every binding to an action `unwanted` says is unwanted, and
    /// a tap to one, so the keys go to the client: a profile's bindings for
    /// a part of the desktop that is off.
    #[must_use]
    pub fn without(mut self, unwanted: impl Fn(&Action) -> bool) -> Self {
        self.keys.retain(|(_, action)| !unwanted(action));
        self.tap.take_if(|action| unwanted(action));
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
            _ => None,
        }
    }

    /// Whether a mouse binding of `chord` in `context` would take the drag
    /// away from every window: the drag's own modifiers, on a button that
    /// drags, in a context that covers a whole window. The binding would
    /// always be found first, and the drag would never happen.
    ///
    /// A titlebar binding takes the drag only from the titlebar, as it takes
    /// a press there from the frame, and the desktop has no window to drag.
    /// A double-click leaves the first press to drag. None of those takes
    /// the drag away.
    #[must_use]
    pub fn takes_drag(&self, context: Context, chord: &MouseChord) -> bool {
        matches!(context, Context::Window | Context::Anywhere)
            && matches!(
                chord.gesture,
                Gesture::Press(button) if self.drag(chord.mods, button).is_some()
            )
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

    #[test]
    fn a_window_binding_on_the_drag_takes_it_and_a_titlebar_one_does_not() {
        let classic = Bindings::classic();
        let alt = |gesture| MouseChord {
            mods: Mods::alt(),
            gesture,
        };
        for button in [Button::Left, Button::Right] {
            for context in [Context::Window, Context::Anywhere] {
                assert!(
                    classic.takes_drag(context, &alt(Gesture::Press(button))),
                    "{context:?} {button:?}"
                );
            }
            for context in [Context::Titlebar, Context::Desktop] {
                assert!(
                    !classic.takes_drag(context, &alt(Gesture::Press(button))),
                    "{context:?} {button:?}"
                );
            }
            assert!(
                !classic.takes_drag(Context::Window, &alt(Gesture::Double(button))),
                "a double-click leaves the first press to drag"
            );
        }
        assert!(!classic.takes_drag(Context::Window, &alt(Gesture::Press(Button::Middle))));
        assert!(!classic.takes_drag(
            Context::Window,
            &MouseChord {
                mods: Mods::default(),
                gesture: Gesture::Press(Button::Left)
            }
        ));
        assert!(
            !Bindings::default().takes_drag(Context::Window, &alt(Gesture::Press(Button::Left))),
            "no drag modifier, nothing to take"
        );
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
    fn only_switching_layout_works_at_the_lock_screen() {
        assert!(Action::CycleLayout { forward: true }.while_locked());
        assert!(Action::Layout(2).while_locked());
        assert!(
            !Action::Spawn {
                command: vec!["foot".to_owned()],
                agent: false
            }
            .while_locked()
        );
        assert!(!Action::Reload.while_locked());
        assert!(
            !Action::Volume(VolumeKey::Up).while_locked(),
            "the shell is told nothing at the lock screen"
        );
    }

    #[test]
    fn what_acts_on_the_focused_window_and_what_does_not() {
        let on_it = [
            Action::Close,
            Action::CycleFocus,
            Action::MoveToOutput(Towards::Next),
            Action::SendToWorkspace(Direction::Right),
            Action::CarryToWorkspace(Direction::Right),
            Action::ToggleSticky,
            Action::Snap(Direction::Left),
            Action::ToggleMaximize,
            Action::Minimize,
            Action::Shade,
            Action::Unshade,
            Action::ToggleShade,
            Action::CycleTab { forward: true },
            Action::TabWithPrevious,
            Action::DetachTab,
            Action::OpacityUp,
            Action::OpacityDown,
            Action::OpacityReset,
            Action::ToggleAgent,
        ];
        let elsewhere = [
            Action::Workspace(Direction::Right),
            Action::GoToWorkspace(2),
            Action::CycleWorkspace { forward: true },
            Action::Spawn {
                command: vec!["foot".to_owned()],
                agent: false,
            },
            Action::Reload,
            Action::StartMenu,
            Action::RootMenu,
            Action::Pie("main".to_owned()),
            Action::CycleLayout { forward: true },
            Action::Layout(2),
            Action::RevokeAgents,
            Action::Volume(VolumeKey::Up),
        ];
        for action in &on_it {
            assert!(action.on_focused_window(), "{action:?}");
        }
        for action in &elsewhere {
            assert!(!action.on_focused_window(), "{action:?}");
        }
        // Nothing that works at the lock screen acts on a window behind it.
        for action in on_it.iter().chain(&elsewhere) {
            assert!(
                !(action.while_locked() && action.on_focused_window()),
                "{action:?}"
            );
        }
    }

    #[test]
    fn a_part_of_the_desktop_turned_off_takes_its_bindings_and_its_tap_with_it() {
        let up = Chord {
            mods: Mods::default(),
            key: Keysym::XF86_AudioRaiseVolume,
        };
        let bindings = Bindings::classic()
            .bind(up, Action::Volume(VolumeKey::Up))
            .bind_tap(Action::Volume(VolumeKey::Mute))
            .without(|action| matches!(action, Action::Volume(_)));
        assert_eq!(
            bindings.resolve(Mods::default(), &[Keysym::XF86_AudioRaiseVolume]),
            None
        );
        assert_eq!(bindings.tap(), None);
        assert_eq!(
            bindings.resolve(Mods::alt(), &[Keysym::F4]),
            Some(&Action::Close),
            "the rest kept"
        );
        let kept = Bindings::classic()
            .bind_tap(Action::StartMenu)
            .without(|action| matches!(action, Action::Volume(_)));
        assert_eq!(kept.tap(), Some(&Action::StartMenu));
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
