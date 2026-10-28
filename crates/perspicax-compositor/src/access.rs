//! Who may bind the protocols that reach past a client's own windows.
//!
//! The rules are `perspicax_policy::Access`, from `[protocols]` on a seat and
//! open headless. They are held here behind a lock that every such global's
//! filter reads, because the Wayland server asks a global's filter again on
//! every registry listing and every bind: a reload that changes a rule
//! therefore changes who sees the global from the next client on, with no
//! global torn down. A client that already holds one is checked again by
//! the protocol itself, which withdraws what the new rule no longer allows.
//!
//! A client is named by the executable recorded when its connection was
//! accepted (`crate::origin`), never asked for again, with what the path it
//! is named by proved then: whether it is the very file the client runs, and
//! whether only the system could have put it there. Xwayland's own
//! connection carries no such record and is never admitted: an X11 program
//! reaching other windows through it would be a program nobody named.

use std::sync::{Arc, PoisonError, RwLock};

use perspicax_node::Origin;
use perspicax_policy::{Access, Exe, Protocol, Rule};
use smithay::reexports::wayland_server::Client;

use crate::state::ClientState;

/// The rules, shared between the compositor and the globals' filters.
#[derive(Debug, Clone, Default)]
pub(crate) struct Gate(Arc<RwLock<Access>>);

impl Gate {
    pub(crate) fn new(access: Access) -> Self {
        Self(Arc::new(RwLock::new(access)))
    }

    /// Replace the rules, returning the protocols whose existing users have
    /// to be checked again.
    pub(crate) fn set(&self, access: Access) -> Vec<Protocol> {
        let mut rules = self.0.write().unwrap_or_else(PoisonError::into_inner);
        let narrowed = rules.narrowed(&access);
        *rules = access;
        narrowed
    }

    /// The lock's global's filter, for smithay's: whether a client may see
    /// it, asked as [`Filtered`] asks. A client refused it that is called
    /// what a program the rule names is called is noted, for the shell to
    /// say so ([`Self::note_if_a_lock`]).
    pub(crate) fn lock_filter(&self) -> impl Fn(&Client) -> bool + Send + Sync + use<> {
        let gate = self.clone();
        move |client| {
            let admitted = gate.admits(Protocol::SessionLock, client);
            if !admitted {
                gate.note_if_a_lock(client);
            }
            admitted
        }
    }

    /// Note `client`, refused the lock, if it is called what a program the
    /// lock's rule names is called: a lock program, and not the copy the
    /// rule admits, or a program passing itself off as one. Its note is the
    /// program as the rule writes it, never the path the client runs from,
    /// whose folders are its own to name (#178).
    pub(crate) fn note_if_a_lock(&self, client: &Client) {
        let Some(state) = client.get_data::<ClientState>() else {
            return;
        };
        let Origin::Process(process) = state.origin() else {
            return;
        };
        let Some(exe) = process.exe.as_deref() else {
            return;
        };
        let called = std::path::Path::new(exe).file_name();
        let rules = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let Rule::Only(programs) = rules.rule(Protocol::SessionLock) else {
            return;
        };
        let named = programs
            .iter()
            .find(|program| program.file_name().map(std::ffi::OsStr::new) == called);
        if let Some(program) = named {
            state.note_lock_refused(program.spelled().to_owned());
        }
    }

    /// Whether `client` may use `protocol` under the current rules.
    pub(crate) fn admits(&self, protocol: Protocol, client: &Client) -> bool {
        let Some(state) = client.get_data::<ClientState>() else {
            return false;
        };
        let exe = match state.origin() {
            Origin::Process(process) => process.exe,
            _ => None,
        };
        let file = state.file();
        let exe = exe.as_deref().map(|path| Exe {
            path,
            here: file.here,
            installed: file.installed,
        });
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .admits(protocol, exe)
    }
}

impl crate::state::Compositor {
    /// Put new rules in force, and withdraw what they no longer allow from
    /// the clients that hold it.
    pub(crate) fn set_access(&mut self, access: Access) {
        // What a shell is told of a window rests on the handles its taskbar
        // and its pager hold, which a narrowed rule takes back.
        self.shells.windows_changed();
        for protocol in self.gate.set(access) {
            match protocol {
                Protocol::Workspace => self.revoke_pager(),
                Protocol::OutputManagement => self.revoke_displays(),
                Protocol::Shell => self.revoke_shell(),
                Protocol::Settings => self.revoke_settings(),
                // The lock holding the session keeps it: taken back, the
                // session would stay locked with nobody to unlock it. Only a
                // lock asked for from now on is asked the rule again.
                Protocol::SessionLock => {}
                #[cfg(feature = "capture")]
                Protocol::Screencopy => self.revoke_screencopy(),
                _ => self.revoke_toplevels(protocol),
            }
        }
    }
}

/// A global's data: which protocol it is, and the rules to ask. What its
/// `can_view` reads.
#[derive(Debug, Clone)]
pub(crate) struct Filtered {
    pub(crate) gate: Gate,
    pub(crate) protocol: Protocol,
}

impl Filtered {
    pub(crate) fn admits(&self, client: &Client) -> bool {
        self.gate.admits(self.protocol, client)
    }
}
