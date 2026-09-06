//! The agent interface -- an MCP server over stdio.
//!
//! Six tools: `window_list`, `observe`, `resolve`, `act`, `deltas` and
//! `screenshot`. Everything above them is `rmcp` and everything below them is
//! [`Desktop`], a trait with three methods that this crate never implements.
//!
//! # What this crate can and cannot see
//!
//! It depends on `perspicax-index` and `perspicax-node`, and on nothing else in the
//! workspace. No Smithay, no D-Bus, no compositor. That is the same restraint
//! `perspicax-index` observes and it is load-bearing for the same reason: the agent
//! interface is portable, so a port of this project to a GNOME extension or a
//! KWin plugin re-implements [`Desktop`] and gets these six tools unchanged. A
//! crate that cannot see a compositor cannot come to depend on one.
//!
//! The consequence is that the act path is **injected, not imported**. `perspicax`'s
//! `act` module is the composition root of a click -- it is the only place that
//! holds the index, the host and a clock at once -- and it lives in `perspicax` because
//! waiting 200 ms for damage is I/O and `perspicax-index` does none. This crate calls
//! it through [`Desktop::act`] and does not know what is on the other side.
//!
//! # Two rules the tools keep
//!
//! **Receipts, not booleans.** An act returns what happened: which node it
//! resolved to, when it was dispatched and how long that took, focus before and
//! after, and what the pixels did in the window it was given. An agent handed
//! `true` has learned that a function returned, which is not what it asked.
//!
//! **The gate is not here.** [`perspicax_index::check_actable`] lives in `perspicax-index` so
//! that a future host speaking some other protocol cannot route around it. This
//! crate reports what the gate said and adds nothing on top of it -- there is no
//! capability layer in v1, deliberately and for reasons written down in the
//! plan: perspicax is one actuator among several, an agent that is refused a click can
//! run a command instead, and a boundary that can be walked around invites the
//! reliance it cannot support. [`perspicax_index::Refusal::NoCapability`] stays
//! declared and unconstructed until `--seat`, where perspicax will host applications it
//! did not spawn and the question finally has two answers.

pub mod dto;
mod server;

#[cfg(test)]
mod fixture;

use std::sync::Arc;

use perspicax_index::{Delta, HostFacts, Index, Receipt, Refusal, Selector, Verb};

pub use crate::server::Perspicax;

/// What an MCP server needs from the process hosting it.
///
/// Three methods, and the split between them is the crate boundary this
/// project's architecture rests on: two questions about the past, which any
/// reader can answer from a snapshot, and one act, which only the thread that
/// owns the compositor can carry out.
///
/// `Send + Sync + 'static` because the server is handed round an async runtime
/// and its tools run on the blocking pool. Implementors are expected to be
/// cheap to share -- an `Arc` over a lock, not a value that gets cloned.
pub trait Desktop: Send + Sync + 'static {
    /// Read the index and the host's published facts as one consistent view.
    ///
    /// A visitor rather than a return, because the implementor owns the
    /// locking: an `Index` behind a `Mutex` cannot hand out a reference that
    /// outlives its guard, and forcing a clone on every read to work around
    /// that would make the cheap tools expensive.
    ///
    /// # Contract
    ///
    /// `visit` must be called **exactly once**. A host that skips it is saying
    /// the desktop cannot be read, which is not one of the answers -- an empty
    /// [`Index`] and empty [`HostFacts`] say that perfectly well and let every
    /// tool keep working.
    fn read(&self, visit: &mut dyn FnMut(&Index, &HostFacts));

    /// Take everything that has changed since the last call.
    ///
    /// Draining, not reading: a subscriber told twice about one change cannot
    /// tell that it happened once. See [`Index::take_deltas`].
    fn deltas(&self) -> Vec<Delta>;

    /// Resolve a selector, act on what it names, and report what happened.
    ///
    /// **This blocks**, for the length of the damage window and then some, so
    /// the server calls it on a blocking task. An implementation that returned
    /// before the pixels had a chance to move would be handing back a receipt
    /// with its most interesting field guessed.
    ///
    /// # Errors
    ///
    /// [`Denied::Refused`] when the gate said no, [`Denied::Undispatched`] when
    /// the compositor could not carry it out.
    fn act(&self, selector: &Selector, verb: &Verb) -> Result<Receipt, Denied>;
}

/// Why an act produced no receipt.
///
/// Two genuinely different failures, kept apart because an agent can do
/// something about one of them. A [`Refusal`] is a statement about the target --
/// it is covered, it is stale, the selector matched three things -- and each
/// variant carries what recovery would need. The other is a statement about the
/// compositor, and there is nothing to do with it but report it, which is why it
/// arrives as a message rather than a type: the type it came from belongs to a
/// crate this one deliberately cannot see.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Denied {
    /// The gate said no, and said why.
    #[error("refused: {0}")]
    Refused(#[from] Refusal),
    /// The compositor could not carry it out.
    #[error("not dispatched: {0}")]
    Undispatched(String),
}

/// Why the server stopped.
///
/// Both variants carry a message rather than the underlying error, for the same
/// reason [`Denied::Undispatched`] does: `rmcp`'s error types are its own, and
/// re-exporting them here would put this crate's callers on its release
/// schedule for no benefit they can act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServeError {
    /// stdio could not be turned into a transport, or the client never
    /// completed the handshake.
    #[error("the MCP transport could not be established: {0}")]
    Transport(String),
    /// The conversation ended badly. A client that simply closed its pipe is
    /// not this -- that is an ordinary end and returns `Ok`.
    #[error("the MCP server stopped: {0}")]
    Stopped(String),
}

/// Serve the six tools over stdio until the client goes away.
///
/// # stdout is the wire
///
/// Every byte written to stdout by anything other than this transport corrupts
/// a JSON-RPC frame, and the symptom arrives at the client as a parse error a
/// long way from its cause -- so `tracing` must be initialised with
/// `.with_writer(std::io::stderr)` before this is called.
///
/// It is worth being exact about the state of that, because the failure is
/// silent: `perspicax`'s binary initialises `tracing` to **stdout**, which is right
/// while nothing serves this over stdio and is the first thing a `--mcp` flag
/// has to change. Any other host of this server owes the same line.
///
/// # Errors
///
/// [`ServeError`] if the transport could not be established or the conversation
/// ended badly. A client that closes its pipe, or cancels, returns `Ok(())` --
/// both are ordinary ends to a conversation.
pub async fn serve(desktop: Arc<dyn Desktop>) -> Result<(), ServeError> {
    use rmcp::ServiceExt as _;
    use rmcp::service::QuitReason;

    let service = Perspicax::new(desktop)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| ServeError::Transport(error.to_string()))?;

    match service.waiting().await {
        Ok(QuitReason::Closed | QuitReason::Cancelled) => Ok(()),
        // `QuitReason` is `#[non_exhaustive]` and one of the variants it
        // already has is a panicked service task, which arrives here as an
        // `Ok`. Anything this does not recognise as an ordinary end is
        // therefore reported rather than assumed to be one.
        Ok(reason) => Err(ServeError::Stopped(format!("{reason:?}"))),
        Err(error) => Err(ServeError::Stopped(error.to_string())),
    }
}
