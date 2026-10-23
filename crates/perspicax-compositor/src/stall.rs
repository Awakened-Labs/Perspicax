//! Whether a client has stopped reading what this compositor sends it.
//!
//! wayland-backend says nothing while what waits for a client piles up, and
//! when it cuts the client off for it the reason it gives is
//! `ConnectionClosed`, the same as a hang-up's (#101). The one sign it does
//! give is a flush that cannot write: the kernel's buffer to the client is
//! full, so the client has read nothing for a while. What waits for it is
//! then held, up to `MOST_UNREAD`, and past that it is cut off. The pointer's
//! motion, which used to get it there, is owed to it from that first flush,
//! told or not, and paid once a flush goes through (see [`crate::owed`]).
//!
//! A client reading through a burst can fill that buffer for a moment, so a
//! stall is told only once it has lasted [`TOLD_AFTER`], and its end only if
//! it was told. Its length runs from the first flush that could not write,
//! not from the client's last read, which nothing here can see: it is how
//! long what was sent has waited.

use std::{
    io,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How long a client must have read nothing before it is said to have
/// stopped.
const TOLD_AFTER: Duration = Duration::from_secs(1);

/// One client's stall, if it is in one.
#[derive(Default)]
pub(crate) struct Stall(Mutex<State>);

#[derive(Default)]
struct State {
    /// When a flush to it first could not write, since one last could.
    since: Option<Instant>,
    /// Whether that has been told.
    told: bool,
}

/// What a flush showed of a client that is worth telling.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// It has read nothing for this long.
    Stopped(Duration),
    /// It read again, after this long.
    Reading(Duration),
}

impl Stall {
    /// A flush to the client came to `flushed` at `now`: what of it to tell.
    pub(crate) fn flushed(&self, flushed: &io::Result<()>, now: Instant) -> Option<Change> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match flushed {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let since = *state.since.get_or_insert(now);
                let waited = now.saturating_duration_since(since);
                if state.told || waited < TOLD_AFTER {
                    return None;
                }
                state.told = true;
                Some(Change::Stopped(waited))
            }
            Ok(()) => {
                let ended = std::mem::take(&mut *state);
                let since = ended.since.filter(|_| ended.told)?;
                Some(Change::Reading(now.saturating_duration_since(since)))
            }
            // A client gone, which wayland-backend says itself.
            Err(_) => None,
        }
    }

    /// Whether the client has read nothing since a flush to it first could
    /// not write: [`Self::behind`], for whoever needs no length.
    pub(crate) fn stalled(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .since
            .is_some()
    }

    /// How long the client has read nothing at `now`, if it has stopped.
    pub(crate) fn behind(&self, now: Instant) -> Option<Duration> {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .since
            .map(|since| now.saturating_duration_since(since))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> io::Result<()> {
        Err(io::ErrorKind::WouldBlock.into())
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_client_is_stalled_from_its_first_full_flush_until_one_goes_through() {
        let stall = Stall::default();
        let start = Instant::now();
        assert!(!stall.stalled());
        assert_eq!(stall.flushed(&full(), start), None);
        assert!(stall.stalled(), "from the first, told or not");
        assert_eq!(stall.flushed(&full(), start + ms(500)), None);
        assert_eq!(
            stall.flushed(&full(), start + ms(1000)),
            Some(Change::Stopped(ms(1000)))
        );
        assert_eq!(stall.flushed(&full(), start + ms(2000)), None, "told once");
        assert_eq!(
            stall.flushed(&Ok(()), start + ms(3000)),
            Some(Change::Reading(ms(3000)))
        );
        assert_eq!(stall.flushed(&Ok(()), start + ms(4000)), None, "and over");
        assert_eq!(stall.behind(start + ms(4000)), None);
        assert!(!stall.stalled());
    }

    #[test]
    fn a_short_stall_says_nothing() {
        let stall = Stall::default();
        let start = Instant::now();
        assert_eq!(stall.flushed(&full(), start), None);
        assert_eq!(stall.flushed(&Ok(()), start + ms(900)), None, "not told");
        assert_eq!(stall.flushed(&full(), start + ms(1200)), None);
        assert_eq!(
            stall.flushed(&full(), start + ms(2100)),
            None,
            "a new stall, counted from its own start"
        );
    }

    #[test]
    fn a_hang_up_is_not_a_stall() {
        let stall = Stall::default();
        let start = Instant::now();
        let gone = Err(io::ErrorKind::BrokenPipe.into());
        assert_eq!(stall.flushed(&gone, start), None);
        assert_eq!(stall.flushed(&gone, start + ms(2000)), None);
        assert_eq!(stall.behind(start + ms(2000)), None);
    }

    #[test]
    fn a_client_gone_while_stalled_is_said_to_have_stopped_reading() {
        let stall = Stall::default();
        let start = Instant::now();
        stall.flushed(&full(), start);
        assert_eq!(stall.behind(start + ms(300)), Some(ms(300)), "told or not");
        stall.flushed(&full(), start + ms(1500));
        assert_eq!(stall.behind(start + ms(2500)), Some(ms(2500)));
    }
}
