//! What a panel's tooltip does as the pointer moves over the bar: when one
//! shows, for what, and when it goes.
//!
//! A reducer, as [`crate::pie`] is for a pie: an [`Event`] and the [`State`]
//! in, the state changed and the [`Effect`]s to carry out back, tested as
//! values. It knows which part of which bar a tip is for, never its words or
//! where it goes: those are read from the bar as it is drawn, so a window
//! retitled while its tip shows is shown by its new title.
//!
//! A tip shows once the pointer has rested [`DELAY`] on a part that has one:
//! every move starts the wait afresh. Once one has shown, the bar is warm:
//! moving onto another part with a tip shows its tip at once, and onto one
//! without hides it until the pointer is on one again. The bar stays warm
//! until the pointer leaves it, a button goes down on it, or a menu, the
//! mixer or a pie opens, and nothing shows while one is open. A press also
//! keeps the part it was on from showing its tip again until the pointer has
//! been off it, so a task just clicked is not covered by its title a moment
//! later.

use std::time::Duration;

use crate::layout::panel::Part;

/// How long the pointer rests on a part before its tip shows: Plasma's.
pub(crate) const DELAY: Duration = Duration::from_millis(700);

/// Something that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// The pointer came onto the bar of the monitor named `bar`, or moved on
    /// it, and is over `part`, a part with a tip, or over none.
    Motion { bar: String, part: Option<Part> },
    /// It left the bar.
    Leave,
    /// A button went down on the bar.
    Press,
    /// The wait for the pointer to rest is over.
    Due,
    /// Whether a menu, the mixer or a pie is open.
    Covered(bool),
    /// The bar the pointer was on went, or tooltips were turned off.
    Off,
}

/// What to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Wait [`DELAY`] for the pointer to rest, afresh, and then say
    /// [`Event::Due`].
    Wait,
    /// Wait no longer.
    Stop,
    /// Show the tip of `part` of the bar of the monitor named `bar`, in place
    /// of any shown.
    Show { bar: String, part: Part },
    /// Take the tip away.
    Hide,
}

/// Where the pointer is, as far as tips go.
#[derive(Debug, Default)]
pub(crate) struct State {
    /// The bar it is on, if it is on one.
    on: Option<On>,
    /// A menu, the mixer or a pie is open.
    covered: bool,
}

/// The pointer on a bar.
#[derive(Debug)]
struct On {
    /// The monitor's connector name.
    bar: String,
    /// The part with a tip it is over, if any.
    under: Option<Part>,
    /// Waiting for it to rest.
    waiting: bool,
    /// The part whose tip shows.
    shown: Option<Part>,
    /// A tip has shown since it came onto the bar, and nothing has been
    /// pressed or opened since: another shows at once.
    warm: bool,
    /// The part a button went down on, until the pointer has been off it.
    pressed: Option<Part>,
}

impl On {
    fn new(bar: String) -> Self {
        Self {
            bar,
            under: None,
            waiting: false,
            shown: None,
            warm: false,
            pressed: None,
        }
    }

    /// Wait no longer and show nothing, and say what that takes.
    fn quiet(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        if std::mem::take(&mut self.waiting) {
            effects.push(Effect::Stop);
        }
        if self.shown.take().is_some() {
            effects.push(Effect::Hide);
        }
        self.warm = false;
        effects
    }

    /// The tip `under` has, unless the part was pressed and the pointer has
    /// not been off it since.
    fn tip(&self) -> Option<Part> {
        self.under.filter(|&part| self.pressed != Some(part))
    }
}

impl State {
    /// Handle `event`.
    pub(crate) fn update(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Motion { bar, part } => self.moved(bar, part),
            Event::Leave | Event::Off => self.on.take().map_or_else(Vec::new, |mut on| on.quiet()),
            Event::Press => self.on.as_mut().map_or_else(Vec::new, |on| {
                on.pressed = on.under;
                on.quiet()
            }),
            Event::Due => self.due(),
            Event::Covered(covered) => {
                self.covered = covered;
                match &mut self.on {
                    Some(on) if covered => on.quiet(),
                    _ => Vec::new(),
                }
            }
        }
    }

    /// The pointer moved on the bar of the monitor named `bar`, over `part`.
    fn moved(&mut self, bar: String, part: Option<Part>) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.on.as_ref().is_some_and(|on| on.bar != bar) {
            effects = self.update(Event::Leave);
        }
        let on = self.on.get_or_insert_with(|| On::new(bar));
        on.under = part;
        if on.pressed != part {
            on.pressed = None;
        }
        if self.covered {
            return effects;
        }
        let tip = on.tip();
        if on.warm {
            if on.shown != tip {
                on.shown = tip;
                effects.push(tip.map_or(Effect::Hide, |part| Effect::Show {
                    bar: on.bar.clone(),
                    part,
                }));
            }
        } else if tip.is_some() {
            on.waiting = true;
            effects.push(Effect::Wait);
        } else if std::mem::take(&mut on.waiting) {
            effects.push(Effect::Stop);
        }
        effects
    }

    /// The wait is over: show the tip of what the pointer is over, if it
    /// still has one.
    fn due(&mut self) -> Vec<Effect> {
        let Some(on) = self.on.as_mut().filter(|on| on.waiting) else {
            return Vec::new();
        };
        on.waiting = false;
        let Some(part) = on.tip().filter(|_| !self.covered) else {
            return Vec::new();
        };
        on.shown = Some(part);
        on.warm = true;
        vec![Effect::Show {
            bar: on.bar.clone(),
            part,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: Part = Part::Task(1);
    const SECOND: Part = Part::Task(2);

    fn over(state: &mut State, part: Option<Part>) -> Vec<Effect> {
        over_on(state, "HDMI-A-1", part)
    }

    fn over_on(state: &mut State, bar: &str, part: Option<Part>) -> Vec<Effect> {
        state.update(Event::Motion {
            bar: bar.to_owned(),
            part,
        })
    }

    fn show(part: Part) -> Effect {
        Effect::Show {
            bar: "HDMI-A-1".to_owned(),
            part,
        }
    }

    /// A state whose bar is warm, the tip of `part` showing.
    fn showing(part: Part) -> State {
        let mut state = State::default();
        assert_eq!(over(&mut state, Some(part)), [Effect::Wait]);
        assert_eq!(state.update(Event::Due), [show(part)]);
        state
    }

    #[test]
    fn a_task_rested_on_shows_its_tip_once_the_wait_is_over() {
        let mut state = State::default();
        assert_eq!(over(&mut state, Some(FIRST)), [Effect::Wait]);
        assert_eq!(state.update(Event::Due), [show(FIRST)]);
        assert_eq!(
            over(&mut state, Some(FIRST)),
            [],
            "moving on it keeps it showing"
        );
        assert_eq!(state.update(Event::Due), [], "and asks no more");
    }

    #[test]
    fn each_move_starts_the_wait_afresh_and_a_part_without_a_tip_stops_it() {
        let mut state = State::default();
        assert_eq!(over(&mut state, Some(FIRST)), [Effect::Wait]);
        assert_eq!(over(&mut state, Some(FIRST)), [Effect::Wait], "not rested");
        assert_eq!(over(&mut state, Some(SECOND)), [Effect::Wait]);
        assert_eq!(over(&mut state, None), [Effect::Stop]);
        assert_eq!(over(&mut state, None), [], "nothing more to stop");
        assert_eq!(state.update(Event::Due), [], "a wait stopped shows nothing");
    }

    #[test]
    fn once_one_has_shown_another_shows_at_once_and_a_gap_hides_it_but_keeps_the_bar_warm() {
        let mut state = showing(FIRST);
        assert_eq!(over(&mut state, Some(SECOND)), [show(SECOND)]);
        assert_eq!(over(&mut state, None), [Effect::Hide]);
        assert_eq!(over(&mut state, None), []);
        assert_eq!(
            over(&mut state, Some(FIRST)),
            [show(FIRST)],
            "still warm past the gap"
        );
    }

    #[test]
    fn leaving_the_bar_hides_it_and_the_next_waits_again() {
        let mut state = showing(FIRST);
        assert_eq!(state.update(Event::Leave), [Effect::Hide]);
        assert_eq!(over(&mut state, Some(SECOND)), [Effect::Wait], "cold");
        assert_eq!(state.update(Event::Leave), [Effect::Stop]);
        assert_eq!(state.update(Event::Leave), [], "off it already");
    }

    #[test]
    fn a_press_hides_it_and_the_part_pressed_shows_none_until_the_pointer_has_been_off_it() {
        let mut state = showing(FIRST);
        assert_eq!(state.update(Event::Press), [Effect::Hide]);
        assert_eq!(over(&mut state, Some(FIRST)), [], "pressed");
        assert_eq!(state.update(Event::Due), []);
        assert_eq!(
            over(&mut state, Some(SECOND)),
            [Effect::Wait],
            "another, cold again"
        );
        assert_eq!(over(&mut state, Some(FIRST)), [Effect::Wait], "and back");

        let mut waiting = State::default();
        over(&mut waiting, Some(FIRST));
        assert_eq!(waiting.update(Event::Press), [Effect::Stop]);
        assert_eq!(over(&mut waiting, Some(FIRST)), []);
        assert_eq!(over(&mut waiting, None), []);
        assert_eq!(
            over(&mut waiting, Some(FIRST)),
            [Effect::Wait],
            "off it and back"
        );
    }

    #[test]
    fn a_menu_opening_hides_it_and_none_shows_while_one_is_open() {
        let mut state = showing(FIRST);
        assert_eq!(state.update(Event::Covered(true)), [Effect::Hide]);
        assert_eq!(over(&mut state, Some(SECOND)), [], "no wait, no tip");
        assert_eq!(state.update(Event::Due), []);
        assert_eq!(state.update(Event::Covered(true)), [], "said again");
        assert_eq!(state.update(Event::Covered(false)), [], "nothing at once");
        assert_eq!(
            over(&mut state, Some(SECOND)),
            [Effect::Wait],
            "cold once it closed"
        );
        assert_eq!(state.update(Event::Covered(true)), [Effect::Stop]);
        assert_eq!(
            state.update(Event::Due),
            [],
            "a wait that ends after it opened"
        );
    }

    #[cfg(feature = "tray")]
    #[test]
    fn a_status_icon_is_browsed_as_a_task_is() {
        let mut state = showing(FIRST);
        assert_eq!(over(&mut state, None), [Effect::Hide], "the gap between");
        assert_eq!(
            over(&mut state, Some(Part::Tray(7))),
            [show(Part::Tray(7))],
            "at once, the bar warm"
        );
        assert_eq!(state.update(Event::Press), [Effect::Hide]);
        assert_eq!(over(&mut state, Some(Part::Tray(7))), [], "pressed");
    }

    #[test]
    fn another_bar_is_a_fresh_start() {
        let mut state = showing(FIRST);
        assert_eq!(
            over_on(&mut state, "DP-1", Some(SECOND)),
            [Effect::Hide, Effect::Wait],
            "cold on the other"
        );
        assert_eq!(
            state.update(Event::Due),
            [Effect::Show {
                bar: "DP-1".to_owned(),
                part: SECOND,
            }]
        );
    }

    #[test]
    fn a_wait_that_ends_after_the_pointer_has_gone_shows_nothing() {
        for gone in [Event::Leave, Event::Off] {
            let mut state = State::default();
            over(&mut state, Some(FIRST));
            assert_eq!(state.update(gone.clone()), [Effect::Stop], "{gone:?}");
            assert_eq!(state.update(Event::Due), [], "{gone:?}");
        }
        let mut state = showing(FIRST);
        assert_eq!(state.update(Event::Off), [Effect::Hide]);
        assert_eq!(over(&mut state, Some(FIRST)), [Effect::Wait], "cold again");
    }

    #[test]
    fn a_part_without_a_tip_never_waits() {
        let mut state = State::default();
        assert_eq!(over(&mut state, None), []);
        assert_eq!(state.update(Event::Due), []);
        assert_eq!(state.update(Event::Press), []);
        assert_eq!(state.update(Event::Covered(true)), []);
        assert_eq!(state.update(Event::Covered(false)), []);
    }
}
