//! A menu as a tree: what it lists, in order, and what each item does.
//!
//! The root menu is the installed applications, one submenu per group,
//! with Lock and Log Out at its foot, as on Plasma's desktop. A menu file
//! may put items of its own above them, or replace the lot. The start menu
//! is the same, without the menu file's say: it is where every application
//! can always be found. A tray icon's menu is its program's, and what is
//! chosen in it is told back to that program.

use super::{
    apps::{App, Run},
    categories::Category,
    menu_file::{FileItem, MenuFile, Mode},
};

/// A menu: its items, top to bottom.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Menu {
    pub(crate) items: Vec<Item>,
}

/// One line of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    /// What is written on it. Empty for a separator.
    pub(crate) label: String,
    /// An icon's name in the theme, or a path to an image.
    pub(crate) icon: Option<String>,
    /// More words it is found by when a person types, beside its label.
    pub(crate) keywords: Vec<String>,
    pub(crate) does: Does,
}

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Does {
    /// Start a program.
    Run(Run),
    /// Open a submenu.
    Open(Menu),
    /// End the session.
    LogOut,
    /// Tell the program whose menu it is that this, its item, was chosen:
    /// a tray icon's menu, which its program builds and answers itself.
    #[cfg(feature = "tray")]
    Tell(Choice),
    /// Nothing: a line between groups of items.
    Separator,
}

/// An item of a program's own menu, as its tray icon's menu shows it.
#[cfg(feature = "tray")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Choice {
    /// The program's own number for it, which it is told back.
    pub(crate) id: i32,
    /// Greyed out: shown, and not to be chosen.
    pub(crate) enabled: bool,
    /// A tick or a dot beside it, saying whether it is on.
    pub(crate) mark: Option<Mark>,
}

/// What stands beside an item that is on or off.
#[cfg(feature = "tray")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// A tick, on or off by itself.
    Check(bool),
    /// A dot, one of a group of which one is on.
    Radio(bool),
}

/// An item's place in the tree: its index in each menu on the way down
/// from the root.
pub(crate) type Route = Vec<usize>;

/// What the session items at the root menu's foot do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Session {
    /// The command Lock runs, if there is to be a Lock.
    pub(crate) lock: Option<Run>,
    /// Whether there is a Log Out: only with a compositor to ask.
    pub(crate) log_out: bool,
}

impl Item {
    pub(crate) fn separator() -> Self {
        Self {
            label: String::new(),
            icon: None,
            keywords: Vec::new(),
            does: Does::Separator,
        }
    }

    /// Whether it can be chosen: everything but a separator, and an item
    /// its program greyed out.
    pub(crate) fn choosable(&self) -> bool {
        match &self.does {
            Does::Separator => false,
            #[cfg(feature = "tray")]
            Does::Tell(choice) => choice.enabled,
            _ => true,
        }
    }

    /// Whether it is a line between groups of items rather than an item.
    pub(crate) fn is_separator(&self) -> bool {
        matches!(self.does, Does::Separator)
    }

    /// The submenu it opens, if it opens one.
    pub(crate) fn submenu(&self) -> Option<&Menu> {
        match &self.does {
            Does::Open(menu) => Some(menu),
            _ => None,
        }
    }
}

impl Menu {
    /// The item at `route`.
    pub(crate) fn item(&self, route: &[usize]) -> Option<&Item> {
        let (&last, above) = route.split_last()?;
        self.menu(above)?.items.get(last)
    }

    /// The menu at `route`: this one for an empty route, otherwise the
    /// submenu the item there opens.
    pub(crate) fn menu(&self, route: &[usize]) -> Option<&Self> {
        route
            .iter()
            .try_fold(self, |menu, &at| menu.items.get(at)?.submenu())
    }

    /// The routes of the items a person typing `query` is looking for: the
    /// ones that start a program, anywhere in the tree, whose label or
    /// keywords hold it, ignoring case. A label that starts with it comes
    /// first, then a word of the label that does, then the rest; each by
    /// label. An item reachable twice is found once.
    pub(crate) fn find(&self, query: &str) -> Vec<Route> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        self.each_run(&mut Vec::new(), &mut |route, item| {
            let label = item.label.to_lowercase();
            let rank = if label.starts_with(&query) {
                0
            } else if label
                .split_whitespace()
                .any(|word| word.starts_with(&query))
            {
                1
            } else if label.contains(&query)
                || item
                    .keywords
                    .iter()
                    .any(|word| word.to_lowercase().contains(&query))
            {
                2
            } else {
                return;
            };
            found.push((rank, label, route.to_vec()));
        });
        found.sort();
        let mut seen = Vec::new();
        found
            .into_iter()
            .filter(|(_, _, route)| {
                let item = self.item(route);
                let fresh = !seen.contains(&item);
                seen.push(item);
                fresh
            })
            .map(|(_, _, route)| route)
            .collect()
    }

    /// Call `visit` with every item that starts a program, and its route.
    fn each_run(&self, route: &mut Route, visit: &mut impl FnMut(&[usize], &Item)) {
        for (at, item) in self.items.iter().enumerate() {
            route.push(at);
            match &item.does {
                Does::Run(_) => visit(route, item),
                Does::Open(menu) => menu.each_run(route, visit),
                Does::LogOut | Does::Separator => {}
                #[cfg(feature = "tray")]
                Does::Tell(_) => {}
            }
            route.pop();
        }
    }

    /// No separator first, last, or beside another, here or in any submenu.
    pub(crate) fn tidy(mut self) -> Self {
        let mut items: Vec<Item> = Vec::with_capacity(self.items.len());
        for mut item in self.items.drain(..) {
            if let Does::Open(menu) = item.does {
                item.does = Does::Open(menu.tidy());
            }
            let after_separator = items.last().is_none_or(Item::is_separator);
            if !item.is_separator() || !after_separator {
                items.push(item);
            }
        }
        if items.last().is_some_and(Item::is_separator) {
            items.pop();
        }
        Self { items }
    }
}

/// The root menu: the applications by group, then Lock and Log Out; with
/// a menu file's items above them, or in their place.
pub(crate) fn root(apps: &[App], file: Option<&MenuFile>, session: &Session) -> Menu {
    let standard = || {
        let mut items = applications(apps);
        items.push(Item::separator());
        items.extend(session_items(session));
        items
    };
    let items = match file {
        None => standard(),
        Some(file) => {
            let mut items = resolve(&file.items, apps, session);
            if file.mode == Mode::Extend {
                items.push(Item::separator());
                items.extend(standard());
            }
            items
        }
    };
    Menu { items }.tidy()
}

/// The start menu: the applications by group, then Lock and Log Out.
pub(crate) fn start(apps: &[App], session: &Session) -> Menu {
    root(apps, None, session)
}

/// One submenu per group that has an application in it, each listing its
/// applications by name.
fn applications(apps: &[App]) -> Vec<Item> {
    Category::ALL
        .into_iter()
        .filter_map(|category| {
            let mut items: Vec<Item> = apps
                .iter()
                .filter(|app| Category::of(&app.categories) == category)
                .map(|app| launcher(app, None, None))
                .collect();
            items.sort_by_cached_key(|item| item.label.to_lowercase());
            (!items.is_empty()).then(|| Item {
                label: category.label().to_owned(),
                icon: Some(category.icon().to_owned()),
                keywords: Vec::new(),
                does: Does::Open(Menu { items }),
            })
        })
        .collect()
}

/// Lock and Log Out, as far as this session has them.
fn session_items(session: &Session) -> Vec<Item> {
    let lock = session.lock.clone().map(|run| Item {
        label: "Lock".to_owned(),
        icon: Some("system-lock-screen".to_owned()),
        keywords: Vec::new(),
        does: Does::Run(run),
    });
    let log_out = session.log_out.then(|| Item {
        label: "Log Out".to_owned(),
        icon: Some("system-log-out".to_owned()),
        keywords: Vec::new(),
        does: Does::LogOut,
    });
    lock.into_iter().chain(log_out).collect()
}

/// An item that starts `app`, labelled and drawn as given or as the app is.
fn launcher(app: &App, label: Option<&str>, icon: Option<&str>) -> Item {
    Item {
        label: label.unwrap_or(&app.name).to_owned(),
        icon: icon.map(str::to_owned).or_else(|| app.icon.clone()),
        keywords: app.keywords.clone(),
        does: Does::Run(app.run.clone()),
    }
}

/// A menu file's items as menu items. An `app` that is not installed is
/// left out, as a menu item that does nothing would be worse.
fn resolve(written: &[FileItem], apps: &[App], session: &Session) -> Vec<Item> {
    let mut items = Vec::new();
    for item in written {
        match item {
            FileItem::Run { label, icon, run } => items.push(Item {
                label: label.clone(),
                icon: icon.clone(),
                keywords: Vec::new(),
                does: Does::Run(run.clone()),
            }),
            FileItem::App { id, label, icon } => {
                let id = id.strip_suffix(".desktop").unwrap_or(id);
                match apps.iter().find(|app| app.id == id) {
                    Some(app) => items.push(launcher(app, label.as_deref(), icon.as_deref())),
                    None => tracing::info!("the menu file names {id}, which is not installed"),
                }
            }
            FileItem::Open {
                label,
                icon,
                items: below,
            } => items.push(Item {
                label: label.clone(),
                icon: icon.clone(),
                keywords: Vec::new(),
                does: Does::Open(Menu {
                    items: resolve(below, apps, session),
                }),
            }),
            FileItem::Separator => items.push(Item::separator()),
            FileItem::Applications => items.extend(applications(apps)),
            FileItem::Session => items.extend(session_items(session)),
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::menu_file;

    fn app(id: &str, name: &str, categories: &str) -> App {
        App {
            id: id.to_owned(),
            name: name.to_owned(),
            comment: None,
            icon: Some(id.to_owned()),
            run: run(&[id]),
            categories: categories.split(';').map(str::to_owned).collect(),
            keywords: Vec::new(),
            wm_class: None,
        }
    }

    fn run(argv: &[&str]) -> Run {
        Run {
            argv: argv.iter().map(|word| (*word).to_owned()).collect(),
            terminal: false,
            dir: None,
        }
    }

    fn apps() -> Vec<App> {
        vec![
            app("firefox", "Firefox", "Network;WebBrowser"),
            app("foot", "Foot", "System;TerminalEmulator"),
            app("gedit", "Text Editor", "Utility;TextEditor"),
            app("htop", "htop", "System;Monitor"),
        ]
    }

    fn session() -> Session {
        Session {
            lock: Some(run(&["swaylock"])),
            log_out: true,
        }
    }

    fn labels(menu: &Menu) -> Vec<&str> {
        menu.items
            .iter()
            .map(|item| match item.does {
                Does::Separator => "--",
                _ => item.label.as_str(),
            })
            .collect()
    }

    fn file(text: &str) -> MenuFile {
        menu_file::parse(text, None).expect("a menu file")
    }

    #[test]
    fn the_root_menu_is_the_applications_by_group_then_the_session() {
        let menu = root(&apps(), None, &session());
        assert_eq!(
            labels(&menu),
            ["Accessories", "Internet", "System", "--", "Lock", "Log Out"]
        );
        assert_eq!(labels(menu.menu(&[2]).unwrap()), ["Foot", "htop"]);
        assert_eq!(
            menu.item(&[1, 0]).map(|item| &item.does),
            Some(&Does::Run(run(&["firefox"])))
        );
        assert_eq!(
            labels(&root(&[], None, &Session::default())),
            Vec::<&str>::new(),
            "nothing installed and no session: no stray separator"
        );
    }

    #[test]
    fn a_replace_menu_file_is_the_whole_root_menu() {
        let file = file(
            r#"
            mode = "replace"
            [[items]]
            label = "Terminal"
            exec = ["foot"]
            [[items]]
            app = "firefox.desktop"
            label = "Web"
            [[items]]
            app = "not-installed"
            [[items]]
            separator = true
            [[items]]
            label = "Apps"
            items = [{ applications = true }]
            "#,
        );
        let menu = root(&apps(), Some(&file), &session());
        assert_eq!(labels(&menu), ["Terminal", "Web", "--", "Apps"]);
        assert_eq!(
            menu.items[1].icon.as_deref(),
            Some("firefox"),
            "the app's icon"
        );
        assert_eq!(
            labels(menu.menu(&[3]).unwrap()),
            ["Accessories", "Internet", "System"]
        );
    }

    #[test]
    fn an_extend_menu_file_goes_above_the_applications() {
        let file = file(
            r#"
            [[items]]
            label = "Terminal"
            exec = ["foot"]
            "#,
        );
        let menu = root(&apps(), Some(&file), &session());
        assert_eq!(
            labels(&menu),
            [
                "Terminal",
                "--",
                "Accessories",
                "Internet",
                "System",
                "--",
                "Lock",
                "Log Out"
            ]
        );
    }

    #[test]
    fn the_start_menu_lists_every_application_whatever_the_menu_file_says() {
        let replaced = root(&apps(), Some(&file("mode = \"replace\"\n")), &session());
        assert!(replaced.items.is_empty(), "the root menu as the file says");
        assert_eq!(
            labels(&start(&apps(), &session())),
            ["Accessories", "Internet", "System", "--", "Lock", "Log Out"]
        );
    }

    #[test]
    fn typing_finds_programs_whose_names_start_with_it_first() {
        let menu = root(
            &[
                app("files", "Files", "System"),
                app("gimp", "GNU Image Manipulation Program", "Graphics"),
                app("profile", "Profiler", "Development"),
                App {
                    keywords: vec!["browser".to_owned()],
                    ..app("firefox", "Firefox", "Network")
                },
            ],
            None,
            &session(),
        );
        let found = |query: &str| -> Vec<&str> {
            menu.find(query)
                .iter()
                .map(|route| menu.item(route).unwrap().label.as_str())
                .collect()
        };
        assert_eq!(found("fi"), ["Files", "Firefox", "Profiler"]);
        assert_eq!(found("IMAGE"), ["GNU Image Manipulation Program"]);
        assert_eq!(found("brow"), ["Firefox"], "by a keyword");
        assert_eq!(found("lock"), ["Lock"], "the session's programs too");
        assert!(found("  ").is_empty());
    }
}
