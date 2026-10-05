//! The agent interface -- an MCP server over stdio.
//!
//! Eight tools: `window_list`, `observe`, `resolve`, `act`, `window_close`,
//! `tab_forward`, `deltas` and `screenshot`. Everything above them is `rmcp`
//! and everything below them is [`Desktop`], a trait with five methods that
//! this crate never implements.
//!
//! # What this crate can and cannot see
//!
//! It depends on `perspicax-index` and `perspicax-node`, and on nothing else in
//! the workspace. No Smithay, no D-Bus, no compositor. That is the same
//! restraint `perspicax-index` observes and it is load-bearing for the same
//! reason: the agent interface is portable, so a port of this project to a
//! GNOME extension or a KWin plugin re-implements [`Desktop`] and gets these
//! eight tools unchanged. A crate that cannot see a compositor cannot come to
//! depend on one.
//!
//! The consequence is that the act path is **injected, not imported**.
//! `perspicax`'s `act` module is the composition root of a click -- it is the
//! only place that holds the index, the host and a clock at once -- and it
//! lives in `perspicax` because waiting 200 ms for damage is I/O and
//! `perspicax-index` does none. This crate calls it through [`Desktop::act`]
//! and does not know what is on the other side.
//!
//! # Two rules the tools keep
//!
//! **Receipts, not booleans.** An act returns what happened: which node it
//! resolved to, when it was dispatched and how long that took, focus before and
//! after, and what the pixels did in the window it was given. An agent handed
//! `true` has learned that a function returned, which is not what it asked.
//!
//! **The gate is not here.** [`perspicax_index::check_actable`] and the
//! host's [`perspicax_index::Consent`] are consulted in `perspicax-index`, so
//! that a future host speaking some other protocol cannot route around them.
//! This crate reports what the gate said and adds nothing on top of it.
//! Headless, consent is everyone: perspicax is one actuator among several, and
//! a boundary an agent could walk around by running a command would invite the
//! reliance it cannot support. On `--seat`, where the person launched most of
//! what is on screen, consent covers only what perspicax spawned, and
//! [`perspicax_index::Refusal::NoCapability`] is what an agent hears about the
//! rest.

pub mod dto;
mod gate;
mod server;

#[cfg(test)]
mod fixture;

use std::sync::Arc;

use perspicax_index::{
    Delta, HostFacts, Index, Receipt, Refusal, Selector, Shot, ShotTarget, Verb, WindowReceipt,
    WindowVerb,
};
use perspicax_node::SurfaceId;

pub use crate::server::{Perspicax, ScreenshotParams};

/// What an MCP server needs from the process hosting it.
///
/// Five methods, and the split between them is the crate boundary this
/// project's architecture rests on: two questions about the past, which any
/// reader can answer from a snapshot, and three things only the thread that
/// owns the compositor can do -- two acts and a picture.
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

    /// Close a window or bring a tab forward, and report what became of it.
    ///
    /// Blocks, like [`Desktop::act`], for long enough to see the answer.
    ///
    /// # Errors
    ///
    /// As [`Desktop::act`].
    fn act_window(&self, surface: SurfaceId, verb: WindowVerb) -> Result<WindowReceipt, Denied>;

    /// Take a picture of a monitor or a window.
    ///
    /// # Errors
    ///
    /// [`Denied::Refused`] for a window the gate will not let the agent read,
    /// [`Denied::NotBuilt`] from a build that cannot take pictures, and
    /// [`Denied::Undispatched`] when the compositor could not.
    fn capture(&self, target: ShotTarget) -> Result<Shot, Denied>;
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
    /// This build has no such capability: the cargo feature named.
    #[error("not built: this perspicax has no `{0}` feature")]
    NotBuilt(String),
}

/// Why the server stopped.
///
/// Both variants carry a message rather than the underlying error, for the same
/// reason [`Denied::Undispatched`] does: `rmcp`'s error types are its own, and
/// re-exporting them here would put this crate's callers on its release
/// schedule for no benefit they can act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServeError {
    /// stdio could not be turned into a transport, or the handshake itself
    /// failed. Not a client speaking out of turn before it: that is answered
    /// and the server waits on (see [`serve`]).
    #[error("the MCP transport could not be established: {0}")]
    Transport(String),
    /// The conversation ended badly. A client that simply closed its pipe is
    /// not this -- that is an ordinary end and returns `Ok`.
    #[error("the MCP server stopped: {0}")]
    Stopped(String),
}

/// Serve the eight tools over stdio until the client goes away.
///
/// # A conversation begins with `initialize`
///
/// Until it does, a request that cannot open one is answered with an error
/// naming what to send first, and anything that is not a request is dropped --
/// and either way the server goes on waiting. rmcp on its own gives up on the
/// transport instead, and over stdio there is no second one: a single message
/// sent out of turn would have ended the agent interface, and with it, in
/// `perspicax`, the desktop it serves. Issue #26.
///
/// # stdout is the wire
///
/// Every byte written to stdout by anything other than this transport corrupts
/// a JSON-RPC frame, and the symptom arrives at the client as a parse error a
/// long way from its cause -- so `tracing` must be initialised with
/// `.with_writer(std::io::stderr)` before this is called.
///
/// It is worth being exact about the state of that, because the failure is
/// silent: `perspicax`'s binary initialises `tracing` to **stdout**, which is
/// right while nothing serves this over stdio and is the first thing a `--mcp`
/// flag has to change. Any other host of this server owes the same line.
///
/// # Errors
///
/// [`ServeError`] if the transport could not be established or the conversation
/// ended badly. A client that closes its pipe, or cancels, returns `Ok(())` --
/// both are ordinary ends to a conversation.
pub async fn serve(desktop: Arc<dyn Desktop>) -> Result<(), ServeError> {
    serve_over(desktop, rmcp::transport::stdio()).await
}

/// [`serve`], over any transport rather than this process's stdio.
///
/// `serve` is this with stdin and stdout, and nothing else: a test speaking the
/// wire down a `tokio::io::duplex` pair goes the same way a client does.
///
/// # Errors
///
/// As [`serve`].
pub async fn serve_over<T, E, A>(desktop: Arc<dyn Desktop>, transport: T) -> Result<(), ServeError>
where
    T: rmcp::transport::IntoTransport<rmcp::RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    use rmcp::ServiceExt as _;
    use rmcp::service::{QuitReason, ServerInitializeError};

    let service = match Perspicax::new(desktop)
        .serve(gate::Gated::new(transport.into_transport()))
        .await
    {
        Ok(service) => service,
        // Gone before saying anything worth answering: the same ordinary end as
        // a client that closes its pipe after a conversation.
        Err(ServerInitializeError::ConnectionClosed(_) | ServerInitializeError::Cancelled) => {
            return Ok(());
        }
        Err(error) => return Err(ServeError::Transport(error.to_string())),
    };

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
