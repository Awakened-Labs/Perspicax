//! `perspicax doctor`, run as a person runs it, on a machine of the test's
//! own: nothing of the environment it was started from but a home folder and
//! an empty `PATH`.
//!
//! What it finds besides depends on the machine the tests run on -- an
//! accessibility registry, a perspicax-shell built beside the binary -- so
//! these assert on its rows' order, and on what an environment of nothing
//! makes of the rows that read only the environment.

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

/// A home folder of this test's own, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pxd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("bin")).expect("a scratch folder");
        Self(dir)
    }

    /// The doctor, asked about this home with `more` in its environment.
    fn doctor(&self, more: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_perspicax"))
            .arg("doctor")
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", self.0.join("bin"))
            .envs(more.iter().copied())
            .stdin(Stdio::null())
            .output()
            .expect("perspicax runs")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

/// Each row's name and word, in order.
fn rows(output: &Output) -> Vec<(String, String)> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .take_while(|line| !line.is_empty())
        .map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next().unwrap_or_default().to_owned();
            (name, words.next().unwrap_or_default().to_owned())
        })
        .collect()
}

#[test]
fn every_row_is_said_in_order_and_no_runtime_folder_is_a_failure() {
    let scratch = Scratch::new();
    let output = scratch.doctor(&[]);
    let rows = rows(&output);
    let names: Vec<&str> = rows.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "build",
            "runtime-dir",
            "session-bus",
            "dbus",
            "accessibility",
            "shell",
            "xwayland",
            "config",
            "leave",
            "agent-socket",
            "claude-code",
            "ennius"
        ],
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let said = |name: &str| {
        rows.iter()
            .find(|(row, _)| row == name)
            .map(|(_, said)| said.as_str())
    };
    assert_eq!(said("runtime-dir"), Some("fail"));
    assert_eq!(said("session-bus"), Some("warn"));
    assert_eq!(said("claude-code"), Some("skipped"), "no Claude Code here");
    assert_eq!(said("ennius"), Some("skipped"), "no ennius here");
    assert_eq!(output.status.code(), Some(1), "a failure is exit 1");
}

#[test]
fn a_runtime_folder_and_a_bus_are_found_in_the_environment() {
    let scratch = Scratch::new();
    let runtime = scratch.0.display().to_string();
    let output = scratch.doctor(&[
        ("XDG_RUNTIME_DIR", &runtime),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nowhere"),
    ]);
    let rows = rows(&output);
    assert!(
        rows.contains(&("runtime-dir".to_owned(), "ok".to_owned())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("session-bus".to_owned(), "ok".to_owned())),
        "{rows:?}"
    );
}
