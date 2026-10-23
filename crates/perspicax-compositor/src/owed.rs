//! What a client that has stopped reading is owed of the pointer, instead
//! of every motion of it (#169).
//!
//! A client that reads nothing is held up to `MOST_UNREAD` and cut off past
//! it (see [`crate::stall`]), and a pointer moving over it is what got it
//! there: a frozen application cut off for being moved over, half a minute
//! of a fast mouse, a quarter of one with the mouse's own motion bound.
//!
//! Only where the pointer is now matters to it, so while it reads nothing
//! it is owed that: the latest motion, and the mouse's own motion summed,
//! since a game reads that as how far it went. A frame closing only what is
//! owed is not sent either. What is owed is paid before anything that lands
//! where the pointer is -- a press, a scroll, a gesture -- so that it lands
//! there, and as soon as the client reads again. An enter or a leave goes as
//! it is, so the client always knows which surface the pointer is on, and a
//! leave forgets what was owed: the client has no more use for it.
//!
//! What is owed is for one surface, the one it was held for. Every event
//! for another one forgets it, as a leave would have.

use std::sync::{Mutex, PoisonError};

use smithay::input::pointer::{MotionEvent, RelativeMotionEvent};

/// One client's debt of pointer motion.
pub(crate) struct Owed<T>(Mutex<Debt<T>>);

impl<T> Default for Owed<T> {
    fn default() -> Self {
        Self(Mutex::new(Debt {
            target: None,
            motion: None,
            relative: None,
            unframed: false,
        }))
    }
}

struct Debt<T> {
    /// What the motion was held for.
    target: Option<T>,
    /// The latest motion, in the target's own coordinates.
    motion: Option<MotionEvent>,
    /// The mouse's own motion, summed.
    relative: Option<RelativeMotionEvent>,
    /// Whether something a frame closes has gone out since the last one.
    unframed: bool,
}

/// A pointer event, as far as what is owed goes.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Event<'a> {
    /// The pointer left the target. smithay closes it with a frame itself.
    Leave,
    Motion(&'a MotionEvent),
    Relative(&'a RelativeMotionEvent),
    Frame,
    /// Anything else a frame closes: an enter, a press, a scroll.
    Other,
    /// A gesture, which no `wl_pointer.frame` closes: it goes on an object
    /// of its own.
    Gesture,
}

/// What to send for an event.
#[derive(Debug)]
pub(crate) struct Plan {
    /// What is owed, to send first.
    pub(crate) pay: Option<Paid>,
    /// Whether to send the event itself.
    pub(crate) send: bool,
}

impl Plan {
    /// Send the event as it is, and nothing before it.
    pub(crate) fn as_is() -> Self {
        Self {
            pay: None,
            send: true,
        }
    }

    /// Send nothing: the event is owed.
    fn held() -> Self {
        Self {
            pay: None,
            send: false,
        }
    }
}

/// What was owed, to send now.
#[derive(Debug)]
pub(crate) struct Paid {
    pub(crate) motion: Option<MotionEvent>,
    pub(crate) relative: Option<RelativeMotionEvent>,
    /// Whether a frame of its own closes it. Not when the event it is paid
    /// before is a frame, which closes it instead.
    pub(crate) framed: bool,
}

impl<T: PartialEq + Clone> Owed<T> {
    /// What to send for `event` to `target`, whose client has `stalled`.
    pub(crate) fn plan(&self, target: &T, stalled: bool, event: Event<'_>) -> Plan {
        let mut debt = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if debt.target.as_ref().is_some_and(|held| held != target) {
            debt.forget();
        }
        match event {
            Event::Leave => {
                debt.forget();
                debt.unframed = false;
                Plan::as_is()
            }
            Event::Motion(motion) if stalled => {
                debt.target = Some(target.clone());
                debt.motion = Some(motion.clone());
                Plan::held()
            }
            Event::Relative(relative) if stalled => {
                debt.target = Some(target.clone());
                debt.relative = Some(match debt.relative.take() {
                    Some(held) => sum(&held, relative),
                    None => relative.clone(),
                });
                Plan::held()
            }
            Event::Frame if stalled && !debt.unframed => Plan::held(),
            Event::Frame => {
                let pay = debt.take(false);
                debt.unframed = false;
                Plan { pay, send: true }
            }
            Event::Motion(_) | Event::Relative(_) | Event::Other => {
                let pay = debt.take(true);
                debt.unframed = true;
                Plan { pay, send: true }
            }
            Event::Gesture => Plan {
                pay: debt.take(true),
                send: true,
            },
        }
    }

    /// What is owed, to be paid now that the client reads again, and what
    /// it is owed for. Paid with a frame of its own.
    pub(crate) fn take(&self) -> Option<(T, Paid)> {
        let mut debt = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let target = debt.target.clone()?;
        Some((target, debt.take(true)?))
    }
}

impl<T> Debt<T> {
    fn forget(&mut self) {
        self.target = None;
        self.motion = None;
        self.relative = None;
    }

    /// Whatever is owed, to be sent now; `framed` if with a frame of its own.
    fn take(&mut self, framed: bool) -> Option<Paid> {
        self.target.take()?;
        let paid = Paid {
            motion: self.motion.take(),
            relative: self.relative.take(),
            framed,
        };
        if framed {
            self.unframed = false;
        }
        Some(paid)
    }
}

/// Two of the mouse's own motions as one: how far both went, at the time of
/// the later.
fn sum(earlier: &RelativeMotionEvent, later: &RelativeMotionEvent) -> RelativeMotionEvent {
    RelativeMotionEvent {
        delta: earlier.delta + later.delta,
        delta_unaccel: earlier.delta_unaccel + later.delta_unaccel,
        utime: later.utime,
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Point, SERIAL_COUNTER};

    use super::*;

    const WINDOW: u32 = 1;
    const MENU: u32 = 2;

    fn at(x: f64, y: f64) -> MotionEvent {
        MotionEvent {
            location: Point::from((x, y)),
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        }
    }

    fn by(x: f64, y: f64, utime: u64) -> RelativeMotionEvent {
        RelativeMotionEvent {
            delta: Point::from((x, y)),
            delta_unaccel: Point::from((x * 2.0, y * 2.0)),
            utime,
        }
    }

    /// A frame of motion and the mouse's own, as `travel` sends it, to a
    /// client that has `stalled`: what is sent of each event.
    fn moved(owed: &Owed<u32>, stalled: bool, to: (f64, f64), relative: (f64, f64)) -> [bool; 3] {
        [
            owed.plan(&WINDOW, stalled, Event::Motion(&at(to.0, to.1)))
                .send,
            owed.plan(
                &WINDOW,
                stalled,
                Event::Relative(&by(relative.0, relative.1, 0)),
            )
            .send,
            owed.plan(&WINDOW, stalled, Event::Frame).send,
        ]
    }

    fn location(paid: &Paid) -> Option<(f64, f64)> {
        paid.motion
            .as_ref()
            .map(|motion| (motion.location.x, motion.location.y))
    }

    fn delta(paid: &Paid) -> Option<(f64, f64)> {
        paid.relative
            .as_ref()
            .map(|relative| (relative.delta.x, relative.delta.y))
    }

    #[test]
    fn a_client_reading_is_sent_everything_and_owed_nothing() {
        let owed = Owed::default();
        assert_eq!(moved(&owed, false, (1.0, 1.0), (1.0, 0.0)), [true; 3]);
        assert!(owed.take().is_none());
    }

    #[test]
    fn a_stalled_client_is_owed_only_the_latest_motion_and_sent_no_frame_for_it() {
        let owed = Owed::default();
        assert_eq!(moved(&owed, true, (1.0, 1.0), (1.0, 0.0)), [false; 3]);
        assert_eq!(moved(&owed, true, (5.0, 7.0), (2.0, 3.0)), [false; 3]);
        let (target, paid) = owed.take().expect("owed");
        assert_eq!(target, WINDOW);
        assert_eq!(location(&paid), Some((5.0, 7.0)));
        assert!(paid.framed);
        assert!(owed.take().is_none(), "paid once");
    }

    #[test]
    fn the_mouses_own_motion_is_summed_at_the_time_of_the_latest() {
        let owed = Owed::default();
        owed.plan(&WINDOW, true, Event::Relative(&by(30.0, 0.0, 10)));
        owed.plan(&WINDOW, true, Event::Relative(&by(0.0, 20.0, 20)));
        owed.plan(&WINDOW, true, Event::Relative(&by(-1.0, 1.0, 30)));
        let (_, paid) = owed.take().expect("owed");
        let relative = paid.relative.expect("summed");
        assert_eq!((relative.delta.x, relative.delta.y), (29.0, 21.0));
        assert_eq!(
            (relative.delta_unaccel.x, relative.delta_unaccel.y),
            (58.0, 42.0)
        );
        assert_eq!(relative.utime, 30);
        assert!(paid.motion.is_none(), "no motion was held");
    }

    #[test]
    fn a_press_is_preceded_by_what_is_owed_and_a_frame() {
        let owed = Owed::default();
        moved(&owed, true, (4.0, 2.0), (1.0, 1.0));
        let press = owed.plan(&WINDOW, true, Event::Other);
        assert!(press.send);
        let paid = press.pay.expect("paid before the press");
        assert_eq!(location(&paid), Some((4.0, 2.0)));
        assert_eq!(delta(&paid), Some((1.0, 1.0)));
        assert!(paid.framed);
        assert!(
            owed.plan(&WINDOW, true, Event::Frame).send,
            "the press's own frame goes"
        );
        assert!(owed.take().is_none());
    }

    #[test]
    fn a_frame_closing_something_sent_pays_what_is_owed_before_it_and_closes_it() {
        let owed = Owed::default();
        // An enter, then the mouse's own motion, in one frame: a focus
        // change in `travel`.
        assert!(owed.plan(&WINDOW, true, Event::Other).send);
        assert!(
            !owed
                .plan(&WINDOW, true, Event::Relative(&by(3.0, 0.0, 0)))
                .send
        );
        let frame = owed.plan(&WINDOW, true, Event::Frame);
        assert!(frame.send, "it closes the enter");
        let paid = frame.pay.expect("paid inside the frame");
        assert_eq!(delta(&paid), Some((3.0, 0.0)));
        assert!(!paid.framed, "the frame it was paid before closes it");
        assert!(
            !owed.plan(&WINDOW, true, Event::Frame).send,
            "nothing left to close"
        );
    }

    #[test]
    fn a_leave_forgets_what_was_owed_and_is_framed_already() {
        let owed = Owed::default();
        moved(&owed, true, (1.0, 1.0), (1.0, 1.0));
        let leave = owed.plan(&WINDOW, true, Event::Leave);
        assert!(leave.send && leave.pay.is_none());
        assert!(owed.take().is_none());

        // An enter, and the leave that ends it before any frame does.
        assert!(owed.plan(&MENU, true, Event::Other).send);
        assert!(owed.plan(&MENU, true, Event::Leave).send);
        assert!(
            !owed.plan(&WINDOW, true, Event::Frame).send,
            "smithay closed the leave"
        );
    }

    #[test]
    fn a_gesture_pays_what_is_owed_but_needs_no_frame_of_its_own() {
        let owed = Owed::default();
        moved(&owed, true, (1.0, 1.0), (0.0, 0.0));
        let gesture = owed.plan(&WINDOW, true, Event::Gesture);
        assert!(gesture.send);
        assert!(gesture.pay.expect("paid").framed);
        assert!(
            !owed.plan(&WINDOW, true, Event::Frame).send,
            "no wl_pointer event since the paid frame"
        );
        assert!(owed.plan(&WINDOW, true, Event::Gesture).pay.is_none());
    }

    #[test]
    fn what_is_owed_to_one_surface_is_forgotten_at_an_event_for_another() {
        let owed = Owed::default();
        moved(&owed, true, (1.0, 1.0), (1.0, 1.0));
        let enter = owed.plan(&MENU, true, Event::Other);
        assert!(enter.send && enter.pay.is_none());
        assert!(owed.take().is_none());
    }

    #[test]
    fn a_client_reading_again_is_paid_what_it_is_owed_before_anything_else() {
        let owed = Owed::default();
        moved(&owed, true, (2.0, 2.0), (1.0, 0.0));
        let motion = owed.plan(&WINDOW, false, Event::Motion(&at(3.0, 3.0)));
        assert!(motion.send);
        let paid = motion.pay.expect("paid first");
        assert_eq!(location(&paid), Some((2.0, 2.0)));
        assert!(paid.framed);
        assert!(owed.plan(&WINDOW, false, Event::Frame).send);
    }
}
