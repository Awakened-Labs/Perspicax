//! The tray: other programs' status icons, as the StatusNotifierItem
//! specification has them, and their menus, as `com.canonical.dbusmenu`
//! describes them, worked out as values. Reading them off the bus is
//! `crate::tray`'s.
//!
//! A program gives its icon by name, from the icon theme or a folder of its
//! own, or as pictures at the sizes it drew; a name is drawn first, as the
//! specification asks. It says whether it is passive, which the tray does
//! not show, as Plasma's and waybar's do not, active, or asking for
//! attention, when it is drawn with its attention icon if it has one.
//!
//! A left click activates a program's icon and a middle click activates it
//! the other way; a right click opens its menu, or asks the program to show
//! one itself if it has none to read. An icon that is only a menu opens it
//! on a left click too.

use std::path::PathBuf;

use tiny_skia::{IntSize, Pixmap};

use super::{
    Button,
    menu::{Choice, Does, Item as Line, Mark, Menu},
};

/// Where a status icon is served when its program gives only its bus name.
pub(crate) const ITEM_PATH: &str = "/StatusNotifierItem";

/// A program's status icon, as the tray shows it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Item {
    /// What it is called: its tooltip's title, or its title, or its id,
    /// whichever is first not blank.
    pub(crate) title: String,
    pub(crate) status: Status,
    pub(crate) icon: Icon,
    /// Drawn in its place while it asks for attention.
    pub(crate) attention: Icon,
    /// It has a menu to read and show.
    pub(crate) menu: bool,
    /// It is only a menu: a left click opens it too.
    pub(crate) only_menu: bool,
}

/// What a program says of its status icon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Status {
    /// Nothing to see: not shown.
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

/// An icon, as a program gives one: by name, in the icon theme or in a
/// folder of the program's own, or as pictures.
#[derive(Debug, Clone, Default)]
pub(crate) struct Icon {
    pub(crate) name: Option<String>,
    /// The program's own folder of icons, looked in before the theme.
    pub(crate) folder: Option<PathBuf>,
    /// Pictures of it, at the sizes its program drew, as premultiplied RGBA.
    pub(crate) pixmaps: Vec<Pixmap>,
}

/// What a press on a status icon asks of its program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Press {
    /// What a click on it is for.
    Activate,
    /// What a middle click is for, if anything.
    SecondaryActivate,
    /// Read its menu, to open beside it.
    Menu,
    /// Show a menu of its own: it has none to read.
    ContextMenu,
}

impl Status {
    /// The status a program writes as `text`. Anything else is active: a
    /// program that says something unknown still has an icon to show.
    pub(crate) fn parse(text: &str) -> Self {
        match text {
            "Passive" => Self::Passive,
            "NeedsAttention" => Self::NeedsAttention,
            _ => Self::Active,
        }
    }
}

impl Icon {
    /// Whether there is nothing to draw it from.
    pub(crate) fn is_empty(&self) -> bool {
        self.name.is_none() && self.pixmaps.is_empty()
    }
}

impl Item {
    /// Whether the tray shows it: not while it is passive.
    pub(crate) fn shown(&self) -> bool {
        self.status != Status::Passive
    }

    /// The icon to draw: its attention icon while it asks for attention and
    /// has one, otherwise its own.
    pub(crate) fn drawn(&self) -> &Icon {
        if self.status == Status::NeedsAttention && !self.attention.is_empty() {
            &self.attention
        } else {
            &self.icon
        }
    }

    /// What pressing `button` on it asks of its program.
    pub(crate) fn pressed(&self, button: Button) -> Option<Press> {
        match button {
            Button::Left if self.only_menu && self.menu => Some(Press::Menu),
            Button::Left => Some(Press::Activate),
            Button::Middle => Some(Press::SecondaryActivate),
            Button::Right if self.menu => Some(Press::Menu),
            Button::Right => Some(Press::ContextMenu),
            Button::Other => None,
        }
    }
}

/// A picture as the specification sends one, `width` by `height` pixels
/// of four bytes each, alpha, red, green, blue, in network byte order, the
/// colours not multiplied by the alpha; as premultiplied RGBA, which is what
/// tiny-skia draws. `None` for a size the bytes do not fill.
pub(crate) fn pixmap(width: i32, height: i32, argb: &[u8]) -> Option<Pixmap> {
    let size = IntSize::from_wh(u32::try_from(width).ok()?, u32::try_from(height).ok()?)?;
    let length = (size.width() as usize)
        .checked_mul(size.height() as usize)?
        .checked_mul(4)?;
    let pixels = argb.get(..length)?;
    let rgba = pixels
        .chunks_exact(4)
        .flat_map(|pixel| {
            let [a, r, g, b] = [pixel[0], pixel[1], pixel[2], pixel[3]];
            let times = |colour: u8| ((u16::from(colour) * u16::from(a) + 127) / 255) as u8;
            [times(r), times(g), times(b), a]
        })
        .collect();
    Pixmap::from_vec(rgba, size)
}

/// Of `pixmaps`, the one to draw `side` pixels square: the smallest that
/// is at least that big, since a picture shrunk loses less than one blown
/// up; or, with none so big, the largest.
pub(crate) fn chosen(pixmaps: &[Pixmap], side: u32) -> Option<&Pixmap> {
    let shortest = |pixmap: &&Pixmap| pixmap.width().min(pixmap.height());
    pixmaps
        .iter()
        .filter(|pixmap| shortest(pixmap) >= side)
        .min_by_key(shortest)
        .or_else(|| pixmaps.iter().max_by_key(shortest))
}

/// Where the status icon a watcher lists as `service` is served: its bus
/// name, and the path of its object, the specification's own when none is
/// written.
pub(crate) fn address(service: &str) -> (&str, &str) {
    match service.find('/') {
        Some(at) => (&service[..at], &service[at..]),
        None => (service, ITEM_PATH),
    }
}

/// How a watcher lists the status icon `sender` registered as `service`:
/// a bus name as it is, and a path, which is how libappindicator registers,
/// on the bus name of the program that sent it.
pub(crate) fn registered(service: &str, sender: &str) -> String {
    if service.starts_with('/') {
        format!("{sender}{service}")
    } else if service.is_empty() {
        sender.to_owned()
    } else {
        service.to_owned()
    }
}

/// An item of a program's menu, and the items of its submenu, as dbusmenu
/// lays them out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    /// The program's own number for it.
    pub(crate) id: i32,
    /// A line between groups rather than an item.
    pub(crate) separator: bool,
    /// What it says, `_` before the letter that chooses it by key.
    pub(crate) label: String,
    pub(crate) enabled: bool,
    pub(crate) visible: bool,
    /// An icon's name in the theme.
    pub(crate) icon: Option<String>,
    pub(crate) mark: Option<Mark>,
    /// It opens a submenu, which may be filled only when it is about to
    /// be shown.
    pub(crate) submenu: bool,
    pub(crate) children: Vec<Node>,
}

impl Default for Node {
    /// An item as dbusmenu has one with nothing said of it: shown, and
    /// enabled.
    fn default() -> Self {
        Self {
            id: 0,
            separator: false,
            label: String::new(),
            enabled: true,
            visible: true,
            icon: None,
            mark: None,
            submenu: false,
            children: Vec::new(),
        }
    }
}

impl Node {
    /// The numbers of the submenus in it, at any depth, that are to be
    /// filled before they are shown: the ones with nothing in them yet.
    pub(crate) fn unfilled(&self) -> Vec<i32> {
        let mut unfilled = Vec::new();
        let mut below = vec![self];
        while let Some(node) = below.pop() {
            if node.submenu && node.children.is_empty() && node.id != self.id {
                unfilled.push(node.id);
            }
            below.extend(&node.children);
        }
        unfilled
    }
}

/// The menu a program's layout `root` describes: each item it shows, its
/// label without the marks of the key that chooses it, and its submenus.
/// A submenu with nothing in it is an item to choose.
pub(crate) fn menu(root: &Node) -> Menu {
    let items = root
        .children
        .iter()
        .filter(|node| node.visible)
        .map(|node| {
            if node.separator {
                return Line::separator();
            }
            let does = if node.children.is_empty() {
                Does::Tell(Choice {
                    id: node.id,
                    enabled: node.enabled,
                    mark: node.mark,
                })
            } else {
                Does::Open(menu(node))
            };
            Line {
                label: unmarked(&node.label),
                icon: node.icon.clone(),
                keywords: Vec::new(),
                does,
            }
        })
        .collect();
    Menu { items }.tidy()
}

/// `label` without its key marks: an `_` before the letter that chooses an
/// item by key is dropped, and `__` is an `_`.
fn unmarked(label: &str) -> String {
    let mut text = String::with_capacity(label.len());
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        match c {
            '_' => text.extend(chars.next()),
            c => text.push(c),
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The premultiplied RGBA of pixel `x`, `y` of `pixmap`.
    fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * pixmap.width() + x) * 4) as usize;
        pixmap.data()[at..at + 4].try_into().expect("four bytes")
    }

    #[test]
    fn an_icon_pixmap_in_network_order_becomes_premultiplied_rgba() {
        // Two pixels: an opaque orange, and a blue at half alpha.
        let argb = [0xff, 0xff, 0x80, 0x00, 0x80, 0x00, 0x00, 0xff];
        let picture = pixmap(2, 1, &argb).expect("a picture");
        assert_eq!((picture.width(), picture.height()), (2, 1));
        assert_eq!(pixel(&picture, 0, 0), [0xff, 0x80, 0x00, 0xff], "opaque");
        assert_eq!(
            pixel(&picture, 1, 0),
            [0x00, 0x00, 0x80, 0x80],
            "each colour scaled by alpha"
        );

        assert!(pixmap(2, 2, &argb).is_none(), "too few bytes for the size");
        assert!(pixmap(0, 1, &[]).is_none(), "no size");
        assert!(pixmap(-1, 1, &argb).is_none());
        assert!(
            pixmap(1, 1, &argb).is_some(),
            "bytes beyond the size are left"
        );
    }

    #[test]
    fn the_smallest_pixmap_not_smaller_than_asked_is_chosen() {
        let square = |side: u32| Pixmap::new(side, side).expect("a picture");
        let pixmaps = [square(64), square(16), square(32), square(24)];
        let side = |asked: u32| chosen(&pixmaps, asked).map(Pixmap::width);
        assert_eq!(side(22), Some(24), "the next size up, shrunk");
        assert_eq!(side(32), Some(32), "the size asked for");
        assert_eq!(side(44), Some(64));
        assert_eq!(side(96), Some(64), "none so big: the largest");
        assert_eq!(chosen(&[], 22).map(Pixmap::width), None);
    }

    #[test]
    fn a_service_is_found_at_its_bus_name_and_path() {
        assert_eq!(
            address("org.kde.StatusNotifierItem-1234-1"),
            ("org.kde.StatusNotifierItem-1234-1", ITEM_PATH),
            "a bus name alone is served at the specification's path"
        );
        assert_eq!(
            address(":1.42/org/ayatana/NotificationItem/nm"),
            (":1.42", "/org/ayatana/NotificationItem/nm")
        );

        assert_eq!(
            registered("/org/ayatana/NotificationItem/nm", ":1.42"),
            ":1.42/org/ayatana/NotificationItem/nm",
            "a path is on the bus name of the program that sent it"
        );
        assert_eq!(registered("org.example.Item", ":1.42"), "org.example.Item");
        assert_eq!(registered("", ":1.42"), ":1.42");
    }

    #[test]
    fn a_press_asks_what_plasma_asks() {
        let item = |menu: bool, only_menu: bool| Item {
            menu,
            only_menu,
            ..Item::default()
        };
        let with_menu = item(true, false);
        assert_eq!(with_menu.pressed(Button::Left), Some(Press::Activate));
        assert_eq!(with_menu.pressed(Button::Right), Some(Press::Menu));
        assert_eq!(
            with_menu.pressed(Button::Middle),
            Some(Press::SecondaryActivate)
        );
        assert_eq!(with_menu.pressed(Button::Other), None);
        assert_eq!(
            item(false, false).pressed(Button::Right),
            Some(Press::ContextMenu),
            "no menu to read: the program shows its own"
        );
        assert_eq!(
            item(true, true).pressed(Button::Left),
            Some(Press::Menu),
            "an icon that is only a menu opens it on a left click"
        );
    }

    #[test]
    fn a_passive_icon_is_hidden_and_one_asking_for_attention_shows_its_attention_icon() {
        let named = |name: &str| Icon {
            name: Some(name.to_owned()),
            ..Icon::default()
        };
        let mut item = Item {
            status: Status::parse("Passive"),
            icon: named("mail"),
            attention: named("mail-unread"),
            ..Item::default()
        };
        assert!(!item.shown());
        item.status = Status::parse("NeedsAttention");
        assert!(item.shown());
        assert_eq!(item.drawn().name.as_deref(), Some("mail-unread"));
        item.attention = Icon::default();
        assert_eq!(
            item.drawn().name.as_deref(),
            Some("mail"),
            "no attention icon: its own"
        );
        item.status = Status::parse("Active");
        assert_eq!(item.drawn().name.as_deref(), Some("mail"));
        assert_eq!(
            Status::parse("Unheard-of"),
            Status::Active,
            "an unknown status is shown"
        );
    }

    #[test]
    fn a_dbusmenu_layout_becomes_a_menu() {
        let node = |id: i32, label: &str| Node {
            id,
            label: label.to_owned(),
            ..Node::default()
        };
        let root = Node {
            submenu: true,
            children: vec![
                Node {
                    separator: true,
                    ..node(1, "")
                },
                Node {
                    icon: Some("document-open".to_owned()),
                    ..node(2, "_Open")
                },
                Node {
                    mark: Some(Mark::Check(true)),
                    ..node(3, "Do not _disturb")
                },
                Node {
                    enabled: false,
                    ..node(4, "Snake__case")
                },
                Node {
                    visible: false,
                    ..node(5, "Hidden")
                },
                Node {
                    separator: true,
                    ..node(6, "")
                },
                Node {
                    submenu: true,
                    children: vec![
                        Node {
                            mark: Some(Mark::Radio(true)),
                            ..node(8, "Fast")
                        },
                        Node {
                            mark: Some(Mark::Radio(false)),
                            ..node(9, "Slow")
                        },
                    ],
                    ..node(7, "_Speed")
                },
                Node {
                    separator: true,
                    ..node(10, "")
                },
            ],
            ..node(0, "")
        };
        let menu = menu(&root);
        let tell =
            |id: i32, enabled: bool, mark: Option<Mark>| Does::Tell(Choice { id, enabled, mark });
        let lines: Vec<(&str, Option<&str>, &Does)> = menu
            .items
            .iter()
            .map(|item| (item.label.as_str(), item.icon.as_deref(), &item.does))
            .collect();
        assert_eq!(
            lines[..3],
            [
                ("Open", Some("document-open"), &tell(2, true, None)),
                (
                    "Do not disturb",
                    None,
                    &tell(3, true, Some(Mark::Check(true)))
                ),
                ("Snake_case", None, &tell(4, false, None)),
            ],
            "no separator first, the key marks gone, and the hidden left out"
        );
        assert!(menu.items[3].is_separator());
        assert_eq!(menu.items.len(), 5, "and none last");
        let speed = &menu.items[4];
        assert_eq!(speed.label, "Speed");
        let below: Vec<(&str, &Does)> = speed
            .submenu()
            .expect("a submenu")
            .items
            .iter()
            .map(|item| (item.label.as_str(), &item.does))
            .collect();
        assert_eq!(
            below,
            [
                ("Fast", &tell(8, true, Some(Mark::Radio(true)))),
                ("Slow", &tell(9, true, Some(Mark::Radio(false)))),
            ]
        );
        assert!(!menu.items[2].choosable(), "greyed out");

        let lazy = Node {
            children: vec![Node {
                submenu: true,
                children: vec![
                    node(12, "Recent"),
                    Node {
                        submenu: true,
                        ..node(13, "Older")
                    },
                ],
                ..node(11, "Files")
            }],
            ..node(0, "")
        };
        assert_eq!(lazy.unfilled(), [13], "an empty submenu, to be filled");
    }
}
