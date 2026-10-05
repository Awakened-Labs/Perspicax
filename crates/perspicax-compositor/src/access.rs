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
//! accepted (`crate::origin`), never asked for again. Xwayland's own
//! connection carries no such record and is never admitted: an X11 program
//! reaching other windows through it would be a program nobody named.

use std::sync::{Arc, PoisonError, RwLock};

use perspicax_node::Origin;
use perspicax_policy::{Access, Protocol};
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

    /// Whether `client` may use `protocol` under the current rules.
    pub(crate) fn admits(&self, protocol: Protocol, client: &Client) -> bool {
        let Some(state) = client.get_data::<ClientState>() else {
            return false;
        };
        let exe = match state.origin() {
            Origin::Process(process) => process.exe,
            _ => None,
        };
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .admits(protocol, exe.as_deref())
    }
}

impl crate::state::Compositor {
    /// Put new rules in force, and withdraw what they no longer allow from
    /// the clients that hold it.
    pub(crate) fn set_access(&mut self, access: Access) {
        for protocol in self.gate.set(access) {
            match protocol {
                Protocol::Workspace => self.revoke_pager(),
                Protocol::OutputManagement => self.revoke_displays(),
                Protocol::Shell => self.revoke_shell(),
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
