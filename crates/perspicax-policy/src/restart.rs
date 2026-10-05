//! Starting the desktop shell again when it stops.
//!
//! perspicax starts perspicax-shell when a person logs in, and is the one
//! left to notice when it stops. How it stopped decides what happens next.
//! A crash is started again after a short wait, which doubles with each
//! crash that comes soon after its start, so a shell that cannot stay up
//! does not take the machine with it; after five of those in a row it is
//! given up on, and the person keeps a desktop with no shell on it. A shell
//! that refused its config would only refuse it again, so it waits until the
//! file is read again. A shell that exited cleanly chose to, and stays
//! stopped.
//!
//! Fed times from outside, in milliseconds from any fixed origin, as
//! [`crate::EdgeDwell`] is, so it is tested without a clock or a process.

/// The wait before the first start again. Each quick crash in a row doubles
/// it.
const FIRST_WAIT_MS: u64 = 250;

/// The longest wait before starting again.
const LONGEST_WAIT_MS: u64 = 30_000;

/// A crash this soon after the start is a quick one. A shell that ran
/// longer was working, and its crash starts the count afresh.
const QUICK_MS: u64 = 10_000;

/// How many quick crashes in a row before the shell is given up on.
const QUICK_CRASHES: u32 = 5;

/// The exit status of a program that cannot use its config: `EX_CONFIG`,
/// from sysexits.h.
pub const EX_CONFIG: i32 = 78;

/// How the shell's process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It exited with status 0: it chose to stop.
    Clean,
    /// It exited with [`EX_CONFIG`]: it cannot use the config file.
    Refused,
    /// Any other status, or a signal.
    Crashed,
}

impl Ended {
    /// How a process ended, from its exit status: `None` when a signal
    /// killed it.
    #[must_use]
    pub fn from_code(code: Option<i32>) -> Self {
        match code {
            Some(0) => Self::Clean,
            Some(EX_CONFIG) => Self::Refused,
            _ => Self::Crashed,
        }
    }
}

/// What to do about a shell that stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    /// Start it again in this many milliseconds.
    After(u64),
    /// Start it again when the config file is next read.
    WhenConfigChanges,
    /// It crashed quickly too many times in a row. Start it again when the
    /// config file is next read: what made it crash may be in there.
    GiveUp,
    /// Leave it stopped.
    Stay,
}

/// One shell's starts and stops, and what each stop means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Restarts {
    /// When the shell running now was started.
    since: Option<u64>,
    /// Quick crashes in a row.
    quick: u32,
    /// Stopped, until the config file is read again.
    waiting: bool,
}

impl Restarts {
    /// The shell was started at `now`.
    pub fn started(&mut self, now: u64) {
        self.since = Some(now);
        self.waiting = false;
    }

    /// The shell stopped at `now`, as `how` says. What to do about it.
    pub fn ended(&mut self, how: Ended, now: u64) -> Restart {
        // A stop with no start on record is counted as quick: the cautious
        // reading, which ends in giving up rather than in a loop.
        let quick = self
            .since
            .take()
            .is_none_or(|since| now.saturating_sub(since) < QUICK_MS);
        match how {
            Ended::Clean => Restart::Stay,
            Ended::Refused => {
                self.waiting = true;
                Restart::WhenConfigChanges
            }
            Ended::Crashed => {
                // Quick crashes in a row, this one included.
                self.quick = if quick { self.quick + 1 } else { 0 };
                if self.quick >= QUICK_CRASHES {
                    self.waiting = true;
                    return Restart::GiveUp;
                }
                let doublings = self.quick.saturating_sub(1).min(16);
                Restart::After((FIRST_WAIT_MS << doublings).min(LONGEST_WAIT_MS))
            }
        }
    }

    /// The config file was read again. Whether to start the shell now: yes
    /// for one waiting on it, with its crashes forgotten.
    pub fn config_changed(&mut self) -> bool {
        let start = std::mem::take(&mut self.waiting);
        if start {
            self.quick = 0;
        }
        start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell started at `at` that crashed `after` milliseconds later.
    fn crash(restarts: &mut Restarts, at: u64, after: u64) -> Restart {
        restarts.started(at);
        restarts.ended(Ended::Crashed, at + after)
    }

    #[test]
    fn a_shell_that_crashes_is_started_again_after_a_short_wait() {
        let mut restarts = Restarts::default();
        assert_eq!(
            crash(&mut restarts, 0, 3_600_000),
            Restart::After(FIRST_WAIT_MS),
            "a shell that ran for an hour comes back at once"
        );
        assert_eq!(crash(&mut restarts, 4_000_000, 50), Restart::After(250));
        assert_eq!(crash(&mut restarts, 4_001_000, 50), Restart::After(500));
        assert_eq!(crash(&mut restarts, 4_002_000, 50), Restart::After(1_000));
        assert_eq!(
            crash(&mut restarts, 4_003_000, QUICK_MS),
            Restart::After(FIRST_WAIT_MS),
            "one that ran its ten seconds starts the count afresh"
        );
    }

    #[test]
    fn a_shell_that_keeps_crashing_at_once_is_given_up_on() {
        let mut restarts = Restarts::default();
        let waits: Vec<_> = (0..QUICK_CRASHES)
            .map(|nth| crash(&mut restarts, u64::from(nth) * 5_000, 100))
            .collect();
        assert_eq!(
            waits,
            [
                Restart::After(250),
                Restart::After(500),
                Restart::After(1_000),
                Restart::After(2_000),
                Restart::GiveUp,
            ]
        );
        assert!(
            restarts.config_changed(),
            "a config that changed may have fixed it"
        );
        assert_eq!(
            crash(&mut restarts, 60_000, 100),
            Restart::After(FIRST_WAIT_MS),
            "and the crashes before it are forgotten"
        );
    }

    #[test]
    fn a_shell_that_refused_its_config_waits_for_a_change() {
        assert_eq!(Ended::from_code(Some(EX_CONFIG)), Ended::Refused);
        let mut restarts = Restarts::default();
        restarts.started(0);
        assert!(
            !restarts.config_changed(),
            "a running shell is told, not started"
        );
        assert_eq!(
            restarts.ended(Ended::Refused, 100),
            Restart::WhenConfigChanges
        );
        assert!(restarts.config_changed(), "started once the file is read");
        assert!(!restarts.config_changed(), "and only once");
    }

    #[test]
    fn a_shell_that_exited_cleanly_is_left_stopped() {
        assert_eq!(Ended::from_code(Some(0)), Ended::Clean);
        assert_eq!(Ended::from_code(Some(1)), Ended::Crashed);
        assert_eq!(Ended::from_code(None), Ended::Crashed, "killed by a signal");
        let mut restarts = Restarts::default();
        restarts.started(0);
        assert_eq!(restarts.ended(Ended::Clean, 100), Restart::Stay);
        assert!(!restarts.config_changed());
    }
}
