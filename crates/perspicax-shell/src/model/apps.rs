//! The installed applications: the desktop entries in the XDG data folders
//! that a menu should list.
//!
//! Each folder's `applications/` is read, the person's own folder first. An
//! entry is known by its desktop file ID, its path below `applications/`
//! with each `/` a `-`, and the first folder to hold an ID decides it: a
//! person's copy of an entry replaces the system's, and a copy marked
//! `Hidden` takes it away. An entry is left out when it is not an
//! application, is `Hidden` or `NoDisplay`, is meant only for other
//! desktops, names in `TryExec` a program that is not installed, or cannot
//! be read.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{
    desktop::{self, Entry, Locale},
    fs::{Fs, which},
};

/// An application, as a menu lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct App {
    /// Its desktop file ID, without `.desktop`: `org.gnome.Nautilus`.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) comment: Option<String>,
    pub(crate) icon: Option<String>,
    pub(crate) run: Run,
    pub(crate) categories: Vec<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) wm_class: Option<String>,
}

/// A program a menu item starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Run {
    /// The program and its arguments. Never empty.
    pub(crate) argv: Vec<String>,
    /// In a terminal.
    pub(crate) terminal: bool,
    /// The folder to start it in.
    pub(crate) dir: Option<PathBuf>,
}

/// Where to look for applications, and on whose behalf.
#[derive(Debug, Clone, Default)]
pub(crate) struct Places {
    /// The data folders, the person's own first.
    pub(crate) data: Vec<PathBuf>,
    /// The folders programs are found in, for `TryExec`.
    pub(crate) path: Vec<PathBuf>,
    /// The names this desktop goes by, for `OnlyShowIn` and `NotShowIn`.
    pub(crate) desktops: Vec<String>,
    pub(crate) locale: Locale,
}

impl Places {
    /// The places the environment names: `$XDG_DATA_HOME`, then each of
    /// `$XDG_DATA_DIRS`, with the spec's defaults for either unset; `$PATH`;
    /// and `$XDG_CURRENT_DESKTOP`.
    pub(crate) fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let data_home = var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".local/share")));
        let data_dirs =
            var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        Self {
            data: data_home
                .into_iter()
                .chain(folders(&data_dirs))
                .filter(|dir| dir.is_absolute())
                .collect(),
            path: folders(&var("PATH").unwrap_or_default()).collect(),
            desktops: var("XDG_CURRENT_DESKTOP")
                .map(|names| names.split(':').map(str::to_owned).collect())
                .unwrap_or_default(),
            locale: Locale::from_env(),
        }
    }
}

/// The folders of a `:`-separated list.
fn folders(list: &str) -> impl Iterator<Item = PathBuf> + '_ {
    list.split(':')
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// Every application a menu should list, by name.
pub(crate) fn scan(fs: &impl Fs, places: &Places) -> Vec<App> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for data in &places.data {
        let mut found = Vec::new();
        walk(fs, &data.join("applications"), "", &mut found);
        for (id, path) in found {
            // The first folder to hold an ID has it, whatever it says.
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(text) = fs.read(&path) else {
                continue;
            };
            match desktop::parse(&text, &places.locale) {
                Ok(entry) => apps.extend(admit(fs, places, id, entry)),
                Err(why) => tracing::debug!("{} is left out: {why}", path.display()),
            }
        }
    }
    apps.sort_by_cached_key(|app| (app.name.to_lowercase(), app.id.clone()));
    apps
}

/// Every `.desktop` file below `dir`, with its ID, in a stable order.
fn walk(fs: &impl Fs, dir: &Path, prefix: &str, found: &mut Vec<(String, PathBuf)>) {
    let mut entries = fs.list(dir);
    entries.sort();
    for (name, is_dir) in entries {
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_dir {
            walk(fs, &dir.join(name), &format!("{prefix}{name}-"), found);
        } else if let Some(stem) = name.strip_suffix(".desktop") {
            found.push((format!("{prefix}{stem}"), dir.join(name)));
        }
    }
}

/// `entry` as an application to list, or `None` if it is not one to list
/// here.
fn admit(fs: &impl Fs, places: &Places, id: String, entry: Entry) -> Option<App> {
    let here = |names: &[String]| names.iter().any(|name| places.desktops.contains(name));
    let shown = entry.application
        && !entry.hidden
        && !entry.no_display
        && (entry.only_show_in.is_empty() || here(&entry.only_show_in))
        && !here(&entry.not_show_in)
        && entry
            .try_exec
            .as_deref()
            .is_none_or(|program| which(fs, program, &places.path).is_some());
    if !shown {
        return None;
    }
    Some(App {
        id,
        name: entry.name,
        comment: entry.comment,
        icon: entry.icon,
        run: Run {
            argv: entry.exec?,
            terminal: entry.terminal,
            dir: entry.path.map(PathBuf::from),
        },
        categories: entry.categories,
        keywords: entry.keywords,
        wm_class: entry.wm_class,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fs::fake::Files;

    const HOME: &str = "/home/ada/.local/share/applications";
    const SYSTEM: &str = "/usr/share/applications";

    fn places() -> Places {
        Places {
            data: vec!["/home/ada/.local/share".into(), "/usr/share".into()],
            path: vec!["/usr/bin".into()],
            desktops: vec!["perspicax".to_owned()],
            locale: Locale::default(),
        }
    }

    /// An application entry named `name`, running `exec`, with `more` lines.
    fn entry(name: &str, exec: &str, more: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n{more}")
    }

    fn names(files: &Files) -> Vec<String> {
        scan(files, &places())
            .into_iter()
            .map(|app| app.name)
            .collect()
    }

    #[test]
    fn the_users_entry_overrides_the_systems() {
        let files = Files::default()
            .with(
                &format!("{SYSTEM}/firefox.desktop"),
                &entry("Firefox", "firefox %u", ""),
            )
            .with(
                &format!("{HOME}/firefox.desktop"),
                &entry("Firefox (work)", "firefox -P work %u", ""),
            )
            .with(
                &format!("{SYSTEM}/gimp.desktop"),
                &entry("GIMP", "gimp", ""),
            )
            .with(
                &format!("{HOME}/gimp.desktop"),
                &entry("GIMP", "gimp", "Hidden=true"),
            );
        let apps = scan(&files, &places());
        assert_eq!(
            apps.len(),
            1,
            "a hidden copy takes the system's away: {apps:?}"
        );
        assert_eq!(apps[0].name, "Firefox (work)");
        assert_eq!(apps[0].run.argv, ["firefox", "-P", "work"]);
        assert_eq!(apps[0].id, "firefox");
    }

    #[test]
    fn hidden_nodisplay_and_onlyshowin_entries_are_left_out() {
        let files = Files::default()
            .program("/usr/bin/present")
            .with(
                &format!("{SYSTEM}/shown.desktop"),
                &entry("Shown", "shown", ""),
            )
            .with(
                &format!("{SYSTEM}/hidden.desktop"),
                &entry("Hidden", "x", "Hidden=true"),
            )
            .with(
                &format!("{SYSTEM}/helper.desktop"),
                &entry("Helper", "x", "NoDisplay=true"),
            )
            .with(
                &format!("{SYSTEM}/kde.desktop"),
                &entry("KDE only", "x", "OnlyShowIn=KDE;"),
            )
            .with(
                &format!("{SYSTEM}/ours.desktop"),
                &entry("Ours too", "x", "OnlyShowIn=KDE;perspicax;"),
            )
            .with(
                &format!("{SYSTEM}/not.desktop"),
                &entry("Not here", "x", "NotShowIn=perspicax;"),
            )
            .with(
                &format!("{SYSTEM}/try.desktop"),
                &entry("Installed", "x", "TryExec=present"),
            )
            .with(
                &format!("{SYSTEM}/gone.desktop"),
                &entry("Missing", "x", "TryExec=absent"),
            )
            .with(
                &format!("{SYSTEM}/link.desktop"),
                "[Desktop Entry]\nType=Link\nName=A link\nURL=https://example.org\n",
            )
            .with(
                &format!("{SYSTEM}/dbus.desktop"),
                "[Desktop Entry]\nType=Application\nName=Bus only\nDBusActivatable=true\n",
            );
        assert_eq!(names(&files), ["Installed", "Ours too", "Shown"]);
    }

    #[test]
    fn a_malformed_entry_is_skipped() {
        let files = Files::default()
            .with(
                &format!("{SYSTEM}/broken.desktop"),
                "[Desktop Entry]\nName=Broken\nthis is not a key\n",
            )
            .with(
                &format!("{SYSTEM}/fine.desktop"),
                &entry("Fine", "fine", ""),
            )
            .with(
                &format!("{SYSTEM}/notes.txt"),
                &entry("Not an entry", "x", ""),
            );
        assert_eq!(names(&files), ["Fine"]);
    }

    #[test]
    fn an_entry_in_a_subfolder_is_known_by_its_path() {
        let files = Files::default().with(
            &format!("{SYSTEM}/kde4/dolphin.desktop"),
            &entry("Dolphin", "dolphin %u", "Path=/srv\nTerminal=false"),
        );
        let apps = scan(&files, &places());
        assert_eq!(apps[0].id, "kde4-dolphin");
        assert_eq!(apps[0].run.dir.as_deref(), Some(Path::new("/srv")));
    }
}
