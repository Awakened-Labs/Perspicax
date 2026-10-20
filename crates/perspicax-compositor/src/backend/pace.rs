//! When a monitor draws next.
//!
//! A monitor draws when something has changed, and then not again until what
//! it drew has been shown: until the vblank of the frame it queued, or, when
//! it queued none -- nothing new to show, or a frame the CRTC would not take
//! -- until a refresh has passed, which a timer measures in the vblank's
//! place. Without that refresh, a client whose commits change nothing on
//! screen, covered or asking only for its frame callback, would be told at
//! once that it may draw again, and would draw as fast as it could (#130).
//!
//! A change on a still monitor waits only for the loop's idle turn, so
//! however many commits one dispatch brings, the monitor draws once.
//!
//! Kept free of DRM and the loop so the rule can be tested as data, as
//! [`super::connectors`] is.

/// What a monitor waits for before it draws again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cue {
    /// The loop's idle turn, asked for by a change.
    Turn,
    /// The vblank of the frame it queued.
    Vblank,
    /// A refresh, timed, after a render that queued no frame.
    Refresh,
}

/// What a render did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Drew {
    /// Nothing: the session is away, or the monitor has no place on the
    /// desk. What changed is drawn with the next change: coming back, or
    /// being placed, is one.
    Nothing,
    /// A frame, queued on the CRTC.
    Queued,
    /// No frame queued: nothing new to show, or one the CRTC would not take.
    Unqueued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Waiting {
    #[default]
    Nothing,
    For(Cue),
    /// Drawing now: a change meanwhile is taken in by this render.
    Drawing,
}

/// When one monitor draws next.
#[derive(Debug, Default)]
pub(crate) struct Pace {
    /// Something changed that it has not drawn.
    owed: bool,
    waiting: Waiting,
}

impl Pace {
    /// Something changed that this monitor may show. Whether to ask the loop
    /// for a turn to draw it in: only when nothing else is awaited, since
    /// whatever is will draw it.
    pub(crate) fn changed(&mut self) -> bool {
        self.owed = true;
        if self.waiting != Waiting::Nothing {
            return false;
        }
        self.waiting = Waiting::For(Cue::Turn);
        true
    }

    /// `cue` came. Whether to draw now: only if it was what this monitor was
    /// waiting for, and something is owed.
    pub(crate) fn came(&mut self, cue: Cue) -> bool {
        if self.waiting != Waiting::For(cue) {
            return false;
        }
        self.waiting = if self.owed {
            Waiting::Drawing
        } else {
            Waiting::Nothing
        };
        self.owed
    }

    /// The render [`Self::came`] allowed is over: it `drew`, and asks to draw
    /// `again` at the next chance whether or not anything changes, as a
    /// workspace switch or a light coming up does. Whether to time a refresh,
    /// standing in for a vblank that will not come.
    pub(crate) fn drew(&mut self, drew: Drew, again: bool) -> bool {
        debug_assert_eq!(self.waiting, Waiting::Drawing, "a render it allowed");
        self.owed = again;
        self.waiting = match drew {
            Drew::Nothing => Waiting::Nothing,
            Drew::Queued => Waiting::For(Cue::Vblank),
            Drew::Unqueued => Waiting::For(Cue::Refresh),
        };
        self.waiting == Waiting::For(Cue::Refresh)
    }

    /// The seat is back from another VT: a frame queued before it went will
    /// never see its vblank.
    pub(crate) fn resumed(&mut self) {
        if self.waiting == Waiting::For(Cue::Vblank) {
            self.waiting = Waiting::Nothing;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monitor that has drawn and been shown, with nothing changed since.
    fn still() -> Pace {
        Pace::default()
    }

    #[test]
    fn a_change_on_a_still_monitor_asks_for_a_turn_and_is_drawn_in_it() {
        let mut pace = still();
        assert!(pace.changed(), "a turn asked for");
        assert!(pace.came(Cue::Turn), "drawn in it");
    }

    #[test]
    fn however_many_changes_come_before_the_turn_it_draws_once() {
        let mut pace = still();
        assert!(pace.changed());
        assert!(!pace.changed() && !pace.changed(), "one turn is enough");
        assert!(pace.came(Cue::Turn));
        assert!(!pace.drew(Drew::Queued, false));
        assert!(!pace.came(Cue::Vblank), "nothing left to draw");
    }

    #[test]
    fn a_change_while_a_frame_is_queued_waits_for_its_vblank() {
        let mut pace = still();
        pace.changed();
        pace.came(Cue::Turn);
        assert!(!pace.drew(Drew::Queued, false), "the vblank paces it");
        assert!(!pace.changed(), "no turn while the frame is on its way");
        assert!(!pace.came(Cue::Turn));
        assert!(pace.came(Cue::Vblank), "drawn at the vblank");
    }

    /// The reason for a refresh at all: no vblank follows a frame that queued
    /// nothing, and the clients drawn on it were told they may draw again.
    #[test]
    fn a_render_that_queued_nothing_waits_a_refresh_before_the_next() {
        let mut pace = still();
        pace.changed();
        pace.came(Cue::Turn);
        assert!(pace.drew(Drew::Unqueued, false), "a refresh is timed");
        assert!(!pace.changed(), "a client drawing again waits for it");
        assert!(!pace.changed());
        assert!(!pace.came(Cue::Turn) && !pace.came(Cue::Vblank));
        assert!(pace.came(Cue::Refresh), "drawn once the refresh has passed");
        assert!(pace.drew(Drew::Unqueued, false));
        assert!(!pace.came(Cue::Refresh), "and only once");
    }

    #[test]
    fn a_vblank_or_a_refresh_with_nothing_changed_draws_nothing() {
        let mut pace = still();
        pace.changed();
        pace.came(Cue::Turn);
        pace.drew(Drew::Queued, false);
        assert!(!pace.came(Cue::Vblank));
        assert!(
            pace.changed(),
            "still again: the next change asks for a turn"
        );

        pace.came(Cue::Turn);
        pace.drew(Drew::Unqueued, false);
        assert!(!pace.came(Cue::Refresh));
        assert!(pace.changed());
    }

    #[test]
    fn something_moving_draws_again_at_the_next_chance_with_nothing_changed() {
        let mut pace = still();
        pace.changed();
        pace.came(Cue::Turn);
        assert!(!pace.drew(Drew::Queued, true));
        assert!(pace.came(Cue::Vblank), "at the vblank");
        assert!(pace.drew(Drew::Unqueued, true));
        assert!(pace.came(Cue::Refresh), "or the refresh standing in for it");
        pace.drew(Drew::Queued, false);
        assert!(!pace.came(Cue::Vblank), "until it stops");
    }

    #[test]
    fn a_cue_it_was_not_waiting_for_changes_nothing() {
        let mut pace = still();
        assert!(!pace.came(Cue::Refresh), "a timer left from before");
        assert!(!pace.came(Cue::Vblank));
        assert!(!pace.came(Cue::Turn));
        pace.changed();
        pace.came(Cue::Turn);
        pace.drew(Drew::Unqueued, false);
        pace.changed();
        assert!(!pace.came(Cue::Vblank) && !pace.came(Cue::Turn));
        assert!(pace.came(Cue::Refresh), "still waiting for what it was");
    }

    /// What is under the pointer is lit again as a render begins, and that
    /// asks for a frame: it is in the frame being drawn, and needs no other.
    #[test]
    fn a_change_while_drawing_is_taken_in_by_that_render() {
        let mut pace = still();
        pace.changed();
        pace.came(Cue::Turn);
        assert!(!pace.changed(), "no turn asked for while drawing");
        pace.drew(Drew::Queued, false);
        assert!(!pace.came(Cue::Turn) && !pace.came(Cue::Vblank));
    }

    #[test]
    fn back_from_another_vt_a_frame_queued_before_is_not_waited_for() {
        let mut pace = still();
        pace.changed();
        pace.came(Cue::Turn);
        pace.drew(Drew::Queued, false);
        // Committed while away, waiting on the vblank of a frame left behind.
        pace.changed();
        pace.resumed();
        assert!(pace.changed(), "a change asks for a turn again");
        assert!(!pace.came(Cue::Vblank), "one from before draws nothing");
        assert!(pace.came(Cue::Turn));
    }

    #[test]
    fn coming_back_leaves_a_timed_refresh_to_come() {
        let mut pace = still();
        pace.changed();
        pace.came(Cue::Turn);
        pace.drew(Drew::Unqueued, false);
        pace.changed();
        pace.resumed();
        assert!(pace.came(Cue::Refresh));
    }

    #[test]
    fn after_a_render_that_drew_nothing_the_next_change_is_drawn_at_once() {
        let mut pace = still();
        pace.changed();
        pace.came(Cue::Turn);
        assert!(!pace.drew(Drew::Nothing, false), "nothing to pace");
        assert!(!pace.came(Cue::Vblank) && !pace.came(Cue::Refresh));
        assert!(pace.changed(), "a turn asked for");
        assert!(pace.came(Cue::Turn));
    }
}
