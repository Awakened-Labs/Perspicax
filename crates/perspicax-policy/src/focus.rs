//! Which window has the keyboard, and which is on top.
//!
//! Focus and stacking are separate decisions. Under click-to-focus they
//! always happen together, which makes them easy to confuse, but sloppy focus
//! without autoraise is exactly the case where they come apart: the window
//! under the pointer takes the keyboard and stays where it is in the stack.
//! So every decision here names both parts separately.

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
/// bottom to top.
///
/// The bottom one, so pressing the chord repeatedly walks through every
/// window in turn: each press lifts the bottom window to the top, and the
/// next press finds a different one at the bottom. Swapping the top two
/// instead would never reach a third window. `None` with fewer than two
/// windows, because cycling one window is a no-op.
#[must_use]
pub fn cycle<W: Copy>(bottom_to_top: &[W]) -> Option<W> {
    match bottom_to_top {
        [first, _, ..] => Some(*first),
        _ => None,
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
        assert_eq!(cycle(&[1, 2, 3]), Some(1));
    }

    #[test]
    fn cycling_one_window_or_none_does_nothing() {
        assert_eq!(cycle(&[1]), None);
        assert_eq!(cycle::<u32>(&[]), None);
    }
}
