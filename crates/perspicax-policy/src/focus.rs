//! Which window has the keyboard, and which is on top.
//!
//! Focus and stacking are separate decisions. Under click-to-focus they
//! always happen together, which makes them easy to confuse, but sloppy focus
//! without autoraise is exactly the case where they come apart: the window
//! under the pointer takes the keyboard and stays where it is in the stack.
//! So every decision here names both parts separately.

use std::time::{Duration, Instant};

/// How the keyboard follows the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusModel {
    /// A window takes focus when it is clicked. What Plasma and Windows do,
    /// and the default.
    #[default]
    Click,
    /// A window takes focus when the pointer enters it, and keeps it while the
    /// pointer crosses the empty desktop. The usual choice on Fluxbox.
    Sloppy,
    /// A window has focus only while the pointer is over it. Over the empty
    /// desktop, nothing has focus.
    Strict,
}

impl FocusModel {
    /// Every model, in the order the README lists them.
    pub const ALL: [Self; 3] = [Self::Click, Self::Sloppy, Self::Strict];

    /// Its name as `[focus] model` writes it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Sloppy => "sloppy",
            Self::Strict => "strict",
        }
    }
}

/// The focus policy: a model, and whether a window focused by pointing is
/// also raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Focus {
    pub model: FocusModel,
    /// Raise a window when pointing at it focuses it. Has no effect under
    /// [`FocusModel::Click`], where the click raises anyway.
    pub autoraise: bool,
}

/// What happens to keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<W> {
    /// Leave it where it is.
    Keep,
    /// Give it to this window.
    To(W),
    /// Take it away from every window.
    Clear,
}

/// One decision: where the keyboard goes, and what comes to the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision<W> {
    pub focus: Change<W>,
    pub raise: Option<W>,
}

impl<W> Decision<W> {
    /// Nothing changes.
    #[must_use]
    pub fn none() -> Self {
        Self {
            focus: Change::Keep,
            raise: None,
        }
    }
}

impl Focus {
    /// The pointer moved, and is now over `under` (or over no window).
    ///
    /// Called for every motion, not only when the pointer crosses into a new
    /// window, so a window that already has focus decides [`Change::Keep`].
    /// That makes the caller's job "apply what was decided" rather than "work
    /// out whether this was a crossing".
    #[must_use]
    pub fn pointer_over<W: Copy + PartialEq>(
        &self,
        under: Option<W>,
        focused: Option<W>,
    ) -> Decision<W> {
        let entered = under.filter(|&window| Some(window) != focused);
        let focus = match (self.model, under, entered) {
            (FocusModel::Click, _, _) => return Decision::none(),
            (_, _, Some(window)) => Change::To(window),
            (FocusModel::Strict, None, _) if focused.is_some() => Change::Clear,
            _ => Change::Keep,
        };
        let raise = match focus {
            Change::To(window) if self.autoraise => Some(window),
            _ => None,
        };
        Decision { focus, raise }
    }

    /// A button went down over `under` (or over no window).
    ///
    /// Every model raises what is clicked, and every model focuses it. A click
    /// is the one gesture that means "this window" under any policy. A click
    /// on the empty desktop changes nothing: under click-to-focus that is what
    /// Plasma does, and under strict focus the pointer leaving the window has
    /// already cleared it.
    #[must_use]
    pub fn pressed<W: Copy + PartialEq>(
        &self,
        under: Option<W>,
        focused: Option<W>,
    ) -> Decision<W> {
        match under {
            Some(window) => Decision {
                focus: if focused == Some(window) {
                    Change::Keep
                } else {
                    Change::To(window)
                },
                raise: Some(window),
            },
            None => Decision::none(),
        }
    }
}

/// The window a "next window" chord should bring forward, given the stack
/// bottom to top and the window that has the keyboard.
///
/// The bottom one, so pressing the chord repeatedly walks through every
/// window in turn: each press lifts the bottom window to the top, and the
/// next press finds a different one at the bottom. Swapping the top two
/// instead would never reach a third window.
///
/// A lone window is brought forward only if it does not already have the
/// keyboard: a minimized one that is the only window on the workspace comes
/// back, and an unfocused one is focused. Otherwise there is nothing to cycle,
/// and `None`.
#[must_use]
pub fn cycle<W: Copy + PartialEq>(bottom_to_top: &[W], focused: Option<W>) -> Option<W> {
    match bottom_to_top {
        [first, _, ..] => Some(*first),
        [only] if focused != Some(*only) => Some(*only),
        _ => None,
    }
}

/// How old an activation token may be and still move focus.
///
/// Long enough for a slow application to start and map its window after the
/// click that launched it; short enough that a token saved up and spent later
/// cannot pull focus away from whatever the person is doing by then. Sooner
/// than that, a token is refused once the person moves on ([`Attention`]).
pub const ACTIVATION_WINDOW: Duration = Duration::from_secs(10);

/// Whether an `xdg-activation` request may take focus.
///
/// Only a token minted in answer to the person's own input, and only while
/// it is fresh. The compositor says whether input was behind it: for an xdg
/// token, that the client with the keys made it, since a serial any client
/// can name is no proof alone (#187), or the client the person's latest
/// press went to, naming that press, and that the person has not moved on
/// since it was made ([`Attention`], #257); for an X11 request, the X time of
/// the input it carries, believed only while an X11 window has the keys,
/// since any X client can learn the time (#256). Anything else is a window
/// trying to take focus for itself, which is the focus stealing the protocol
/// exists to prevent. A refused activation is not an error. The window simply
/// stays where it is.
#[must_use]
pub fn grants_activation(age: Duration, from_input: bool) -> bool {
    from_input && age < ACTIVATION_WINDOW
}

/// How far the person's attention has come: an activation token is good
/// only on the turn it was made (#257). Stamped on a token as it is made,
/// and compared as it is spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn(u64);

/// Where the person's attention has been, for telling an activation token
/// made since from one made before they moved on (#257).
///
/// It turns with every key or button the person presses, and every time the
/// keys go to another client: a window opening, a click, Alt+Tab, the
/// pointer under sloppy focus, a workspace switch, the lock. Except as they
/// leave a launcher, which keeps them while it is up (#131), so that they
/// leave one only as it goes, and the program it started spends its token
/// then. A token made on an earlier turn was made before the person moved
/// on: spent now, it would pull a window forward into whatever they type
/// next, a password at a terminal's `sudo` prompt say. Releases and motion
/// are no turn. Whatever is pressed to open a link or start a program is
/// pressed before its token is made, and a release that moves the keys is
/// counted as they move.
///
/// It also keeps the person's latest press, and which client it reached. A
/// notification or a dock never has the keys, and the click it was sent is
/// what vouches for a token it makes. Any turn after it forgets it.
#[derive(Debug, Clone)]
pub struct Attention<S, C> {
    turns: u64,
    /// Which client has the keys, and whether as a launcher. `None` while
    /// nothing has them.
    keys: Option<(C, bool)>,
    press: Option<Latest<S, C>>,
}

/// The person's latest press.
#[derive(Debug, Clone)]
struct Latest<S, C> {
    /// What it was sent with, if a client was sent it.
    serial: S,
    at: Instant,
    /// The client it reached, once it has.
    reached: Option<C>,
}

impl<S, C> Default for Attention<S, C> {
    fn default() -> Self {
        Self {
            turns: 0,
            keys: None,
            press: None,
        }
    }
}

impl<S: PartialEq, C: PartialEq> Attention<S, C> {
    /// The turn it is now.
    #[must_use]
    pub fn turn(&self) -> Turn {
        Turn(self.turns)
    }

    /// The person pressed a key or a button at `at`, to be sent as `serial`
    /// to whatever takes it. Whether a binding takes it, or a client, it is
    /// a turn.
    pub fn pressed(&mut self, serial: S, at: Instant) {
        self.turns += 1;
        self.press = Some(Latest {
            serial,
            at,
            reached: None,
        });
    }

    /// The keys went to `client`, or to nothing, and to a launcher if
    /// `launcher`. A turn if another client has them now, unless a launcher
    /// had them. One that keeps them is no turn: its menu, a dialog, another
    /// of its windows.
    pub fn keys_to(&mut self, client: Option<C>, launcher: bool) {
        let left_a_launcher = matches!(self.keys, Some((_, true)));
        let kept = self.keys.as_ref().map(|(held, _)| held) == client.as_ref();
        if !kept && !left_a_launcher {
            self.turns += 1;
            self.press = None;
        }
        self.keys = client.map(|client| (client, launcher));
    }

    /// A press sent as `serial` reached `client`. Remembered only if it is
    /// the person's latest: any other is an agent's, or long gone.
    pub fn reached(&mut self, serial: &S, client: C) {
        if let Some(press) = &mut self.press
            && press.serial == *serial
        {
            press.reached = Some(client);
        }
    }

    /// Whether `client` was sent the person's latest press as `serial`, less
    /// than [`ACTIVATION_WINDOW`] before `now`, and nothing has turned since.
    #[must_use]
    pub fn vouches(&self, serial: &S, client: &C, now: Instant) -> bool {
        self.press.as_ref().is_some_and(|press| {
            press.serial == *serial
                && press.reached.as_ref() == Some(client)
                && now.saturating_duration_since(press.at) < ACTIVATION_WINDOW
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(model: FocusModel, autoraise: bool) -> Focus {
        Focus { model, autoraise }
    }

    #[test]
    fn click_to_focus_ignores_the_pointer_moving() {
        let decision = Focus::default().pointer_over(Some(2), Some(1));
        assert_eq!(decision, Decision::none());
    }

    #[test]
    fn a_click_focuses_and_raises_under_every_model() {
        for model in [FocusModel::Click, FocusModel::Sloppy, FocusModel::Strict] {
            let decision = policy(model, false).pressed(Some(2), Some(1));
            assert_eq!(decision.focus, Change::To(2), "{model:?}");
            assert_eq!(decision.raise, Some(2), "{model:?}");
        }
    }

    #[test]
    fn clicking_the_focused_window_raises_it_without_refocusing() {
        let decision = Focus::default().pressed(Some(1), Some(1));
        assert_eq!(decision.focus, Change::Keep);
        assert_eq!(decision.raise, Some(1));
    }

    #[test]
    fn clicking_the_empty_desktop_changes_nothing() {
        assert_eq!(
            Focus::default().pressed::<u32>(None, Some(1)),
            Decision::none()
        );
    }

    #[test]
    fn sloppy_focus_follows_the_pointer_into_a_window_without_raising_it() {
        let decision = policy(FocusModel::Sloppy, false).pointer_over(Some(2), Some(1));
        assert_eq!(decision.focus, Change::To(2));
        assert_eq!(decision.raise, None);
    }

    #[test]
    fn sloppy_focus_keeps_the_window_when_the_pointer_crosses_the_desktop() {
        let decision = policy(FocusModel::Sloppy, false).pointer_over(None, Some(1));
        assert_eq!(decision.focus, Change::Keep);
    }

    #[test]
    fn strict_focus_clears_over_the_desktop() {
        let decision = policy(FocusModel::Strict, false).pointer_over(None, Some(1));
        assert_eq!(decision.focus, Change::Clear);
    }

    #[test]
    fn strict_focus_over_the_desktop_with_nothing_focused_is_quiet() {
        let decision = policy(FocusModel::Strict, false).pointer_over::<u32>(None, None);
        assert_eq!(decision, Decision::none());
    }

    #[test]
    fn autoraise_raises_what_pointing_focused() {
        let decision = policy(FocusModel::Sloppy, true).pointer_over(Some(2), Some(1));
        assert_eq!(decision.raise, Some(2));
    }

    #[test]
    fn moving_within_the_focused_window_decides_nothing() {
        let decision = policy(FocusModel::Sloppy, true).pointer_over(Some(1), Some(1));
        assert_eq!(decision, Decision::none());
    }

    #[test]
    fn cycling_brings_the_bottom_window_forward() {
        assert_eq!(cycle(&[1, 2, 3], Some(3)), Some(1));
    }

    #[test]
    fn cycling_the_focused_window_alone_or_none_does_nothing() {
        assert_eq!(cycle(&[1], Some(1)), None);
        assert_eq!(cycle::<u32>(&[], None), None);
    }

    /// Issue #6: the only window on a workspace, minimized or merely
    /// unfocused, is still somewhere Alt+Tab can go.
    #[test]
    fn cycling_brings_back_a_lone_window_that_does_not_have_the_keyboard() {
        assert_eq!(cycle(&[7], None), Some(7), "minimized, or nothing focused");
        assert_eq!(cycle(&[7], Some(9)), Some(7), "focus is somewhere else");
    }

    #[test]
    fn a_fresh_token_from_a_click_may_take_focus() {
        assert!(grants_activation(Duration::from_millis(300), true));
    }

    #[test]
    fn a_token_with_no_input_behind_it_may_not() {
        assert!(!grants_activation(Duration::ZERO, false));
    }

    #[test]
    fn a_stale_token_may_not() {
        assert!(!grants_activation(ACTIVATION_WINDOW, true));
    }

    /// Serials are numbers, clients letters.
    fn attention() -> Attention<u32, char> {
        Attention::default()
    }

    #[test]
    fn every_press_is_a_turn() {
        let mut attention = attention();
        let before = attention.turn();
        attention.pressed(1, Instant::now());
        let after = attention.turn();
        assert_ne!(before, after);
        attention.pressed(2, Instant::now());
        assert_ne!(after, attention.turn());
    }

    #[test]
    fn the_keys_going_to_another_client_or_none_are_a_turn() {
        let mut attention = attention();
        let mut turns = vec![attention.turn()];
        attention.keys_to(Some('a'), false);
        turns.push(attention.turn());
        attention.keys_to(Some('b'), false);
        turns.push(attention.turn());
        attention.keys_to(None, false);
        turns.push(attention.turn());
        turns.dedup();
        assert_eq!(turns.len(), 4, "nothing to a, a to b, b to nothing");
    }

    #[test]
    fn the_keys_kept_by_their_client_are_no_turn() {
        let mut attention = attention();
        attention.keys_to(Some('a'), false);
        let turn = attention.turn();
        attention.keys_to(Some('a'), false);
        attention.keys_to(None, false);
        attention.keys_to(None, false);
        assert_eq!(attention.turn().0, turn.0 + 1, "only a to nothing");
    }

    /// Fuzzel goes, the keys go back to the window under it, and the program
    /// it started spends its token.
    #[test]
    fn the_keys_leaving_a_launcher_are_no_turn_but_going_to_one_are() {
        let mut attention = attention();
        attention.keys_to(Some('a'), false);
        let before = attention.turn();
        attention.keys_to(Some('l'), true);
        let up = attention.turn();
        assert_ne!(before, up, "a to the launcher");
        attention.keys_to(Some('a'), false);
        assert_eq!(up, attention.turn(), "the launcher back to a");
        attention.keys_to(Some('l'), true);
        attention.keys_to(None, false);
        assert_ne!(up, attention.turn(), "a to the launcher again");
    }

    /// A click on a notification, which never has the keys.
    #[test]
    fn the_latest_press_vouches_for_the_client_it_reached() {
        let mut attention = attention();
        let at = Instant::now();
        attention.pressed(7, at);
        assert!(!attention.vouches(&7, &'n', at), "reached nobody yet");
        attention.reached(&7, 'n');
        assert!(attention.vouches(&7, &'n', at));
        assert!(!attention.vouches(&8, &'n', at), "another serial");
        assert!(!attention.vouches(&7, &'m', at), "another client");
    }

    /// An agent's click, sent with a serial of its own.
    #[test]
    fn a_press_not_the_persons_latest_reaches_nothing() {
        let mut attention = attention();
        let at = Instant::now();
        attention.pressed(7, at);
        attention.reached(&9, 'n');
        assert!(!attention.vouches(&9, &'n', at));
        assert!(!attention.vouches(&7, &'n', at));
    }

    #[test]
    fn a_turn_after_the_press_forgets_it() {
        let at = Instant::now();
        let clicked = || {
            let mut attention = attention();
            attention.keys_to(Some('t'), false);
            attention.pressed(7, at);
            attention.reached(&7, 'n');
            attention
        };
        let mut pressed_again = clicked();
        pressed_again.pressed(8, at);
        assert!(!pressed_again.vouches(&7, &'n', at), "pressed again");
        let mut moved = clicked();
        moved.keys_to(Some('u'), false);
        assert!(!moved.vouches(&7, &'n', at), "the keys moved on");
        let mut kept = clicked();
        kept.keys_to(Some('t'), false);
        assert!(kept.vouches(&7, &'n', at), "the keys stayed put");
    }

    #[test]
    fn a_press_vouches_for_no_longer_than_the_activation_window() {
        let mut attention = attention();
        let at = Instant::now();
        attention.pressed(7, at);
        attention.reached(&7, 'n');
        let just = ACTIVATION_WINDOW - Duration::from_millis(1);
        assert!(attention.vouches(&7, &'n', at + just));
        assert!(!attention.vouches(&7, &'n', at + ACTIVATION_WINDOW));
    }
}
