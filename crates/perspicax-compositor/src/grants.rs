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
//! - Anyone else's process is held by a pidfd, opened as its client was
//!   accepted and vouched for there (see [`crate::origin::attest`]). The
//!   pidfd becomes readable the moment the process exits, which is before
//!   its parent can reap it, and the loop ends the grant when it does. A
//!   process nothing could be vouched for is never granted: its grant could
//!   not be made to end.

use std::{ffi::OsStr, os::fd::OwnedFd, path::Path};

use perspicax_index::Consent;
use perspicax_node::Origin;
use perspicax_policy::Rule;
use smithay::{
    reexports::{
        calloop::{
            self, Interest, LoopHandle, Mode, PostAction, RegistrationToken, generic::Generic,
        },
        wayland_server::Resource as _,
    },
    wayland::seat::WaylandFocus as _,
};

use crate::{
    framed::Framed,
    host::LaunchError,
    origin,
    shell::id_of,
    state::{ClientState, Compositor},
};

/// Why a process was granted: for the log, which is where a person who
/// wonders why an agent could act on something will look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// `--spawn`: started for an agent as the compositor came up.
    Spawn,
    /// The person granted its window, with a key.
    Key,
    /// A binding the person marked `agent = true` started it.
    Binding,
    /// An `autostart` entry the person marked `agent = true` started it.
    #[cfg_attr(
        not(feature = "seat"),
        expect(dead_code, reason = "a seat's autostart")
    )]
    Autostart,
    /// An agent started it for itself, with `launch`, as the rule admits.
    Launch,
    /// A program an agent launched started it, and runs yet.
    Lineage,
}

/// One process granted.
#[derive(Debug)]
struct Held {
    pid: u32,
    why: Why,
    /// The pidfd watch that ends the grant, for a process that is not a
    /// child of this compositor; a child's ends with its reap.
    watch: Option<RegistrationToken>,
    /// The launch it descends from, whose end ends it too: for
    /// [`Why::Lineage`].
    under: Option<u32>,
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

    /// Whether a window `origin` drew is marked out as one an agent may act
    /// on: where consent is by grant, and a grant covers it. Never where
    /// consent is everyone's, which would mark every window there is.
    pub(crate) fn marks(&self, origin: &Origin) -> bool {
        self.by_grant && self.consent.permits(origin)
    }

    /// Whether `pid` is a program an agent launched, granted yet.
    fn launched(&self, pid: u32) -> bool {
        self.held
            .iter()
            .any(|held| held.pid == pid && held.why == Why::Launch)
    }

    fn recount(&mut self) {
        self.consent = if self.by_grant {
            Consent::Granted(self.held.iter().map(|held| held.pid).collect())
        } else {
            Consent::Everyone
        };
    }
}

/// Call `gone` once, when the process `pidfd` names exits.
///
/// Level-triggered and removed after the first call: a pidfd stays readable
/// from the exit on, so one call is all there is to say.
fn watch<D: 'static>(
    handle: &LoopHandle<'static, D>,
    pidfd: OwnedFd,
    mut gone: impl FnMut(&mut D) + 'static,
) -> Result<RegistrationToken, calloop::Error> {
    handle
        .insert_source(
            Generic::new(pidfd, Interest::READ, Mode::Level),
            move |_, _, data| {
                gone(data);
                Ok(PostAction::Remove)
            },
        )
        .map_err(|failed| failed.error)
}

impl Compositor {
    /// Start a program an agent may act on, in `dir` if one is named: as
    /// every program is started (see [`crate::Launch`]), kept in `spawned`
    /// to be reaped as it exits and killed with the session, and granted
    /// until it is reaped. Its pid, or why it would not start.
    pub(crate) fn start_for_agent(
        &mut self,
        command: &[impl AsRef<OsStr>],
        dir: Option<&Path>,
        why: Why,
    ) -> Result<u32, crate::Error> {
        let Some(launch) = self.launch.clone() else {
            return Err(crate::Error::Spawn {
                command: String::new(),
                source: std::io::Error::other("no socket to start it against yet"),
            });
        };
        let child = launch.spawn_in(command, dir)?;
        let pid = child.id();
        self.grant(pid, why);
        self.spawned.push(child);
        Ok(pid)
    }

    /// Start a program an agent asked for, in `cwd` if it named one, and
    /// grant it: what the `launch` tool does. Its pid, or why not.
    ///
    /// Refused while the session is locked, as every act is: the person has
    /// walked away. Otherwise only what the rule admits is started (see
    /// [`Rule::launches`]): headless, what the test asked, and any program
    /// by default; on a seat, `[agent] launch` as last saved, which is off
    /// until the person names programs.
    pub(crate) fn launch_for_agent(
        &mut self,
        command: &[String],
        cwd: Option<&Path>,
    ) -> Result<u32, LaunchError> {
        if self.lock.is_some() {
            return Err(LaunchError::Locked);
        }
        let rule = self.backend.launching();
        if *rule == Rule::Off {
            return Err(LaunchError::Off);
        }
        let Some(program) = command.first() else {
            return Err(LaunchError::NotStarted("an empty command".to_owned()));
        };
        if !rule.launches(program) {
            return Err(LaunchError::NotAdmitted {
                program: program.clone(),
                admitted: rule.named().into_iter().map(str::to_owned).collect(),
            });
        }
        self.start_for_agent(command, cwd, Why::Launch)
            .map_err(|error| LaunchError::NotStarted(error.to_string()))
    }

    /// Grant `pid`, a child of this compositor, to the agent: until it is
    /// reaped (see [`Self::reap_spawned`]). Nothing, where consent is not by
    /// grant. A process already granted keeps its first grant.
    pub(crate) fn grant(&mut self, pid: u32, why: Why) {
        self.keep_grant(pid, why, None, None);
    }

    /// Grant a client's process, `pid`, if a program an agent launched
    /// started it and runs yet: until either exits. Asked as each client is
    /// accepted, and held by the pidfd it was vouched for with.
    ///
    /// What a launched program starts is the agent's because the person's
    /// `[agent] launch` named the program that started it, and so bounds it.
    /// What `--spawn`, a key or `agent = true` grants covers that process
    /// alone. A program that starts its window through a second fork, or
    /// asks D-Bus or a running copy of itself to, is not found: its window's
    /// process does not descend from the launch.
    pub(crate) fn grant_lineage(&mut self, pid: u32, client: &ClientState) {
        if !self.grants.by_grant || !self.grants.held.iter().any(|held| held.why == Why::Launch) {
            return;
        }
        let root = origin::ancestors(pid)
            .into_iter()
            .find(|&ancestor| self.grants.launched(ancestor));
        if let Some((root, pidfd)) = root.zip(client.pidfd()) {
            self.grant_watched(pid, pidfd, Why::Lineage, Some(root));
        }
    }

    /// Grant `pid`, anyone's process, to the agent: until `pidfd`, which
    /// names it, says it exited, or the launch it is `under` ends. Nothing,
    /// where consent is not by grant.
    fn grant_watched(&mut self, pid: u32, pidfd: OwnedFd, why: Why, under: Option<u32>) {
        if !self.grants.by_grant || self.grants.holds(pid) {
            return;
        }
        match watch(&self.loop_handle, pidfd, move |state: &mut Self| {
            state.exited(pid);
        }) {
            Ok(token) => self.keep_grant(pid, why, Some(token), under),
            // A grant nothing will end is one not made.
            Err(error) => tracing::warn!(pid, %error, "not granted: its exit could not be watched"),
        }
    }

    fn keep_grant(
        &mut self,
        pid: u32,
        why: Why,
        watch: Option<RegistrationToken>,
        under: Option<u32>,
    ) {
        if !self.grants.by_grant || self.grants.holds(pid) {
            return;
        }
        tracing::info!(pid, ?why, ?under, "granted to the agent");
        self.grants.held.push(Held {
            pid,
            why,
            watch,
            under,
        });
        self.regranted();
    }

    /// The grants changed: publish the consent they now add up to, and draw
    /// the windows again, whose marks follow it.
    fn regranted(&mut self) {
        self.grants.recount();
        self.publish_facts();
        self.backend.redraw();
    }

    /// A watched process exited. Its watch removes itself.
    fn exited(&mut self, pid: u32) {
        if let Some(held) = self.grants.held.iter_mut().find(|held| held.pid == pid) {
            held.watch = None;
        }
        self.revoke(pid);
    }

    /// End `pid`'s grant, if it holds one, and those of what it started
    /// while it was a launch.
    pub(crate) fn revoke(&mut self, pid: u32) {
        let (ended, kept) = std::mem::take(&mut self.grants.held)
            .into_iter()
            .partition::<Vec<_>, _>(|held| held.pid == pid || held.under == Some(pid));
        self.grants.held = kept;
        if ended.is_empty() {
            return;
        }
        for held in ended {
            if let Some(token) = held.watch {
                self.loop_handle.remove(token);
            }
            tracing::info!(pid = held.pid, why = ?held.why, "no longer granted to the agent");
        }
        self.regranted();
    }

    /// End every grant: what the person's `revoke-agent-all` does. What
    /// `--spawn` started goes with the rest; an agent has nothing left.
    pub(crate) fn revoke_all(&mut self) {
        if self.grants.held.is_empty() {
            return;
        }
        for held in std::mem::take(&mut self.grants.held) {
            if let Some(token) = held.watch {
                self.loop_handle.remove(token);
            }
        }
        tracing::info!("nothing is granted to the agent any more");
        self.regranted();
    }

    /// Grant the process that drew `window`, or take its grant back: what
    /// the person's `toggle-agent` does to the focused window.
    ///
    /// Only a process its client was accepted for, with a pidfd to hold it
    /// by. An X11 window is refused, because consent for X11 is Xwayland's:
    /// every X client shares one server and may read and drive the others,
    /// so granting one X11 window would grant all of them. Each refusal is
    /// logged, saying why.
    pub(crate) fn toggle_agent(&mut self, window: &Framed) {
        let Some(id) = id_of(window) else {
            return;
        };
        if !self.grants.by_grant {
            tracing::info!(surface = ?id, "nothing to grant: an agent may act on everything here");
            return;
        }
        let origin = self
            .facts
            .read()
            .surface(id)
            .map(|facts| facts.origin.clone())
            .unwrap_or_default();
        let pid = match origin {
            Origin::Process(process) => process.pid,
            Origin::X11(_) => {
                tracing::warn!(
                    surface = ?id,
                    "not granted: consent for X11 is Xwayland's, and granting one X11 window \
                     would grant every one"
                );
                return;
            }
            Origin::Unattributed => {
                tracing::warn!(surface = ?id, "not granted: nobody knows which process drew it");
                return;
            }
        };
        if self.grants.holds(pid) {
            self.revoke(pid);
            return;
        }
        match self.pidfd_of(window) {
            Some(pidfd) => self.grant_watched(pid, pidfd, Why::Key, None),
            None => tracing::warn!(
                surface = ?id,
                pid,
                "not granted: its process could not be held, so its grant could not end with it"
            ),
        }
    }

    /// A pidfd of its own on the process whose Wayland client drew `window`,
    /// as its connection was vouched for when it was accepted. For an X11
    /// window that client is Xwayland, which is why [`Self::toggle_agent`]
    /// refuses one before it asks.
    fn pidfd_of(&self, window: &Framed) -> Option<OwnedFd> {
        let surface = window.wl_surface()?;
        let client = self.display.get_client(surface.id()).ok()?;
        client.get_data::<ClientState>()?.pidfd()
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
    use std::time::{Duration, Instant};

    use perspicax_index::Consent;
    use perspicax_node::{Origin, ProcessOrigin};
    use smithay::reexports::{
        calloop::EventLoop,
        rustix::process::{Pid, PidfdFlags, pidfd_open},
    };

    use super::{Grants, Held, Why, watch};

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
            watch: None,
            under: None,
        });
        grants.recount();
        assert!(grants.consent().permits(&process(7)));
        assert!(!grants.consent().permits(&process(8)));
    }

    /// Everyone's consent permits every window, and marking every window
    /// would say nothing: only a grant marks one out.
    #[test]
    fn a_window_is_marked_out_only_where_consent_is_by_grant() {
        assert!(!Grants::new(false).marks(&process(7)));

        let mut grants = Grants::new(true);
        grants.held.push(Held {
            pid: 7,
            why: Why::Key,
            watch: None,
            under: None,
        });
        grants.recount();
        assert!(grants.marks(&process(7)));
        assert!(!grants.marks(&process(8)));
        assert!(!grants.marks(&Origin::Unattributed));
    }

    /// The watch every grant to someone else's process is held by: told once
    /// the process exits, and not before.
    #[test]
    fn a_watched_process_is_heard_going() {
        let mut child = std::process::Command::new("sleep")
            .arg("0.2")
            .spawn()
            .expect("sleep is in coreutils");
        let pidfd = pidfd_open(Pid::from_child(&child), PidfdFlags::empty()).expect("a pidfd");
        let mut event_loop: EventLoop<'static, u32> = EventLoop::try_new().expect("a loop");
        watch(&event_loop.handle(), pidfd, |heard: &mut u32| *heard += 1).expect("watched");

        let mut heard = 0;
        event_loop
            .dispatch(Some(Duration::from_millis(50)), &mut heard)
            .expect("dispatched");
        assert_eq!(heard, 0, "still running");

        let deadline = Instant::now() + Duration::from_secs(5);
        while heard == 0 && Instant::now() < deadline {
            event_loop
                .dispatch(Some(Duration::from_millis(50)), &mut heard)
                .expect("dispatched");
        }
        assert_eq!(heard, 1, "heard going");
        event_loop
            .dispatch(Some(Duration::from_millis(50)), &mut heard)
            .expect("dispatched");
        assert_eq!(heard, 1, "and only once");
        child.wait().expect("reaped");
    }
}
