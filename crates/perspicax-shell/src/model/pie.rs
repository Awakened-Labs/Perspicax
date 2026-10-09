//! A pie, as the config writes it, turned into what it shows: a slot for
//! each program, application and submenu, with the icons to try for each.
//!
//! An icon is looked for first in the pie's own folder of icons, as
//! `<name>.png` (or `.svg`), whatever the case of the name: PieDock's
//! `~/.piedock/icons`, kept as it is. Then in the icon theme. A program's
//! icon is its label's unless it names one; an application's is its own,
//! from its desktop entry, unless the pie names another. An application
//! is named by its desktop file ID, or by the path to a desktop file, one
//! with a `/` in it, which is read even when it says not to be listed: the
//! person named it.
//!
//! A running window is known by a name, as PieDock knew one: the first of
//! the pie's aliases that matches its app-id or title, else its app-id, in
//! lower case. A program's slot stands for the windows of its label's
//! name, an application's for those its desktop entry claims too, and a
//! submenu's for all of those below it. Where the first ring has
//! `running`, each running application no slot of the first ring stands
//! for gets a slot of its own there, though a submenu may have it: the
//! first ring is where any window is one click away.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use perspicax_config::{
    desktop::{self, Locale},
    menu::Item,
    pie::{Alias, Match},
};

use super::{
    apps::{self, App, Run},
    fs::Fs,
    image, menu_file,
};

/// The icon of a program that has none of its own.
const PROGRAM: &str = "application-x-executable";
/// The icon of a submenu that has none of its own.
const SUBMENU: &str = "folder";

/// One place on a pie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slot {
    /// What it is called: shown in the middle while it is pointed at.
    pub(crate) label: String,
    /// The icons to try, the first found drawn: a file's path, or a name
    /// in the icon theme.
    pub(crate) icons: Vec<String>,
    pub(crate) does: Does,
    /// The running windows it stands for, by serial, in the order they
    /// opened.
    pub(crate) windows: Vec<u64>,
}

/// What a slot does when it is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Does {
    /// Start a program, or bring its next window forward if it has any.
    Launch(Run),
    /// Open a pie of its own.
    Open(Vec<Slot>),
    /// Bring a running application's next window forward; and, when its
    /// desktop entry is known, start another of it.
    Switch(Option<Run>),
}

/// A running window, as a pie sees it when it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) serial: u64,
    pub(crate) app_id: String,
    /// What it is known by, in lower case.
    pub(crate) name: String,
    /// The same as written: an alias's, or its app-id.
    pub(crate) label: String,
}

/// The windows a pie shows, each `(serial, app-id, title)`, named as
/// `aliases` name them, less those `ignore` matches.
pub(crate) fn seen<'w>(
    windows: impl IntoIterator<Item = (u64, &'w str, &'w str)>,
    aliases: &[Alias],
    ignore: &[Match],
) -> Vec<Seen> {
    windows
        .into_iter()
        .filter(|&(_, app_id, title)| !ignore.iter().any(|matches| matches.matches(app_id, title)))
        .map(|(serial, app_id, title)| {
            let label = aliases
                .iter()
                .find(|alias| alias.matches.matches(app_id, title))
                .map(|alias| alias.name.as_str())
                .or((!app_id.is_empty()).then_some(app_id))
                .unwrap_or(title)
                .to_owned();
            Seen {
                serial,
                app_id: app_id.to_owned(),
                name: label.to_lowercase(),
                label,
            }
        })
        .collect()
}

/// A folder of icons named for what they stand for, by name in lower case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Folder {
    by_name: HashMap<String, PathBuf>,
}

impl Folder {
    /// The PNG and SVG images in `dir`, by name. A name with both is the
    /// PNG's, as PieDock read only PNGs.
    pub(crate) fn read(fs: &impl Fs, dir: &Path) -> Self {
        let mut by_name = HashMap::new();
        let mut names: Vec<String> = fs
            .list(dir)
            .into_iter()
            .filter(|(_, is_dir)| !is_dir)
            .filter_map(|(name, _)| name.into_string().ok())
            .collect();
        // SVGs first, so a PNG of the same name takes its place.
        names.sort_by_key(|name| !name.to_lowercase().ends_with(".svg"));
        for name in names {
            let lower = name.to_lowercase();
            if let Some(stem) = lower
                .strip_suffix(".png")
                .or_else(|| lower.strip_suffix(".svg"))
            {
                by_name.insert(stem.to_owned(), dir.join(&name));
            }
        }
        Self { by_name }
    }

    /// The image for `name`, whatever its case.
    pub(crate) fn find(&self, name: &str) -> Option<&Path> {
        self.by_name.get(&name.to_lowercase()).map(PathBuf::as_path)
    }
}

/// What a pie is resolved against: the installed applications, the pie's
/// folder of icons, and where `~` and a desktop file's path lead.
pub(crate) struct Sources<'a, F> {
    pub(crate) fs: &'a F,
    pub(crate) apps: &'a [App],
    pub(crate) folder: &'a Folder,
    pub(crate) home: Option<&'a Path>,
    /// The config file, for a path written beside it.
    pub(crate) config: Option<&'a Path>,
    pub(crate) locale: &'a Locale,
    /// The windows running, named.
    pub(crate) running: &'a [Seen],
}

impl<F: Fs> Sources<'_, F> {
    /// The first ring of a pie of `items`, its `running` filled in.
    pub(crate) fn pie(&self, items: &[Item]) -> Vec<Slot> {
        let mut slots = Vec::new();
        let mut running_at = None;
        for item in items {
            if let Item::Running = item {
                running_at = Some(slots.len());
            } else {
                slots.extend(self.slot(item));
            }
        }
        if let Some(at) = running_at {
            let claimed: HashSet<u64> = slots
                .iter()
                .filter(|slot| !matches!(slot.does, Does::Open(_)))
                .flat_map(|slot| slot.windows.iter().copied())
                .collect();
            let running = self.running_apps(&claimed);
            slots.splice(at..at, running);
        }
        slots
    }

    /// The slots of `items`. An application that is not installed, or a
    /// desktop file that cannot be read, is left out with a log line, as
    /// the menu file's is: the same config may travel to a machine without
    /// it.
    pub(crate) fn slots(&self, items: &[Item]) -> Vec<Slot> {
        items.iter().filter_map(|item| self.slot(item)).collect()
    }

    /// A slot for each running application with a window none of `claimed`
    /// is, in the order their first windows opened.
    fn running_apps(&self, claimed: &HashSet<u64>) -> Vec<Slot> {
        let mut groups: Vec<(&Seen, Vec<u64>)> = Vec::new();
        for seen in self
            .running
            .iter()
            .filter(|seen| !claimed.contains(&seen.serial))
        {
            match groups.iter_mut().find(|(first, _)| first.name == seen.name) {
                Some((_, windows)) => windows.push(seen.serial),
                None => groups.push((seen, vec![seen.serial])),
            }
        }
        groups
            .into_iter()
            .map(|(first, windows)| {
                let app = apps::of_window(self.apps, &first.app_id)
                    .or_else(|| apps::of_window(self.apps, &first.name));
                let mut names = vec![first.name.as_str()];
                names.extend(app.and_then(|app| app.icon.as_deref()));
                names.extend([first.app_id.as_str(), PROGRAM]);
                Slot {
                    label: app.map_or_else(|| first.label.clone(), |app| app.name.clone()),
                    icons: self.icons(names.into_iter().filter(|name| !name.is_empty())),
                    does: Does::Switch(app.map(|app| app.run.clone())),
                    windows,
                }
            })
            .collect()
    }

    /// The running windows named `label`, or claimed by `app`'s desktop
    /// entry.
    fn windows_of(&self, label: &str, app: Option<&App>) -> Vec<u64> {
        let name = label.to_lowercase();
        self.running
            .iter()
            .filter(|seen| {
                seen.name == name
                    || app.is_some_and(|app| {
                        let one = std::slice::from_ref(app);
                        apps::of_window(one, &seen.app_id).is_some()
                            || apps::of_window(one, &seen.name).is_some()
                    })
            })
            .map(|seen| seen.serial)
            .collect()
    }

    fn slot(&self, item: &Item) -> Option<Slot> {
        Some(match item {
            Item::Run {
                label,
                icon,
                exec,
                terminal,
            } => Slot {
                label: label.clone(),
                icons: self.icons([icon.as_deref().unwrap_or(label), PROGRAM]),
                does: Does::Launch(Run {
                    argv: exec.iter().map(|word| self.tilde(word)).collect(),
                    terminal: *terminal,
                    dir: None,
                }),
                windows: self.windows_of(label, None),
            },
            Item::App { app, label, icon } => {
                let found = self.app(app)?;
                let label = label.clone().unwrap_or_else(|| found.name.clone());
                let mut names = Vec::new();
                names.extend(icon.as_deref());
                names.push(&label);
                names.extend(found.icon.as_deref());
                names.extend([found.id.as_str(), PROGRAM]);
                Slot {
                    icons: self.icons(names),
                    windows: self.windows_of(&label, Some(&found)),
                    label,
                    does: Does::Launch(found.run),
                }
            }
            Item::Open { label, icon, items } => {
                let below = self.slots(items);
                let mut windows: Vec<u64> = Vec::new();
                for serial in below.iter().flat_map(|slot| &slot.windows) {
                    if !windows.contains(serial) {
                        windows.push(*serial);
                    }
                }
                Slot {
                    label: label.clone(),
                    icons: self.icons([icon.as_deref().unwrap_or(label), SUBMENU]),
                    does: Does::Open(below),
                    windows,
                }
            }
            Item::Running | Item::Separator | Item::Applications | Item::Session => return None,
        })
    }

    /// The application `written` names: by path, with a `/`, or by ID.
    fn app(&self, written: &str) -> Option<App> {
        if written.contains('/') {
            let path = image::locate(Path::new(written), self.config, self.home);
            let found = self
                .fs
                .read(&path)
                .and_then(|text| desktop::parse(&text, self.locale).ok())
                .and_then(|entry| {
                    let id = path.file_stem()?.to_string_lossy().into_owned();
                    apps::from_entry(id, entry)
                });
            if found.is_none() {
                tracing::info!(
                    "the pie's {} is not a desktop file that runs anything; left out",
                    path.display()
                );
            }
            return found;
        }
        let id = written.strip_suffix(".desktop").unwrap_or(written);
        let found = self.apps.iter().find(|app| app.id == id).cloned();
        if found.is_none() {
            tracing::info!("the pie's application {id} is not installed; left out");
        }
        found
    }

    /// Each of `names` as the icons to try: the folder's image of it first,
    /// then it in the theme; a path, with `/`, as the file it is.
    fn icons<'n>(&self, names: impl IntoIterator<Item = &'n str>) -> Vec<String> {
        let mut icons: Vec<String> = Vec::new();
        for name in names {
            let tried = if name.contains('/') {
                vec![self.tilde(name)]
            } else {
                let folder = self
                    .folder
                    .find(name)
                    .map(|path| path.display().to_string());
                folder.into_iter().chain([name.to_owned()]).collect()
            };
            for icon in tried {
                if !icons.contains(&icon) {
                    icons.push(icon);
                }
            }
        }
        icons
    }

    /// `word` with a leading `~` as the home folder.
    fn tilde(&self, word: &str) -> String {
        menu_file::tilde(word, self.home)
    }
}

#[cfg(test)]
mod tests {
    use perspicax_config::menu::{self, Vocabulary};

    use super::*;
    use crate::model::fs::fake::Files;

    fn items(text: &str) -> Vec<Item> {
        #[derive(serde::Deserialize)]
        struct Raw {
            items: Vec<menu::RawItem>,
        }
        let raw: Raw = toml::from_str(text).expect("items");
        menu::check(raw.items, Vocabulary::Pie).expect("a pie")
    }

    fn firefox() -> App {
        App {
            id: "org.mozilla.firefox".to_owned(),
            name: "Firefox".to_owned(),
            comment: None,
            icon: Some("firefox".to_owned()),
            run: Run {
                argv: vec!["firefox".to_owned()],
                terminal: false,
                dir: None,
            },
            categories: Vec::new(),
            keywords: Vec::new(),
            wm_class: None,
        }
    }

    fn resolve(fs: &Files, text: &str, apps: &[App]) -> Vec<Slot> {
        resolve_with(fs, text, apps, &[])
    }

    fn resolve_with(fs: &Files, text: &str, apps: &[App], running: &[Seen]) -> Vec<Slot> {
        let folder = Folder::read(fs, Path::new("/home/ada/.piedock/icons"));
        Sources {
            fs,
            apps,
            folder: &folder,
            home: Some(Path::new("/home/ada")),
            config: Some(Path::new("/home/ada/.config/perspicax/config.toml")),
            locale: &Locale::default(),
            running,
        }
        .pie(&items(text))
    }

    /// Windows `(serial, app-id, title)`, named with no aliases.
    fn running(windows: &[(u64, &str, &str)]) -> Vec<Seen> {
        seen(windows.iter().copied(), &[], &[])
    }

    fn labels(slots: &[Slot]) -> Vec<(&str, &[u64])> {
        slots
            .iter()
            .map(|slot| (slot.label.as_str(), slot.windows.as_slice()))
            .collect()
    }

    #[test]
    fn a_window_is_named_by_the_first_alias_that_matches_else_its_app_id() {
        let aliases = [
            Alias {
                matches: Match::AppId("com.mitchellh.ghostty".to_owned()),
                name: "Ghostty".to_owned(),
            },
            Alias {
                matches: Match::Title("Steam".to_owned()),
                name: "steam".to_owned(),
            },
        ];
        let ignore = [Match::Title("100".to_owned())];
        let named = seen(
            [
                (1, "com.mitchellh.ghostty", "~"),
                (2, "steamwebhelper", "Steam"),
                (3, "Firefox", "Mozilla Firefox"),
                (4, "xclock", "100"),
                (5, "", "Untitled"),
            ],
            &aliases,
            &ignore,
        );
        let names: Vec<_> = named
            .iter()
            .map(|seen| (seen.serial, seen.name.as_str(), seen.label.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (1, "ghostty", "Ghostty"),
                (2, "steam", "steam"),
                (3, "firefox", "Firefox"),
                (5, "untitled", "Untitled"),
            ]
        );
    }

    #[test]
    fn running_applications_fill_the_first_ring_unless_a_slot_there_stands_for_them() {
        let fs = Files::default().with("/home/ada/.piedock/icons/ghostty.png", "");
        let windows = running(&[
            (1, "ghostty", "a"),
            (2, "firefox", "b"),
            (3, "slack", "c"),
            (4, "ghostty", "d"),
            (5, "firefox", "e"),
        ]);
        let slots = resolve_with(
            &fs,
            r#"items = [
                { label = "terminal", exec = ["foot"] },
                { label = "internet", items = [
                    { label = "slack", exec = ["slack"] },
                    { app = "org.mozilla.firefox" },
                ] },
                { running = true },
                { label = "Ghostty", exec = ["ghostty"] },
            ]"#,
            &[firefox()],
            &windows,
        );
        assert_eq!(
            labels(&slots),
            [
                ("terminal", &[][..]),
                ("internet", &[3, 2, 5][..]),
                // In the submenu, and still on the first ring.
                ("Firefox", &[2, 5][..]),
                ("slack", &[3][..]),
                ("Ghostty", &[1, 4][..]),
            ],
            "ghostty's windows are its own slot's, so it is not added"
        );
        assert_eq!(
            slots[2].does,
            Does::Switch(Some(firefox().run)),
            "an application's, which the middle button starts again"
        );
        assert_eq!(slots[3].does, Does::Switch(None));
        assert_eq!(slots[3].icons, ["slack", "application-x-executable"]);
        let Does::Open(below) = &slots[1].does else {
            panic!("a submenu");
        };
        assert_eq!(
            labels(below),
            [("slack", &[3][..]), ("Firefox", &[2, 5][..])]
        );
    }

    #[test]
    fn with_no_running_marker_running_applications_get_no_slots() {
        let windows = running(&[(1, "slack", "c")]);
        let slots = resolve_with(
            &Files::default(),
            r#"items = [{ label = "slack", exec = ["slack"] }, { label = "foot", exec = ["foot"] }]"#,
            &[],
            &windows,
        );
        assert_eq!(labels(&slots), [("slack", &[1][..]), ("foot", &[][..])]);
    }

    #[test]
    fn the_folder_is_read_whatever_the_case_and_a_png_beats_an_svg() {
        let fs = Files::default()
            .with("/icons/VM.png", "")
            .with("/icons/obs studio.png", "")
            .with("/icons/gimp.svg", "")
            .with("/icons/gimp.PNG", "")
            .with("/icons/notes.txt", "");
        let folder = Folder::read(&fs, Path::new("/icons"));
        assert_eq!(folder.find("vm"), Some(Path::new("/icons/VM.png")));
        assert_eq!(folder.find("VM"), Some(Path::new("/icons/VM.png")));
        assert_eq!(
            folder.find("OBS Studio"),
            Some(Path::new("/icons/obs studio.png"))
        );
        assert_eq!(folder.find("gimp"), Some(Path::new("/icons/gimp.PNG")));
        assert_eq!(folder.find("notes"), None);
        assert_eq!(Folder::read(&fs, Path::new("/nowhere")).find("vm"), None);
    }

    #[test]
    fn programs_take_their_labels_icon_from_the_folder_before_the_theme() {
        let fs = Files::default()
            .with("/home/ada/.piedock/icons/terminal.png", "")
            .with("/home/ada/.piedock/icons/vm.png", "");
        let slots = resolve(
            &fs,
            r#"items = [
                { label = "terminal", exec = ["term", "~/notes"] },
                { label = "editor", exec = ["gedit"], icon = "~/pics/pen.png" },
                { label = "VM", items = [{ label = "vm5", exec = ["vm5"] }] },
            ]"#,
            &[],
        );
        assert_eq!(slots[0].label, "terminal");
        assert_eq!(
            slots[0].icons,
            [
                "/home/ada/.piedock/icons/terminal.png",
                "terminal",
                "application-x-executable"
            ]
        );
        assert_eq!(
            slots[0].does,
            Does::Launch(Run {
                argv: vec!["term".into(), "/home/ada/notes".into()],
                terminal: false,
                dir: None,
            })
        );
        assert_eq!(
            slots[1].icons,
            ["/home/ada/pics/pen.png", "application-x-executable"]
        );
        assert_eq!(
            slots[2].icons,
            ["/home/ada/.piedock/icons/vm.png", "VM", "folder"]
        );
        let Does::Open(below) = &slots[2].does else {
            panic!("a submenu");
        };
        assert_eq!(below[0].label, "vm5");
    }

    #[test]
    fn an_application_is_found_by_id_or_by_its_desktop_files_path() {
        let fs = Files::default()
            .with(
                "/home/ada/apps/tool.desktop",
                "[Desktop Entry]\nType=Application\nName=Tool\nExec=tool %U\n\
                 Icon=tool-icon\nNoDisplay=true\n",
            )
            .with(
                "/home/ada/.config/perspicax/beside.desktop",
                "[Desktop Entry]\nType=Application\nName=Beside\nExec=beside\n",
            );
        let slots = resolve(
            &fs,
            r#"items = [
                { app = "org.mozilla.firefox.desktop" },
                { app = "org.mozilla.firefox", label = "web", icon = "globe" },
                { app = "~/apps/tool.desktop" },
                { app = "./beside.desktop" },
                { app = "not-installed" },
                { app = "~/apps/missing.desktop" },
            ]"#,
            &[firefox()],
        );
        let labels: Vec<_> = slots.iter().map(|slot| slot.label.as_str()).collect();
        assert_eq!(labels, ["Firefox", "web", "Tool", "Beside"]);
        assert_eq!(
            slots[0].icons,
            [
                "Firefox",
                "firefox",
                "org.mozilla.firefox",
                "application-x-executable"
            ]
        );
        assert_eq!(slots[1].icons[0], "globe");
        assert_eq!(
            slots[2].does,
            Does::Launch(Run {
                argv: vec!["tool".into()],
                terminal: false,
                dir: None,
            }),
            "a desktop file named by path runs even though it is not listed"
        );
        assert_eq!(slots[2].icons[1], "tool-icon");
    }
}
