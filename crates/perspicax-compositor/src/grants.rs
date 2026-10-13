//! Which processes an agent may act on where a person has to agree to it,
//! and for how long: each grant, held until its process exits.
//!
//! Headless, consent is everyone, and nothing is ever granted: nobody sits
//! at the desk, and every client on it is one an agent set up. On a seat --
//! and on a headless desk told to consent as one does, for the tests of
//! exactly this -- an agent may act only on what something granted it.
//!
//! A grant names a pid, and a pid is not an identity. The kernel reissues the
//! number once its process has exited and been reaped, so a grant that
//! outlived its process would pass its consent on to whatever program was
//! given the number next, with nothing on screen to say so. Every grant
//! therefore ends with its process, and is held only where the end is
//! learned of before the number can be given to anyone else:
//!
//! - A child of this compositor keeps its pid, even exited, until this
//!   compositor reaps it. The reap that collects it ends its grant in the
//!   same turn of the loop, before another client can be accepted, so no
//!   process can arrive with that pid while it is still granted.

use perspicax_index::Consent;

use crate::state::Compositor;

/// Why a process was granted: for the log, which is where a person who
/// wonders why an agent could act on something will look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// `--spawn`: started for an agent as the compositor came up.
    Spawn,
}

/// One process granted.
#[derive(Debug)]
struct Held {
    pid: u32,
    why: Why,
}

/// Every grant, and the consent they add up to.
#[derive(Debug)]
pub(crate) struct Grants {
    /// Whether consent is by grant at all. Not, headless, unless a test
    /// asked for a desk that consents as a seat does.
    by_grant: bool,
    held: Vec<Held>,
    /// What `held` adds up to, kept beside it so the gates that ask on
    /// every snapshot and every picture ask without building it.
    consent: Consent,
}

impl Grants {
    /// No grant yet. `by_grant` false is everyone, for good.
    pub(crate) fn new(by_grant: bool) -> Self {
        let mut grants = Self {
            by_grant,
            held: Vec::new(),
            consent: Consent::Everyone,
        };
        grants.recount();
        grants
    }

    /// The consent policy the grants add up to, as published to readers.
    pub(crate) fn consent(&self) -> &Consent {
        &self.consent
    }

    fn holds(&self, pid: u32) -> bool {
        self.held.iter().any(|held| held.pid == pid)
    }

    fn recount(&mut self) {
        self.consent = if self.by_grant {
            Consent::Granted(self.held.iter().map(|held| held.pid).collect())
        } else {
            Consent::Everyone
        };
    }
}

impl Compositor {
    /// Grant `pid`, a child of this compositor, to the agent: until it is
    /// reaped (see [`Self::reap_spawned`]). Nothing, where consent is not by
    /// grant. A process already granted keeps its first grant.
    pub(crate) fn grant(&mut self, pid: u32, why: Why) {
        if !self.grants.by_grant || self.grants.holds(pid) {
            return;
        }
        tracing::info!(pid, ?why, "granted to the agent");
        self.grants.held.push(Held { pid, why });
        self.grants.recount();
        self.publish_facts();
    }

    /// End `pid`'s grant, if it holds one.
    pub(crate) fn revoke(&mut self, pid: u32) {
        let Some(at) = self.grants.held.iter().position(|held| held.pid == pid) else {
            return;
        };
        let held = self.grants.held.remove(at);
        tracing::info!(pid, why = ?held.why, "no longer granted to the agent");
        self.grants.recount();
        self.publish_facts();
    }

    /// Collect the agent's children that have exited, so they do not sit as
    /// zombies until the session ends, and end their grants as they go.
    ///
    /// Both in one place, because together they are what makes a child's
    /// grant safe to hold by pid: the pid cannot be given to another process
    /// until the reap here, and the reap here ends the grant before the loop
    /// accepts another client. A spawned program that finished is gone from
    /// `spawned`, and nothing is left to kill for it at the end.
    pub(crate) fn reap_spawned(&mut self) {
        let mut exited = Vec::new();
        self.spawned.retain_mut(|child| {
            let running = matches!(child.try_wait(), Ok(None));
            if !running {
                exited.push(child.id());
            }
            running
        });
        for pid in exited {
            self.revoke(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use perspicax_index::Consent;
    use perspicax_node::{Origin, ProcessOrigin};

    use super::{Grants, Held, Why};

    fn process(pid: u32) -> Origin {
        Origin::Process(Box::new(ProcessOrigin {
            pid,
            exe: None,
            cgroup: None,
            sandbox: None,
        }))
    }

    #[test]
    fn grants_that_are_not_by_grant_consent_to_everyone() {
        let grants = Grants::new(false);
        assert_eq!(grants.consent(), &Consent::Everyone);
        assert!(grants.consent().permits(&process(4242)));
    }

    #[test]
    fn grants_by_grant_consent_to_what_they_hold_and_nothing_else() {
        let mut grants = Grants::new(true);
        assert_eq!(grants.consent(), &Consent::Granted(Vec::new()));
        grants.held.push(Held {
            pid: 7,
            why: Why::Spawn,
        });
        grants.recount();
        assert!(grants.consent().permits(&process(7)));
        assert!(!grants.consent().permits(&process(8)));
    }
}
