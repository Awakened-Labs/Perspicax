//! Owning the accessibility session, rather than hoping somebody else did.
//!
//! A toolkit only joins the accessibility bus if three separate things are
//! true, and on a desktop all three are arranged by a session manager nobody
//! thinks about. In a container none of them are, and every one of them fails
//! *silently* -- the application starts, draws its window, and simply never
//! appears on the bus.
//!
//! This compositor spawns the applications it hosts, so it is the session
//! manager for them, and arranging this is its job rather than a preamble in
//! whatever script happens to run it:
//!
//! 1. **The registry has to be running before the clients start.** GTK asks
//!    once and gives up; Qt activates it on demand. Start both toolkits at once
//!    against a cold bus and the Qt one arrives, the GTK one does not, and the
//!    symptom looks exactly like an ingest that cannot read GTK.
//! 2. **`org.a11y.Status.IsEnabled` has to be true.** Qt's bridge gates on it
//!    and GTK's ignores it, so a false flag loses one toolkit and keeps the
//!    other. A desktop sets it from dconf; a fresh `HOME` has no dconf state,
//!    so it defaults to false exactly where it is hardest to notice.
//! 3. **The toolkits have to be told.** `GTK_A11Y`, `QT_ACCESSIBILITY`, and
//!    `QT_LINUX_ACCESSIBILITY_ALWAYS_ON`, which bypasses the flag above.
//!
//! All three were learned the expensive way while measuring M1 and are written
//! down here so that they are a property of the program rather than of a
//! runbook.

use std::{
    fs,
    process::{Child, Command, Stdio},
    time::Duration,
};

use anyhow::{Context as _, Result};

/// Environment every child of this compositor gets, so that a toolkit which
/// would otherwise decide nobody is listening joins the bus anyway.
///
/// `QT_LINUX_ACCESSIBILITY_ALWAYS_ON` is the load-bearing one: it bypasses the
/// `IsEnabled` check entirely, which matters because that flag lives in dconf
/// and is invisible to anyone debugging with `env`.
#[must_use]
pub fn accessibility_env() -> Vec<(String, String)> {
    [
        ("GTK_A11Y", "atspi"),
        ("QT_ACCESSIBILITY", "1"),
        ("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

/// A registry this process started and is responsible for stopping.
pub struct Registry(Option<Child>);

impl Registry {
    /// Make sure an accessibility registry is running, starting one if not.
    ///
    /// Returns a handle that stops it again only if we were the ones to start
    /// it: on a real desktop the session's own registry is already up and
    /// serving every other application on the machine, and taking it down on
    /// the way out would be this program breaking somebody else's screen
    /// reader.
    ///
    /// # Errors
    ///
    /// Only if no registry is running *and* none can be started.
    pub fn ensure() -> Result<Self> {
        if registry_is_running() {
            tracing::debug!("an accessibility registry is already running");
            return Ok(Self(None));
        }

        let child = Command::new("/usr/libexec/at-spi2-registryd")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("no accessibility registry is running and none could be started")?;
        tracing::info!(pid = child.id(), "started an accessibility registry");

        // Give it a moment to claim its bus name. A client that asks before it
        // has is the exact race this whole type exists to avoid, and polling
        // for the name is not better: the name appears before the registry is
        // ready to answer about children.
        std::thread::sleep(Duration::from_millis(500));
        Ok(Self(Some(child)))
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Whether something is already serving the accessibility registry.
///
/// Read out of `/proc` rather than by shelling out to `pgrep`, for two
/// reasons. A minimal container image need not have `procps` at all, and the
/// absence would be indistinguishable from "no registry is running" -- so this
/// compositor would start a second one on a desktop that already had one.
/// And `pgrep` without `-f` matches `/proc/<pid>/comm`, which the kernel
/// truncates to fifteen characters: `at-spi2-registryd` is seventeen, so the
/// obvious spelling silently matches nothing.
fn registry_is_running() -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        fs::read(entry.path().join("cmdline")).is_ok_and(|cmdline| {
            // Arguments are NUL-separated; only the executable is of interest,
            // so that a command line merely *mentioning* the registry -- a
            // shell running this very check, for instance -- is not mistaken
            // for the registry itself.
            cmdline
                .split(|byte| *byte == 0)
                .next()
                .is_some_and(|argv0| argv0.ends_with(b"at-spi2-registryd"))
        })
    })
}
