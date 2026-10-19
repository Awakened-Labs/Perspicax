//! perspicax-shell: started when the person logs in, and started again
//! when it stops.
//!
//! The binary is looked for beside perspicax's own, where `cargo build` and
//! a package both put it, and then on `PATH`. Without one the session runs
//! with no desktop but the grey backdrop, and the log says why.
//!
//! It is started with `--config` naming the file perspicax read, once the
//! agent's programs are up and before autostart. When it stops,
//! [`Restarts`] decides what happens: a crash is started again after a
//! short wait, and so is a shell that lost its connection to perspicax,
//! which perspicax, running on, did not end; a shell that refused its config
//! waits for the file to be read again, and a clean exit is left alone.
//!
//! A save that changes `[shell]` reaches the shell as `reconfigure`, and so
//! does one that changes `[protocols]`, whose rules decide which of the
//! taskbar's and the pager's protocols the shell may hold: told, it takes
//! back any the rules give back. A shell that does not hold
//! `perspicax-shell-v1` cannot hear that, so it is started again instead,
//! which reads the file and the rules anew. Turning `enabled` off stops it,
//! and turning it on starts it.
//!
//! Like everything else the person's session starts, it gets no agent
//! consent: an agent can read it, but not click it.

use std::{
    fmt::Display,
    path::Path,
    process::Child,
    time::{Duration, Instant},
};

use perspicax_config::{Shell, find_program};
use perspicax_policy::{Ended, Restart, Restarts};
use smithay::reexports::calloop::{
    RegistrationToken,
    timer::{TimeoutAction, Timer},
};

use super::Session;
use crate::{Launch, backend::Running, state::Compositor};

/// The binary's name, beside perspicax's and on `PATH`.
const PROGRAM: &str = "perspicax-shell";

/// The shell, as a session supervises it.
pub(super) struct Supervisor {
    /// The shell running now.
    child: Option<Child>,
    restarts: Restarts,
    /// A start again, waiting on its timer.
    timer: Option<RegistrationToken>,
    /// The origin of the times `restarts` is given.
    epoch: Instant,
}

impl Supervisor {
    pub(super) fn new() -> Self {
        Self {
            child: None,
            restarts: Restarts::default(),
            timer: None,
            epoch: Instant::now(),
        }
    }

    /// Milliseconds since the session began.
    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Start the shell, if the config wants one and none is running.
pub(crate) fn start(state: &mut Compositor) {
    let launch = state.launch.clone();
    if let Running::Seat(session) = &mut state.backend {
        session.start_shell(launch.as_ref());
    }
}

/// The config was read again, and `before` is the `[shell]` table that was
/// in force until now; `rules_changed` says whether `[protocols]` changed
/// too. Start, stop, tell or restart the shell to match.
pub(crate) fn reloaded(state: &mut Compositor, before: &Shell, rules_changed: bool) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    if !session.settings.shell.enabled {
        session.stop_shell();
        return;
    }
    let waiting = session.shell.restarts.config_changed();
    let changed = session.settings.shell != *before || rules_changed;
    let Some(pid) = session.shell.child.as_ref().map(Child::id) else {
        if waiting || !before.enabled {
            start(state);
        }
        return;
    };
    if !changed {
        return;
    }
    state.reconfigure_shells();
    if !state.shell_listening(pid) {
        tracing::info!(
            pid,
            "the shell is not listening for a changed config; restarting it"
        );
        if let Running::Seat(session) = &mut state.backend {
            session.stop_shell();
        }
        start(state);
    }
}

impl Session {
    fn start_shell(&mut self, launch: Option<&Launch>) {
        if !self.settings.shell.enabled || self.shell.child.is_some() {
            return;
        }
        let Some(launch) = launch else {
            return;
        };
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_owned));
        let path = std::env::var_os("PATH");
        let Some(program) = find_program(PROGRAM, beside.as_deref(), path.as_deref()) else {
            tracing::warn!(
                "{PROGRAM} is neither beside perspicax nor on PATH; the desktop stays bare"
            );
            return;
        };
        let mut command = vec![program.into_os_string()];
        if let Some(config) = &self.config_path {
            command.extend(["--config".into(), config.clone().into_os_string()]);
        }
        let now = self.shell.now();
        match launch.spawn(&command) {
            Ok(child) => {
                self.shell.restarts.started(now);
                self.shell.child = Some(child);
            }
            Err(error) => self.shell_stopped(Ended::Crashed, &error),
        }
    }

    /// Stop the shell on purpose, and forget its crashes: what it does next
    /// time is a fresh start.
    fn stop_shell(&mut self) {
        if let Some(timer) = self.shell.timer.take() {
            self.handle.remove(timer);
        }
        if let Some(mut child) = self.shell.child.take() {
            super::settings::stop(&mut child);
        }
        self.shell.restarts = Restarts::default();
    }

    /// Collect the shell if it stopped by itself, and decide whether it
    /// comes back.
    pub(super) fn reap_shell(&mut self) {
        let Some(child) = &mut self.shell.child else {
            return;
        };
        let status = match child.try_wait() {
            Ok(None) => return,
            Ok(Some(status)) => status,
            Err(error) => {
                tracing::warn!(%error, "could not ask whether the shell is running");
                return;
            }
        };
        self.shell.child = None;
        match Ended::from_code(status.code()) {
            // The status alone would say only "exit status: 69".
            Ended::Lost => self.shell_stopped(
                Ended::Lost,
                &format_args!("it lost its connection to perspicax, {status}"),
            ),
            ended => self.shell_stopped(ended, &status),
        }
    }

    /// The shell stopped, as `ended` and `why` say: start it again, or wait.
    fn shell_stopped(&mut self, ended: Ended, why: &dyn Display) {
        let now = self.shell.now();
        match self.shell.restarts.ended(ended, now) {
            Restart::After(wait) => {
                tracing::warn!("the shell is not running ({why}); trying again in {wait} ms");
                self.start_shell_after(Duration::from_millis(wait));
            }
            Restart::WhenConfigChanges => tracing::error!(
                "the shell cannot use the config ({why}); it starts again when the file is saved"
            ),
            Restart::GiveUp => tracing::error!(
                "the shell keeps stopping ({why}); not starting it again until the config is saved"
            ),
            Restart::Stay => tracing::info!("the shell exited ({why}); leaving it stopped"),
        }
    }

    fn start_shell_after(&mut self, wait: Duration) {
        let armed = self
            .handle
            .insert_source(Timer::from_duration(wait), |_, (), state| {
                if let Running::Seat(session) = &mut state.backend {
                    session.shell.timer = None;
                }
                start(state);
                TimeoutAction::Drop
            });
        match armed {
            Ok(timer) => self.shell.timer = Some(timer),
            Err(error) => tracing::warn!(%error, "could not schedule the shell's restart"),
        }
    }
}

impl Drop for Supervisor {
    /// The shell ends with the session that started it.
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            super::settings::stop(child);
        }
    }
}
