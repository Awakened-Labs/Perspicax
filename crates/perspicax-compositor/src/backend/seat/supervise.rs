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
//! short wait, a shell that refused its config waits for the file to be
//! read again, and a clean exit is left alone.
//!
//! A save that changes `[shell]` reaches the shell as `reconfigure`. A shell
//! that does not hold `perspicax-shell-v1` cannot hear that, so it is
//! started again instead, which reads the file anew. Turning `enabled` off
//! stops it, and turning it on starts it.
//!
//! Like everything else the person's session starts, it gets no agent
//! consent: an agent can read it, but not click it.

use std::{
    ffi::OsStr,
    fmt::Display,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Child,
    time::{Duration, Instant},
};

use perspicax_config::Shell;
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
/// in force until now. Start, stop, tell or restart the shell to match.
pub(crate) fn reloaded(state: &mut Compositor, before: &Shell) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    if !session.settings.shell.enabled {
        session.stop_shell();
        return;
    }
    let waiting = session.shell.restarts.config_changed();
    let changed = session.settings.shell != *before;
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
        let Some(program) = find(beside.as_deref(), std::env::var_os("PATH").as_deref()) else {
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
        self.shell_stopped(Ended::from_code(status.code()), &status);
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

/// The shell's binary: in `beside`, then in the first directory of `path`
/// that has one. Only a file that may be run counts.
fn find(beside: Option<&Path>, path: Option<&OsStr>) -> Option<PathBuf> {
    let on_path = path.into_iter().flat_map(std::env::split_paths);
    beside
        .map(Path::to_owned)
        .into_iter()
        .chain(on_path)
        .map(|directory| directory.join(PROGRAM))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use super::*;

    /// A fresh directory for one test, emptied when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                env::temp_dir().join(format!("perspicax-supervise-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("a scratch directory");
            Self(dir)
        }

        /// A directory in it holding a `perspicax-shell` with `mode`.
        fn shell(&self, directory: &str, mode: u32) -> PathBuf {
            let directory = self.0.join(directory);
            fs::create_dir_all(&directory).expect("a directory");
            let program = directory.join(PROGRAM);
            fs::write(&program, "").expect("a program");
            fs::set_permissions(&program, fs::Permissions::from_mode(mode)).expect("its mode");
            directory
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn the_shell_beside_perspicax_comes_before_the_one_on_path() {
        let scratch = Scratch::new("beside");
        let beside = scratch.shell("bin", 0o755);
        let installed = scratch.shell("usr-bin", 0o755);
        let path = env::join_paths([&installed]).unwrap();
        assert_eq!(find(Some(&beside), Some(&path)), Some(beside.join(PROGRAM)));
        assert_eq!(
            find(Some(&scratch.0), Some(&path)),
            Some(installed.join(PROGRAM)),
            "and with none beside, the one on PATH"
        );
    }

    #[test]
    fn a_file_that_may_not_be_run_is_not_the_shell() {
        let scratch = Scratch::new("mode");
        let beside = scratch.shell("bin", 0o644);
        assert_eq!(find(Some(&beside), None), None);
    }
}
