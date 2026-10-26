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
//!
//! Some programs are never granted at all, whatever asks: those that exist
//! so the person can oversee an agent, which an agent driving them would
//! oversee itself (see [`Never`]). The settings app is one everywhere, and
//! on a seat `[agent] never` names the rest, ennius by default. Every way to
//! grant asks before it does, by the executable: the file a command will
//! run, found and with its links followed, for what is started for an
//! agent, and the one its process runs for a window or a launch's
//! descendant. What is started is still started, since the person asked for
//! it, and is not the agent's. The consent the grants publish carries the
//! list too, so a way to grant that forgot to ask, or a granted process that
//! has since become one of them, still gives the agent nothing to act on.

use std::{ffi::OsStr, os::fd::OwnedFd, path::Path, time::Duration};

use perspicax_index::Consent;
use perspicax_node::{Origin, SurfaceId};
use perspicax_policy::{Never, Rule};
use smithay::{
    reexports::{
        calloop::{
            self, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
            generic::Generic,
            timer::{TimeoutAction, Timer},
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
    shell_protocol::Refused,
    state::{ClientState, Compositor},
};

/// How long a window shows the person that the grant they asked for on it
/// was refused.
const REFUSED_FOR: Duration = Duration::from_millis(800);

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
    /// What is never granted, whatever asks: the settings app, and on a
    /// seat `[agent] never`.
    never: Never,
    /// What `held` adds up to, kept beside it so the gates that ask on
    /// every snapshot and every picture ask without building it.
    consent: Consent,
    /// The window showing the person that a grant they asked for was
    /// refused, and the timer that ends it.
    refused: Option<(SurfaceId, RegistrationToken)>,
}

impl Grants {
    /// No grant yet. `by_grant` false is everyone, for good. `never` is
    /// what is never granted.
    pub(crate) fn new(by_grant: bool, never: Never) -> Self {
        let mut grants = Self {
            by_grant,
            held: Vec::new(),
            never,
            consent: Consent::Everyone,
            refused: None,
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

    /// The program, as written, that keeps whoever drew `origin` from every
    /// agent, if one does: known by the executable its process runs. An
    /// X11 window is Xwayland's to consent for, so none is named here.
    fn withholds(&self, origin: &Origin) -> Option<&str> {
        match origin {
            Origin::Process(process) => self.never.naming(process.exe.as_deref()?),
            Origin::X11(_) | Origin::Unattributed => None,
        }
    }

    fn recount(&mut self) {
        self.consent = if self.by_grant {
            Consent::Granted {
                pids: self.held.iter().map(|held| held.pid).collect(),
                never: self.never.clone(),
            }
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
    ///
    /// A program no agent is given is started all the same, since someone
    /// asked for it, and not granted: `--spawn`, a binding or an autostart
    /// entry the person wrote names it some way the config could not see
    /// through, a link of another name, say. An agent's own launch is
    /// refused before it gets here (see [`Self::launch_for_agent`]).
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
        let withheld = self.withholds(command, dir);
        let child = launch.spawn_in(command, dir)?;
        let pid = child.id();
        match withheld {
            Some(program) => tracing::warn!(
                pid,
                ?why,
                program,
                "started, and not granted: no agent is ever given it"
            ),
            None => self.grant(pid, why),
        }
        self.spawned.push(child);
        Ok(pid)
    }

    /// The program, as written, that keeps what `command` would run, in
    /// `dir`, from every agent, if one does. Asked of the command's first
    /// word as written, then of the file it names, found as it would be run
    /// and with every link followed, so a link of another name to a program
    /// the list names is known for it. Asked before the program starts,
    /// because what `/proc` says of a process just started can still be the
    /// compositor itself.
    fn withholds(&self, command: &[impl AsRef<OsStr>], dir: Option<&Path>) -> Option<String> {
        let program = command.first()?.as_ref();
        let named = |file: &Path| {
            self.grants
                .never
                .naming(&file.to_string_lossy())
                .map(str::to_owned)
        };
        named(Path::new(program)).or_else(|| {
            let file = self.launch.as_ref()?.resolve(program, dir)?;
            named(&std::fs::canonicalize(&file).unwrap_or(file))
        })
    }

    /// Start a program an agent asked for, in `cwd` if it named one, and
    /// grant it: what the `launch` tool does. Its pid, or why not.
    ///
    /// Refused while the session is locked, as every act is: the person has
    /// walked away. A program no agent is ever given is refused next, and
    /// before any rule is asked, so the agent is not told the person could
    /// admit it (see [`Never`]). Otherwise only what the rule admits is
    /// started (see [`Rule::launches`]): headless, what the test asked, and
    /// any program by default; on a seat, `[agent] launch` as last saved,
    /// which is off until the person names programs.
    pub(crate) fn launch_for_agent(
        &mut self,
        command: &[String],
        cwd: Option<&Path>,
    ) -> Result<u32, LaunchError> {
        if self.lock.is_some() {
            return Err(LaunchError::Locked);
        }
        if let Some(program) = self.withholds(command, cwd) {
            return Err(LaunchError::NeverGranted { program });
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
        let Some((root, pidfd)) = root.zip(client.pidfd()) else {
            return;
        };
        if let Some(program) = self.grants.withholds(&client.origin()) {
            tracing::warn!(
                pid,
                under = root,
                program,
                "not granted: no agent is ever given it, whatever started it"
            );
            return;
        }
        self.grant_watched(pid, pidfd, Why::Lineage, Some(root));
    }

    /// `[agent] never` changed: what is granted from now on is held to
    /// `never`. A grant held already on a program it now names stays until
    /// its process exits, but the consent published here refuses that
    /// program's windows at once, and takes the mark off them.
    #[cfg_attr(not(feature = "seat"), expect(dead_code, reason = "a seat's reload"))]
    pub(crate) fn withhold(&mut self, never: Never) {
        self.grants.never = never;
        self.regranted();
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
    /// so granting one X11 window would grant all of them. So is a program
    /// no agent is ever given. A grant already held is taken back whatever
    /// the program, so the person can always take one back. Each refusal is
    /// logged, saying why, shown on the window, and told to the shell, which
    /// says why in words (see [`Self::refuse`]).
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
        let pid = match &origin {
            Origin::Process(process) => process.pid,
            Origin::X11(_) => {
                tracing::warn!(
                    surface = ?id,
                    "not granted: consent for X11 is Xwayland's, and granting one X11 window \
                     would grant every one"
                );
                self.refuse(id, Refused::GrantX11);
                return;
            }
            Origin::Unattributed => {
                tracing::warn!(surface = ?id, "not granted: nobody knows which process drew it");
                self.refuse(id, Refused::GrantUnattributed);
                return;
            }
        };
        if self.grants.holds(pid) {
            self.revoke(pid);
            return;
        }
        if let Some(program) = self.grants.withholds(&origin) {
            tracing::warn!(
                surface = ?id,
                pid,
                program,
                "not granted: no agent is ever given it, whatever grants it"
            );
            self.refuse(id, Refused::GrantNever(program.to_owned()));
            return;
        }
        match self.pidfd_of(window) {
            Some(pidfd) => self.grant_watched(pid, pidfd, Why::Key, None),
            None => {
                tracing::warn!(
                    surface = ?id,
                    pid,
                    "not granted: its process could not be held, so its grant could not end \
                     with it"
                );
                self.refuse(id, Refused::GrantUnheld);
            }
        }
    }

    /// Show the person that the grant they asked for on the window `id` was
    /// refused, and why: the line round it drawn for [`REFUSED_FOR`] in
    /// `[decorations.agent] refused-colour`, and `why` told to the shell,
    /// which says it in words. A key that granted nothing would otherwise
    /// change nothing they can see. The line shows whether or not a shell
    /// is listening, and the log says why either way.
    fn refuse(&mut self, id: SurfaceId, why: Refused) {
        self.tell_shell_refused(why);
        if let Some((_, showing)) = self.grants.refused.take() {
            self.loop_handle.remove(showing);
        }
        let armed =
            self.loop_handle
                .insert_source(Timer::from_duration(REFUSED_FOR), |_, (), state| {
                    state.grants.refused = None;
                    state.publish_facts();
                    state.backend.redraw();
                    TimeoutAction::Drop
                });
        match armed {
            Ok(token) => self.grants.refused = Some((id, token)),
            Err(error) => {
                tracing::warn!(%error, "a refused grant is not shown");
                return;
            }
        }
        // The line is part of what the window covers, as every line is.
        self.publish_facts();
        self.backend.redraw();
    }

    /// Whether `window` is showing the person that a grant was refused.
    pub(crate) fn shows_refused(&self, window: &Framed) -> bool {
        self.grants
            .refused
            .as_ref()
            .is_some_and(|(id, _)| id_of(window) == Some(*id))
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

    use perspicax_policy::{Never, Program};

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
        let grants = Grants::new(false, Never::FLOOR);
        assert_eq!(grants.consent(), &Consent::Everyone);
        assert!(grants.consent().permits(&process(4242)));
    }

    #[test]
    fn grants_by_grant_consent_to_what_they_hold_and_nothing_else() {
        let mut grants = Grants::new(true, Never::FLOOR);
        assert_eq!(grants.consent(), &Consent::granted(Vec::new()));
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
        assert!(!Grants::new(false, Never::FLOOR).marks(&process(7)));

        let mut grants = Grants::new(true, Never::FLOOR);
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

    /// What is never granted goes out with the grants, so the consent they
    /// publish refuses it whatever pid it runs as, a pid granted before the
    /// list named it too, and its window is not marked.
    #[test]
    fn what_is_never_granted_is_refused_by_the_consent_the_grants_publish() {
        let mut grants = Grants::new(true, Never::new(vec![Program::parse("ennius")]));
        grants.held.push(Held {
            pid: 7,
            why: Why::Key,
            watch: None,
            under: None,
        });
        grants.recount();
        let ennius = Origin::Process(Box::new(ProcessOrigin {
            pid: 7,
            exe: Some("/usr/local/bin/ennius".to_owned()),
            cgroup: None,
            sandbox: None,
        }));
        assert!(!grants.consent().permits(&ennius));
        assert!(!grants.marks(&ennius));
        assert_eq!(grants.withholds(&ennius), Some("ennius"));
        assert_eq!(
            grants.withholds(&process(7)),
            None,
            "nobody knows its program"
        );
        assert!(grants.marks(&process(7)));
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
