//! What an agent reads of the shell: one accessibility tree per surface,
//! served on the accessibility bus by AccessKit.
//!
//! Each tree's root is a `Window` named after its surface's layer namespace,
//! `perspicax-desktop-DP-1`. perspicax joins an accessibility window to a
//! surface by its title, and a layer surface has none, so the namespace
//! stands in for it. Naming the window the same is what lets one process's
//! wallpaper, panel and menu be told apart, each joined to its own surface.
//!
//! Bounds are in the surface's own logical pixels, from its top-left corner:
//! all a Wayland client knows of where it is. perspicax reads them relative
//! to the window, and places them on the screen with what only the
//! compositor knows.
//!
//! Each tree is built here from what the surface shows, as a value, and
//! handed to [`adapter`] to serve.

pub(crate) mod adapter;

use accesskit::{Node, NodeId, Rect, Role, TreeId, TreeInfo, TreeUpdate};

/// Every tree's root: the surface itself.
const ROOT: NodeId = NodeId(0);

/// A monitor's desktop: one window covering it, named after its namespace.
/// Its size is `None` until the compositor has said what it is.
pub(crate) fn desktop(namespace: &str, size: Option<(u32, u32)>) -> TreeUpdate {
    let mut root = Node::new(Role::Window);
    root.set_label(namespace);
    if let Some((width, height)) = size {
        root.set_bounds(Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
    }
    TreeUpdate {
        nodes: vec![(ROOT, root)],
        tree: Some(TreeInfo::new(ROOT)),
        tree_id: TreeId::ROOT,
        focus: ROOT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(tree: &TreeUpdate) -> &Node {
        let root = tree.tree.as_ref().expect("a whole tree").root;
        &tree
            .nodes
            .iter()
            .find(|(id, _)| *id == root)
            .expect("the root is in it")
            .1
    }

    #[test]
    fn the_desktop_is_a_window_named_after_its_namespace() {
        let tree = desktop("perspicax-desktop-DP-1", Some((1920, 1080)));
        assert_eq!(root(&tree).role(), Role::Window);
        assert_eq!(root(&tree).label(), Some("perspicax-desktop-DP-1"));
        assert_eq!(tree.nodes.len(), 1, "nothing on it yet");
    }

    #[test]
    fn node_bounds_are_surface_local() {
        // A second monitor, to the right of a first: the tree knows nothing
        // of where it sits, only how big it is.
        let tree = desktop("perspicax-desktop-HDMI-A-1", Some((2560, 1440)));
        assert_eq!(
            root(&tree).bounds(),
            Some(Rect::new(0.0, 0.0, 2560.0, 1440.0))
        );
        assert_eq!(
            root(&desktop("perspicax-desktop-DP-1", None)).bounds(),
            None,
            "and no size before the compositor gives one"
        );
    }
}
