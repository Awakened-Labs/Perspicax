//! What the loop is asked to run once it is idle.
//!
//! calloop has idles of its own, and they are not used here. It runs them
//! once per `dispatch`, after its wait, and only those queued before it
//! began: `dispatch_idles` takes the list whole. One queued by an idle waits
//! for the next dispatch, whose poll knows nothing of it -- up to a frame on
//! a still desktop. The clients are dispatched outside calloop, to stay
//! clear of `unsafe`, so what they asked for waited the same way until a
//! second dispatch with no wait ran it (#130); what *that* asked for still
//! waited (#224).
//!
//! So the compositor keeps the list itself, and `run` in `lib.rs` runs it
//! where it chooses: after the clients, before anything is flushed, and in
//! rounds, each running what the last queued, so what an idle asks for runs
//! in the turn that asked. The rounds are bounded: a chain that never ends
//! gets [`MOST_ROUNDS`] of them and the rest wait for the next turn -- a
//! turn, as before, and not the loop.
//!
//! Kept generic over the state so the rule can be tested as data, as
//! `backend::pace` is.

use std::{cell::RefCell, rc::Rc};

/// How deep a chain of idles may go in one turn. The deepest known is three:
/// the keys put back on the lock screen (#197), which asks for a frame, whose
/// render relights what is under the pointer, which asks another monitor for
/// its frame. A round past the first empty one costs nothing, so there is
/// room above that; only a cycle reaches this.
pub(crate) const MOST_ROUNDS: usize = 8;

type Idle<S> = Box<dyn FnOnce(&mut S)>;

/// What waits for the loop's idle turn. Every clone is the same list, so the
/// compositor and the seat queue onto one.
pub(crate) struct Idles<S>(Rc<RefCell<Vec<Idle<S>>>>);

// By hand: a derive would ask `S: Clone`, and the state is never cloned.
impl<S> Clone for Idles<S> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<S> Default for Idles<S> {
    fn default() -> Self {
        Self(Rc::default())
    }
}

impl<S> Idles<S> {
    /// Run `idle` once the loop is idle, in this turn.
    pub(crate) fn queue(&self, idle: impl FnOnce(&mut S) + 'static) {
        self.0.borrow_mut().push(Box::new(idle));
    }

    /// Run what is queued, then what that queued, round after round, until a
    /// round queues nothing or `most` rounds have run. How many are left for
    /// the next turn: none, unless the chain did not end.
    #[must_use]
    pub(crate) fn run(&self, state: &mut S, most: usize) -> usize {
        for _ in 0..most {
            // Taken whole before any runs, so one may queue another.
            let batch = std::mem::take(&mut *self.0.borrow_mut());
            if batch.is_empty() {
                return 0;
            }
            for idle in batch {
                idle(state);
            }
        }
        self.0.borrow().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Log = Vec<&'static str>;

    #[test]
    fn what_is_queued_runs_once_and_in_order() {
        let idles: Idles<Log> = Idles::default();
        let mut log = Log::new();
        idles.queue(|log| log.push("a"));
        idles.queue(|log| log.push("b"));
        assert_eq!(idles.run(&mut log, MOST_ROUNDS), 0);
        assert_eq!(log, ["a", "b"]);
        assert_eq!(idles.run(&mut log, MOST_ROUNDS), 0, "nothing left");
        assert_eq!(log, ["a", "b"], "and nothing ran twice");
    }

    /// The reason for the list (#224): calloop would have left "b" for the
    /// next dispatch, and "c" for the one after.
    #[test]
    fn what_an_idle_queues_runs_in_the_same_turn() {
        let idles: Idles<Log> = Idles::default();
        let mut log = Log::new();
        let again = idles.clone();
        idles.queue(move |log| {
            log.push("a");
            let once_more = again.clone();
            again.queue(move |log| {
                log.push("b");
                once_more.queue(|log| log.push("c"));
            });
        });
        assert_eq!(idles.run(&mut log, MOST_ROUNDS), 0);
        assert_eq!(log, ["a", "b", "c"]);
    }

    /// An idle that queues itself `more` times over.
    fn chain(idles: &Idles<Log>, more: usize) {
        let again = idles.clone();
        idles.queue(move |log| {
            log.push("link");
            if more > 0 {
                chain(&again, more - 1);
            }
        });
    }

    #[test]
    fn a_chain_runs_as_deep_as_the_bound_allows() {
        let idles: Idles<Log> = Idles::default();
        let mut log = Log::new();
        chain(&idles, 3);
        assert_eq!(idles.run(&mut log, 4), 0, "four links in four rounds");
        assert_eq!(log.len(), 4);

        let mut log = Log::new();
        chain(&idles, 4);
        assert_eq!(idles.run(&mut log, 4), 1, "the fifth waits");
        assert_eq!(log.len(), 4);
    }

    #[test]
    fn a_chain_that_does_not_end_is_left_for_the_next_turn() {
        fn forever(idles: &Idles<Log>) {
            let again = idles.clone();
            idles.queue(move |log| {
                log.push("link");
                forever(&again);
            });
        }
        let idles: Idles<Log> = Idles::default();
        let mut log = Log::new();
        forever(&idles);
        assert_eq!(idles.run(&mut log, 3), 1, "one left over");
        assert_eq!(log.len(), 3, "three rounds and no more");
        assert_eq!(idles.run(&mut log, 3), 1, "the next turn runs on");
        assert_eq!(log.len(), 6);
    }

    /// What keeps the compositor's list and the seat's one list.
    #[test]
    fn a_clone_queues_onto_the_same_list() {
        let idles: Idles<Log> = Idles::default();
        let mut log = Log::new();
        idles.clone().queue(|log| log.push("through a clone"));
        assert_eq!(idles.run(&mut log, MOST_ROUNDS), 0);
        assert_eq!(log, ["through a clone"]);
    }
}
