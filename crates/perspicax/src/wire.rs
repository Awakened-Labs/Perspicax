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
//!
//! A holder it cannot see is not refused, and there is one it cannot tell
//! from the client: `dbus-run-session` keeps its own copy of whatever it
//! passes on, the wire on fds 3 and 4 included, for as long as the run lasts,
//! where any process running as this user can reopen it through `/proc`
//! (issue #161). A pipe's two ends are one file, so that copy and the client's
//! own end look alike. So a bus is had the other way round: this process
//! starts its own ([`crate::session::Bus`]), and gives it nothing of the wire.
//!
//! Nor may a copy be *taken* without being given (issue #118). By default the
//! kernel lets any process running as this user reopen a pipe this one holds
//! through `/proc/<pid>/fd/<n>`, take a descriptor whole with `pidfd_getfd`, or
//! attach with ptrace and rewrite whatever this process says, and a hosted
//! program is such a process. So [`Wire::take`] first keeps them all out of
//! this one ([`keep_others_out`]). The client's own end is the client's to
//! keep: a pipe it holds can be reopened through its own `/proc/<pid>/fd` just
//! the same, where a socket end cannot be.

use std::{
    fs::{self, File},
    io::{self, IsTerminal as _},
    os::{
        fd::{AsFd as _, BorrowedFd, OwnedFd},
        unix::fs::{FileTypeExt as _, MetadataExt as _},
    },
    path::Path,
};

use rustix::process::DumpableBehavior;

/// The agent's two ends, off fds 0 and 1.
#[derive(Debug)]
pub struct Wire {
    input: OwnedFd,
    output: OwnedFd,
}

impl Wire {
    /// Take stdin and stdout for the agent interface, leaving `/dev/null` and
    /// stderr in their place, and keep every other process running as this
    /// user out of this one ([`keep_others_out`]), so that none can take the
    /// wire from it either.
    ///
    /// Called before anything is started and before any thread runs, so that
    /// nothing has inherited the wire or is part-way through a line on it.
    /// After whatever would run this process again for a bus -- a
    /// `dbus-run-session`, or this process itself for one of its own -- never
    /// before: the copies are closed on exec, and the process run again would
    /// have none.
    ///
    /// # Errors
    ///
    /// When others cannot be kept out, a descriptor cannot be copied, or
    /// `/dev/null` opened. Nothing has moved unless both copies were made.
    pub fn take() -> io::Result<Self> {
        keep_others_out()?;
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

/// Keep every other process running as this user out of this one (issue
/// #118), by making it one the kernel will not let them look into. What it
/// gives a process's own user by default, and now refuses anybody without
/// `CAP_SYS_PTRACE`:
///
/// - reopening a descriptor through `/proc/<pid>/fd/<n>`, which opens the very
///   pipe, FIFO or file it is open on, to write a reply this process did not
///   send;
/// - `pidfd_getfd`, which takes a descriptor whole, a socket's as much as a
///   pipe's;
/// - ptrace and `/proc/<pid>/mem`, which read and rewrite anything this
///   process holds: the index, and every verdict it gives.
///
/// Yama, where a kernel has it, keeps the last two to a process's ancestors
/// already, and lets the first through.
///
/// It costs this process its core dump, and its `/proc/<pid>` becomes root's,
/// so looking at its descriptors from outside takes root; its own it reads as
/// before. A program it starts is not kept out with it: an `exec` undoes this.
///
/// # Errors
///
/// When the kernel refuses it, which `prctl` does only for an argument it does
/// not know.
pub fn keep_others_out() -> io::Result<()> {
    rustix::process::set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        io::{BufRead as _, BufReader, PipeReader, PipeWriter, Read as _, Write as _},
        os::fd::{AsRawFd as _, RawFd},
        process::{Child, Command, Stdio},
    };

    use rustix::{
        io::Errno,
        process::{Pid, PidfdFlags, PidfdGetfdFlags},
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

    /// Set for the copy of this binary that [`take_probe`] runs in.
    const PROBE: &str = "PERSPICAX_WIRE_PROBE";

    /// Where this process's `fd` points, as `/proc` says: `/dev/null`, or
    /// `pipe:[N]`.
    fn link(fd: RawFd) -> String {
        fs::read_link(format!("/proc/self/fd/{fd}"))
            .expect("a /proc fd link")
            .display()
            .to_string()
    }

    /// Whether this process may look into any other, `CAP_SYS_PTRACE` being
    /// what the kernel lets past one that keeps the rest out.
    fn looks_into_anything() -> bool {
        fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                let held = status
                    .lines()
                    .find_map(|line| line.strip_prefix("CapEff:"))?;
                u64::from_str_radix(held.trim(), 16).ok()
            })
            .is_some_and(|held| held & (1 << 19) != 0)
    }

    /// A process that has taken the wire, run in a copy of this binary given
    /// three different pipes, so that where its fds 0 and 1 went proves
    /// something. Looked at from here, the process that started it: the
    /// ancestor Yama lets in where a kernel has it, and so one only the probe
    /// itself can keep out (issue #118).
    #[test]
    fn a_process_that_took_the_wire_keeps_every_other_out() {
        let mut probe = Command::new(std::env::current_exe().expect("this test's binary"))
            .args(["--exact", "wire::tests::take_probe", "--ignored"])
            .args(["--test-threads=1", "--nocapture"])
            .env(PROBE, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the probe runs");
        let log = link(probe.stderr.as_ref().expect("its stderr").as_raw_fd());
        // After what the test harness says first, on the stdout that is the
        // wire until the probe takes it: the test's name, on the same line.
        let report = BufReader::new(probe.stdout.take().expect("its stdout"))
            .lines()
            .map_while(Result::ok)
            .find_map(|line| Some(line.split_once("taken ")?.1.to_owned()));
        let Some(report) = report else {
            let ended = probe.wait_with_output().expect("the probe ends");
            panic!(
                "the probe never said it took the wire: {}",
                String::from_utf8_lossy(&ended.stderr)
            );
        };
        let [dumpable, stdin, stdout, output] = report
            .split(' ')
            .collect::<Vec<_>>()
            .try_into()
            .unwrap_or_else(|said| panic!("not a report: {said:?}"));

        assert_eq!(dumpable, "NotDumpable");
        assert_eq!(stdin, "/dev/null", "a read reads nothing");
        assert_eq!(stdout, log, "a `println!` lands in the log");
        if looks_into_anything() {
            eprintln!(
                "this test holds CAP_SYS_PTRACE, which nothing keeps out: the probe's \
                 own word is all there is to check"
            );
        } else {
            let pid = probe.id();
            // The issue's: the wire reopened, for a reply perspicax did not
            // send.
            let reopened = OpenOptions::new()
                .write(true)
                .open(format!("/proc/{pid}/fd/{output}"));
            assert_eq!(
                reopened.map(drop).map_err(|error| error.kind()),
                Err(io::ErrorKind::PermissionDenied),
                "the wire reopened through /proc"
            );
            // What ptrace reads and rewrites.
            let memory = OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!("/proc/{pid}/mem"));
            assert_eq!(
                memory.map(drop).map_err(|error| error.kind()),
                Err(io::ErrorKind::PermissionDenied),
                "its memory"
            );
            // An end taken whole, as a socket's would be.
            let pidfd = Pid::from_raw(i32::try_from(pid).expect("a pid"))
                .map(|pid| rustix::process::pidfd_open(pid, PidfdFlags::empty()))
                .expect("a pid")
                .expect("a pidfd");
            let output = output.parse().expect("a descriptor");
            assert_eq!(
                rustix::process::pidfd_getfd(pidfd, output, PidfdGetfdFlags::empty()).map(drop),
                Err(Errno::PERM),
                "the wire taken whole"
            );
        }

        // The probe waits on the wire for the far end to finish.
        drop(probe.stdin.take());
        let ended = probe.wait_with_output().expect("the probe ends");
        let said = String::from_utf8_lossy(&ended.stderr);
        // A filter that matched nothing passes too, having run nothing.
        assert!(
            ended.status.success() && said.contains("1 passed"),
            "{said}"
        );
    }

    #[test]
    #[ignore = "run by a_process_that_took_the_wire_keeps_every_other_out"]
    fn take_probe() {
        if std::env::var_os(PROBE).is_none() {
            return;
        }
        let wire = Wire::take().expect("the wire");
        let dumpable = rustix::process::dumpable_behavior().expect("an answer");
        let (input, output) = (File::from(wire.input), File::from(wire.output));
        writeln!(
            &output,
            "taken {dumpable:?} {} {} {}",
            link(0),
            link(1),
            output.as_raw_fd()
        )
        .expect("the wire");
        let _ = (&input).read_to_end(&mut Vec::new());
    }
}
