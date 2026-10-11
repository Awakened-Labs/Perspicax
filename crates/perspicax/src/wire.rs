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
//!
//! What it cannot move is a copy somebody else was given before this process
//! ran (issue #52). A `dbus-run-session` handed the wire starts its bus daemon
//! with it, and every service the bus starts inherits it from there -- one a
//! hosted program asks for by name, or writes a `.service` file for, included.
//! A stderr that is the wire too is the same thing from the other side, since
//! every program started here prints to it. So [`Wire::shares`] and
//! [`Wire::held_by`] say when another holder is there, and the run is refused
//! rather than served on a wire it cannot keep.

use std::{
    fs::{self, File},
    io::{self, IsTerminal as _},
    os::{
        fd::{AsFd as _, BorrowedFd, OwnedFd},
        unix::fs::{FileTypeExt as _, MetadataExt as _},
    },
    path::Path,
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

    /// Whether `fd` is open on an end of the wire: this process's stderr,
    /// say, which under `2>&1` is the wire too.
    ///
    /// # Errors
    ///
    /// When `fd` or an end cannot be looked at.
    pub fn shares(&self, fd: BorrowedFd<'_>) -> io::Result<bool> {
        let file = File::from(fd.try_clone_to_owned()?).metadata()?;
        Ok(self.carrying()?.contains(&(file.dev(), file.ino())))
    }

    /// Which of process `pid`'s descriptors 0, 1 and 2 is open on an end of
    /// the wire, if one is: the three a program inherits from whatever starts
    /// it, as every service a bus daemon starts inherits the daemon's.
    ///
    /// # Errors
    ///
    /// When `/proc` will not show `pid`'s descriptors: a process that is not
    /// there, or one of another user's.
    pub fn held_by(&self, pid: u32) -> io::Result<Option<u8>> {
        let ends = self.carrying()?;
        if ends.is_empty() {
            return Ok(None);
        }
        let fds = Path::new("/proc").join(pid.to_string()).join("fd");
        // Listed first, so that a process this one cannot look into is an
        // error rather than three descriptors that seem to be closed.
        fs::read_dir(&fds)?;
        for fd in 0..=2 {
            // Followed, as the link `/proc` keeps for an open file is, to the
            // file itself: a pipe's, a socket's or a terminal's inode as much
            // as a file's.
            match fs::metadata(fds.join(fd.to_string())) {
                Ok(file) if ends.contains(&(file.dev(), file.ino())) => return Ok(Some(fd)),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    /// The ends another holder could carry something on, as the device and
    /// inode a descriptor open on one would have.
    ///
    /// Not a terminal: a person's terminal is theirs to begin with, and any
    /// program of theirs can open it by name, so another copy of it gives
    /// nothing away -- and counting one would refuse `perspicax --mcp` typed
    /// in a terminal, where the log is on the same screen anyway. Nor
    /// `/dev/null`, which carries nothing at all, and which is where a bus
    /// daemon's stdin always is.
    fn carrying(&self) -> io::Result<Vec<(u64, u64)>> {
        let null = fs::metadata("/dev/null")?.rdev();
        let mut ends = Vec::with_capacity(2);
        for end in [&self.input, &self.output] {
            if end.is_terminal() {
                continue;
            }
            let file = File::from(end.try_clone()?).metadata()?;
            if file.file_type().is_char_device() && file.rdev() == null {
                continue;
            }
            ends.push((file.dev(), file.ino()));
        }
        Ok(ends)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        io::{PipeReader, PipeWriter},
        process::{Child, Command, Stdio},
    };

    use super::*;

    /// A wire on two pipes of the test's own, and the other end of each: what
    /// the agent writes requests to, and reads the replies from.
    fn wire() -> (Wire, PipeWriter, PipeReader) {
        let (input, asking) = io::pipe().expect("a pipe for the requests");
        let (answers, output) = io::pipe().expect("a pipe for the replies");
        let wire = Wire {
            input: input.into(),
            output: output.into(),
        };
        (wire, asking, answers)
    }

    fn null() -> File {
        File::open("/dev/null").expect("/dev/null")
    }

    #[test]
    fn a_descriptor_on_either_end_is_the_wire_and_one_on_another_pipe_is_not() {
        let (wire, asking, answers) = wire();
        assert!(wire.shares(wire.input.as_fd()).expect("an answer"));
        assert!(wire.shares(wire.output.as_fd()).expect("an answer"));
        // The other end of a pipe is the same pipe.
        assert!(wire.shares(asking.as_fd()).expect("an answer"));
        assert!(wire.shares(answers.as_fd()).expect("an answer"));

        let (elsewhere, _) = io::pipe().expect("another pipe");
        assert!(!wire.shares(elsewhere.as_fd()).expect("an answer"));
        assert!(!wire.shares(null().as_fd()).expect("an answer"));
    }

    #[test]
    fn dev_null_is_never_the_wire() {
        let (_, output) = io::pipe().expect("a pipe for the replies");
        let wire = Wire {
            input: OwnedFd::from(null()),
            output: output.into(),
        };
        assert!(!wire.shares(null().as_fd()).expect("an answer"));
        assert!(!wire.shares(wire.input.as_fd()).expect("an answer"));
        assert!(wire.shares(wire.output.as_fd()).expect("an answer"));
    }

    #[test]
    fn a_terminal_is_never_the_wire() {
        let terminal = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/ptmx")
            .expect("a terminal of the test's own");
        let wire = Wire {
            input: terminal.try_clone().expect("a second handle").into(),
            output: terminal.into(),
        };
        assert!(!wire.shares(wire.input.as_fd()).expect("an answer"));
        assert_eq!(wire.held_by(std::process::id()).expect("an answer"), None);
    }

    /// `sleep`, given `stdio` for its descriptors 0, 1 and 2.
    fn holding(stdio: [Stdio; 3]) -> Child {
        let [stdin, stdout, stderr] = stdio;
        Command::new("sleep")
            .arg("30")
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("sleep")
    }

    #[test]
    fn a_process_given_an_end_holds_the_wire_and_one_given_dev_null_does_not() {
        let (wire, _asking, _answers) = wire();
        for fd in 0..=2 {
            // A copy of the end a program would inherit there, as a bus
            // daemon `dbus-run-session` started inherits it.
            let end = if fd == 0 { &wire.input } else { &wire.output };
            let mut stdio = [Stdio::null(), Stdio::null(), Stdio::null()];
            stdio[usize::from(fd)] = Stdio::from(end.try_clone().expect("a copy"));
            let mut child = holding(stdio);
            let held = wire.held_by(child.id());
            child.kill().ok();
            child.wait().ok();
            assert_eq!(held.expect("an answer"), Some(fd), "fd {fd}");
        }

        let mut child = holding([Stdio::null(), Stdio::null(), Stdio::null()]);
        let held = wire.held_by(child.id());
        child.kill().ok();
        child.wait().ok();
        assert_eq!(held.expect("an answer"), None);

        assert!(wire.held_by(u32::MAX).is_err(), "no such process");
    }
}
