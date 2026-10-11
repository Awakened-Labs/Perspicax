//! The agent's wire, under `--mcp`: this process's stdin and stdout, kept for
//! the agent interface alone (issue #43).
//!
//! Every program this process starts inherits fds 0 and 1, and so does every
//! line anything in it prints. With the wire on them, a program's chatter
//! landed between the replies, one that read stdin could take the agent's
//! requests, and a line of valid JSON-RPC from either would pass for
//! perspicax's own -- past `untrusted_text`, which is the only way an
//! application's words are meant to reach the agent.
//!
//! The compositor's `Launch` already starts every program with nothing to read
//! and its output on stderr, but not every program goes through it:
//! Xwayland is smithay's to start, and smithay gives it this process's stdin.
//! So rather than a rule every spawn has to keep, the wire is moved off fds 0
//! and 1 before anything is started, to copies no program can inherit. What is
//! left there is `/dev/null` to read and stderr to print to, which is what a
//! program started here, or a stray `println!`, now gets.

use std::{
    fs::File,
    io,
    os::fd::{AsFd as _, OwnedFd},
};

/// The agent's two ends, off fds 0 and 1.
#[derive(Debug)]
pub struct Wire {
    input: OwnedFd,
    output: OwnedFd,
}

impl Wire {
    /// Take stdin and stdout for the agent interface, leaving `/dev/null` and
    /// stderr in their place.
    ///
    /// Called before anything is started and before any thread runs, so that
    /// nothing has inherited the wire or is part-way through a line on it.
    /// After a `dbus-run-session` that would run this process again, never
    /// before: the copies are closed on exec, and its child would have none.
    ///
    /// # Errors
    ///
    /// When a descriptor cannot be copied, or `/dev/null` opened. Nothing has
    /// moved unless both copies were made.
    pub fn take() -> io::Result<Self> {
        // Close-on-exec, and numbered from 3: no program started can have them.
        let input = io::stdin().as_fd().try_clone_to_owned()?;
        let output = io::stdout().as_fd().try_clone_to_owned()?;
        rustix::stdio::dup2_stdin(File::open("/dev/null")?)?;
        rustix::stdio::dup2_stdout(io::stderr())?;
        Ok(Self { input, output })
    }

    /// The two ends, to read the agent's requests from and write the replies
    /// to. Used inside a runtime: their reads and writes run on its blocking
    /// pool, as `tokio::io::stdin` and `stdout`'s do, so a tty, a pipe and a
    /// socket all serve.
    #[must_use]
    pub fn into_halves(self) -> (tokio::fs::File, tokio::fs::File) {
        (
            tokio::fs::File::from_std(File::from(self.input)),
            tokio::fs::File::from_std(File::from(self.output)),
        )
    }
}
