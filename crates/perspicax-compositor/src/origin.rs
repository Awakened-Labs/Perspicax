//! Who drew this, from the kernel rather than from the client.
//!
//! A Wayland client's connection is a Unix socket, and a Unix socket carries
//! the peer's credentials as an attestation the kernel makes about the process
//! on the other end. That is the whole basis of provenance in this system: not
//! a pid an application volunteered over an accessibility bus, not a window
//! property it set, but the identity of the process holding the far end of a
//! socket this compositor accepted.
//!
//! The distinction matters because of what provenance is *for*. A universal
//! screen API makes every rendered pixel an instruction channel, and the only
//! defence is being able to say which process authored a string before an agent
//! is allowed to believe it. A number the author supplies about itself is not
//! that defence.
//!
//! Everything derived from the pid afterwards -- the executable, the cgroup, a
//! sandbox identity -- is best-effort and independently optional, because a
//! process can exit between connecting and being asked about. The pid stays
//! attested even when the rest is gone, so [`Origin::Process`] is still the
//! right answer with `exe: None`; a process that has died is not an
//! unattributed one.
//!
//! What a pid does *not* survive is being handed to somebody else, which is why
//! nothing here is read by path. See [`Pinned`].

use std::{
    fs::{self, File},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::fs::MetadataExt as _,
    },
    path::{Path, PathBuf},
};

use perspicax_node::{Origin, ProcessOrigin};
use perspicax_policy::{Owner, Person, Process};
use smithay::reexports::rustix::process::{Pid, PidfdFlags, pidfd_open};

/// The origin of a process, by pid: an X client's, which nothing will hold a
/// grant by, since X11 consent is Xwayland's.
#[cfg_attr(
    not(any(feature = "xwayland", test)),
    expect(dead_code, reason = "Xwayland's X clients")
)]
pub(crate) fn of_pid(pid: i32) -> Origin {
    let Ok(pid) = u32::try_from(pid) else {
        return Origin::Unattributed;
    };
    describe(pid, Pinned::open(pid).as_ref())
}

/// The origin of a process, by pid, and a pidfd on that same process when one
/// can be had: what a grant to it is held by, and learns its exit from. See
/// [`crate::grants`].
///
/// A pidfd is opened by number, so it names the process the pin holds only if
/// that process had not been reaped when it was opened: until then the
/// number is the pinned process's and nobody else's. Reading through the pin
/// after the pidfd is open is what shows that, since a pin reads nothing
/// once its process is reaped; a pidfd the pin cannot vouch for is dropped.
///
/// And what the path its executable is named by proves, judged for `person`:
/// see [`ExeFile`].
pub(crate) fn attest(pid: i32, person: Option<&Person>) -> Attested {
    let Ok(number) = u32::try_from(pid) else {
        return Attested::default();
    };
    let pinned = Pinned::open(number);
    let origin = describe(number, pinned.as_ref());
    let pidfd = pinned.as_ref().and_then(|pinned| {
        let pidfd = pidfd_open(Pid::from_raw(pid)?, PidfdFlags::empty()).ok()?;
        pinned.unreaped().then_some(pidfd)
    });
    let file = match (&origin, &pinned) {
        (Origin::Process(process), Some(pinned)) => process
            .exe
            .as_deref()
            .map(|exe| pinned.file(exe, person))
            .unwrap_or_default(),
        _ => ExeFile::default(),
    };
    Attested {
        origin,
        pidfd,
        file,
    }
}

/// What [`attest`] vouches for.
#[derive(Debug, Default)]
pub(crate) struct Attested {
    pub(crate) origin: Origin,
    /// A pidfd on that same process, when one can be had.
    pub(crate) pidfd: Option<OwnedFd>,
    pub(crate) file: ExeFile,
}

/// What the path the kernel names a process's executable by proves, as
/// [`perspicax_policy::Exe`] carries it: whether the file at that path here
/// is the one it runs, and whether only the system could have put it there.
///
/// The kernel names the file by the path the process's own mounts give it,
/// and mounts of its own need no root, so the path is checked against the
/// file: the inode `exe` leads to, wherever it is mounted, against the inode
/// at that path as this compositor finds it. Nothing but the path's own
/// entries is followed, so a link where the path should be is not the file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ExeFile {
    pub(crate) here: bool,
    pub(crate) installed: bool,
}

/// Whom this compositor runs as, for [`attest`]: `None` if its own status
/// cannot be read, and then nothing is installed by the system.
pub(crate) fn this_person() -> Option<Person> {
    Person::from_status(&fs::read_to_string("/proc/self/status").ok()?)
}

/// The longest chain of parents [`ancestors`] follows. Far past any real
/// one, and a bound on what a client can make this compositor read.
const FARTHEST: usize = 64;

/// The processes `pid` descends from, its parent first, up to but not
/// including the init process. Empty for a process that has gone.
pub(crate) fn ancestors(pid: u32) -> Vec<u32> {
    lineage(pid)
        .into_iter()
        .skip(1)
        .map(|process| process.pid)
        .collect()
}

/// `pid` and the processes it descends from, each with when it started: the
/// process itself first, then its parent, up to but not including the init
/// process. Empty for a process that has gone. What tells a program started
/// by the person's press, under what it reached, from one that was already
/// running (#259).
///
/// Each parent is pinned, then the child is asked again who its parent is.
/// A parent that exits hands its children to another before its own pid can
/// be given to anyone else, so a child still naming it after the pin means
/// the pin holds that parent, not a process given its pid since.
pub(crate) fn lineage(pid: u32) -> Vec<Process> {
    let mut chain = Vec::new();
    let Some(mut child) = Pinned::open(pid) else {
        return chain;
    };
    let Some(started) = child.started() else {
        return chain;
    };
    chain.push(Process { pid, started });
    while chain.len() <= FARTHEST {
        let Some(parent) = child.parent().filter(|&parent| parent > 1) else {
            break;
        };
        let Some(pinned) = Pinned::open(parent) else {
            break;
        };
        if child.parent() != Some(parent) {
            break;
        }
        let Some(started) = pinned.started() else {
            break;
        };
        chain.push(Process {
            pid: parent,
            started,
        });
        child = pinned;
    }
    chain
}

/// `pid`, with when it started: the first of its [`lineage`], alone. `None`
/// for a process that has gone.
pub(crate) fn process(pid: u32) -> Option<Process> {
    let started = Pinned::open(pid)?.started()?;
    Some(Process { pid, started })
}

/// Now, in the clock a process's start is told in: the kernel's ticks since
/// boot, suspend included, as `stat` has counted a start since Linux 5.3.
/// Floored to the tick, as a start is.
pub(crate) fn boot_ticks() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Boottime);
    let per_second = rustix::param::clock_ticks_per_second();
    let seconds = u64::try_from(now.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(now.tv_nsec).unwrap_or(0);
    seconds * per_second + nanos * per_second / 1_000_000_000
}

/// The parent's pid in a `stat` line. Read past the last `)`, since what
/// comes before it is the program's name in brackets, and a name may hold a
/// bracket or a space of its own.
fn parent_in(stat: &str) -> Option<u32> {
    after_name(stat)?.split_whitespace().nth(1)?.parse().ok()
}

/// When the process started, in ticks since boot, from a `stat` line: its
/// 22nd field, read past the name as [`parent_in`] reads.
fn started_in(stat: &str) -> Option<u64> {
    after_name(stat)?.split_whitespace().nth(19)?.parse().ok()
}

/// A `stat` line from its third field on: past the last `)`.
fn after_name(stat: &str) -> Option<&str> {
    Some(&stat[stat.rfind(')')? + 1..])
}

/// Everything a pin can say about its process.
fn describe(pid: u32, pinned: Option<&Pinned>) -> Origin {
    // Pin first, then derive everything from the one handle. Opening the
    // directory per field would reopen it by path each time, which is the race
    // this exists to close, once per field.
    Origin::Process(Box::new(ProcessOrigin {
        pid,
        exe: pinned.and_then(Pinned::exe),
        cgroup: pinned.and_then(Pinned::cgroup),
        sandbox: pinned.and_then(Pinned::sandbox),
    }))
}

/// One process's `/proc` directory, held open for as long as it is asked
/// about.
///
/// A pid is not an identity. The kernel recycles it, and between accepting a
/// connection and asking `/proc` about the peer that process can exit and its
/// number be reissued -- so every answer read by path afterwards would describe
/// a different program while carrying the same confidence. That is the failure
/// worth engineering against here: not a missing answer, which is visible, but
/// a plausible wrong one, which is not.
///
/// An open directory descriptor closes the window. It refers to *this*
/// process's procfs directory rather than to the name `/proc/<pid>`, so a
/// recycled pid gets a fresh directory this descriptor never reaches; once the
/// process is reaped, reads through it fail with `ESRCH` rather than answering
/// about its successor. Absence is the correct answer there, and the pid --
/// which the kernel attested at accept -- is unaffected.
///
/// Reads path through `/proc/self/fd/<n>`, which resolves to the descriptor's
/// own directory. That is `openat` semantics spelled in a way that needs
/// neither libc nor `unsafe`, which matters in the one crate in this workspace
/// whose manifest cannot hold `forbid(unsafe_code)`.
struct Pinned(File);

impl Pinned {
    /// Pin a pid, or fail because there is nothing behind it any more.
    fn open(pid: u32) -> Option<Self> {
        File::open(format!("/proc/{pid}")).ok().map(Self)
    }

    /// The path that reaches the pinned directory, and only it.
    fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.0.as_raw_fd()))
    }

    /// Whether the process has not been reaped: running, or exited with its
    /// parent yet to collect it. Either way its pid is still its own.
    fn unreaped(&self) -> bool {
        fs::read(self.path().join("stat")).is_ok()
    }

    /// The pid of its parent, while it has one.
    fn parent(&self) -> Option<u32> {
        parent_in(&self.stat()?)
    }

    /// When it started, in ticks since boot.
    fn started(&self) -> Option<u64> {
        started_in(&self.stat()?)
    }

    fn stat(&self) -> Option<String> {
        fs::read_to_string(self.path().join("stat")).ok()
    }

    /// The executable behind the pid.
    ///
    /// Honest about a ceiling the Wine attestation spike measured: for every
    /// Wine client this is `wine-preloader`, because that genuinely is the
    /// executable the kernel loaded. The Windows program's name survives only
    /// in `cmdline`, which Wine demonstrably rewrites, so it is not attested
    /// and does not belong in an attestation. `exe` names the process; it does
    /// not name the program running inside it, and under a loader or an
    /// interpreter those are different questions.
    fn exe(&self) -> Option<String> {
        fs::read_link(self.path().join("exe"))
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }

    /// What `exe`, the path [`Self::exe`] read, proves of the file the
    /// process runs. See [`ExeFile`].
    fn file(&self, exe: &str, person: Option<&Person>) -> ExeFile {
        let Ok(runs) = fs::metadata(self.path().join("exe")) else {
            return ExeFile::default();
        };
        let Ok(named) = fs::symlink_metadata(exe) else {
            return ExeFile::default();
        };
        let here = (runs.dev(), runs.ino()) == (named.dev(), named.ino());
        let installed = here
            && person.is_some_and(|person| {
                perspicax_policy::installed(Path::new(exe), person, |entry| {
                    let entry = fs::symlink_metadata(entry).ok()?;
                    Some(Owner {
                        uid: entry.uid(),
                        gid: entry.gid(),
                        mode: entry.mode(),
                    })
                })
            });
        ExeFile { here, installed }
    }

    /// The process's cgroup path.
    ///
    /// The v2 line is `0::/path`, so the path is whatever follows the second
    /// colon. Worth having because it is the handle a policy actually wants
    /// when the process is one of many inside a container or a user service,
    /// where every executable path is the same one.
    fn cgroup(&self) -> Option<String> {
        let contents = fs::read_to_string(self.path().join("cgroup")).ok()?;
        let line = contents.lines().next()?;
        let path = line.splitn(3, ':').nth(2)?;
        (!path.is_empty()).then(|| path.to_owned())
    }

    /// A Flatpak or Snap identity, when the process carries one.
    ///
    /// A sandboxed application has a stabler name than its executable path,
    /// which for Flatpak is a path inside a runtime that says nothing about the
    /// app. The Flatpak answer is read from inside the sandbox's own mount
    /// namespace, which is readable for a child this compositor spawned and may
    /// not be for anything else -- so absence here means "not established",
    /// never "not sandboxed".
    fn sandbox(&self) -> Option<String> {
        if let Ok(info) = fs::read_to_string(self.path().join("root/.flatpak-info")) {
            let name = info
                .lines()
                .find_map(|line| line.strip_prefix("name="))
                .map(str::trim);
            if let Some(name) = name {
                return Some(format!("flatpak:{name}"));
            }
        }

        // Snap leaves its identity in the cgroup path: `.../snap.<name>.<app>...`.
        let cgroup = self.cgroup()?;
        let start = cgroup.find("snap.")?;
        let rest = &cgroup[start + "snap.".len()..];
        let name = rest.split(['.', '/', '-']).next()?;
        (!name.is_empty()).then(|| format!("snap:{name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one process whose answers can be checked against something already
    /// known: this one.
    #[test]
    fn a_live_process_is_attributed_to_its_own_executable() {
        let pid = std::process::id();
        let Origin::Process(origin) = of_pid(i32::try_from(pid).unwrap()) else {
            panic!("a running process must be attributable");
        };
        assert_eq!(origin.pid, pid);
        let exe = origin
            .exe
            .expect("/proc/self/exe is readable for ourselves");
        assert!(
            exe.contains("origin") || exe.contains("perspicax"),
            "expected this test binary's path, got {exe}"
        );
    }

    /// A pid nothing is using loses the executable and keeps the attestation.
    /// The failure this guards against is a compositor that downgrades a dead
    /// client to `Unattributed`, which reads as "we do not know who this was"
    /// when in fact we do.
    #[test]
    fn a_process_that_has_gone_is_still_attributed() {
        // Above any plausible live pid, and below the kernel's ceiling.
        let Origin::Process(origin) = of_pid(0x7FFF_FFFE) else {
            panic!("a pid with no process behind it is still a pid");
        };
        assert_eq!(origin.pid, 0x7FFF_FFFE);
        assert!(origin.exe.is_none());
    }

    /// A live process is attested with a pidfd to hold a grant by.
    #[test]
    fn a_live_process_is_attested_with_a_pidfd() {
        let attested = attest(i32::try_from(std::process::id()).unwrap(), None);
        assert!(matches!(attested.origin, Origin::Process(_)));
        assert!(
            attested.pidfd.is_some(),
            "this process is alive, and pidfds are Linux 5.3+"
        );
    }

    /// A pid with nothing behind it is attributed, as [`of_pid`] says, and
    /// yields nothing to hold a grant by: nothing could ever say it ended.
    #[test]
    fn a_pid_with_no_process_cannot_be_held() {
        let attested = attest(0x7FFF_FFFE, this_person().as_ref());
        assert!(matches!(attested.origin, Origin::Process(_)));
        assert!(attested.pidfd.is_none());
        assert_eq!(attested.file, ExeFile::default(), "nor any file proven");
    }

    /// This test binary is the file at the path the kernel names it by, and
    /// the person running the tests could have written it: it is not the
    /// system's. As root, nothing is.
    #[test]
    fn a_process_is_the_file_at_its_path_here() {
        let attested = attest(
            i32::try_from(std::process::id()).unwrap(),
            this_person().as_ref(),
        );
        assert_eq!(
            attested.file,
            ExeFile {
                here: true,
                installed: false
            }
        );
    }

    /// `sleep` from coreutils, as the system installed it: named so only
    /// where the person is not root and the system's folders are root's.
    #[test]
    fn a_program_the_system_installed_is_said_to_be() {
        let Some(person) = this_person().filter(|person| person.uid != 0) else {
            return;
        };
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep is in coreutils");
        let attested = attest(i32::try_from(child.id()).unwrap(), Some(&person));
        child.kill().expect("the child is ours to kill");
        child.wait().expect("and ours to reap");
        assert_eq!(
            attested.file,
            ExeFile {
                here: true,
                installed: true
            },
            "{:?}",
            attested.origin
        );
    }

    #[test]
    fn a_parent_is_read_past_a_name_with_a_parenthesis() {
        assert_eq!(parent_in("4242 (a) b (c) S 17 4242 4242 0 -1"), Some(17));
        assert_eq!(parent_in("1 (init) S 0 1 1 0 -1"), Some(0));
        assert_eq!(parent_in("4242 (truncated"), None);
    }

    #[test]
    fn a_start_is_read_past_a_name_with_a_parenthesis() {
        let stat = "4242 (a) b (c) S 17 4242 4242 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 \
                    98765 1 2";
        assert_eq!(started_in(stat), Some(98_765));
        assert_eq!(started_in("4242 (truncated) S 17"), None);
    }

    /// A child started now started no sooner than now, and its lineage names
    /// it with its start, then this process with this one's.
    #[test]
    fn a_child_started_now_is_told_from_its_parent_by_its_start() {
        let before = boot_ticks();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep is in coreutils");
        let chain = lineage(child.id());
        child.kill().expect("the child is ours to kill");
        child.wait().expect("and ours to reap");

        let (own, parent) = (chain[0], chain[1]);
        assert_eq!(own.pid, child.id(), "{chain:?}");
        assert!(own.started >= before, "{chain:?} started before {before}");
        assert_eq!(parent.pid, std::process::id(), "{chain:?}");
        assert!(parent.started <= own.started, "{chain:?}");
        assert!(
            lineage(0x7FFF_FFFE).is_empty(),
            "a pid with nothing behind it"
        );
    }

    /// A child of this process descends from it, and the chain ends before
    /// the init process.
    #[test]
    fn a_child_descends_from_this_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep is in coreutils");
        let chain = ancestors(child.id());
        child.kill().expect("the child is ours to kill");
        child.wait().expect("and ours to reap");

        assert_eq!(chain.first(), Some(&std::process::id()), "{chain:?}");
        assert!(!chain.contains(&1), "{chain:?}");
        assert!(
            ancestors(0x7FFF_FFFE).is_empty(),
            "a pid with nothing behind it"
        );
    }

    /// Negative pids are what a failed lookup looks like in C, and they are not
    /// process identifiers. Refuse rather than cast.
    #[test]
    fn a_negative_pid_is_not_an_identity() {
        assert_eq!(of_pid(-1), Origin::Unattributed);
    }

    /// The pin's whole purpose, and the only test here that would have failed
    /// before it existed: once the process is gone the handle answers nothing,
    /// rather than answering about whoever inherits the number. A recycled pid
    /// cannot be reached through it, which is unobservable directly -- the
    /// kernel will not reissue a pid on demand -- so this asserts the property
    /// that makes the reissue harmless.
    #[test]
    fn a_pin_answers_nothing_once_its_process_is_reaped() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep is in coreutils, and the test image has it");
        let pinned = Pinned::open(child.id()).expect("a live child has a /proc directory");
        assert!(
            pinned.exe().is_some(),
            "the pin must read while the process lives, or it proves nothing below"
        );

        child.kill().expect("the child is ours to kill");
        child.wait().expect("and ours to reap");

        assert!(!pinned.unreaped(), "it is reaped, and says so");
        assert_eq!(pinned.exe(), None, "a reaped process has no executable");
        assert_eq!(pinned.cgroup(), None, "nor a cgroup");
        assert_eq!(pinned.sandbox(), None, "nor a sandbox identity");
    }
}
