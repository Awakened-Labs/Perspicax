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

use wm_node::{Node, NodeId, ObservedNode, Origin, SurfaceId, Visibility};

/// A rectangle in some coordinate space the sender and receiver agree on.
///
/// Which space that is matters more here than the arithmetic does. Accessibility
/// bridges report bounds that a Wayland client cannot compute correctly -- a
/// client does not know its own position on screen -- so bounds crossing this
/// boundary are **window-relative**, and only a [`HostView`] may turn them into
/// anything global. Nothing in this crate assumes otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

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
pub trait Ingest {
    /// The implementation's own error type -- D-Bus failures for an AT-SPI
    /// bridge, protocol errors for a Wayland one.
    type Error;

    /// Read a subtree in as few round trips as the source permits.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the underlying source cannot be read.
    fn snapshot(&mut self, root: NodeId) -> Result<Vec<ObservedNode>, Self::Error>;

    /// Drain whatever the source has volunteered since the last call.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the underlying source cannot be read.
    fn drain_changes(&mut self) -> Result<Vec<Change>, Self::Error>;
}

/// A change the ingest path volunteered.
#[derive(Debug, Clone)]
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
}
