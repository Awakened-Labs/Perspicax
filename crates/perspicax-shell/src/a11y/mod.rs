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
            let mut state = State::new(root(&apps, None, &session), start(&apps, None, &session));
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

    #[cfg(feature = "audio")]
    use accesskit::Toggled;
    #[cfg(feature = "audio")]
    use perspicax_config::mixer::Piece;

    use super::{ROOT, rect, window};
    #[cfg(feature = "audio")]
    use crate::layout::panel::Sounding;
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
    #[cfg(feature = "audio")]
    const MIXER: NodeId = NodeId(8);
    /// A task's node, a workspace's and a status icon's: the serial of its
    /// window or its workspace, or the icon's key, which stays its own
    /// while it lasts, above one of these.
    const TASK: u64 = 1;
    const WORKSPACE: u64 = 2;
    #[cfg(feature = "tray")]
    const STATUS: u64 = 3;
    /// A piece of the mixer's node: its place in [`Piece::ALL`] above this.
    #[cfg(feature = "audio")]
    const PIECE: u64 = 4;
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
            #[cfg(feature = "audio")]
            (_, PIECE) => usize::try_from(serial)
                .ok()
                .and_then(|at| Piece::ALL.get(at))
                .map(|&piece| Part::Mixer(piece)),
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
    /// next. The mixer is a `Group`, "Sound", of a node for each piece: the
    /// speaker that mutes is a `Switch`, "Mute", on while the sound is
    /// muted, described by how loud it is; the volume is a `Slider`,
    /// "Volume", from silence to the limit, or to where it is past it, that
    /// steps by `step` and can be set, described as muted while it is; the
    /// output is a `Button`, "Output", whose value is the output's name and
    /// whose description is its whole name, and a click switches to the
    /// next; the mixer's button is a `Button`, "Mixer", that opens a
    /// dialog, expanded while it is open; the microphone is a `Switch`,
    /// "Mute microphone", as the speaker is, and with no input to record
    /// from, disabled and described so; the moving bars are a `Meter`,
    /// "Sound levels", with no value, since they move faster than a tree is
    /// worth sending. With no sound server, each is disabled, and described
    /// so. What takes no room on the panel is not in the tree.
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
                #[cfg(feature = "audio")]
                Item::Mixer => {
                    let mut group = Node::new(Role::Group);
                    group.set_label("Sound");
                    for &(piece, place) in &placed.mixer {
                        let mut node = sound(piece, &placed.sound);
                        node.set_bounds(rect(place));
                        let at = Piece::ALL.iter().position(|&each| each == piece);
                        let id = node_of(PIECE, at.unwrap_or_default() as u64);
                        group.push_child(id);
                        nodes.push((id, node));
                    }
                    (MIXER, group)
                }
                #[cfg(not(feature = "audio"))]
                Item::Mixer => continue,
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

    /// The node of a piece of the mixer, showing `sound`.
    #[cfg(feature = "audio")]
    fn sound(piece: Piece, sound: &Sounding) -> Node {
        let level = sound.level;
        match piece {
            Piece::Mute => {
                let mut switch = Node::new(Role::Switch);
                switch.set_label("Mute");
                match level {
                    Some(level) => {
                        switch.set_toggled(if level.muted {
                            Toggled::True
                        } else {
                            Toggled::False
                        });
                        switch.set_description(if level.muted {
                            "Muted".to_owned()
                        } else {
                            format!("{}%", level.percent)
                        });
                        switch.add_action(Action::Click);
                    }
                    None => {
                        switch.set_disabled();
                        switch.set_description("No sound server");
                    }
                }
                switch
            }
            Piece::Volume => {
                let mut slider = Node::new(Role::Slider);
                slider.set_label("Volume");
                match level {
                    Some(level) => {
                        let most = sound.limit.max(level.percent);
                        slider.set_numeric_value(f64::from(level.percent));
                        slider.set_min_numeric_value(0.0);
                        slider.set_max_numeric_value(f64::from(most));
                        slider.set_numeric_value_step(f64::from(sound.step));
                        slider.set_value(format!("{}%", level.percent));
                        if level.muted {
                            slider.set_description("Muted");
                        }
                        for action in [Action::Increment, Action::Decrement, Action::SetValue] {
                            slider.add_action(action);
                        }
                    }
                    None => {
                        slider.set_disabled();
                        slider.set_description("No sound server");
                    }
                }
                slider
            }
            Piece::Mic => {
                let mut switch = Node::new(Role::Switch);
                switch.set_label("Mute microphone");
                match sound.mic {
                    Some(mic) => {
                        switch.set_toggled(if mic.muted {
                            Toggled::True
                        } else {
                            Toggled::False
                        });
                        switch.set_description(if mic.muted {
                            "Muted".to_owned()
                        } else {
                            format!("{}%", mic.percent)
                        });
                        switch.add_action(Action::Click);
                    }
                    None => {
                        switch.set_disabled();
                        switch.set_description("No microphone");
                    }
                }
                switch
            }
            Piece::Open => {
                let mut button = Node::new(Role::Button);
                button.set_label("Mixer");
                button.set_has_popup(HasPopup::Dialog);
                button.set_expanded(sound.open);
                button.add_action(Action::Click);
                button
            }
            Piece::Levels => {
                let mut meter = Node::new(Role::Meter);
                meter.set_label("Sound levels");
                if level.is_none() {
                    meter.set_disabled();
                    meter.set_description("No sound server");
                }
                meter
            }
            Piece::Output => {
                let mut button = Node::new(Role::Button);
                button.set_label("Output");
                match &sound.output {
                    Some(playing) => {
                        button.set_value(playing.name.as_str());
                        button.set_description(playing.long.as_str());
                        button.add_action(Action::Click);
                    }
                    None => {
                        button.set_disabled();
                        button.set_description("No sound server");
                    }
                }
                button
            }
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
                #[cfg(feature = "audio")]
                mixer: crate::layout::panel::Mixing::default(),
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
                    #[cfg(feature = "audio")]
                    mixer: crate::layout::panel::Mixing::default(),
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

        #[cfg(feature = "audio")]
        #[test]
        fn the_mixer_is_a_group_whose_speaker_is_a_switch_that_mutes() {
            use accesskit::Toggled;

            use crate::{
                layout::panel::{Mixing, Playing, Sounding},
                model::sound::{Kind, Level},
            };

            let tree_of = |level: Option<Level>| {
                let placed = lay_out(
                    &[Item::Start, Item::Mixer, Item::Clock],
                    Holding {
                        time: "14:05",
                        layout: None,
                        tasks: Vec::new(),
                        task_titles: true,
                        cells: Vec::new(),
                        #[cfg(feature = "tray")]
                        tray: Vec::new(),
                        mixer: Mixing {
                            pieces: &[Piece::Mute, Piece::Volume, Piece::Levels, Piece::Mic],
                            sound: Sounding {
                                level,
                                step: 5,
                                limit: 100,
                                output: None,
                                mic: level,
                                open: false,
                            },
                        },
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
                (placed, tree)
            };
            let (placed, tree) = tree_of(Some(Level {
                percent: 24,
                muted: false,
            }));
            assert_eq!(node(&tree, TOOLBAR).children(), [START, MIXER, CLOCK]);
            let mixer = node(&tree, MIXER);
            assert_eq!((mixer.role(), mixer.label()), (Role::Group, Some("Sound")));
            let [mute, volume, levels, mic] = mixer.children() else {
                panic!("four pieces: {:?}", mixer.children());
            };
            let switch = node(&tree, *mic);
            assert_eq!(
                (switch.role(), switch.label(), switch.toggled()),
                (Role::Switch, Some("Mute microphone"), Some(Toggled::False))
            );
            assert_eq!(part_of(*mic), Some(Part::Mixer(Piece::Mic)));
            let meter = node(&tree, *levels);
            assert_eq!(
                (meter.role(), meter.label()),
                (Role::Meter, Some("Sound levels"))
            );
            assert_eq!(meter.numeric_value(), None, "it moves too fast to tell");
            assert_eq!(part_of(*levels), Some(Part::Mixer(Piece::Levels)));
            assert_eq!(
                part_of(*mute),
                Some(Part::Mixer(Piece::Mute)),
                "a click on it is a press"
            );
            let switch = node(&tree, *mute);
            assert_eq!(
                (switch.role(), switch.label()),
                (Role::Switch, Some("Mute"))
            );
            assert_eq!(switch.toggled(), Some(Toggled::False));
            assert_eq!(switch.description(), Some("24%"));
            assert!(switch.supports_action(Action::Click));
            assert_eq!(switch.bounds(), Some(rect(placed.mixer[0].1)));
            assert_eq!(part_of(*volume), Some(Part::Mixer(Piece::Volume)));
            let slider = node(&tree, *volume);
            assert_eq!(
                (slider.role(), slider.label()),
                (Role::Slider, Some("Volume"))
            );
            assert_eq!(
                (
                    slider.numeric_value(),
                    slider.min_numeric_value(),
                    slider.max_numeric_value(),
                    slider.numeric_value_step()
                ),
                (Some(24.0), Some(0.0), Some(100.0), Some(5.0))
            );
            assert_eq!(slider.value(), Some("24%"));
            for action in [Action::Increment, Action::Decrement, Action::SetValue] {
                assert!(slider.supports_action(action), "{action:?}");
            }
            assert_eq!(slider.description(), None);

            let (_, muted) = tree_of(Some(Level {
                percent: 24,
                muted: true,
            }));
            let switch = node(&muted, *mute);
            assert_eq!(switch.toggled(), Some(Toggled::True));
            assert_eq!(switch.description(), Some("Muted"));
            assert_eq!(node(&muted, *volume).description(), Some("Muted"));
            let (_, loud) = tree_of(Some(Level {
                percent: 120,
                muted: false,
            }));
            assert_eq!(
                node(&loud, *volume).max_numeric_value(),
                Some(120.0),
                "past the limit, the slider reaches where it is"
            );

            let output = |playing: Option<Playing>| {
                let placed = lay_out(
                    &[Item::Mixer],
                    Holding {
                        time: "14:05",
                        layout: None,
                        tasks: Vec::new(),
                        task_titles: true,
                        cells: Vec::new(),
                        #[cfg(feature = "tray")]
                        tray: Vec::new(),
                        mixer: Mixing {
                            pieces: &[Piece::Output],
                            sound: Sounding {
                                output: playing,
                                ..Sounding::default()
                            },
                        },
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
                let id = node(&tree, MIXER).children()[0];
                (part_of(id), node(&tree, id).clone())
            };
            let (part, button) = output(Some(Playing {
                kind: Kind::Headset,
                name: "E8".to_owned(),
                long: "E8 \u{2014} Headphones \u{b7} Bluetooth".to_owned(),
            }));
            assert_eq!(part, Some(Part::Mixer(Piece::Output)));
            assert_eq!(
                (button.role(), button.label()),
                (Role::Button, Some("Output"))
            );
            assert_eq!(button.value(), Some("E8"));
            assert_eq!(
                button.description(),
                Some("E8 \u{2014} Headphones \u{b7} Bluetooth")
            );
            assert!(button.supports_action(Action::Click), "a click switches");
            let (_, none) = output(None);
            assert!(none.is_disabled() && !none.supports_action(Action::Click));

            let (_, gone) = tree_of(None);
            let switch = node(&gone, *mute);
            assert!(switch.is_disabled());
            assert!(!switch.supports_action(Action::Click));
            assert_eq!(switch.description(), Some("No sound server"));
            let microphone = node(&gone, *mic);
            assert!(microphone.is_disabled());
            assert_eq!(microphone.description(), Some("No microphone"));
            let slider = node(&gone, *volume);
            assert!(slider.is_disabled() && !slider.supports_action(Action::SetValue));
            assert_eq!(slider.numeric_value(), None);
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
                #[cfg(feature = "audio")]
                mixer: crate::layout::panel::Mixing::default(),
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
                #[cfg(feature = "audio")]
                mixer: crate::layout::panel::Mixing::default(),
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

#[cfg(feature = "audio")]
pub(crate) use mixers::{control_of, mixer};

#[cfg(feature = "audio")]
mod mixers {
    use accesskit::{Action, Node, NodeId, Role, Toggled, TreeId, TreeInfo, TreeUpdate};

    use super::{ROOT, rect, window};
    use crate::{
        layout::mixer::{Control, Listed},
        mixer::View,
        model::sound::{self, Side, Whose},
    };

    /// The mixer itself, its sections, and the sides' lists of devices.
    const DIALOG: NodeId = NodeId(1);
    const OUTPUT: NodeId = NodeId(2);
    const INPUT: NodeId = NodeId(3);
    const OUTPUTS: NodeId = NodeId(4);
    const INPUTS: NodeId = NodeId(5);
    const PROGRAMS: NodeId = NodeId(6);
    /// A side's control that is no device's: its place in
    /// [`Control::FIXED`] past this.
    const CONTROLS: u64 = 10;
    /// Any other node says what it is in its top four bits, past `KIND`,
    /// and which in the rest: a device's key's number, with `ON_INPUT` set for
    /// an input's, or a program's stream's, so that each stays the same
    /// while others come and go.
    const KIND: u32 = 60;
    const ON_INPUT: u64 = 1 << 59;
    const DEVICE: u64 = 1;
    const MORE: u64 = 2;
    const RENAME: u64 = 3;
    const HIDE: u64 = 4;
    const HIDE_CARD: u64 = 5;
    const PROGRAM_MUTE: u64 = 6;
    const PROGRAM_VOLUME: u64 = 7;
    const PROGRAM: u64 = 8;
    /// An output a program can be moved to: its stream's number, then the
    /// sink's in the low `SINK` bits.
    const MOVE: u64 = 9;
    const SINK: u32 = 27;
    /// A program's list of the outputs it can be moved to.
    const PROGRAM_OUTPUTS: u64 = 10;
    const TEST: u64 = 11;

    fn tagged(kind: u64, rest: u64) -> NodeId {
        NodeId(kind << KIND | rest)
    }

    /// A device's control of `kind`, on `side`, of the device numbered
    /// `number`.
    fn of_device(kind: u64, side: Side, number: u64) -> NodeId {
        let input = if side == Side::Input { ON_INPUT } else { 0 };
        tagged(kind, input | number)
    }

    fn id(control: Control) -> NodeId {
        match control {
            Control::Choose(side, number) => of_device(DEVICE, side, number),
            Control::More(side, number) => of_device(MORE, side, number),
            Control::Rename(side, number) => of_device(RENAME, side, number),
            Control::Test(side, number) => of_device(TEST, side, number),
            Control::Hide(side, number) => of_device(HIDE, side, number),
            Control::HideCard(side, number) => of_device(HIDE_CARD, side, number),
            Control::Mute(Whose::App(stream)) => tagged(PROGRAM_MUTE, stream.into()),
            Control::Volume(Whose::App(stream)) => tagged(PROGRAM_VOLUME, stream.into()),
            Control::Plays(stream) => tagged(PROGRAM, stream.into()),
            Control::Move(stream, sink) => tagged(
                MOVE,
                u64::from(stream) << SINK | u64::from(sink) & ((1 << SINK) - 1),
            ),
            fixed => {
                let at = Control::FIXED.iter().position(|&had| had == fixed);
                NodeId(CONTROLS + at.unwrap_or_default() as u64)
            }
        }
    }

    /// The control the node `id` is, if it is one.
    pub(crate) fn control_of(id: NodeId) -> Option<Control> {
        let rest = id.0 & ((1 << KIND) - 1);
        let stream = u32::try_from(rest);
        let side = if rest & ON_INPUT == 0 {
            Side::Output
        } else {
            Side::Input
        };
        let number = rest & !ON_INPUT;
        match id.0 >> KIND {
            0 => {
                let at = rest.checked_sub(CONTROLS)?;
                Control::FIXED.get(usize::try_from(at).ok()?).copied()
            }
            DEVICE => Some(Control::Choose(side, number)),
            MORE => Some(Control::More(side, number)),
            RENAME => Some(Control::Rename(side, number)),
            TEST => Some(Control::Test(side, number)),
            HIDE => Some(Control::Hide(side, number)),
            HIDE_CARD => Some(Control::HideCard(side, number)),
            PROGRAM_MUTE => Some(Control::Mute(Whose::App(stream.ok()?))),
            PROGRAM_VOLUME => Some(Control::Volume(Whose::App(stream.ok()?))),
            PROGRAM => Some(Control::Plays(stream.ok()?)),
            MOVE => Some(Control::Move(
                u32::try_from(rest >> SINK).ok()?,
                (rest & ((1 << SINK) - 1)) as u32,
            )),
            _ => None,
        }
    }

    /// The mixer: a window named `namespace` covering its monitor, holding a
    /// `Dialog`, "Sound", of a `Group` for the output and one for the
    /// input, each named for its device. In each, the button that mutes is
    /// a `Switch`, on while it is muted, and the volume a `Slider`, from
    /// silence to the limit, or to where it is past it, that steps by
    /// `step` and can be set; a side with no device has them disabled.
    /// Then a `RadioGroup`, "Outputs" or "Inputs", of a `RadioButton` for
    /// each device listed, named as drawn, on for the one the sound is on
    /// and described if nothing is plugged into it, that a click moves the
    /// sound to, with the button at the end of each row, "Details of" it;
    /// for the device whose details are unfolded, that button again,
    /// described by them, expanded; a `TextInput`, "Name of" it, that can
    /// be set; and the buttons that hide it and its card, named as drawn;
    /// and the folds, each a `Button` named as drawn, expanded while what
    /// it holds is listed. While programs play, a `Group`,
    /// "Applications", holds for each a `Button` named for it, described by
    /// what it plays on, expanded while the outputs it can be moved to are
    /// listed; its `Switch` and its `Slider`, named for it; and those
    /// outputs, a `RadioGroup` of a `RadioButton` each, on for the one it
    /// plays on. The control the keyboard is on has the focus. Closed, the
    /// window alone.
    pub(crate) fn mixer(
        namespace: &str,
        size: Option<(u32, u32)>,
        view: Option<&View<'_>>,
    ) -> TreeUpdate {
        let mut root = window(namespace, size);
        let mut nodes = Vec::new();
        let mut focus = ROOT;
        if let Some(view) = view {
            let mut dialog = Node::new(Role::Dialog);
            dialog.set_label("Sound");
            dialog.set_bounds(rect(view.placed.rect));
            for (group_id, list_id, side) in [
                (OUTPUT, OUTPUTS, Side::Output),
                (INPUT, INPUTS, Side::Input),
            ] {
                let mut group = Node::new(Role::Group);
                let named = match side {
                    Side::Output => "Output",
                    Side::Input => "Input",
                };
                group.set_label(match view.mixed.on(side) {
                    Some(master) => format!("{named} \u{2014} {}", master.name),
                    None => named.to_owned(),
                });
                let mut list = Node::new(Role::RadioGroup);
                list.set_label(match side {
                    Side::Output => "Outputs",
                    Side::Input => "Inputs",
                });
                let (mut details, mut folds) = (Vec::new(), Vec::new());
                for &(control, place) in &view.placed.controls {
                    if control.side() != Some(side) {
                        continue;
                    }
                    let Some(mut node) = node(view, control) else {
                        continue;
                    };
                    node.set_bounds(rect(place));
                    match control {
                        Control::More(at, number)
                            if view.placed.unfolded.more == Some((at, number)) =>
                        {
                            details.push(id(control));
                        }
                        Control::Choose(..) | Control::More(..) => list.push_child(id(control)),
                        Control::Rename(..)
                        | Control::Test(..)
                        | Control::Hide(..)
                        | Control::HideCard(..) => details.push(id(control)),
                        Control::Fold(_) | Control::Hidden(_) => folds.push(id(control)),
                        _ => group.push_child(id(control)),
                    }
                    nodes.push((id(control), node));
                }
                if !list.children().is_empty() {
                    group.push_child(list_id);
                    nodes.push((list_id, list));
                }
                for child in details.into_iter().chain(folds) {
                    group.push_child(child);
                }
                dialog.push_child(group_id);
                nodes.push((group_id, group));
            }
            if !view.mixed.programs.is_empty() {
                let mut group = Node::new(Role::Group);
                group.set_label("Applications");
                for program in &view.mixed.programs {
                    let stream = program.stream;
                    let mut outputs = Node::new(Role::RadioGroup);
                    outputs.set_label(format!("{} plays on", program.name));
                    for &(control, place) in &view.placed.controls {
                        let of = match control {
                            Control::Plays(of)
                            | Control::Mute(Whose::App(of))
                            | Control::Volume(Whose::App(of))
                            | Control::Move(of, _) => of,
                            _ => continue,
                        };
                        let Some(mut node) = node(view, control).filter(|_| of == stream) else {
                            continue;
                        };
                        node.set_bounds(rect(place));
                        match control {
                            Control::Move(..) => outputs.push_child(id(control)),
                            _ => group.push_child(id(control)),
                        }
                        nodes.push((id(control), node));
                    }
                    if !outputs.children().is_empty() {
                        let outputs_id = tagged(PROGRAM_OUTPUTS, stream.into());
                        group.push_child(outputs_id);
                        nodes.push((outputs_id, outputs));
                    }
                }
                dialog.push_child(PROGRAMS);
                nodes.push((PROGRAMS, group));
            }
            focus = id(view.focus);
            root.push_child(DIALOG);
            nodes.insert(0, (DIALOG, dialog));
        }
        nodes.insert(0, (ROOT, root));
        TreeUpdate {
            nodes,
            tree: Some(TreeInfo::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus,
        }
    }

    /// The node of `control` as `view` shows it, but for where it is; or
    /// `None` for a device or a program no longer there. What can be used
    /// can be focused, and a side's mute button and slider with no device
    /// are disabled.
    fn node(view: &View<'_>, control: Control) -> Option<Node> {
        let named = |whose| match whose {
            Whose::App(stream) => view
                .mixed
                .program(stream)
                .map(|program| program.name.as_str()),
            Whose::Side(_) => Some(""),
        };
        let mut node = match control {
            Control::Mute(whose) => {
                let mut switch = Node::new(Role::Switch);
                switch.set_label(match whose {
                    Whose::Side(Side::Output) => "Mute".to_owned(),
                    Whose::Side(Side::Input) => "Mute microphone".to_owned(),
                    Whose::App(_) => format!("Mute {}", named(whose)?),
                });
                let Some(level) = view.mixed.level(whose) else {
                    switch.set_disabled();
                    return Some(switch);
                };
                switch.set_toggled(toggled(level.muted));
                switch.add_action(Action::Click);
                switch
            }
            Control::Volume(whose) => {
                let mut slider = Node::new(Role::Slider);
                slider.set_label(match whose {
                    Whose::Side(Side::Output) => "Volume".to_owned(),
                    Whose::Side(Side::Input) => "Microphone volume".to_owned(),
                    Whose::App(_) => format!("{} volume", named(whose)?),
                });
                let (Some(_), Some(percent)) = (view.mixed.level(whose), view.shown(whose)) else {
                    slider.set_disabled();
                    return Some(slider);
                };
                slider.set_numeric_value(f64::from(percent));
                slider.set_min_numeric_value(0.0);
                slider.set_max_numeric_value(f64::from(view.mixed.most(whose)));
                slider.set_numeric_value_step(f64::from(view.mixed.step));
                slider.set_value(format!("{percent}%"));
                for action in [Action::Increment, Action::Decrement, Action::SetValue] {
                    slider.add_action(action);
                }
                slider
            }
            Control::Choose(side, number) => {
                let listed = view.mixed.find(side, number)?;
                radio(listed, listed.chosen)
            }
            Control::Fold(side) => {
                let mut button = Node::new(Role::Button);
                button.set_label(format!(
                    "Not connected ({})",
                    view.placed.unplugged[side.at()]
                ));
                button.set_expanded(view.placed.unfolded.unplugged[side.at()]);
                button.add_action(Action::Click);
                button
            }
            Control::Hidden(side) => {
                let mut button = Node::new(Role::Button);
                button.set_label(format!("Hidden ({})", view.placed.hidden[side.at()]));
                button.set_expanded(view.placed.unfolded.hidden[side.at()]);
                button.add_action(Action::Click);
                button
            }
            Control::More(side, number) => {
                let listed = view.mixed.find(side, number)?;
                let mut button = Node::new(Role::Button);
                button.set_label(format!("Details of {}", listed.label()));
                let open = view.placed.unfolded.more == Some((side, number));
                if open {
                    let told: Vec<String> = listed
                        .details
                        .iter()
                        .map(|(label, value)| format!("{label}: {value}"))
                        .collect();
                    button.set_description(told.join(", "));
                }
                button.set_expanded(open);
                button.add_action(Action::Click);
                button
            }
            Control::Rename(side, number) => {
                let listed = view.mixed.find(side, number)?;
                let mut field = Node::new(Role::TextInput);
                let own = sound::long(&listed.device, listed.jack.as_deref());
                field.set_label(format!("Name of {own}"));
                let typed = view
                    .naming
                    .filter(|&(naming, _)| naming == control)
                    .map(|(_, typed)| typed);
                if let Some(value) = typed.or(listed.name.as_deref()) {
                    field.set_value(value);
                }
                field.add_action(Action::Click);
                field.add_action(Action::SetValue);
                field
            }
            Control::Test(side, number) => {
                let listed = view.mixed.find(side, number)?;
                let mut button = Node::new(Role::Button);
                button.set_label(format!("Test sound on {}", listed.label()));
                if listed.live.is_none() {
                    button.set_description("Choose it to hear it");
                    button.set_disabled();
                    return Some(button);
                }
                button.add_action(Action::Click);
                button
            }
            Control::Hide(side, number) => {
                let listed = view.mixed.find(side, number)?;
                let mut button = Node::new(Role::Button);
                let does = if listed.hidden { "Unhide" } else { "Hide" };
                button.set_label(format!("{does} {}", listed.label()));
                button.add_action(Action::Click);
                button
            }
            Control::HideCard(side, number) => {
                let card = view.mixed.find(side, number)?.card.as_ref()?;
                let mut button = Node::new(Role::Button);
                let does = if card.hidden { "Unhide" } else { "Hide" };
                button.set_label(format!("{does} all of {}", card.name));
                button.add_action(Action::Click);
                button
            }
            Control::Plays(stream) => {
                let program = view.mixed.program(stream)?;
                let mut button = Node::new(Role::Button);
                button.set_label(program.name.as_str());
                if let Some(on) = view.mixed.playing_on(program) {
                    button.set_description(format!("Plays on {}", on.short));
                }
                button.set_expanded(view.placed.unfolded.programs.contains(&stream));
                button.add_action(Action::Click);
                button
            }
            Control::Move(stream, sink) => {
                let program = view.mixed.program(stream)?;
                let (_, listed) = view.mixed.sinks.iter().find(|&&(had, _)| had == sink)?;
                radio(listed, program.sink == sink)
            }
        };
        node.add_action(Action::Focus);
        Some(node)
    }

    fn toggled(on: bool) -> Toggled {
        if on { Toggled::True } else { Toggled::False }
    }

    /// A device listed, as a `RadioButton`, on if it is `chosen`.
    fn radio(listed: &Listed, chosen: bool) -> Node {
        let mut radio = Node::new(Role::RadioButton);
        radio.set_label(listed.label());
        radio.set_toggled(toggled(chosen));
        if !listed.connected {
            radio.set_description("Nothing plugged in");
        }
        radio.add_action(Action::Click);
        radio
    }

    #[cfg(test)]
    mod tests {
        use perspicax_config::Edge;

        use super::*;
        use crate::{
            layout::{
                Rect,
                mixer::{Anchor, Master, Mixed, Program, number},
            },
            mixer::{Event, State},
            model::sound::{Kind, Level},
        };

        fn opened(mixed: Mixed) -> State {
            let mut state = State::default();
            state.update(Event::Open {
                output: "DP-1".to_owned(),
                anchor: Anchor {
                    button: Rect::new(1180, 760, 100, 40),
                    edge: Edge::Bottom,
                    area: Rect::new(0, 0, 1280, 760),
                },
                mixed,
            });
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

        #[test]
        fn the_mixer_is_a_dialog_of_a_switch_and_a_slider_for_each_side() {
            let state = opened(Mixed {
                output: Some(Master {
                    name: "Speakers".to_owned(),
                    level: Level {
                        percent: 40,
                        muted: true,
                    },
                }),
                input: None,
                step: 5,
                limit: 100,
                ..Mixed::default()
            });
            let view = state.view().expect("open");
            let tree = mixer("perspicax-mixer-DP-1", Some((1280, 800)), Some(&view));
            let dialog = node(&tree, DIALOG);
            assert_eq!(
                (dialog.role(), dialog.label()),
                (Role::Dialog, Some("Sound"))
            );
            assert_eq!(dialog.children(), [OUTPUT, INPUT]);
            assert_eq!(
                node(&tree, OUTPUT).label(),
                Some("Output \u{2014} Speakers")
            );
            let mute = node(&tree, id(Control::Mute(Whose::Side(Side::Output))));
            assert_eq!(mute.role(), Role::Switch);
            assert_eq!(mute.toggled(), Some(Toggled::True));
            let volume = node(&tree, id(Control::Volume(Whose::Side(Side::Output))));
            assert_eq!(volume.role(), Role::Slider);
            assert_eq!(
                (volume.numeric_value(), volume.max_numeric_value()),
                (Some(40.0), Some(100.0))
            );
            assert!(volume.supports_action(Action::SetValue));
            assert_eq!(
                tree.focus,
                id(Control::Volume(Whose::Side(Side::Output))),
                "the keyboard's"
            );
            let input = node(&tree, id(Control::Volume(Whose::Side(Side::Input))));
            assert!(input.is_disabled() && !input.supports_action(Action::SetValue));
            assert_eq!(node(&tree, INPUT).label(), Some("Input"));
            assert_eq!(
                node(&tree, OUTPUT).children(),
                [
                    id(Control::Mute(Whose::Side(Side::Output))),
                    id(Control::Volume(Whose::Side(Side::Output)))
                ],
                "no list with nothing in it"
            );
            let others = [
                Control::Choose(Side::Output, number("speakers")),
                Control::Choose(Side::Input, number("speakers")),
                Control::Mute(Whose::App(u32::MAX)),
                Control::Volume(Whose::App(40)),
                Control::Plays(40),
                Control::Move(u32::MAX, (1 << SINK) - 1),
                Control::Move(40, 0),
            ];
            for control in Control::FIXED.into_iter().chain(others) {
                assert_eq!(control_of(id(control)), Some(control), "{control:?}");
            }
            assert_ne!(id(others[0]), id(others[1]), "a side apiece");
            let number = number("speakers");
            for side in [Side::Output, Side::Input] {
                for control in [
                    Control::More(side, number),
                    Control::Rename(side, number),
                    Control::Test(side, number),
                    Control::Hide(side, number),
                    Control::HideCard(side, number),
                ] {
                    assert_eq!(control_of(id(control)), Some(control), "{control:?}");
                }
            }
            assert_eq!(control_of(tagged(PROGRAM_OUTPUTS, 40)), None, "no control");
            assert_eq!(control_of(OUTPUT), None);
            let closed = mixer("perspicax-mixer-DP-1", None, None);
            assert_eq!(closed.nodes.len(), 1, "the window alone");
        }

        #[test]
        fn a_devices_details_are_a_button_described_by_them_a_field_and_buttons_to_hide() {
            let device = |key: &str, hidden| Listed {
                key: key.to_owned(),
                device: "TU104".to_owned(),
                jack: Some(key.to_owned()),
                short: key.to_owned(),
                name: None,
                kind: Kind::Display,
                live: None,
                meter: None,
                connected: true,
                chosen: false,
                hidden,
                card: Some(crate::layout::mixer::Owner {
                    key: "nvidia".to_owned(),
                    name: "TU104".to_owned(),
                    hidden: false,
                }),
                details: vec![("Chip", "GPU".to_owned()), ("Key", key.to_owned())],
            };
            let mut state = opened(Mixed {
                outputs: vec![device("HDMI", false), device("HDMI 2", true)],
                step: 5,
                limit: 100,
                ..Mixed::default()
            });
            let hdmi = number("HDMI");
            let more = Control::More(Side::Output, hdmi);
            state.update(Event::Act(more, crate::mixer::Act::Click));
            state.update(Event::Act(
                Control::Hidden(Side::Output),
                crate::mixer::Act::Click,
            ));
            let view = state.view().expect("open");
            let tree = mixer("perspicax-mixer-DP-1", Some((1280, 800)), Some(&view));
            let button = node(&tree, id(more));
            assert_eq!(
                (button.label(), button.description(), button.is_expanded()),
                (
                    Some("Details of TU104 \u{2014} HDMI"),
                    Some("Chip: GPU, Key: HDMI"),
                    Some(true)
                )
            );
            let test = node(&tree, id(Control::Test(Side::Output, hdmi)));
            assert_eq!(
                (test.label(), test.description(), test.is_disabled()),
                (
                    Some("Test sound on TU104 \u{2014} HDMI"),
                    Some("Choose it to hear it"),
                    true
                ),
                "no sink plays to it"
            );
            let field = node(&tree, id(Control::Rename(Side::Output, hdmi)));
            assert_eq!(
                (field.role(), field.label()),
                (Role::TextInput, Some("Name of TU104 \u{2014} HDMI"))
            );
            assert!(field.supports_action(Action::SetValue));
            assert_eq!(
                node(&tree, id(Control::Hide(Side::Output, hdmi))).label(),
                Some("Hide TU104 \u{2014} HDMI")
            );
            assert_eq!(
                node(&tree, id(Control::HideCard(Side::Output, hdmi))).label(),
                Some("Hide all of TU104")
            );
            let hidden = node(&tree, id(Control::Hidden(Side::Output)));
            assert_eq!(
                (hidden.label(), hidden.is_expanded()),
                (Some("Hidden (1)"), Some(true))
            );
            let children = node(&tree, OUTPUT).children();
            let at = |control| children.iter().position(|&child| child == id(control));
            assert!(
                at(more) < at(Control::Rename(Side::Output, hdmi))
                    && at(Control::HideCard(Side::Output, hdmi))
                        < at(Control::Hidden(Side::Output)),
                "its button, what it unfolds, then the folds"
            );
            let other = number("HDMI 2");
            assert!(
                node(&tree, OUTPUTS)
                    .children()
                    .contains(&id(Control::More(Side::Output, other))),
                "another's button on its row"
            );
        }

        #[test]
        fn each_program_is_a_button_a_switch_a_slider_and_unfolded_its_outputs() {
            let device = |key: &str| Listed {
                key: key.to_owned(),
                device: key.to_owned(),
                jack: None,
                short: key.to_owned(),
                kind: Kind::Speaker,
                live: None,
                meter: None,
                connected: true,
                chosen: false,
                name: None,
                hidden: false,
                card: None,
                details: Vec::new(),
            };
            let mut state = opened(Mixed {
                programs: vec![Program {
                    stream: 40,
                    name: "Firefox".to_owned(),
                    icon: None,
                    level: Level {
                        percent: 30,
                        muted: true,
                    },
                    sink: 1,
                }],
                sinks: vec![(0, device("Speakers")), (1, device("E8"))],
                step: 5,
                limit: 100,
                ..Mixed::default()
            });
            let view = state.view().expect("open");
            let tree = mixer("perspicax-mixer-DP-1", Some((1280, 800)), Some(&view));
            assert_eq!(node(&tree, DIALOG).children(), [OUTPUT, INPUT, PROGRAMS]);
            let group = node(&tree, PROGRAMS);
            assert_eq!(group.label(), Some("Applications"));
            let firefox = Whose::App(40);
            assert_eq!(
                group.children(),
                [
                    id(Control::Plays(40)),
                    id(Control::Mute(firefox)),
                    id(Control::Volume(firefox))
                ],
                "its outputs folded away"
            );
            let plays = node(&tree, id(Control::Plays(40)));
            assert_eq!(
                (plays.role(), plays.label(), plays.description()),
                (Role::Button, Some("Firefox"), Some("Plays on E8"))
            );
            assert_eq!(plays.is_expanded(), Some(false));
            let mute = node(&tree, id(Control::Mute(firefox)));
            assert_eq!(
                (mute.label(), mute.toggled()),
                (Some("Mute Firefox"), Some(Toggled::True))
            );
            let volume = node(&tree, id(Control::Volume(firefox)));
            assert_eq!(
                (volume.label(), volume.numeric_value()),
                (Some("Firefox volume"), Some(30.0))
            );
            state.update(Event::Act(Control::Plays(40), crate::mixer::Act::Click));
            let view = state.view().expect("open");
            let tree = mixer("perspicax-mixer-DP-1", Some((1280, 800)), Some(&view));
            assert_eq!(
                node(&tree, id(Control::Plays(40))).is_expanded(),
                Some(true)
            );
            let outputs = node(&tree, tagged(PROGRAM_OUTPUTS, 40));
            assert_eq!(
                (outputs.role(), outputs.label()),
                (Role::RadioGroup, Some("Firefox plays on"))
            );
            assert_eq!(
                outputs.children(),
                [id(Control::Move(40, 0)), id(Control::Move(40, 1))]
            );
            assert_eq!(
                node(&tree, id(Control::Move(40, 1))).toggled(),
                Some(Toggled::True),
                "where it plays"
            );
            assert_eq!(
                node(&tree, id(Control::Move(40, 0))).toggled(),
                Some(Toggled::False)
            );
        }

        #[test]
        fn each_sides_devices_are_a_radio_group_and_its_fold_a_button_that_expands() {
            let device = |key: &str, connected, chosen| Listed {
                key: key.to_owned(),
                device: "Built-in Audio".to_owned(),
                jack: Some(key.to_owned()),
                short: key.to_owned(),
                kind: Kind::Speaker,
                live: None,
                meter: None,
                connected,
                chosen,
                name: None,
                hidden: false,
                card: None,
                details: Vec::new(),
            };
            let mixed = Mixed {
                outputs: vec![
                    device("Speakers", true, true),
                    device("Headphones", false, false),
                    device("HDMI", true, false),
                ],
                step: 5,
                limit: 100,
                ..Mixed::default()
            };
            let mut state = opened(mixed);
            let speakers = id(Control::Choose(Side::Output, number("Speakers")));
            let hdmi = id(Control::Choose(Side::Output, number("HDMI")));
            let fold = id(Control::Fold(Side::Output));
            let view = state.view().expect("open");
            let tree = mixer("perspicax-mixer-DP-1", Some((1280, 800)), Some(&view));
            assert_eq!(
                node(&tree, OUTPUT).children()[2..],
                [OUTPUTS, fold],
                "the list, then its fold"
            );
            let list = node(&tree, OUTPUTS);
            assert_eq!(
                (list.role(), list.label()),
                (Role::RadioGroup, Some("Outputs"))
            );
            let more = |key| id(Control::More(Side::Output, number(key)));
            assert_eq!(
                list.children(),
                [speakers, more("Speakers"), hdmi, more("HDMI")],
                "folded away, the headphones"
            );
            let chosen = node(&tree, speakers);
            assert_eq!(
                (chosen.role(), chosen.label(), chosen.toggled()),
                (
                    Role::RadioButton,
                    Some("Built-in Audio \u{2014} Speakers"),
                    Some(Toggled::True)
                )
            );
            assert_eq!(node(&tree, hdmi).toggled(), Some(Toggled::False));
            assert!(node(&tree, hdmi).supports_action(Action::Click));
            let button = node(&tree, fold);
            assert_eq!(
                (button.role(), button.label(), button.is_expanded()),
                (Role::Button, Some("Not connected (1)"), Some(false))
            );
            assert!(
                !node(&tree, INPUT).children().contains(&INPUTS),
                "no inputs, no list"
            );
            state.update(Event::Act(
                Control::Fold(Side::Output),
                crate::mixer::Act::Click,
            ));
            let view = state.view().expect("open");
            let tree = mixer("perspicax-mixer-DP-1", Some((1280, 800)), Some(&view));
            assert_eq!(node(&tree, fold).is_expanded(), Some(true));
            let headphones = id(Control::Choose(Side::Output, number("Headphones")));
            assert_eq!(
                node(&tree, OUTPUTS).children()[4],
                headphones,
                "after the speakers and HDMI, and their buttons"
            );
            assert_eq!(
                node(&tree, headphones).description(),
                Some("Nothing plugged in")
            );
        }
    }
}

#[cfg(feature = "audio")]
pub(crate) use osds::osd;

#[cfg(feature = "audio")]
mod osds {
    use accesskit::{Live, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};

    use super::{ROOT, window};
    use crate::layout::osd::Indicator;

    const STATUS: NodeId = NodeId(1);

    /// The volume shown on the screen: a window named `namespace`, holding
    /// a `Status`, "Volume", whose value is how loud the output is and
    /// which it is, "40%, Speakers" or "Muted, Speakers", read out politely
    /// as it changes. With nothing shown, the window alone.
    pub(crate) fn osd(
        namespace: &str,
        size: Option<(u32, u32)>,
        indicator: Option<&Indicator>,
    ) -> TreeUpdate {
        let mut root = window(namespace, size);
        let mut nodes = Vec::new();
        if let Some(indicator) = indicator {
            let mut status = Node::new(Role::Status);
            status.set_label("Volume");
            let level = &indicator.level;
            let loud = if level.muted {
                "Muted".to_owned()
            } else {
                format!("{}%", level.percent)
            };
            status.set_value(format!("{loud}, {}", indicator.name));
            status.set_live(Live::Polite);
            root.push_child(STATUS);
            nodes.push((STATUS, status));
        }
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
        use crate::model::sound::Level;

        #[test]
        fn the_volume_shown_is_a_polite_status_of_how_loud_and_where() {
            let shown = |percent, muted| {
                let indicator = Indicator {
                    name: "Speakers".to_owned(),
                    level: Level { percent, muted },
                };
                let tree = osd("perspicax-osd", Some((280, 64)), Some(&indicator));
                let (_, status) = tree
                    .nodes
                    .iter()
                    .find(|(id, _)| *id == STATUS)
                    .expect("a status")
                    .clone();
                status
            };
            let status = shown(40, false);
            assert_eq!(
                (status.role(), status.label(), status.value(), status.live()),
                (
                    Role::Status,
                    Some("Volume"),
                    Some("40%, Speakers"),
                    Some(Live::Polite)
                )
            );
            assert_eq!(shown(40, true).value(), Some("Muted, Speakers"));
            assert_eq!(osd("perspicax-osd", None, None).nodes.len(), 1);
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
                // Where it is spun to, not where it is drawn gliding there.
                let ((x, y), side) = view.ring.place(index, f64::from(view.spin), view.pointer);
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
            layout::{
                Rect as Area,
                pie::{Ring, Shape},
            },
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
                ring: Ring::new(3, Shape::default(), (960, 500), Area::new(0, 0, 1920, 1048)),
                slots: &slots,
                spin: 0,
                turn: 0.0,
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
            let ((x, y), side) = view.ring.place(0, 0.0, view.pointer);
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
