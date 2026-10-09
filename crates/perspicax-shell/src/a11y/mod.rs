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

#[cfg(feature = "icons")]
pub(crate) use folder::{folder, icon_at, is_icon};

#[cfg(feature = "icons")]
mod folder {
    use std::{
        hash::{DefaultHasher, Hash, Hasher},
        path::Path,
    };

    use accesskit::{Action, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};

    use super::{ROOT, rect, window};
    use crate::{
        layout::{Rect, folder::Spot},
        model::folder::Folder,
    };

    /// The list of icons.
    const LIST: NodeId = NodeId(1);
    /// An icon's node: its file's path, hashed, with this bit set to keep
    /// clear of the list and the root. The same while the file is there,
    /// whatever comes and goes beside it.
    const ICON: u64 = 1 << 63;

    fn node_of(path: &Path) -> NodeId {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        NodeId(ICON | hasher.finish())
    }

    /// Whether `node` is an icon's.
    pub(crate) fn is_icon(node: NodeId) -> bool {
        node.0 & ICON != 0
    }

    /// Which of `folder`'s icons the node `node` is, if it is one.
    pub(crate) fn icon_at(folder: &Folder, node: NodeId) -> Option<usize> {
        folder
            .icons()
            .iter()
            .position(|icon| node_of(&icon.path) == node)
    }

    /// The desktop that holds the folder's icons: a window named `namespace`
    /// covering it, holding a `List` named "Desktop" of a `ListItem` for
    /// each icon placed at `spots`, named as drawn, the one selected
    /// selected. Clicking an item opens it; focusing it selects it.
    pub(crate) fn folder(
        namespace: &str,
        size: Option<(u32, u32)>,
        folder: &Folder,
        spots: &[Spot],
    ) -> TreeUpdate {
        let mut root = window(namespace, size);
        let mut list = Node::new(Role::List);
        list.set_label("Desktop");
        let mut nodes = Vec::new();
        let mut around: Option<Rect> = None;
        for (icon, spot) in folder.icons().iter().zip(spots) {
            let id = node_of(&icon.path);
            let mut item = Node::new(Role::ListItem);
            item.set_label(icon.name.as_str());
            item.set_bounds(rect(spot.place));
            item.set_selected(folder.is_selected(icon));
            item.add_action(Action::Click);
            item.add_action(Action::Focus);
            list.push_child(id);
            nodes.push((id, item));
            around = Some(around.map_or(spot.place, |around| union(around, spot.place)));
        }
        if let Some(around) = around {
            list.set_bounds(rect(around));
        }
        root.push_child(LIST);
        nodes.insert(0, (LIST, list));
        nodes.insert(0, (ROOT, root));
        TreeUpdate {
            nodes,
            tree: Some(TreeInfo::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus: ROOT,
        }
    }

    /// The smallest rectangle around `a` and `b`.
    fn union(a: Rect, b: Rect) -> Rect {
        let (x, y) = (a.x.min(b.x), a.y.min(b.y));
        Rect::new(
            x,
            y,
            a.right().max(b.right()) - x,
            a.bottom().max(b.bottom()) - y,
        )
    }

    #[cfg(test)]
    mod tests {
        use std::path::PathBuf;

        use super::*;
        use crate::{
            layout::{Monospace, folder::lay_out},
            model::{
                Button,
                apps::Run,
                folder::{Folder, Icon},
            },
        };

        fn icon(name: &str) -> Icon {
            Icon {
                path: PathBuf::from(format!("/home/ada/Desktop/{name}")),
                name: name.to_owned(),
                image: "text-x-generic".to_owned(),
                fallback: "text-x-generic",
                opens: Run {
                    argv: vec!["xdg-open".to_owned(), name.to_owned()],
                    terminal: false,
                    dir: None,
                },
                is_folder: false,
            }
        }

        fn node(tree: &TreeUpdate, id: NodeId) -> &Node {
            &tree
                .nodes
                .iter()
                .find(|(at, _)| *at == id)
                .expect("in the tree")
                .1
        }

        #[test]
        fn the_desktop_icons_are_a_list_of_items_named_as_drawn() {
            let mut folder = Folder::new(perspicax_config::DOUBLE_CLICK_MS);
            folder.show(vec![icon("notes.txt"), icon("plan.pdf")]);
            folder.press(Some(1), Button::Left, 0);
            let spots = lay_out(
                folder.icons().iter().map(|icon| icon.name.as_str()),
                Rect::new(0, 0, 1280, 760),
                &mut Monospace(7.0),
            );
            let tree = super::folder("perspicax-desktop-DP-1", Some((1280, 800)), &folder, &spots);
            let root = node(&tree, ROOT);
            assert_eq!(
                (root.role(), root.label()),
                (Role::Window, Some("perspicax-desktop-DP-1"))
            );
            let list = node(&tree, LIST);
            assert_eq!((list.role(), list.label()), (Role::List, Some("Desktop")));
            let items: Vec<_> = list
                .children()
                .iter()
                .map(|&id| {
                    let item = node(&tree, id);
                    assert_eq!(item.role(), Role::ListItem);
                    assert!(item.supports_action(Action::Click));
                    (
                        item.label().unwrap_or_default().to_owned(),
                        item.is_selected(),
                    )
                })
                .collect();
            assert_eq!(
                items,
                [
                    ("notes.txt".to_owned(), Some(false)),
                    ("plan.pdf".to_owned(), Some(true)),
                ]
            );
            let second = list.children()[1];
            assert_eq!(node(&tree, second).bounds(), Some(rect(spots[1].place)));
            assert!(is_icon(second) && !is_icon(LIST) && !is_icon(ROOT));
            assert_eq!(icon_at(&folder, second), Some(1), "known by its file");

            folder.show(vec![icon("a.txt"), icon("notes.txt"), icon("plan.pdf")]);
            assert_eq!(
                icon_at(&folder, second),
                Some(2),
                "and still, with another before it"
            );
        }
    }
}

#[cfg(feature = "menus")]
pub(crate) use menus::{menu, route_of};

#[cfg(feature = "menus")]
mod menus {
    use accesskit::{Action, HasPopup, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};

    use super::{ROOT, rect, window};
    use crate::{
        model::menu::{Item, Route},
        update::View,
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
    /// that is not a separator, named as drawn: one its program greyed out
    /// disabled, and one with a tick or a dot a `MenuItemCheckBox` or a
    /// `MenuItemRadio`, toggled as drawn. The focus is the line the keyboard
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
            let first = view.map_or("Root menu", |view| view.name);
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
                let Some(id) = item(line.route).filter(|_| !line.item.is_separator()) else {
                    continue;
                };
                let mut entry = entry(line.item);
                entry.set_bounds(rect(line.rect));
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

    /// A menu item, as `item` is: chosen by a click, or moved to by focus,
    /// unless it is greyed out.
    fn entry(item: &Item) -> Node {
        #[cfg(feature = "tray")]
        if let crate::model::menu::Does::Tell(choice) = item.does {
            use accesskit::Toggled;

            use crate::model::menu::Mark;

            let (role, on) = match choice.mark {
                Some(Mark::Check(on)) => (Role::MenuItemCheckBox, Some(on)),
                Some(Mark::Radio(on)) => (Role::MenuItemRadio, Some(on)),
                None => (Role::MenuItem, None),
            };
            let mut entry = Node::new(role);
            entry.set_label(item.label.as_str());
            if let Some(on) = on {
                entry.set_toggled(if on { Toggled::True } else { Toggled::False });
            }
            if choice.enabled {
                entry.add_action(Action::Click);
                entry.add_action(Action::Focus);
            } else {
                entry.set_disabled();
            }
            return entry;
        }
        let mut entry = Node::new(Role::MenuItem);
        entry.set_label(item.label.as_str());
        entry.add_action(Action::Click);
        entry.add_action(Action::Focus);
        entry
    }

    #[cfg(test)]
    mod tests {
        use perspicax_config::Leave;

        use super::*;
        use crate::{
            layout::{self, Monospace},
            model::{
                Button,
                apps::{App, Run},
                menu::{Does, Session, root, start},
            },
            update::{Event, Key, State},
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
            let loginctl = |verb: &str| {
                Does::Run(Run {
                    argv: vec!["loginctl".to_owned(), verb.to_owned()],
                    terminal: false,
                    dir: None,
                })
            };
            let session = Session {
                leave: vec![
                    (Leave::LogOut, Does::LogOut),
                    (Leave::Suspend, loginctl("suspend")),
                    (Leave::Reboot, loginctl("reboot")),
                    (Leave::PowerOff, loginctl("poweroff")),
                ],
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
                    (Role::MenuItem, "Suspend".to_owned()),
                    (Role::MenuItem, "Restart".to_owned()),
                    (Role::MenuItem, "Shut Down".to_owned()),
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

#[cfg(feature = "panel")]
pub(crate) use panels::{panel, part_of};

#[cfg(feature = "panel")]
mod panels {
    use accesskit::{Action, HasPopup, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};
    use perspicax_config::Item;

    use super::{ROOT, rect, window};
    use crate::{
        layout::{
            Rect,
            panel::{Part, Placed},
        },
        model::layouts::Layout,
    };

    /// What the panel holds, as one bar.
    const TOOLBAR: NodeId = NodeId(1);
    const START: NodeId = NodeId(2);
    const CLOCK: NodeId = NodeId(3);
    const TASKBAR: NodeId = NodeId(4);
    const PAGER: NodeId = NodeId(5);
    #[cfg(feature = "tray")]
    const TRAY: NodeId = NodeId(6);
    const LAYOUT: NodeId = NodeId(7);
    /// A task's node, a workspace's and a status icon's: the serial of its
    /// window or its workspace, or the icon's key, which stays its own
    /// while it lasts, above one of these.
    const TASK: u64 = 1;
    const WORKSPACE: u64 = 2;
    #[cfg(feature = "tray")]
    const STATUS: u64 = 3;
    const SERIAL_BITS: u32 = 40;

    /// What on a panel the node `id` is, if it is something to press.
    pub(crate) fn part_of(id: NodeId) -> Option<Part> {
        let serial = id.0 & ((1 << SERIAL_BITS) - 1);
        match (id, id.0 >> SERIAL_BITS) {
            (START, _) => Some(Part::Start),
            (LAYOUT, _) => Some(Part::Layout),
            (_, TASK) => Some(Part::Task(serial)),
            (_, WORKSPACE) => Some(Part::Workspace(serial)),
            #[cfg(feature = "tray")]
            (_, STATUS) => Some(Part::Tray(serial)),
            _ => None,
        }
    }

    fn node_of(kind: u64, serial: u64) -> NodeId {
        NodeId(kind << SERIAL_BITS | serial)
    }

    /// A panel: a window named `namespace` covering it, holding a `Toolbar`
    /// of what it holds, where it is `placed`. The start button is a
    /// `Button` that opens a menu, expanded while the start menu is `open`.
    /// The taskbar is a `TabList` of a `Tab` for each window, named by its
    /// title, the one with the keyboard selected and a minimized one
    /// described so; the pager is a `TabList` of a `Tab` for each workspace,
    /// the one showing selected. The tray is a `Group` of a `Button` for
    /// each status icon, named as its program names it, that opens a menu
    /// if it has one. The clock is a `Status` whose value is the
    /// `time` it shows, and whose description is too, for a reader of names
    /// and descriptions alone (as perspicax is, for now). The layout
    /// indicator is a `Button`, "Keyboard layout", whose value and
    /// description are the `layout` in use's name; a click moves to the
    /// next. What takes no room on the panel is not in the tree.
    pub(crate) fn panel(
        namespace: &str,
        size: Option<(u32, u32)>,
        placed: &Placed,
        time: &str,
        open: bool,
        layout: Option<&Layout>,
    ) -> TreeUpdate {
        let mut root = window(namespace, size);
        let mut bar = Node::new(Role::Toolbar);
        bar.set_label("Panel");
        if size.is_some() {
            bar.set_bounds(rect(placed.bar));
        }
        let mut nodes = Vec::new();
        for &(item, place) in placed.items.iter().filter(|(_, place)| place.w > 0) {
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
                Item::Layout => {
                    let mut button = Node::new(Role::Button);
                    button.set_label("Keyboard layout");
                    if let Some(layout) = layout {
                        button.set_value(layout.name.as_str());
                        button.set_description(layout.name.as_str());
                    }
                    button.add_action(Action::Click);
                    (LAYOUT, button)
                }
                Item::Taskbar => {
                    let tabs = placed.tasks.iter().map(|(task, place)| {
                        let mut tab = tab(&task.title, *place, task.active);
                        if task.minimized {
                            tab.set_description("Minimized");
                        }
                        (node_of(TASK, task.serial), tab)
                    });
                    (TASKBAR, tab_list("Taskbar", tabs, &mut nodes))
                }
                Item::Pager => {
                    let tabs = placed.cells.iter().map(|(cell, place)| {
                        (
                            node_of(WORKSPACE, cell.serial),
                            tab(&cell.name, *place, cell.active),
                        )
                    });
                    (PAGER, tab_list("Workspaces", tabs, &mut nodes))
                }
                #[cfg(feature = "tray")]
                Item::Tray => {
                    let mut group = Node::new(Role::Group);
                    group.set_label("Tray");
                    for (icon, place) in &placed.tray {
                        let mut button = Node::new(Role::Button);
                        button.set_label(icon.title.as_str());
                        button.set_bounds(rect(*place));
                        button.add_action(Action::Click);
                        if icon.menu {
                            button.set_has_popup(HasPopup::Menu);
                        }
                        let id = node_of(STATUS, icon.key);
                        group.push_child(id);
                        nodes.push((id, button));
                    }
                    (TRAY, group)
                }
                #[cfg(not(feature = "tray"))]
                Item::Tray => continue,
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

    /// A list of `tabs` named `label`, each put in `nodes`.
    fn tab_list(
        label: &str,
        tabs: impl Iterator<Item = (NodeId, Node)>,
        nodes: &mut Vec<(NodeId, Node)>,
    ) -> Node {
        let mut list = Node::new(Role::TabList);
        list.set_label(label);
        for (id, tab) in tabs {
            list.push_child(id);
            nodes.push((id, tab));
        }
        list
    }

    /// A tab named `label`, at `place`, `selected` or not, pressed by a click.
    fn tab(label: &str, place: Rect, selected: bool) -> Node {
        let mut tab = Node::new(Role::Tab);
        tab.set_label(label);
        tab.set_bounds(rect(place));
        tab.set_selected(selected);
        tab.add_action(Action::Click);
        tab
    }

    #[cfg(test)]
    mod tests {
        use perspicax_config::{Align, PanelWidth};

        use super::*;
        use crate::layout::{
            Monospace,
            panel::{Cell, Holding, Task, lay_out},
        };

        /// The panel's whole width, as by default.
        const FULL: (PanelWidth, Align) = (PanelWidth::FULL, Align::Center);

        fn node(tree: &TreeUpdate, id: NodeId) -> &Node {
            &tree
                .nodes
                .iter()
                .find(|(at, _)| *at == id)
                .expect("in the tree")
                .1
        }

        fn laid(tasks: Vec<Task>, cells: Vec<Cell>) -> Placed {
            laid_titled(tasks, cells, true)
        }

        /// As [`laid`], its tasks showing their titles or not.
        fn laid_titled(tasks: Vec<Task>, cells: Vec<Cell>, task_titles: bool) -> Placed {
            let items = [Item::Start, Item::Taskbar, Item::Pager, Item::Clock];
            let holding = Holding {
                time: "14:05",
                layout: None,
                tasks,
                task_titles,
                cells,
                #[cfg(feature = "tray")]
                tray: Vec::new(),
            };
            lay_out(&items, holding, (1280, 40), FULL, &mut Monospace(8.0))
        }

        #[cfg(feature = "tray")]
        #[test]
        fn the_tray_is_a_group_of_buttons_named_by_their_programs() {
            use crate::layout::panel::TrayIcon;

            let icon = |key: u64, title: &str, menu: bool| TrayIcon {
                key,
                title: title.to_owned(),
                menu,
            };
            let placed = lay_out(
                &[Item::Start, Item::Tray, Item::Clock],
                Holding {
                    time: "14:05",
                    layout: None,
                    tasks: Vec::new(),
                    task_titles: true,
                    cells: Vec::new(),
                    tray: vec![icon(3, "Network", true), icon(8, "Updates", false)],
                },
                (1280, 40),
                FULL,
                &mut Monospace(8.0),
            );
            let tree = panel(
                "perspicax-panel-DP-1",
                Some((1280, 40)),
                &placed,
                "14:05",
                false,
                None,
            );
            assert_eq!(node(&tree, TOOLBAR).children(), [START, TRAY, CLOCK]);
            let tray = node(&tree, TRAY);
            assert_eq!((tray.role(), tray.label()), (Role::Group, Some("Tray")));
            assert_eq!(tray.bounds(), Some(rect(placed.items[1].1)));
            let buttons: Vec<_> = tray
                .children()
                .iter()
                .map(|&id| {
                    let button = node(&tree, id);
                    (
                        part_of(id),
                        button.role(),
                        button.label(),
                        button.has_popup(),
                    )
                })
                .collect();
            assert_eq!(
                buttons,
                [
                    (
                        Some(Part::Tray(3)),
                        Role::Button,
                        Some("Network"),
                        Some(HasPopup::Menu)
                    ),
                    (Some(Part::Tray(8)), Role::Button, Some("Updates"), None),
                ],
                "a click on each is a press on its icon"
            );
            let second = node(&tree, tray.children()[1]);
            assert_eq!(second.bounds(), Some(rect(placed.tray[1].1)));
        }

        #[test]
        fn a_narrow_panels_toolbar_is_its_bar_and_its_window_the_strip() {
            let holding = Holding {
                time: "14:05",
                layout: None,
                tasks: Vec::new(),
                task_titles: true,
                cells: Vec::new(),
                #[cfg(feature = "tray")]
                tray: Vec::new(),
            };
            let placed = lay_out(
                &[Item::Start, Item::Clock],
                holding,
                (1280, 40),
                (PanelWidth::Pixels(600), Align::Right),
                &mut Monospace(8.0),
            );
            let tree = panel(
                "perspicax-panel-DP-1",
                Some((1280, 40)),
                &placed,
                "14:05",
                false,
                None,
            );
            assert_eq!(
                node(&tree, TOOLBAR).bounds(),
                Some(rect(Rect::new(680, 0, 600, 40)))
            );
            assert_eq!(
                node(&tree, ROOT).bounds(),
                Some(rect(Rect::new(0, 0, 1280, 40))),
                "the surface, the whole strip"
            );
            assert_eq!(
                node(&tree, START).bounds(),
                Some(rect(Rect::new(680, 0, 40, 40)))
            );
        }

        #[test]
        fn the_panel_is_a_toolbar_with_a_start_button_and_a_clock() {
            let placed = laid(Vec::new(), Vec::new());
            let tree = panel(
                "perspicax-panel-DP-1",
                Some((1280, 40)),
                &placed,
                "14:05",
                false,
                None,
            );
            let root = node(&tree, ROOT);
            assert_eq!(root.role(), Role::Window);
            assert_eq!(root.label(), Some("perspicax-panel-DP-1"));
            assert_eq!(root.children(), [TOOLBAR]);
            let bar = node(&tree, TOOLBAR);
            assert_eq!(bar.role(), Role::Toolbar);
            assert_eq!(
                bar.children(),
                [START, TASKBAR, CLOCK],
                "no pager with no workspaces to page"
            );

            let start = node(&tree, START);
            assert_eq!((start.role(), start.label()), (Role::Button, Some("Start")));
            assert_eq!(
                start.bounds(),
                Some(rect(placed.items[0].1)),
                "where it is drawn"
            );
            assert_eq!(start.has_popup(), Some(HasPopup::Menu));
            assert_eq!(start.is_expanded(), Some(false));
            let clock = node(&tree, CLOCK);
            assert_eq!((clock.role(), clock.label()), (Role::Status, Some("Clock")));
            assert_eq!(clock.value(), Some("14:05"));
            assert_eq!(clock.description(), Some("14:05"));

            let open = panel("perspicax-panel-DP-1", None, &placed, "14:05", true, None);
            assert_eq!(
                node(&open, START).is_expanded(),
                Some(true),
                "expanded while the start menu is open"
            );
        }

        #[test]
        fn the_layout_indicator_is_a_button_whose_value_is_the_layouts_name() {
            let items = [Item::Taskbar, Item::Layout, Item::Clock];
            let holding = |layout| Holding {
                time: "14:05",
                layout,
                tasks: Vec::new(),
                task_titles: true,
                cells: Vec::new(),
                #[cfg(feature = "tray")]
                tray: Vec::new(),
            };
            let russian = Layout {
                name: "Russian".to_owned(),
                short: "RU".to_owned(),
            };
            let placed = lay_out(
                &items,
                holding(Some("RU")),
                (1280, 40),
                FULL,
                &mut Monospace(8.0),
            );
            let tree = panel(
                "perspicax-panel-DP-1",
                None,
                &placed,
                "14:05",
                false,
                Some(&russian),
            );
            assert_eq!(node(&tree, TOOLBAR).children(), [TASKBAR, LAYOUT, CLOCK]);
            let button = node(&tree, LAYOUT);
            assert_eq!(
                (button.role(), button.label()),
                (Role::Button, Some("Keyboard layout"))
            );
            assert_eq!(button.value(), Some("Russian"));
            assert_eq!(button.description(), Some("Russian"));
            assert_eq!(button.bounds(), Some(rect(placed.items[1].1)));
            assert_eq!(part_of(LAYOUT), Some(Part::Layout));

            let one = lay_out(&items, holding(None), (1280, 40), FULL, &mut Monospace(8.0));
            let tree = panel("perspicax-panel-DP-1", None, &one, "14:05", false, None);
            assert_eq!(
                node(&tree, TOOLBAR).children(),
                [TASKBAR, CLOCK],
                "one layout: no indicator"
            );
        }

        #[test]
        fn the_taskbar_is_a_tab_list_with_the_active_window_selected() {
            let task = |serial: u64, title: &str, active: bool, minimized: bool| Task {
                serial,
                title: title.to_owned(),
                icon: None,
                active,
                minimized,
            };
            let cell = |serial: u64, name: &str, column: u32, active: bool| Cell {
                serial,
                name: name.to_owned(),
                column,
                row: 0,
                active,
            };
            let placed = laid(
                vec![
                    task(3, "Editor", false, false),
                    task(7, "Mail", true, false),
                    task(9, "Notes", false, true),
                ],
                vec![cell(0, "1", 0, false), cell(1, "2", 1, true)],
            );
            let tree = panel("perspicax-panel-DP-1", None, &placed, "14:05", false, None);
            assert_eq!(
                node(&tree, TOOLBAR).children(),
                [START, TASKBAR, PAGER, CLOCK]
            );

            let taskbar = node(&tree, TASKBAR);
            assert_eq!(
                (taskbar.role(), taskbar.label()),
                (Role::TabList, Some("Taskbar"))
            );
            let tabs: Vec<_> = taskbar
                .children()
                .iter()
                .map(|&id| {
                    let tab = node(&tree, id);
                    assert_eq!(tab.role(), Role::Tab);
                    (
                        tab.label().unwrap_or_default().to_owned(),
                        tab.is_selected(),
                        tab.description().map(str::to_owned),
                    )
                })
                .collect();
            assert_eq!(
                tabs,
                [
                    ("Editor".to_owned(), Some(false), None),
                    ("Mail".to_owned(), Some(true), None),
                    (
                        "Notes".to_owned(),
                        Some(false),
                        Some("Minimized".to_owned())
                    ),
                ]
            );
            let mail = taskbar.children()[1];
            assert_eq!(
                node(&tree, mail).bounds(),
                Some(rect(placed.tasks[1].1)),
                "where it is drawn"
            );
            assert_eq!(part_of(mail), Some(Part::Task(7)), "known by its serial");
            assert!(node(&tree, mail).supports_action(Action::Click));

            let pager = node(&tree, PAGER);
            assert_eq!(
                (pager.role(), pager.label()),
                (Role::TabList, Some("Workspaces"))
            );
            let second = pager.children()[1];
            assert_eq!(node(&tree, second).label(), Some("2"));
            assert_eq!(node(&tree, second).is_selected(), Some(true));
            assert_eq!(part_of(second), Some(Part::Workspace(1)));
            assert_eq!(part_of(START), Some(Part::Start));
            assert_eq!(part_of(TASKBAR), None);
            assert_eq!(part_of(ROOT), None);
        }

        #[test]
        fn a_taskbar_of_icons_alone_still_names_each_task_by_its_title() {
            use crate::layout::panel::TASK_LEAST;

            let task = |serial: u64, title: &str| Task {
                serial,
                title: title.to_owned(),
                icon: Some("firefox".to_owned()),
                active: false,
                minimized: false,
            };
            let placed = laid_titled(vec![task(3, "Editor"), task(7, "Mail")], Vec::new(), false);
            let tree = panel("perspicax-panel-DP-1", None, &placed, "14:05", false, None);
            let tabs: Vec<_> = node(&tree, TASKBAR)
                .children()
                .iter()
                .map(|&id| {
                    let tab = node(&tree, id);
                    (tab.label().map(str::to_owned), tab.bounds())
                })
                .collect();
            assert_eq!(
                tabs,
                [
                    (
                        Some("Editor".to_owned()),
                        Some(rect(Rect::new(40, 0, TASK_LEAST, 40)))
                    ),
                    (
                        Some("Mail".to_owned()),
                        Some(rect(Rect::new(40 + TASK_LEAST, 0, TASK_LEAST, 40)))
                    ),
                ],
                "what a person no longer reads, a screen reader still does"
            );
        }
    }
}

#[cfg(feature = "pie")]
pub(crate) use pies::{pie, slot_of};

#[cfg(feature = "pie")]
mod pies {
    use accesskit::{Action, HasPopup, Node, NodeId, Rect, Role, TreeId, TreeInfo, TreeUpdate};

    use super::{ROOT, rect, window};
    use crate::{model::pie::Does, pie::View};

    /// The ring open's node.
    const RING: NodeId = NodeId(1 << 62);
    /// A slot's node: its place in the ring open below this, one more than
    /// its index. Stable while that ring is open.
    const SLOT: u64 = 1 << 60;

    /// The index of the slot a node is, if it is a slot's.
    pub(crate) fn slot_of(id: NodeId) -> Option<usize> {
        (id.0 & !(SLOT - 1) == SLOT && id.0 != SLOT).then(|| (id.0 - SLOT - 1) as usize)
    }

    /// The pie's surface: a window named `namespace` covering it, holding
    /// a `Menu` for the ring open, named for its pie or its submenu, and in
    /// it a `MenuItem` for each slot, named as its title is and bounded by
    /// its icon as drawn: a submenu's says so, and one with windows open
    /// says how many. The focus is the slot pointed at, or the ring.
    pub(crate) fn pie(
        namespace: &str,
        size: Option<(u32, u32)>,
        view: Option<&View<'_>>,
    ) -> TreeUpdate {
        let mut root = window(namespace, size);
        let mut nodes = Vec::new();
        let mut focus = ROOT;
        if let Some(view) = view {
            root.push_child(RING);
            focus = RING;
            let mut ring = Node::new(Role::Menu);
            ring.set_label(view.label);
            ring.set_bounds(rect(view.ring.square()));
            for (index, slot) in view.slots.iter().enumerate() {
                let id = NodeId(SLOT + 1 + index as u64);
                let mut entry = Node::new(Role::MenuItem);
                entry.set_label(slot.label.as_str());
                let ((x, y), side) = view.ring.place(index, view.spin, view.pointer);
                let half = side / 2.0;
                entry.set_bounds(Rect::new(x - half, y - half, x + half, y + half));
                if let Does::Open(_) = slot.does {
                    entry.set_has_popup(HasPopup::Menu);
                }
                match slot.windows.len() {
                    0 => {}
                    1 => entry.set_description("running, 1 window"),
                    windows => entry.set_description(format!("running, {windows} windows")),
                }
                if view.picked == Some(index) {
                    entry.set_selected(true);
                    focus = id;
                }
                entry.add_action(Action::Click);
                entry.add_action(Action::Focus);
                ring.push_child(id);
                nodes.push((id, entry));
            }
            nodes.push((RING, ring));
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
            layout::{Rect as Area, pie::Ring},
            model::{
                apps::Run,
                pie::{Does, Slot},
            },
        };

        fn slot(label: &str, does: Does, windows: Vec<u64>) -> Slot {
            Slot {
                label: label.to_owned(),
                icons: Vec::new(),
                does,
                windows,
            }
        }

        fn node(tree: &TreeUpdate, id: NodeId) -> &Node {
            &tree
                .nodes
                .iter()
                .find(|(known, _)| *known == id)
                .expect("the node")
                .1
        }

        #[test]
        fn a_pie_is_a_menu_of_its_slots_with_the_one_pointed_at_focused() {
            let run = Run {
                argv: vec!["foot".to_owned()],
                terminal: false,
                dir: None,
            };
            let slots = vec![
                slot("terminal", Does::Launch(run), vec![3, 4]),
                slot("games", Does::Open(Vec::new()), Vec::new()),
                slot("slack", Does::Switch(None), vec![9]),
            ];
            let view = View {
                output: "eDP-1",
                label: "launchers",
                ring: Ring::new(3, 512, (960, 500), Area::new(0, 0, 1920, 1048)),
                slots: &slots,
                spin: 0,
                pointer: Some((960.0, 2.0)),
                picked: Some(0),
            };
            let tree = pie("perspicax-pie-eDP-1", Some((1920, 1080)), Some(&view));
            let ring = node(&tree, RING);
            assert_eq!((ring.role(), ring.label()), (Role::Menu, Some("launchers")));
            assert_eq!(ring.children().len(), 3);
            let first = node(&tree, ring.children()[0]);
            assert_eq!(first.label(), Some("terminal"));
            assert_eq!(first.description(), Some("running, 2 windows"));
            assert!(first.is_selected().unwrap_or(false));
            assert_eq!(tree.focus, ring.children()[0]);
            let ((x, y), side) = view.ring.place(0, 0, view.pointer);
            assert_eq!(
                first.bounds(),
                Some(Rect::new(
                    x - side / 2.0,
                    y - side / 2.0,
                    x + side / 2.0,
                    y + side / 2.0
                ))
            );
            let games = node(&tree, ring.children()[1]);
            assert_eq!(games.has_popup(), Some(HasPopup::Menu));
            assert_eq!(games.description(), None);
            assert_eq!(
                node(&tree, ring.children()[2]).description(),
                Some("running, 1 window")
            );
            assert_eq!(
                ring.children()
                    .iter()
                    .map(|&id| slot_of(id))
                    .collect::<Vec<_>>(),
                [Some(0), Some(1), Some(2)]
            );
            assert_eq!(slot_of(RING), None);
            assert_eq!(slot_of(ROOT), None);
        }

        #[test]
        fn a_closed_pie_is_its_window_alone() {
            let tree = pie("perspicax-pie-eDP-1", None, None);
            assert_eq!(tree.nodes.len(), 1);
            assert_eq!(tree.focus, ROOT);
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
