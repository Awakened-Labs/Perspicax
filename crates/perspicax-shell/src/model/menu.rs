//! A menu as a tree: what it lists, in order, and what each item does.
//!
//! The root menu is the installed applications, one submenu per group,
//! with the ways to leave at its foot, as on Plasma's desktop: Lock, Log
//! Out, Suspend, Restart and Shut Down, as `[shell] leave` lists them and
//! as far as each can work here. Choosing one is the whole of it: there is
//! no dialog to confirm, and Suspend does not lock first (that is for
//! swayidle's `before-sleep`, so that every way to sleep locks). A menu file
//! may put items of its own above them, or replace the lot. The start menu
//! is built the same way, from `[shell.start-menu]`'s items and never the
//! menu file's: with none written, or with items that go above the rest, it
//! is where every application can always be found, and one written in
//! place of the rest can still find them all by typing (`search = "all"`).
//! A tray icon's menu is its program's, and what is
//! chosen in it is told back to that program.

use std::path::PathBuf;

use perspicax_config::{Leave, Shell, start_menu::Search};

use super::{
    apps::{App, Run},
    categories::Category,
    fs::{Fs, which},
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

/// The session items at the root menu's foot: each way to leave, in order,
/// and what choosing it does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Session {
    pub(crate) leave: Vec<(Leave, Does)>,
}

impl Session {
    /// The ways to leave `shell` lists that can work here: Log Out with a
    /// compositor to ask (`log_out`), and each of the others with its
    /// program installed, found on `path` in `fs`. An item that would do
    /// nothing is worse than none.
    pub(crate) fn of(shell: &Shell, log_out: bool, fs: &impl Fs, path: &[PathBuf]) -> Self {
        let leave = shell
            .leave
            .iter()
            .filter_map(|&leave| {
                let does = match shell.runs(leave) {
                    None => log_out.then_some(Does::LogOut)?,
                    Some(argv) => {
                        which(fs, argv.first()?, path)?;
                        Does::Run(Run {
                            argv: argv.to_vec(),
                            terminal: false,
                            dir: None,
                        })
                    }
                };
                Some((leave, does))
            })
            .collect();
        Self { leave }
    }
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

/// The root menu: the applications by group, then the ways to leave; with
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

/// The start menu: the applications by group, then the ways to leave; with
/// the items `[shell.start-menu]` writes above them, or in their place, as
/// a menu file's go in the root menu.
pub(crate) fn start(apps: &[App], written: Option<&MenuFile>, session: &Session) -> Menu {
    root(apps, written, session)
}

/// What typing in the start menu looks through.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum Finds {
    /// The start menu itself.
    #[default]
    Itself,
    /// This, which holds more: the start menu's own items first, each where
    /// the start menu has it, and then the rest.
    Among(Menu),
    /// Nothing: the start menu has no search line.
    Nothing,
}

/// What typing in the start menu built from `written` finds, as `search`
/// says. Finding everything in a menu written in place of everything is
/// finding in it as it would be above everything; a menu with everything
/// in it already finds everything in itself.
pub(crate) fn finds(
    apps: &[App],
    written: Option<&MenuFile>,
    search: Search,
    session: &Session,
) -> Finds {
    match (search, written) {
        (Search::Nothing, _) => Finds::Nothing,
        (Search::All, Some(written)) if written.mode == Mode::Replace => {
            let above = MenuFile {
                mode: Mode::Extend,
                items: written.items.clone(),
            };
            Finds::Among(start(apps, Some(&above), session))
        }
        _ => Finds::Itself,
    }
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

/// The ways to leave, named and drawn as Plasma's are. Typing finds the
/// ones that run a program by another desktop's word for them too.
fn session_items(session: &Session) -> Vec<Item> {
    session
        .leave
        .iter()
        .map(|(leave, does)| {
            let (label, icon, keywords): (_, _, &[&str]) = match leave {
                Leave::Lock => ("Lock", "system-lock-screen", &[]),
                Leave::LogOut => ("Log Out", "system-log-out", &[]),
                Leave::Suspend => ("Suspend", "system-suspend", &["sleep"]),
                Leave::Reboot => ("Restart", "system-reboot", &["reboot"]),
                Leave::PowerOff => ("Shut Down", "system-shutdown", &["power off"]),
            };
            Item {
                label: label.to_owned(),
                icon: Some(icon.to_owned()),
                keywords: keywords.iter().map(|&word| word.to_owned()).collect(),
                does: does.clone(),
            }
        })
        .collect()
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

/// A menu file's items, or the start menu's, as menu items. An `app` that is
/// not installed is left out, as a menu item that does nothing would be
/// worse: the same config may be used where it is not.
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
                    None => tracing::info!("the application {id} is not installed; left out"),
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
    use perspicax_config::{Profile, ShellBuilt};

    use super::*;
    use crate::model::{fs::fake::Files, menu_file};

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
            leave: vec![
                (Leave::Lock, Does::Run(run(&["swaylock"]))),
                (Leave::LogOut, Does::LogOut),
            ],
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

    /// The start menu `[shell.start-menu]` writes with `text`, as the shell
    /// takes it from the config.
    fn written(text: &str) -> MenuFile {
        let shell =
            perspicax_config::shell(&format!("[shell.start-menu]\n{text}"), ShellBuilt::FULL)
                .expect("a config");
        let start_menu = shell.start_menu.expect("a start menu");
        menu_file::written(start_menu.mode, start_menu.items, None)
    }

    #[test]
    fn with_no_start_menu_written_the_start_menu_is_every_application_whatever_the_menu_file_says()
    {
        let replaced = root(&apps(), Some(&file("mode = \"replace\"\n")), &session());
        assert!(replaced.items.is_empty(), "the root menu as the file says");
        let menu = start(&apps(), None, &session());
        assert_eq!(
            labels(&menu),
            ["Accessories", "Internet", "System", "--", "Lock", "Log Out"]
        );
        assert_eq!(menu, root(&apps(), None, &session()), "as it always was");
    }

    #[test]
    fn a_replacing_start_menu_is_its_items_then_the_ways_to_leave_where_written() {
        let written = written(
            r#"
            mode = "replace"
            items = [{ app = "firefox" }, { separator = true }, { session = true }]
            "#,
        );
        let menu = start(&apps(), Some(&written), &session());
        assert_eq!(labels(&menu), ["Firefox", "--", "Lock", "Log Out"]);
        assert_eq!(
            menu.items[0].does,
            Does::Run(run(&["firefox"])),
            "the application, as installed"
        );
    }

    #[test]
    fn a_start_menu_of_only_the_session_is_the_ways_to_leave() {
        let written = written("mode = \"replace\"\nitems = [{ session = true }]");
        let menu = start(&apps(), Some(&written), &session());
        assert_eq!(labels(&menu), ["Lock", "Log Out"]);
    }

    #[test]
    fn an_extending_start_menu_goes_above_the_applications() {
        let written = written("items = [{ label = \"Terminal\", exec = [\"foot\", \"~/x\"] }]");
        let menu = start(&apps(), Some(&written), &session());
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
    fn a_start_menu_app_that_is_not_installed_is_left_out() {
        let written = written(
            "mode = \"replace\"\nitems = [{ app = \"steam\" }, { app = \"gedit.desktop\" }]",
        );
        let menu = start(&apps(), Some(&written), &session());
        assert_eq!(labels(&menu), ["Text Editor"]);
    }

    #[test]
    fn a_start_menu_finds_what_it_holds_everything_or_nothing_as_written() {
        let favourites =
            written("mode = \"replace\"\nitems = [{ app = \"firefox\" }, { separator = true }]");
        let menu = start(&apps(), Some(&favourites), &session());
        let searching = |search| finds(&apps(), Some(&favourites), search, &session());
        assert_eq!(searching(Search::Menu), Finds::Itself);
        assert_eq!(searching(Search::Nothing), Finds::Nothing);
        let Finds::Among(all) = searching(Search::All) else {
            panic!("a tree of everything");
        };
        assert_eq!(
            labels(&all),
            [
                "Firefox",
                "--",
                "Accessories",
                "Internet",
                "System",
                "--",
                "Lock",
                "Log Out"
            ]
        );
        assert_eq!(
            all.items[..menu.items.len()],
            menu.items,
            "the start menu's own items where it has them"
        );
        assert_eq!(all.find("htop"), [vec![4, 1]], "and what it does not hold");

        let above = written("items = [{ app = \"firefox\" }]");
        assert_eq!(
            finds(&apps(), Some(&above), Search::All, &session()),
            Finds::Itself,
            "above everything, it holds everything"
        );
        assert_eq!(finds(&apps(), None, Search::All, &session()), Finds::Itself);
    }

    /// A favourite above the groups is also in its group; typing finds it
    /// once.
    #[test]
    fn an_app_listed_and_in_its_group_is_found_once() {
        let written = written("items = [{ app = \"firefox\" }]");
        let menu = start(&apps(), Some(&written), &session());
        assert_eq!(menu.find("fire"), [vec![0]], "the first it comes to");
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

    #[test]
    fn a_menu_files_session_item_is_the_ways_to_leave() {
        let session = Session {
            leave: vec![
                (Leave::PowerOff, Does::Run(run(&["loginctl", "poweroff"]))),
                (Leave::Lock, Does::Run(run(&["swaylock"]))),
            ],
        };
        let file = file("mode = \"replace\"\n[[items]]\nsession = true\n");
        assert_eq!(
            labels(&root(&apps(), Some(&file), &session)),
            ["Shut Down", "Lock"]
        );
    }

    #[test]
    fn each_way_to_leave_shows_in_its_order_where_it_can_work() {
        let files = Files::default().program("/usr/bin/loginctl");
        let path = [PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")];
        let shell = Shell::profile(Profile::Classic, ShellBuilt::FULL);
        let menu = start(&[], None, &Session::of(&shell, true, &files, &path));
        assert_eq!(
            labels(&menu),
            ["Log Out", "Suspend", "Restart", "Shut Down"],
            "no swaylock installed, so no Lock"
        );
        let icons: Vec<_> = menu.items.iter().map(|item| item.icon.as_deref()).collect();
        assert_eq!(
            icons,
            [
                Some("system-log-out"),
                Some("system-suspend"),
                Some("system-reboot"),
                Some("system-shutdown"),
            ]
        );
        assert_eq!(
            menu.items[3].does,
            Does::Run(run(&["loginctl", "poweroff"])),
            "the program, not the path it was found at"
        );

        let shell = Shell {
            leave: vec![Leave::PowerOff, Leave::Lock, Leave::LogOut],
            lock: vec!["/opt/lock/bin/lock".to_owned(), "-f".to_owned()],
            ..shell
        };
        let files = files.program("/opt/lock/bin/lock");
        assert_eq!(
            labels(&start(
                &[],
                None,
                &Session::of(&shell, false, &files, &path)
            )),
            ["Shut Down", "Lock"],
            "in the order written, a path as written, and no compositor to log out of"
        );
        assert_eq!(
            labels(&start(
                &[],
                None,
                &Session::of(&shell, true, &Files::default(), &path)
            )),
            ["Log Out"],
            "nothing installed"
        );
    }

    #[test]
    fn a_way_to_leave_is_found_by_another_desktops_word_for_it() {
        let files = Files::default().program("/usr/bin/loginctl");
        let shell = Shell::profile(Profile::Classic, ShellBuilt::FULL);
        let session = Session::of(&shell, true, &files, &[PathBuf::from("/usr/bin")]);
        let menu = start(&[], None, &session);
        let found = |query: &str| -> Vec<&str> {
            menu.find(query)
                .iter()
                .map(|route| menu.item(route).unwrap().label.as_str())
                .collect()
        };
        assert_eq!(found("sleep"), ["Suspend"]);
        assert_eq!(found("reboot"), ["Restart"]);
        assert_eq!(found("power"), ["Shut Down"]);
        assert_eq!(found("shut"), ["Shut Down"]);
    }
}
