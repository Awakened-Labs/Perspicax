//! What a mouse button, a double-click or a notch of the wheel means, where
//! the pointer is.
//!
//! A mouse binding is a chord, as a key's is: modifiers held, and a button
//! pressed or the wheel turned. What makes it the mouse's is where it
//! happens. Over the empty desktop the thumb button can mean "next
//! workspace", while over Firefox the same button is Back and must stay
//! Firefox's. So every binding names a [`Context`], and a press is looked up
//! in the most particular context the pointer is in first, then in the
//! broader ones.
//!
//! Resolution only, as in [`crate::Bindings`]. Which bindings exist is
//! config's decision, and what is under the pointer is the compositor's.

use crate::{Action, Mods, Press, is_double};

/// Every button with a name, in the order of their codes.
const NAMED: [Button; 8] = [
    Button::Left,
    Button::Right,
    Button::Middle,
    Button::Side,
    Button::Extra,
    Button::Forward,
    Button::Back,
    Button::Task,
];

/// A pointer button, as far as a binding cares.
///
/// Linux numbers a mouse's buttons from `BTN_LEFT`, and the eight it names
/// are named here. Anything else, a gaming mouse's twelfth button say, is
/// kept by its code. [`Button::from_code`] is the way in from a device, and
/// it never makes a `Code` for a button that has a name, so one button is
/// never two values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
    /// The thumb button most mice have nearer the wrist, which browsers take
    /// as Back: X's button 8.
    Side,
    /// The thumb button further forward, which browsers take as Forward:
    /// X's button 9.
    Extra,
    Forward,
    Back,
    Task,
    /// A button without a name here, by its Linux code.
    Code(u32),
}

impl Button {
    /// The button a device reported by its Linux code.
    #[must_use]
    pub fn from_code(code: u32) -> Self {
        NAMED
            .into_iter()
            .find(|named| named.code() == code)
            .unwrap_or(Self::Code(code))
    }

    /// The button's Linux code, as a device reports it and a client hears
    /// it: `BTN_LEFT` to `BTN_TASK` in `linux/input-event-codes.h`.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Left => 0x110,
            Self::Right => 0x111,
            Self::Middle => 0x112,
            Self::Side => 0x113,
            Self::Extra => 0x114,
            Self::Forward => 0x115,
            Self::Back => 0x116,
            Self::Task => 0x117,
            Self::Code(code) => code,
        }
    }
}

/// A notch of the wheel, by which way it turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    Up,
    Down,
    Left,
    Right,
}

impl Wheel {
    /// Which way a scroll of `amount` along one axis went, as Wayland counts
    /// it: negative is up, or left. Natural scrolling is already in the
    /// sign, as libinput reports it. `None` for no movement at all.
    #[must_use]
    pub fn of(vertical: bool, amount: f64) -> Option<Self> {
        match (vertical, amount) {
            (_, amount) if amount == 0.0 || amount.is_nan() => None,
            (true, amount) if amount < 0.0 => Some(Self::Up),
            (true, _) => Some(Self::Down),
            (false, amount) if amount < 0.0 => Some(Self::Left),
            (false, _) => Some(Self::Right),
        }
    }
}

/// What the mouse did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// A button went down.
    Press(Button),
    /// A button went down a second time, soon after the first and close to
    /// it. See [`Clicks`].
    Double(Button),
    /// The wheel turned one notch.
    Wheel(Wheel),
}

/// A mouse gesture and the modifiers held with it: the mouse's
/// [`crate::Chord`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseChord {
    pub mods: Mods,
    pub gesture: Gesture,
}

/// Where the pointer is, as far as a mouse binding cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// A window's title, or one of its tabs.
    Titlebar,
    /// Anywhere on a window: its frame, or what the application drew.
    Window,
    /// The empty desktop: no window there, and no panel or menu over the
    /// wallpaper.
    Desktop,
    /// Wherever the pointer is. Where a press is looked up last.
    Anywhere,
}

impl Context {
    /// Where to look when this context binds nothing: a titlebar is part of
    /// a window, and a window and the desktop are both somewhere.
    #[must_use]
    pub fn broader(self) -> Option<Self> {
        match self {
            Self::Titlebar => Some(Self::Window),
            Self::Window | Self::Desktop => Some(Self::Anywhere),
            Self::Anywhere => None,
        }
    }

    /// This context and every broader one, most particular first.
    fn widening(self) -> impl Iterator<Item = Self> {
        std::iter::successors(Some(self), |context| context.broader())
    }
}

/// A table of mouse chords, each in a context, and what each one does there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MouseBindings {
    /// `None` is a chord handed back in its context: nothing is bound there,
    /// and a broader context's binding does not reach it either.
    bound: Vec<(Context, MouseChord, Option<Action>)>,
}

impl MouseBindings {
    /// Bind a chord in a context, replacing whatever it meant there before.
    #[must_use]
    pub fn bind(self, context: Context, chord: MouseChord, action: Action) -> Self {
        self.with(context, chord, Some(action))
    }

    /// Hand a chord back in a context: the press goes on to the compositor's
    /// own handling, or to the client, even where a broader context binds
    /// it. `[mouse.window] "Mouse8" = "none"` keeps Firefox's Back while
    /// `[mouse.anywhere] "Mouse8"` flips workspaces everywhere else.
    #[must_use]
    pub fn unbind(self, context: Context, chord: MouseChord) -> Self {
        self.with(context, chord, None)
    }

    fn with(mut self, context: Context, chord: MouseChord, action: Option<Action>) -> Self {
        self.bound
            .retain(|&(bound_in, bound, _)| (bound_in, bound) != (context, chord));
        self.bound.push((context, chord, action));
        self
    }

    /// Every binding, in the order they were made, `None` for a chord
    /// handed back.
    pub fn iter(&self) -> impl Iterator<Item = (Context, &MouseChord, Option<&Action>)> {
        self.bound
            .iter()
            .map(|(context, chord, action)| (*context, chord, action.as_ref()))
    }

    /// What a gesture with `mods` held means with the pointer in `context`,
    /// if anything.
    ///
    /// Each context is tried from the most particular out, and the first
    /// that says anything about the chord decides: a titlebar binding beats
    /// a window one, either beats `anywhere`, and a chord handed back stops
    /// the search. Modifiers must match exactly, as for keys. A double-click
    /// is a double-click first, everywhere, and only then the press it also
    /// is: a `Double+Mouse1` anywhere beats a plain `Mouse1` on the
    /// titlebar.
    #[must_use]
    pub fn resolve(&self, context: Context, mods: Mods, gesture: Gesture) -> Option<&Action> {
        let single = match gesture {
            Gesture::Double(button) => Some(Gesture::Press(button)),
            Gesture::Press(_) | Gesture::Wheel(_) => None,
        };
        std::iter::once(gesture).chain(single).find_map(|gesture| {
            let chord = MouseChord { mods, gesture };
            context
                .widening()
                .find_map(|context| {
                    self.bound
                        .iter()
                        .find(|&&(bound_in, bound, _)| (bound_in, bound) == (context, chord))
                })
                .and_then(|(_, _, action)| action.as_ref())
        })
    }
}

/// Watches the buttons for double-clicks, and remembers which presses a
/// binding took, so that their releases are taken too.
///
/// A client that saw a button come up without having seen it go down would
/// be as confused as one that saw only the press, so the two go together.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clicks {
    /// The last press that could begin a double-click.
    last: Option<(Button, Press)>,
    /// Buttons still down whose press a binding took.
    taken: Vec<Button>,
}

impl Clicks {
    /// A button went down, `within_ms` of the last press counting as a
    /// double-click. Whether this is a press, or the second of a
    /// double-click.
    ///
    /// A double-click begins nothing: the press after it is a press again,
    /// so three quick clicks are a double-click and a click, as everywhere
    /// else.
    pub fn press(&mut self, button: Button, press: Press, within_ms: u32) -> Gesture {
        let double = self
            .last
            .take()
            .is_some_and(|(was, first)| was == button && is_double(first, press, within_ms));
        if double {
            return Gesture::Double(button);
        }
        self.last = Some((button, press));
        Gesture::Press(button)
    }

    /// A binding took this button's press.
    pub fn take(&mut self, button: Button) {
        if !self.taken.contains(&button) {
            self.taken.push(button);
        }
    }

    /// A button came up. Whether its press was taken, and so its release is
    /// taken too.
    #[must_use]
    pub fn release(&mut self, button: Button) -> bool {
        match self.taken.iter().position(|&taken| taken == button) {
            Some(at) => {
                self.taken.swap_remove(at);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Direction;

    fn plain(gesture: Gesture) -> MouseChord {
        MouseChord {
            mods: Mods::default(),
            gesture,
        }
    }

    const SIDE: Gesture = Gesture::Press(Button::Side);
    const NEXT: Action = Action::Workspace(Direction::Right);

    #[test]
    fn the_named_buttons_are_linuxs_and_any_other_is_kept_by_its_code() {
        assert_eq!(Button::from_code(0x110), Button::Left);
        assert_eq!(Button::from_code(0x111), Button::Right);
        assert_eq!(Button::from_code(0x112), Button::Middle);
        assert_eq!(Button::from_code(275), Button::Side);
        assert_eq!(Button::from_code(0x117), Button::Task);
        assert_eq!(Button::from_code(0x118), Button::Code(0x118));
        assert_eq!(Button::from_code(0x100), Button::Code(0x100));
        for code in 0x100..0x120 {
            assert_eq!(Button::from_code(code).code(), code, "{code:#x}");
        }
    }

    #[test]
    fn a_scroll_up_or_left_is_negative_and_none_is_no_notch() {
        assert_eq!(Wheel::of(true, -15.0), Some(Wheel::Up));
        assert_eq!(Wheel::of(true, 120.0), Some(Wheel::Down));
        assert_eq!(Wheel::of(false, -1.0), Some(Wheel::Left));
        assert_eq!(Wheel::of(false, 1.0), Some(Wheel::Right));
        assert_eq!(Wheel::of(true, 0.0), None);
    }

    #[test]
    fn a_desktop_binding_is_found_over_the_desktop_and_nowhere_else() {
        let bindings = MouseBindings::default().bind(Context::Desktop, plain(SIDE), NEXT);
        let none = Mods::default();
        assert_eq!(bindings.resolve(Context::Desktop, none, SIDE), Some(&NEXT));
        for elsewhere in [Context::Window, Context::Titlebar, Context::Anywhere] {
            assert_eq!(
                bindings.resolve(elsewhere, none, SIDE),
                None,
                "{elsewhere:?}"
            );
        }
    }

    #[test]
    fn anywhere_is_the_fallback_and_a_titlebar_is_part_of_its_window() {
        let bindings = MouseBindings::default()
            .bind(Context::Anywhere, plain(SIDE), Action::Close)
            .bind(Context::Window, plain(SIDE), Action::Minimize);
        let none = Mods::default();
        assert_eq!(
            bindings.resolve(Context::Titlebar, none, SIDE),
            Some(&Action::Minimize)
        );
        assert_eq!(
            bindings.resolve(Context::Desktop, none, SIDE),
            Some(&Action::Close)
        );
        assert_eq!(
            bindings.resolve(Context::Anywhere, none, SIDE),
            Some(&Action::Close)
        );
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let bindings = MouseBindings::default().bind(Context::Desktop, plain(SIDE), NEXT);
        assert_eq!(bindings.resolve(Context::Desktop, Mods::alt(), SIDE), None);
    }

    #[test]
    fn a_double_click_is_a_double_click_everywhere_before_it_is_a_press() {
        let bindings = MouseBindings::default()
            .bind(
                Context::Titlebar,
                plain(Gesture::Press(Button::Left)),
                Action::Close,
            )
            .bind(
                Context::Anywhere,
                plain(Gesture::Double(Button::Left)),
                Action::ToggleMaximize,
            );
        let none = Mods::default();
        assert_eq!(
            bindings.resolve(Context::Titlebar, none, Gesture::Double(Button::Left)),
            Some(&Action::ToggleMaximize)
        );
        assert_eq!(
            bindings.resolve(Context::Titlebar, none, Gesture::Press(Button::Left)),
            Some(&Action::Close)
        );
    }

    #[test]
    fn a_double_click_with_no_binding_of_its_own_is_a_press() {
        let bindings = MouseBindings::default().bind(Context::Desktop, plain(SIDE), NEXT);
        assert_eq!(
            bindings.resolve(
                Context::Desktop,
                Mods::default(),
                Gesture::Double(Button::Side)
            ),
            Some(&NEXT)
        );
    }

    #[test]
    fn a_later_binding_replaces_an_earlier_one_and_unbinding_removes_it() {
        let bindings = MouseBindings::default()
            .bind(Context::Desktop, plain(SIDE), Action::Close)
            .bind(Context::Desktop, plain(SIDE), NEXT);
        assert_eq!(bindings.iter().count(), 1);
        assert_eq!(
            bindings.resolve(Context::Desktop, Mods::default(), SIDE),
            Some(&NEXT)
        );
        let bindings = bindings.unbind(Context::Desktop, plain(SIDE));
        assert_eq!(
            bindings.resolve(Context::Desktop, Mods::default(), SIDE),
            None
        );
    }

    #[test]
    fn a_chord_handed_back_in_a_context_is_not_found_in_a_broader_one_there() {
        let bindings = MouseBindings::default()
            .bind(Context::Anywhere, plain(SIDE), NEXT)
            .unbind(Context::Window, plain(SIDE));
        let none = Mods::default();
        assert_eq!(bindings.resolve(Context::Window, none, SIDE), None);
        assert_eq!(
            bindings.resolve(Context::Titlebar, none, SIDE),
            None,
            "a titlebar is part of the window"
        );
        assert_eq!(bindings.resolve(Context::Desktop, none, SIDE), Some(&NEXT));
    }

    #[test]
    fn a_wheel_notch_resolves_by_its_direction() {
        let down = Gesture::Wheel(Wheel::Down);
        let bindings = MouseBindings::default().bind(Context::Desktop, plain(down), NEXT);
        let none = Mods::default();
        assert_eq!(bindings.resolve(Context::Desktop, none, down), Some(&NEXT));
        assert_eq!(
            bindings.resolve(Context::Desktop, none, Gesture::Wheel(Wheel::Up)),
            None
        );
    }

    #[test]
    fn a_second_quick_near_press_of_the_same_button_is_a_double_click() {
        let mut clicks = Clicks::default();
        assert_eq!(
            clicks.press(Button::Left, (1000, (10.0, 10.0)), 400),
            Gesture::Press(Button::Left)
        );
        assert_eq!(
            clicks.press(Button::Left, (1300, (12.0, 11.0)), 400),
            Gesture::Double(Button::Left)
        );
        assert_eq!(
            clicks.press(Button::Left, (1400, (12.0, 11.0)), 400),
            Gesture::Press(Button::Left),
            "a third click begins again"
        );
    }

    #[test]
    fn another_button_or_a_slow_or_distant_press_is_no_double_click() {
        let mut clicks = Clicks::default();
        clicks.press(Button::Left, (1000, (10.0, 10.0)), 400);
        assert_eq!(
            clicks.press(Button::Right, (1100, (10.0, 10.0)), 400),
            Gesture::Press(Button::Right)
        );
        assert_eq!(
            clicks.press(Button::Right, (1600, (10.0, 10.0)), 400),
            Gesture::Press(Button::Right),
            "too slow"
        );
        assert_eq!(
            clicks.press(Button::Right, (1700, (40.0, 10.0)), 400),
            Gesture::Press(Button::Right),
            "too far"
        );
    }

    #[test]
    fn only_a_taken_press_has_its_release_taken_and_only_once() {
        let mut clicks = Clicks::default();
        clicks.take(Button::Side);
        assert!(!clicks.release(Button::Left));
        assert!(clicks.release(Button::Side));
        assert!(!clicks.release(Button::Side));
    }
}
