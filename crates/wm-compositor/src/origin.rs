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

use std::{fs, path::PathBuf};

use wm_node::{Origin, ProcessOrigin};

/// The origin of a process, by pid.
pub(crate) fn of_pid(pid: i32) -> Origin {
    let Ok(pid) = u32::try_from(pid) else {
        return Origin::Unattributed;
    };
    Origin::Process(Box::new(ProcessOrigin {
        pid,
        exe: fs::read_link(PathBuf::from(format!("/proc/{pid}/exe")))
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        cgroup: cgroup(pid),
        sandbox: sandbox(pid),
    }))
}

/// The process's cgroup path.
///
/// The v2 line is `0::/path`, so the path is whatever follows the second colon.
/// Worth having because it is the handle a policy actually wants when the
/// process is one of many inside a container or a user service, where every
/// executable path is the same one.
fn cgroup(pid: u32) -> Option<String> {
    let contents = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let line = contents.lines().next()?;
    let path = line.splitn(3, ':').nth(2)?;
    (!path.is_empty()).then(|| path.to_owned())
}

/// A Flatpak or Snap identity, when the process carries one.
///
/// A sandboxed application has a stabler name than its executable path, which
/// for Flatpak is a path inside a runtime that says nothing about the app. The
/// Flatpak answer is read from inside the sandbox's own mount namespace, which
/// is readable for a child this compositor spawned and may not be for anything
/// else -- so absence here means "not established", never "not sandboxed".
fn sandbox(pid: u32) -> Option<String> {
    if let Ok(info) = fs::read_to_string(format!("/proc/{pid}/root/.flatpak-info")) {
        let name = info
            .lines()
            .find_map(|line| line.strip_prefix("name="))
            .map(str::trim);
        if let Some(name) = name {
            return Some(format!("flatpak:{name}"));
        }
    }

    // Snap leaves its identity in the cgroup path: `.../snap.<name>.<app>...`.
    let cgroup = cgroup(pid)?;
    let start = cgroup.find("snap.")?;
    let rest = &cgroup[start + "snap.".len()..];
    let name = rest.split(['.', '/', '-']).next()?;
    (!name.is_empty()).then(|| format!("snap:{name}"))
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
            exe.contains("origin") || exe.contains("wm"),
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
}
