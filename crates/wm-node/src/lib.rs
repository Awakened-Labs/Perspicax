//! The node schema.
//!
//! Two rules govern this crate, and both are about restraint.
//!
//! **We do not define a node schema.** [`accesskit`] already is one, and GTK
//! 4.20 ships a `GtkAccessKitContext` behind `GTK_A11Y=accesskit` -- so these
//! are the types the toolkit itself will eventually hand us, and adopting them
//! makes the fast path a transport swap instead of a translation layer we own
//! forever. Roles, actions and node contents are re-exported below unchanged.
//!
//! **We add exactly two things**, because they are exactly the two an
//! accessibility bridge cannot know and a compositor cannot avoid knowing:
//! [`Origin`] (who drew this) and [`Visibility`] (can it actually be seen).
//! Everything this project claims over a library that scrapes AT-SPI from
//! outside reduces to those two fields being present and honest.

/// The upstream schema, re-exported so downstream crates need not depend on
/// `accesskit` directly and cannot drift onto a different version of it.
///
/// [`Rect`] is in this list for the same reason as everything else in it: a
/// geometry type defined next to a node schema that already ships one is a
/// parallel schema wearing a different hat. Note its shape before using it --
/// it is kurbo-derived, so it is two corners (`x0`, `y0`, `x1`, `y1`) and not
/// an origin plus a size.
///
/// [`Toggled`] and [`Orientation`] joined the list when `wm-atspi` needed to
/// project AT-SPI's state bits onto a node: a tri-state checkbox is
/// `Toggled::Mixed` and not a `bool`, and inventing either type locally would
/// be the same parallel-schema mistake in miniature.
///
/// [`Vec2`] joined it for `wm-index`'s host facts, which need to express the
/// offset between two coordinate spaces. A displacement is not a position, and
/// the type that already ships beside [`Rect`] says so in its name.
pub use accesskit::{Action, Node, NodeId, Orientation, Rect, Role, Toggled, Vec2};

/// A compositor surface. Opaque, and meaningful only to the [`HostView`] that
/// minted it.
///
/// [`HostView`]: ../wm_index/trait.HostView.html
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurfaceId(pub u64);

/// Who drew a node.
///
/// Stamped by the compositor from the Wayland client's credentials, never by
/// the application's own claim about itself -- that is the whole point. A
/// universal screen API makes every rendered pixel an instruction channel, and
/// the only defence is being able to say which process authored a given string
/// before an agent is allowed to believe it.
///
/// `Unattributed` is a real state, not a placeholder to be papered over: it is
/// what an ingest path reports before a compositor has joined its nodes to a
/// surface. Acting on an unattributed node is refused.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Origin {
    /// No compositor has attributed this node yet.
    #[default]
    Unattributed,
    /// Attributed to a process the compositor holds credentials for.
    Process(Box<ProcessOrigin>),
}

/// The provenance of an attributed node.
///
/// Boxed inside [`Origin`] deliberately: the overwhelming majority of nodes in
/// a tree share one origin, and a large enum variant would be paid per node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOrigin {
    /// From the Wayland client's own credentials, not from anything it said.
    pub pid: u32,
    /// Resolved from `/proc/<pid>/exe`, when it is still readable.
    pub exe: Option<String>,
    /// Resolved from `/proc/<pid>/cgroup`. The handle a policy actually wants
    /// when the process is one of many inside a container or a user service.
    pub cgroup: Option<String>,
    /// Flatpak or Snap identity, when the process carries one. A sandboxed app
    /// has a stabler name than its executable path.
    pub sandbox: Option<String>,
}

/// Whether a node can actually be seen.
///
/// The distinction this type exists to draw is between "the application says
/// it has this" and "a human looking at the screen could point at it". Every
/// tool that reads an accessibility tree from outside reports the former and
/// silently implies the latter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Visibility {
    /// No compositor has judged this node yet. Not actable.
    #[default]
    Unknown,
    /// On screen and unobstructed.
    Visible,
    /// Covered by another surface. Carries the culprit, because an agent that
    /// is told *what* is in the way can raise it and retry, whereas one told
    /// only "no" can only guess.
    Occluded { by: SurfaceId },
    /// Inside its own window but scrolled or clipped out of the viewport --
    /// the application's doing, not another window's, and so a different
    /// remedy: scroll it into view rather than raise anything.
    Clipped,
    /// Its own surface is not mapped -- a hidden window, a closed menu.
    Unmapped,
}

impl Visibility {
    /// Whether an agent may act on a node in this state.
    ///
    /// Only [`Visible`] qualifies, and `Unknown` deliberately does not: an
    /// un-judged node is indistinguishable from an occluded one, and defaulting
    /// an unknown to actable would quietly restore the failure mode this whole
    /// project exists to remove.
    ///
    /// [`Visible`]: Visibility::Visible
    #[must_use]
    pub fn is_actable(&self) -> bool {
        matches!(self, Self::Visible)
    }
}

/// A node as this system knows it: the upstream node, plus the two answers
/// only a compositor can give.
#[derive(Debug, Clone)]
pub struct ObservedNode {
    /// The provider-local identifier, unique only within `surface`.
    pub id: NodeId,
    /// The surface this node was drawn on, once a host has joined it.
    pub surface: Option<SurfaceId>,
    /// The upstream node: role, name, value, bounds, supported actions.
    pub node: Node,
    /// Who drew it.
    pub origin: Origin,
    /// Whether it can be seen.
    pub visibility: Visibility,
}

impl ObservedNode {
    /// A node from an ingest path that has not yet been joined to a surface.
    ///
    /// This is the honest starting state for every node `wm-atspi` produces:
    /// the accessibility bus knows the role and the name, and knows nothing
    /// whatsoever about origin or visibility.
    #[must_use]
    pub fn unjoined(id: NodeId, node: Node) -> Self {
        Self {
            id,
            surface: None,
            node,
            origin: Origin::Unattributed,
            visibility: Visibility::Unknown,
        }
    }

    /// The node's bounds, **window-relative**, if the ingest path supplied any.
    ///
    /// The coordinate space is the entire point of this accessor existing
    /// rather than callers reaching through to [`Node::bounds`] themselves. An
    /// accessibility bridge reports what the toolkit believes, and a Wayland
    /// client cannot know where it sits on screen -- so these numbers are
    /// window-relative at best and meaningless at worst. Treating them as
    /// global is the single easiest way to build a system that appears to work
    /// and silently clicks the wrong place. Only a `HostView` may turn them
    /// into anything global, because only a compositor knows a window's origin.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        self.node.bounds()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_visible_nodes_are_actable() {
        assert!(Visibility::Visible.is_actable());
        assert!(!Visibility::Occluded { by: SurfaceId(1) }.is_actable());
        assert!(!Visibility::Clipped.is_actable());
        assert!(!Visibility::Unmapped.is_actable());
    }

    /// The default must be un-actable. An ingest path that forgets to set
    /// visibility has to fail closed, because the alternative -- an unjudged
    /// node being clickable -- is precisely the bug this project exists to fix.
    #[test]
    fn an_unjudged_node_is_not_actable() {
        assert!(!Visibility::default().is_actable());
        assert!(
            !ObservedNode::unjoined(NodeId(1), Node::new(Role::Button))
                .visibility
                .is_actable()
        );
    }

    #[test]
    fn an_unjoined_node_is_unattributed() {
        let n = ObservedNode::unjoined(NodeId(7), Node::new(Role::Button));
        assert_eq!(n.origin, Origin::Unattributed);
        assert!(n.surface.is_none());
    }
}
