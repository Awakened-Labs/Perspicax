//! XDG autostart: what the system and the person installed to start with
//! every desktop session, PipeWire and a keyring among them.
//!
//! Read to the Desktop Application Autostart Specification, 0.5. Each
//! program is a desktop entry in an `autostart` folder, the person's
//! `$XDG_CONFIG_HOME/autostart` first and then each of `$XDG_CONFIG_DIRS`'s,
//! and is known by its file name: the first folder to hold a name decides
//! it, so a person's copy of an entry replaces the system's, and a copy
//! marked `Hidden` turns it off.
//!
//! Deciding what to start is here, from the files' text. Reading the folders
//! and starting the programs, once as the session begins, is the
//! compositor's.

use std::{collections::BTreeMap, path::PathBuf};

use crate::desktop::{self, Locale, Malformed};

/// A program to start: an autostart entry's `Exec`, in its `Path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The program and its arguments, field codes dropped. Never empty.
    pub argv: Vec<String>,
    /// The folder to start it in.
    pub dir: Option<PathBuf>,
}

/// Why an autostart entry is not started.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Skip {
    #[error("it could not be read")]
    Unreadable,
    #[error(transparent)]
    Malformed(#[from] Malformed),
    #[error("it is hidden")]
    Hidden,
    #[error("X-GNOME-Autostart-enabled turns it off")]
    TurnedOff,
    #[error("it is not an application")]
    NotApplication,
    #[error("it is not for this desktop")]
    NotHere,
    #[error("it has no Exec")]
    NoExec,
    #[error("its TryExec, {0}, is not installed")]
    NotInstalled(String),
    /// Last of the reasons, so it is given only for an entry that would
    /// otherwise start.
    #[error("it runs in a terminal, and autostart opens none")]
    Terminal,
}

/// The autostart folders, the most important first: the one in
/// `$XDG_CONFIG_HOME` (by default `~/.config`), then the one in each of
/// `$XDG_CONFIG_DIRS` (by default `/etc/xdg`). A relative folder is not one,
/// as the base directory spec says.
///
/// `var` reads the environment; a function rather than the process's own, so
/// the rule can be checked without changing the test's environment.
#[must_use]
pub fn folders(var: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let var = |name: &str| var(name).filter(|value| !value.is_empty());
    let config_home = var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".config")));
    let config_dirs = var("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".to_owned());
    config_home
        .into_iter()
        .chain(
            config_dirs
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("autostart"))
        .collect()
}

/// The names this desktop goes by, from `$XDG_CURRENT_DESKTOP`, for
/// `OnlyShowIn` and `NotShowIn`.
#[must_use]
pub fn desktops(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    var("XDG_CURRENT_DESKTOP")
        .map(|names| {
            names
                .split(':')
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Each autostart entry, by file name, with whether to start it or why not.
///
/// `found` is every `.desktop` file in the [`folders`], the most important
/// folder's first, each a file name and its text, or `None` for a file that
/// could not be read. `desktops` are this desktop's [names](desktops), and
/// `installed` says whether a program `TryExec` names is installed.
pub fn select(
    found: impl IntoIterator<Item = (String, Option<String>)>,
    desktops: &[String],
    installed: impl Fn(&str) -> bool,
) -> Vec<(String, Result<Start, Skip>)> {
    let mut decided = BTreeMap::new();
    for (name, text) in found {
        // The first folder to hold a name has it, whatever it says.
        decided.entry(name).or_insert(text);
    }
    decided
        .into_iter()
        .map(|(name, text)| {
            let start = text
                .ok_or(Skip::Unreadable)
                .and_then(|text| choose(&text, desktops, &installed));
            (name, start)
        })
        .collect()
}

/// The program one entry starts, or why it starts none. `NoDisplay` is not
/// a reason: it keeps an entry out of menus, and an autostart entry is
/// rarely one to list.
fn choose(
    text: &str,
    desktops: &[String],
    installed: impl Fn(&str) -> bool,
) -> Result<Start, Skip> {
    let entry = desktop::parse(text, &Locale::default())?;
    let here = |names: &[String]| names.iter().any(|name| desktops.contains(name));
    if entry.hidden {
        return Err(Skip::Hidden);
    }
    if entry.autostart_off {
        return Err(Skip::TurnedOff);
    }
    if !entry.application {
        return Err(Skip::NotApplication);
    }
    if !entry.only_show_in.is_empty() && !here(&entry.only_show_in) || here(&entry.not_show_in) {
        return Err(Skip::NotHere);
    }
    let argv = entry.exec.ok_or(Skip::NoExec)?;
    if let Some(program) = entry.try_exec.filter(|program| !installed(program)) {
        return Err(Skip::NotInstalled(program));
    }
    if entry.terminal {
        return Err(Skip::Terminal);
    }
    Ok(Start {
        argv,
        dir: entry.path.map(PathBuf::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An application entry named `name`, running `exec`, with `more` lines.
    fn entry(name: &str, exec: &str, more: &str) -> Option<String> {
        Some(format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n{more}"
        ))
    }

    fn file(name: &str, text: Option<String>) -> (String, Option<String>) {
        (name.to_owned(), text)
    }

    /// What `found` starts on a desktop called `perspicax`, where only
    /// `/usr/bin/pipewire` is installed.
    fn select_here(found: Vec<(String, Option<String>)>) -> Vec<(String, Result<Start, Skip>)> {
        select(found, &["perspicax".to_owned()], |program| {
            program == "/usr/bin/pipewire" || program == "pipewire"
        })
    }

    fn started(argv: &[&str]) -> Result<Start, Skip> {
        Ok(Start {
            argv: argv.iter().map(|&word| word.to_owned()).collect(),
            dir: None,
        })
    }

    #[test]
    fn the_users_entry_overrides_the_systems_and_a_hidden_one_turns_it_off() {
        let found = vec![
            // ~/.config/autostart first.
            file("teams.desktop", entry("Teams", "teams --minimized", "")),
            file(
                "remmina-applet.desktop",
                entry("Remmina", "x", "Hidden=true"),
            ),
            // Then /etc/xdg/autostart.
            file("pipewire.desktop", entry("PipeWire", "pipewire", "")),
            file("teams.desktop", entry("Teams", "teams", "")),
            file("remmina-applet.desktop", entry("Remmina", "remmina -i", "")),
        ];
        assert_eq!(
            select_here(found),
            [
                ("pipewire.desktop".to_owned(), started(&["pipewire"])),
                ("remmina-applet.desktop".to_owned(), Err(Skip::Hidden)),
                (
                    "teams.desktop".to_owned(),
                    started(&["teams", "--minimized"])
                ),
            ],
            "by file name, each decided by the first folder to hold it"
        );
    }

    #[test]
    fn each_entry_left_out_says_why() {
        let skipped = |text: Option<String>| {
            let mut decided = select_here(vec![file("x.desktop", text)]);
            decided.pop().expect("one entry").1.unwrap_err()
        };
        assert_eq!(skipped(None), Skip::Unreadable);
        assert_eq!(
            skipped(Some(
                "[Desktop Entry]\nType=Application\nExec=x\n".to_owned()
            )),
            Skip::Malformed(Malformed::NoName)
        );
        assert_eq!(
            skipped(entry("X", "x", "X-GNOME-Autostart-enabled=false")),
            Skip::TurnedOff
        );
        assert_eq!(
            skipped(Some(
                "[Desktop Entry]\nType=Link\nName=X\nURL=https://example.org\n".to_owned()
            )),
            Skip::NotApplication
        );
        assert_eq!(
            skipped(entry("X", "x", "OnlyShowIn=GNOME;KDE;")),
            Skip::NotHere
        );
        assert_eq!(
            skipped(entry("X", "x", "NotShowIn=perspicax;")),
            Skip::NotHere
        );
        assert_eq!(
            skipped(Some(
                "[Desktop Entry]\nType=Application\nName=X\nDBusActivatable=true\n".to_owned()
            )),
            Skip::NoExec
        );
        assert_eq!(
            skipped(entry("X", "x", "TryExec=/usr/bin/geoclue-agent")),
            Skip::NotInstalled("/usr/bin/geoclue-agent".to_owned())
        );
        assert_eq!(skipped(entry("X", "x", "Terminal=true")), Skip::Terminal);
        assert_eq!(
            skipped(entry("X", "x", "Terminal=true\nHidden=true")),
            Skip::Hidden,
            "a terminal is named only for what would otherwise start"
        );
    }

    #[test]
    fn what_starts_drops_its_field_codes_and_keeps_its_folder() {
        let found = vec![
            file(
                "pipewire.desktop",
                entry(
                    "PipeWire",
                    "pipewire %U",
                    "TryExec=pipewire\nNoDisplay=true\nOnlyShowIn=perspicax;GNOME;\nPath=/srv",
                ),
            ),
            file(
                "notes.desktop",
                entry("Notes", "notes", "TryExec=/usr/bin/pipewire"),
            ),
        ];
        assert_eq!(
            select_here(found),
            [
                ("notes.desktop".to_owned(), started(&["notes"])),
                (
                    "pipewire.desktop".to_owned(),
                    Ok(Start {
                        argv: vec!["pipewire".to_owned()],
                        dir: Some("/srv".into()),
                    })
                ),
            ],
            "NoDisplay is no reason to skip an entry"
        );
    }

    #[test]
    fn the_folders_are_the_persons_then_the_systems() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(
            folders(env(&[("HOME", "/home/ada")])),
            [
                PathBuf::from("/home/ada/.config/autostart"),
                PathBuf::from("/etc/xdg/autostart"),
            ],
            "the defaults"
        );
        assert_eq!(
            folders(env(&[
                ("HOME", "/home/ada"),
                ("XDG_CONFIG_HOME", "/cfg"),
                ("XDG_CONFIG_DIRS", "/etc/xdg/plasma:relative::/etc/xdg"),
            ])),
            [
                PathBuf::from("/cfg/autostart"),
                PathBuf::from("/etc/xdg/plasma/autostart"),
                PathBuf::from("/etc/xdg/autostart"),
            ],
            "a relative folder is not one"
        );
        assert_eq!(
            folders(env(&[])),
            [PathBuf::from("/etc/xdg/autostart")],
            "no home"
        );
        assert_eq!(
            desktops(env(&[("XDG_CURRENT_DESKTOP", "perspicax:GNOME")])),
            ["perspicax", "GNOME"]
        );
        assert!(desktops(env(&[])).is_empty());
    }
}
