//! One conversation with the desktop at a time.
//!
//! [`Desktop::deltas`] drains, and there is one queue behind it: two
//! conversations open at once would each be told part of what changed, and
//! neither could tell. So a desktop holds at most one, and a [`Slot`] is where
//! that is decided -- at `initialize`, when a conversation actually begins, and
//! not when a connection is made. A connection that never opens one holds
//! nothing, however long it stays.
//!
//! The slot is also where the queue is let go of. A conversation's deltas
//! begin with it, so whatever was pending when it opens is dropped, and while
//! nobody holds the slot nobody is listening, so nothing is kept for them.
//! Both happen under the slot's lock, so a conversation can never open between
//! the check and the drain and lose its first change. The lock order is the
//! slot's, then whatever [`Desktop::deltas`] takes, and nothing takes them the
//! other way round.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::Desktop;

/// The desktop's one conversation, shared by every connection that might hold
/// it.
#[derive(Clone)]
pub(crate) struct Slot {
    held: Arc<Mutex<Option<Holder>>>,
    desktop: Arc<dyn Desktop>,
}

/// Who holds the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Holder {
    /// The process on the other end, when the transport can say.
    pid: Option<u32>,
}

impl Slot {
    /// A free slot in front of `desktop`.
    pub(crate) fn new(desktop: Arc<dyn Desktop>) -> Self {
        Self {
            held: Arc::default(),
            desktop,
        }
    }

    /// Open a conversation for `pid`, or say who already has one.
    ///
    /// What was pending goes, because nothing pending was said to this
    /// conversation: its first `deltas` answers what changed since it opened.
    pub(crate) fn claim(&self, pid: Option<u32>) -> Result<Claim, Busy> {
        let mut held = self.lock();
        if let Some(holder) = *held {
            return Err(Busy { pid: holder.pid });
        }
        *held = Some(Holder { pid });
        drop(self.desktop.deltas());
        Ok(Claim {
            held: Arc::clone(&self.held),
        })
    }

    /// Let go of what has changed, if nobody is listening for it.
    ///
    /// A desktop runs on with nobody connected, and its queue has no bound of
    /// its own: kept for nobody, it would grow for as long as the session ran.
    pub(crate) fn let_go_if_idle(&self) {
        let held = self.lock();
        if held.is_none() {
            drop(self.desktop.deltas());
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Holder>> {
        lock(&self.held)
    }
}

/// The slot, held. Dropping it ends the conversation as far as the desktop is
/// concerned.
pub(crate) struct Claim {
    held: Arc<Mutex<Option<Holder>>>,
}

impl Drop for Claim {
    fn drop(&mut self) {
        *lock(&self.held) = None;
    }
}

/// Why a conversation could not open: another one has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "another agent's conversation is open on this session{}: perspicax serves one at a time, \
     so try again once it ends",
    pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default()
)]
pub(crate) struct Busy {
    /// The process holding it, when its transport could say.
    pub(crate) pid: Option<u32>,
}

/// The slot's lock, through a poisoning.
///
/// Nothing under it can be left half-done -- it is one word, written whole --
/// so a panic elsewhere while it was held leaves nothing to distrust, and a
/// `Claim` dropped during that unwinding must still free the slot.
fn lock(held: &Mutex<Option<Holder>>) -> MutexGuard<'_, Option<Holder>> {
    held.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use perspicax_index::Refusal;

    use super::*;
    use crate::Denied;
    use crate::fixture::Fake;

    /// A desktop with something pending.
    fn pending() -> Arc<Fake> {
        let desktop = Arc::new(Fake::answering(Err(Denied::Refused(Refusal::NotFound))));
        desktop.change();
        desktop
    }

    #[test]
    fn a_conversation_begins_with_nothing_pending() {
        let desktop = pending();
        let slot = Slot::new(desktop.clone());

        let _claim = slot.claim(Some(7)).expect("a free slot");
        assert_eq!(desktop.deltas(), []);
    }

    #[test]
    fn a_second_conversation_is_told_who_holds_the_first() {
        let slot = Slot::new(pending());
        let _first = slot.claim(Some(7)).expect("a free slot");

        let busy = slot.claim(Some(8)).err().expect("the slot is held");
        assert_eq!(busy, Busy { pid: Some(7) });
        assert!(busy.to_string().contains("(pid 7)"), "{busy}");
        assert!(
            !Busy { pid: None }.to_string().contains("pid"),
            "a holder whose pid is unknown is not given one"
        );
    }

    #[test]
    fn the_slot_is_free_once_its_conversation_ends() {
        let slot = Slot::new(pending());
        drop(slot.claim(Some(7)).expect("a free slot"));
        assert!(slot.claim(Some(8)).is_ok());
    }

    #[test]
    fn what_changes_with_nobody_listening_is_let_go_of_and_kept_while_somebody_is() {
        let desktop = pending();
        let slot = Slot::new(desktop.clone());

        let claim = slot.claim(None).expect("a free slot");
        desktop.change();
        slot.let_go_if_idle();
        assert_eq!(
            desktop.deltas().len(),
            1,
            "a conversation's changes are its own to drain"
        );

        desktop.change();
        drop(claim);
        slot.let_go_if_idle();
        assert_eq!(desktop.deltas(), []);
    }
}
