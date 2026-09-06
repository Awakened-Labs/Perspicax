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
    os::fd::AsRawFd,
    path::PathBuf,
};

use perspicax_node::{Origin, ProcessOrigin};

/// The origin of a process, by pid.
pub(crate) fn of_pid(pid: i32) -> Origin {
    let Ok(pid) = u32::try_from(pid) else {
        return Origin::Unattributed;
    };
    // Pin first, then derive everything from the one handle. Opening the
    // directory per field would reopen it by path each time, which is the race
    // this exists to close, once per field.
    let pinned = Pinned::open(pid);
    Origin::Process(Box::new(ProcessOrigin {
        pid,
        exe: pinned.as_ref().and_then(Pinned::exe),
        cgroup: pinned.as_ref().and_then(Pinned::cgroup),
        sandbox: pinned.as_ref().and_then(Pinned::sandbox),
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

        assert_eq!(pinned.exe(), None, "a reaped process has no executable");
        assert_eq!(pinned.cgroup(), None, "nor a cgroup");
        assert_eq!(pinned.sandbox(), None, "nor a sandbox identity");
    }
}
