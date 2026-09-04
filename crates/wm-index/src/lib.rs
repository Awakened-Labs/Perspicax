//! The semantic index -- the portable core.
//!
//! Everything above this crate (the MCP server) and everything below it (an
//! accessibility bridge, a compositor) is replaceable. This is not, so it is
//! the crate that must not acquire opinions about transport. It imports
//! `wm-node` and nothing else.
//!
//! Two traits describe the replaceable halves:
//!
//! - [`Ingest`] -- where nodes come from. AT-SPI2 over D-Bus today; a private
//!   Wayland protocol carrying AccessKit nodes later. The index does not care.
//! - [`HostView`] -- what only a compositor knows: is this rect visible, who
//!   owns this surface, dispatch this input. Smithay today; a GNOME extension
//!   or a KWin plugin later. The index does not care about that either.
//!
//! The refusal gate ([`Refusal`], [`check_actable`]) lives here rather than in
//! the MCP server, and that placement is deliberate: a future host that talks
//! to agents some other way must not be able to route around it.
//!
//! Between those two traits sit the three things that make a tree of nodes
//! addressable:
//!
//! - [`Interner`] -- an ingest path's own keys in, opaque [`NodeId`]s out, and
//!   never the same id twice.
//! - [`Selector`] -- the addressing scheme a human or a model can write down,
//!   as opposed to one minted at runtime.
//! - [`Index`] -- the cache, the tree order both of the above depend on, and
//!   the [`Delta`] computation that keeps a subscription from being a poll.

pub mod cache;
pub mod id;
pub mod selector;

use core::future::Future;

use wm_node::{Node, NodeId, ObservedNode, Origin, Rect, SurfaceId, Visibility};

pub use crate::{
    cache::{Delta, Index},
    id::Interner,
    selector::{Selector, SelectorParseError},
};

/// Why an agent was not allowed to act.
///
/// Refusals are a feature and not an error path. Each variant carries what the
/// agent would need to recover on its own -- the occluding surface so it can
/// raise it, the match count so it can narrow a selector -- because a refusal
/// that only says "no" converts a recoverable situation into a retry loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The node is real but covered. Raise `by` and try again.
    Occluded { by: SurfaceId },
    /// Inside its window but scrolled out of view. Scroll it in and try again.
    Clipped,
    /// Its surface is not mapped -- a hidden window, an unopened menu.
    Unmapped,
    /// No compositor has judged this node's visibility. Fails closed.
    Unjudged,
    /// No compositor has attributed this node to a process, so no capability
    /// decision can be made about it. Fails closed.
    Unattributed,
    /// The node's surface has changed since this node was read, and the index
    /// has not caught up. Acting on it would be acting on the past.
    Stale { frames: u32 },
    /// The agent holds no capability for this node's origin.
    NoCapability { origin: Box<Origin> },
    /// The selector matched more than one node. Carries the count so the agent
    /// can decide whether to narrow it or enumerate.
    AmbiguousSelector { matches: usize },
    /// The selector matched nothing.
    NotFound,
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Occluded { by } => write!(f, "node occluded by surface {}", by.0),
            Self::Clipped => write!(f, "node is scrolled out of its own viewport"),
            Self::Unmapped => write!(f, "node's surface is not mapped"),
            Self::Unjudged => write!(f, "node visibility has not been judged by a compositor"),
            Self::Unattributed => write!(f, "node has no attributed origin"),
            Self::Stale { frames } => write!(f, "node is stale by {frames} frame(s)"),
            Self::NoCapability { origin } => write!(f, "no capability for origin {origin:?}"),
            Self::AmbiguousSelector { matches } => write!(f, "selector matched {matches} nodes"),
            Self::NotFound => write!(f, "selector matched no nodes"),
        }
    }
}

impl core::error::Error for Refusal {}

/// The gate. Every act path goes through here.
///
/// Both conditions fail closed, and both defaults are un-actable, so an ingest
/// or host implementation that simply forgets to fill a field cannot produce a
/// clickable node by omission.
///
/// # Errors
///
/// Returns the [`Refusal`] describing why the node may not be acted on.
pub fn check_actable(node: &ObservedNode) -> Result<(), Refusal> {
    match &node.origin {
        Origin::Unattributed => return Err(Refusal::Unattributed),
        Origin::Process(_) => {}
    }
    match &node.visibility {
        Visibility::Visible => Ok(()),
        Visibility::Occluded { by } => Err(Refusal::Occluded { by: *by }),
        Visibility::Clipped => Err(Refusal::Clipped),
        Visibility::Unmapped => Err(Refusal::Unmapped),
        Visibility::Unknown => Err(Refusal::Unjudged),
    }
}

/// Where nodes come from.
///
/// Implementations are expected to be *push*-shaped wherever the underlying
/// source allows it. Polling a whole tree is the failure mode this project was
/// started to fix, not a fallback to reach for.
///
/// # Why this trait is async, and why that costs this crate nothing
///
/// Every real source of nodes is asynchronous. AT-SPI2 is D-Bus, which the
/// `atspi` crate exposes over zbus; the eventual fast path is a Wayland
/// protocol. Making implementations hide that behind a blocking facade would
/// mean burying a runtime inside a library and, worse, would put a *polling*
/// shape on `drain_changes` -- the exact thing the paragraph above rejects.
///
/// Being async does not compromise this crate's portability, because
/// [`Future`] lives in `core`: there is no runtime dependency here, and there
/// is not going to be one. The runtime belongs to whoever drives the trait.
///
/// # Why `impl Future + Send` rather than `async fn`
///
/// Written as a bare `async fn`, the returned future carries no auto-trait
/// bounds, so a caller that wants to `spawn` the driver onto a multi-threaded
/// runtime cannot ask for `Send` and gets a famously indirect error instead.
/// Spelling the bound here makes it an obligation of the implementation, which
/// is where it can actually be satisfied -- zbus proxies are `Send`, so this
/// costs the AT-SPI path nothing, and it fails loudly at the definition rather
/// than quietly at the first `tokio::spawn`.
pub trait Ingest {
    /// The implementation's own error type -- D-Bus failures for an AT-SPI
    /// bridge, protocol errors for a Wayland one.
    type Error;

    /// Read a subtree in as few round trips as the source permits.
    ///
    /// "As few as the source permits" is doing real work in that sentence, and
    /// what a source permits is not what it advertises. A warm GTK application
    /// answers in one `Cache.GetItems` call; the Qt widget gallery offers the
    /// same interface and answers it with an empty array; a cold GTK one
    /// answers with a fraction of its tree. All three must be walked node by
    /// node to get an answer, and only the first can avoid it. Every one of
    /// those is a legitimate implementation of this method, and the distance
    /// between them is the measurement the project turns on.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the underlying source cannot be read.
    fn snapshot(
        &mut self,
        root: NodeId,
    ) -> impl Future<Output = Result<Vec<ObservedNode>, Self::Error>> + Send;

    /// Drain whatever the source has volunteered since the last call.
    ///
    /// Volunteered, not fetched. An implementation that answers this by going
    /// and looking has misread the trait.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the underlying source cannot be read.
    fn drain_changes(&mut self) -> impl Future<Output = Result<Vec<Change>, Self::Error>> + Send;
}

/// A change the ingest path volunteered.
///
/// `PartialEq` because a change is a value, and two of them being equal is a
/// question worth asking -- an ingest path that reports the same change twice
/// has turned a delta stream back into a poll, and a test can only say so if
/// changes compare.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A node appeared or its contents changed.
    Upserted { id: NodeId, node: Box<Node> },
    /// A node went away.
    Removed { id: NodeId },
    /// A subtree changed shape and must be re-read.
    SubtreeInvalidated { root: NodeId },
}

/// What only a compositor knows.
///
/// The three questions here are the entire reason the semantic index lives
/// inside a compositor rather than beside one. Nothing else in this crate is
/// unusual; this trait is the argument.
pub trait HostView {
    /// The implementation's own error type.
    type Error;

    /// Who owns this surface. From the client's credentials, never its claims.
    fn origin(&self, surface: SurfaceId) -> Origin;

    /// Whether a window-relative rect on this surface can actually be seen,
    /// accounting for z-order, the opaque regions of everything above it, and
    /// whether the surface is mapped at all.
    fn visibility(&self, surface: SurfaceId, rect: Rect) -> Visibility;

    /// How many frames of damage this surface has taken that the index has not
    /// yet reconciled. Non-zero means any node read from it is stale.
    fn staleness(&self, surface: SurfaceId) -> u32;

    /// Dispatch input through the compositor's own input path, so focus,
    /// grabs and z-order stay correct by construction.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the input could not be dispatched.
    fn dispatch(&mut self, surface: SurfaceId, action: &Action) -> Result<(), Self::Error>;
}

/// An input action to be dispatched at a node.
///
/// Deliberately not AccessKit's `Action`: that describes what a *widget*
/// supports ("this is a button, it can be invoked"), whereas this describes
/// what a *seat* does ("move the pointer here and press button 1"). Going
/// through real input rather than an accessibility bridge's `DoAction` is what
/// keeps focus and grabs honest, so the two vocabularies stay separate.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Move the pointer to a point and click it.
    Click { at: Rect, button: PointerButton },
    /// Type text through the seat's keyboard, xkb-mapped.
    Type { text: String },
    /// Scroll at a point.
    Scroll { at: Rect, dx: f64, dy: f64 },
    /// Give this surface keyboard focus.
    Focus,
}

/// A pointer button, in the usual left/middle/right sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
}

#[cfg(test)]
mod tests {
    use core::{
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use super::*;
    use wm_node::{ProcessOrigin, Role};

    fn attributed() -> Origin {
        Origin::Process(Box::new(ProcessOrigin {
            pid: 9182,
            exe: Some("/usr/bin/gedit".into()),
            cgroup: None,
            sandbox: None,
        }))
    }

    fn node_with(origin: Origin, visibility: Visibility) -> ObservedNode {
        let mut n = ObservedNode::unjoined(NodeId(1), Node::new(Role::Button));
        n.origin = origin;
        n.visibility = visibility;
        n
    }

    #[test]
    fn a_visible_attributed_node_is_actable() {
        assert!(check_actable(&node_with(attributed(), Visibility::Visible)).is_ok());
    }

    #[test]
    fn an_occluded_node_names_what_covers_it() {
        let err = check_actable(&node_with(
            attributed(),
            Visibility::Occluded { by: SurfaceId(7) },
        ))
        .unwrap_err();
        assert_eq!(err, Refusal::Occluded { by: SurfaceId(7) });
        assert_eq!(err.to_string(), "node occluded by surface 7");
    }

    /// The two fail-closed cases, which are the ones that matter. A node
    /// straight off an ingest path has neither field set, and must be refused
    /// on both counts rather than sliding through on a default.
    #[test]
    fn an_unjoined_node_fails_closed() {
        let raw = ObservedNode::unjoined(NodeId(1), Node::new(Role::Button));
        assert_eq!(check_actable(&raw).unwrap_err(), Refusal::Unattributed);
    }

    #[test]
    fn attribution_alone_is_not_enough() {
        let judged_by_nobody = node_with(attributed(), Visibility::Unknown);
        assert_eq!(
            check_actable(&judged_by_nobody).unwrap_err(),
            Refusal::Unjudged
        );
    }

    /// A minimal executor, so this crate can exercise its own async seam
    /// without acquiring a runtime -- not even as a dev-dependency. The mock
    /// below never suspends, so one poll always completes it; reaching
    /// `Pending` would mean it had grown a suspension point it is not supposed
    /// to have, which is worth a panic rather than a spin.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => unreachable!("MockIngest must not suspend"),
        }
    }

    /// The smallest thing that can satisfy [`Ingest`]. It exists to hold the
    /// trait's shape still: that it is implementable with a plain `async fn`,
    /// and that what comes back is `Send`.
    struct MockIngest {
        nodes: Vec<ObservedNode>,
        changes: Vec<Change>,
    }

    impl Ingest for MockIngest {
        type Error = core::convert::Infallible;

        async fn snapshot(&mut self, _root: NodeId) -> Result<Vec<ObservedNode>, Self::Error> {
            Ok(self.nodes.clone())
        }

        async fn drain_changes(&mut self) -> Result<Vec<Change>, Self::Error> {
            Ok(core::mem::take(&mut self.changes))
        }
    }

    #[test]
    fn the_async_seam_round_trips() {
        let mut ingest = MockIngest {
            nodes: vec![ObservedNode::unjoined(NodeId(1), Node::new(Role::Button))],
            changes: vec![Change::Removed { id: NodeId(1) }],
        };

        assert_eq!(block_on(ingest.snapshot(NodeId(0))).unwrap().len(), 1);
        assert_eq!(block_on(ingest.drain_changes()).unwrap().len(), 1);

        // Drained means drained. A source that re-reports what it has already
        // volunteered turns a delta stream back into a poll.
        assert!(block_on(ingest.drain_changes()).unwrap().is_empty());
    }

    /// A compile-time assertion wearing a test's clothes. If [`Ingest`]'s
    /// futures ever stop being `Send`, this stops building -- which is the
    /// whole reason the bound is spelled on the trait rather than left to
    /// inference at each call site.
    #[test]
    fn ingest_futures_are_send() {
        fn assert_send<T: Send>(_: T) {}

        let mut ingest = MockIngest {
            nodes: Vec::new(),
            changes: Vec::new(),
        };
        assert_send(ingest.snapshot(NodeId(0)));
        assert_send(ingest.drain_changes());
    }
}
