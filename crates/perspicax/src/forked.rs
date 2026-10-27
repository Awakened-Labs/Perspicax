//! For the tests: what another test's fork holds a moment (#272).
//!
//! The tests run in parallel in one process, and many of them start
//! programs. A fork takes every file this process has open into the child,
//! where it stays open until the child runs its program. So a stand-in a
//! test has just written, still open for writing when another test forks,
//! is busy (`ETXTBSY`) if it is run in that moment; and a socket a test has
//! just closed still answers while a child holds it.
//!
//! A stand-in is therefore written by a process of its own, a shell that
//! writes it and exits: this process never has it open, and a fork here has
//! nothing of it to take. perspicax-settings holds a lock instead
//! (`judge.rs`), which works only while every test that starts a program,
//! through the code under test included, holds it too. And a socket closed
//! is waited on until it stops answering: [`soon`].

use std::{
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

/// Write `text` to `path`, a program that may be run, from a process of its
/// own: see the module's doc.
pub(crate) fn program(path: &Path, text: &str) {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).expect("its folder");
    }
    let written = Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$1" > "$0" && chmod 755 "$0""#])
        .arg(path)
        .arg(text)
        .status()
        .expect("a shell to write it");
    assert!(written.success(), "{} not written", path.display());
}

/// Whether `check` comes to hold within a second: long enough for any child
/// forked meanwhile to have run its program, and let go of what it took.
pub(crate) fn soon(check: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}
