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

use accesskit::{Node, NodeId, Rect, Role};
#[cfg(feature = "wallpaper")]
use accesskit::{TreeId, TreeInfo, TreeUpdate};

/// Every tree's root: the surface itself.
const ROOT: NodeId = NodeId(0);

/// A monitor's desktop: one window covering it, named after its namespace.
/// Its size is `None` until the compositor has said what it is.
#[cfg(feature = "wallpaper")]
pub(crate) fn desktop(namespace: &str, size: Option<(u32, u32)>) -> TreeUpdate {
    TreeUpdate {
        nodes: vec![(ROOT, window(namespace, size))],
        tree: Some(TreeInfo::new(ROOT)),
        tree_id: TreeId::ROOT,
        focus: ROOT,
    }
}

/// A rectangle of a surface, as AccessKit bounds it.
#[cfg(any(feature = "menus", feature = "panel"))]
fn rect(rect: crate::layout::Rect) -> Rect {
    Rect::new(
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.right()),
        f64::from(rect.bottom()),
    )
}

/// A surface's root: a window named `namespace`, covering it.
fn window(namespace: &str, size: Option<(u32, u32)>) -> Node {
    let mut root = Node::new(Role::Window);
    root.set_label(namespace);
    if let Some((width, height)) = size {
        root.set_bounds(Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
    }
    root
}

#[cfg(feature = "menus")]
pub(crate) use menus::{menu, route_of};

#[cfg(feature = "menus")]
mod menus {
    use accesskit::{Action, HasPopup, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};

    use super::{ROOT, rect, window};
    use crate::{
        model::menu::Route,
        update::{View, Which},
    };

    /// An open menu's node: its place in the cascade, below this.
    const MENU: u64 = 1 << 62;
    /// What a person typed.
    const TYPED: NodeId = NodeId(1 << 61);
    /// An item's node: its route below this, each index one more than it
    /// is, twelve bits to an index. Stable while the menu is open, and
    /// across opens while the menu is the same, whatever else is shown.
    const ITEM: u64 = 1 << 60;
    const BITS: usize = 12;
    const DEEPEST: usize = 5;

    /// The node of the item at `route`. An item nested deeper than any
    /// menu goes, or past the four thousandth line, has none.
    fn item(route: &[usize]) -> Option<NodeId> {
        let limit = (1 << BITS) - 1;
        if route.len() > DEEPEST || route.iter().any(|&at| at >= limit) {
            return None;
        }
        let packed = route.iter().enumerate().fold(0, |id, (depth, &at)| {
            id | ((at as u64 + 1) << (BITS * depth))
        });
        Some(NodeId(ITEM | packed))
    }

    /// The route of the item a node is, if it is an item's.
    pub(crate) fn route_of(id: NodeId) -> Option<Route> {
        if id.0 & !(ITEM - 1) != ITEM {
            return None;
        }
        let mut bits = id.0 & (ITEM - 1);
        let mut route = Vec::new();
        while bits != 0 {
            route.push(((bits & ((1 << BITS) - 1)) - 1) as usize);
            bits >>= BITS;
        }
        Some(route)
    }

    /// The menus' surface: a window named `namespace` covering it, holding
    /// a `Menu` for each open menu, each holding a `MenuItem` for each line
    /// that can be chosen, named as drawn. The focus is the line the keyboard
    /// is on, or the root menu.
    pub(crate) fn menu(
        namespace: &str,
        size: Option<(u32, u32)>,
        view: Option<&View<'_>>,
    ) -> TreeUpdate {
        let mut root = window(namespace, size);
        let mut nodes = Vec::new();
        let mut focus = ROOT;
        for (level, menu) in view.iter().flat_map(|view| view.menus.iter().enumerate()) {
            let id = NodeId(MENU | level as u64);
            root.push_child(id);
            if level == 0 && focus == ROOT {
                focus = id;
            }
            let mut node = Node::new(Role::Menu);
            let first = match view.map(|view| view.which) {
                Some(Which::Start) => "Start menu",
                _ => "Root menu",
            };
            node.set_label(menu.opened_by.map_or(first, |item| item.label.as_str()));
            node.set_bounds(rect(menu.rect));
            if let (Some(header), Some(query)) = (menu.header, view.and_then(|view| view.query)) {
                let mut typed = Node::new(Role::SearchInput);
                typed.set_label("Search");
                typed.set_value(query);
                typed.set_bounds(rect(header));
                typed.set_read_only();
                node.push_child(TYPED);
                nodes.push((TYPED, typed));
            }
            for line in &menu.lines {
                let Some(id) = item(line.route).filter(|_| line.item.choosable()) else {
                    continue;
                };
                let mut entry = Node::new(Role::MenuItem);
                entry.set_label(line.item.label.as_str());
                entry.set_bounds(rect(line.rect));
                entry.add_action(Action::Click);
                entry.add_action(Action::Focus);
                if line.item.submenu().is_some() {
                    entry.set_has_popup(HasPopup::Menu);
                    entry.set_expanded(line.open);
                }
                if line.selected {
                    entry.set_selected(true);
                }
                if line.focused {
                    focus = id;
                }
                node.push_child(id);
                nodes.push((id, entry));
            }
            nodes.push((id, node));
        }
        nodes.insert(0, (ROOT, root));
        TreeUpdate {
            nodes,
            tree: Some(TreeInfo::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{
            layout::{self, Monospace},
            model::{
                apps::{App, Run},
                menu::{Session, root, start},
            },
            update::{Button, Event, Key, State},
        };

        fn opened() -> State {
            let app = |id: &str, name: &str| App {
                id: id.to_owned(),
                name: name.to_owned(),
                comment: None,
                icon: None,
                run: Run {
                    argv: vec![id.to_owned()],
                    terminal: false,
                    dir: None,
                },
                categories: vec!["Utility".to_owned()],
                keywords: Vec::new(),
                wm_class: None,
            };
            let apps = [app("xcalc", "Calculator"), app("gedit", "Text Editor")];
            let session = Session {
                lock: None,
                log_out: true,
            };
            let mut state = State::new(root(&apps, None, &session), start(&apps, &session));
            state.update(
                Event::DesktopPress {
                    output: "DP-1".to_owned(),
                    area: layout::Rect::new(0, 0, 800, 600),
                    at: (40.0, 30.0),
                    button: Button::Right,
                },
                &mut Monospace(8.0),
            );
            state
        }

        fn node(tree: &TreeUpdate, id: NodeId) -> &Node {
            &tree
                .nodes
                .iter()
                .find(|(at, _)| *at == id)
                .expect("in the tree")
                .1
        }

        fn labels(tree: &TreeUpdate, of: NodeId) -> Vec<(Role, String)> {
            node(tree, of)
                .children()
                .iter()
                .map(|&child| {
                    let child = node(tree, child);
                    (child.role(), child.label().unwrap_or_default().to_owned())
                })
                .collect()
        }

        #[test]
        fn the_root_menu_is_menu_items_named_as_drawn() {
            let mut state = opened();
            let view = state.view().unwrap();
            let tree = menu("perspicax-menu-DP-1", Some((800, 600)), Some(&view));
            let root = node(&tree, ROOT);
            assert_eq!(root.role(), Role::Window);
            assert_eq!(root.label(), Some("perspicax-menu-DP-1"));
            assert_eq!(labels(&tree, ROOT), [(Role::Menu, "Root menu".to_owned())]);
            let first = root.children()[0];
            assert_eq!(
                labels(&tree, first),
                [
                    (Role::MenuItem, "Accessories".to_owned()),
                    (Role::MenuItem, "Log Out".to_owned()),
                ],
                "the separator between them is not an item"
            );
            let accessories = node(&tree, node(&tree, first).children()[0]);
            assert_eq!(accessories.has_popup(), Some(HasPopup::Menu));
            assert_eq!(accessories.is_expanded(), Some(false));
            assert_eq!(
                accessories.bounds(),
                Some(rect(view.menus[0].lines[0].rect)),
                "where it is drawn, on the surface"
            );
            assert_eq!(tree.focus, first, "the menu, with no line selected yet");

            state.update(Event::Key(Key::Down), &mut Monospace(8.0));
            state.update(Event::Key(Key::Right), &mut Monospace(8.0));
            let view = state.view().unwrap();
            let tree = menu("perspicax-menu-DP-1", Some((800, 600)), Some(&view));
            let submenu = node(&tree, ROOT).children()[1];
            assert_eq!(node(&tree, submenu).label(), Some("Accessories"));
            assert_eq!(
                labels(&tree, submenu),
                [
                    (Role::MenuItem, "Calculator".to_owned()),
                    (Role::MenuItem, "Text Editor".to_owned()),
                ]
            );
            assert_eq!(
                route_of(tree.focus),
                Some(vec![0, 0]),
                "the keyboard's line"
            );
            assert_eq!(node(&tree, tree.focus).is_selected(), Some(true));
        }

        #[test]
        fn what_was_typed_is_a_search_line_above_what_it_found() {
            let mut state = opened();
            state.update(
                Event::Key(Key::Text("calc".to_owned())),
                &mut Monospace(8.0),
            );
            let view = state.view().unwrap();
            let tree = menu("perspicax-menu-DP-1", None, Some(&view));
            let first = node(&tree, ROOT).children()[0];
            assert_eq!(
                labels(&tree, first),
                [
                    (Role::SearchInput, "Search".to_owned()),
                    (Role::MenuItem, "Calculator".to_owned()),
                ]
            );
            assert_eq!(node(&tree, TYPED).value(), Some("calc"));
        }

        #[test]
        fn the_start_menu_is_named_so_and_has_its_search_line_from_the_start() {
            let mut state = opened();
            state.update(
                Event::StartMenu {
                    output: "DP-1".to_owned(),
                    area: layout::Rect::new(0, 0, 800, 560),
                    button: layout::Rect::new(0, 560, 40, 40),
                    edge: perspicax_config::Edge::Bottom,
                },
                &mut Monospace(8.0),
            );
            let view = state.view().unwrap();
            let tree = menu("perspicax-menu-DP-1", None, Some(&view));
            let first = node(&tree, ROOT).children()[0];
            assert_eq!(node(&tree, first).label(), Some("Start menu"));
            assert_eq!(
                labels(&tree, first)[0],
                (Role::SearchInput, "Search".to_owned())
            );
            assert_eq!(node(&tree, TYPED).value(), Some(""));
        }

        #[test]
        fn an_items_node_and_its_route_are_one_another() {
            for route in [vec![0], vec![3, 0, 7], vec![4094, 1, 2, 3, 4]] {
                assert_eq!(item(&route).and_then(route_of), Some(route));
            }
            assert_eq!(item(&[4095]), None);
            assert_eq!(item(&[0; 6]), None, "too deep");
            assert_eq!(route_of(TYPED), None);
            assert_eq!(route_of(NodeId(MENU)), None);
            assert_eq!(route_of(ROOT), None);
        }
    }
}

#[cfg(all(feature = "panel", feature = "menus"))]
pub(crate) use panels::START;
#[cfg(feature = "panel")]
pub(crate) use panels::panel;

#[cfg(feature = "panel")]
mod panels {
    use accesskit::{Action, HasPopup, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};
    use perspicax_config::Item;

    use super::{ROOT, rect, window};
    use crate::layout::Rect;

    /// What the panel holds, as one bar.
    const TOOLBAR: NodeId = NodeId(1);
    /// The start button.
    pub(crate) const START: NodeId = NodeId(2);
    const CLOCK: NodeId = NodeId(3);

    /// A panel: a window named `namespace` covering it, holding a `Toolbar`
    /// of what it holds, where it is `placed`. The start button is a
    /// `Button` that opens a menu, expanded while the start menu is `open`;
    /// the clock is a `Status` whose value is the `time` it shows, and whose
    /// description is too, for a reader of names and descriptions alone (as
    /// perspicax is, for now).
    pub(crate) fn panel(
        namespace: &str,
        size: Option<(u32, u32)>,
        placed: &[(Item, Rect)],
        time: &str,
        open: bool,
    ) -> TreeUpdate {
        let mut root = window(namespace, size);
        let mut bar = Node::new(Role::Toolbar);
        bar.set_label("Panel");
        if let Some((width, height)) = size {
            bar.set_bounds(rect(Rect::new(0, 0, width as i32, height as i32)));
        }
        let mut nodes = Vec::new();
        for &(item, place) in placed {
            let (id, mut node) = match item {
                Item::Start => {
                    let mut button = Node::new(Role::Button);
                    button.set_label("Start");
                    button.set_has_popup(HasPopup::Menu);
                    button.set_expanded(open);
                    button.add_action(Action::Click);
                    (START, button)
                }
                Item::Clock => {
                    let mut clock = Node::new(Role::Status);
                    clock.set_label("Clock");
                    clock.set_value(time);
                    clock.set_description(time);
                    (CLOCK, clock)
                }
                Item::Taskbar | Item::Pager | Item::Tray => continue,
            };
            node.set_bounds(rect(place));
            bar.push_child(id);
            nodes.push((id, node));
        }
        root.push_child(TOOLBAR);
        nodes.insert(0, (TOOLBAR, bar));
        nodes.insert(0, (ROOT, root));
        TreeUpdate {
            nodes,
            tree: Some(TreeInfo::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus: ROOT,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::layout::{Monospace, panel::lay_out};

        fn node(tree: &TreeUpdate, id: NodeId) -> &Node {
            &tree
                .nodes
                .iter()
                .find(|(at, _)| *at == id)
                .expect("in the tree")
                .1
        }

        #[test]
        fn the_panel_is_a_toolbar_with_a_start_button_and_a_clock() {
            let items = [Item::Start, Item::Taskbar, Item::Clock];
            let placed = lay_out(&items, "14:05", (1280, 40), &mut Monospace(8.0));
            let tree = panel(
                "perspicax-panel-DP-1",
                Some((1280, 40)),
                &placed,
                "14:05",
                false,
            );
            let root = node(&tree, ROOT);
            assert_eq!(root.role(), Role::Window);
            assert_eq!(root.label(), Some("perspicax-panel-DP-1"));
            assert_eq!(root.children(), [TOOLBAR]);
            let bar = node(&tree, TOOLBAR);
            assert_eq!(bar.role(), Role::Toolbar);
            assert_eq!(
                bar.children(),
                [START, CLOCK],
                "the taskbar shows nothing yet"
            );

            let start = node(&tree, START);
            assert_eq!((start.role(), start.label()), (Role::Button, Some("Start")));
            assert_eq!(start.bounds(), Some(rect(placed[0].1)), "where it is drawn");
            assert_eq!(start.has_popup(), Some(HasPopup::Menu));
            assert_eq!(start.is_expanded(), Some(false));
            let clock = node(&tree, CLOCK);
            assert_eq!((clock.role(), clock.label()), (Role::Status, Some("Clock")));
            assert_eq!(clock.value(), Some("14:05"));
            assert_eq!(clock.description(), Some("14:05"));

            let open = panel("perspicax-panel-DP-1", None, &placed, "14:05", true);
            assert_eq!(
                node(&open, START).is_expanded(),
                Some(true),
                "expanded while the start menu is open"
            );
        }
    }
}

#[cfg(all(test, feature = "wallpaper"))]
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
