//! `[shell.pie]`: pie menus, PieDock's way, opened at the pointer by a
//! binding's `{ pie = "<name>" }`.
//!
//! ```toml
//! [shell.pie]
//! size = 512                     # across, in logical pixels
//! icons = "~/.piedock/icons"     # <name>.png, looked in before the icon theme
//! aliases = [{ app-id = "com.mitchellh.ghostty", as = "ghostty" }]
//! ignore = [{ title = "Picture-in-Picture" }]
//!
//! [shell.pie.menus]
//! launchers = [
//!     { label = "terminal", exec = ["foot"] },
//!     { app = "firefox" },
//!     { label = "games", items = [{ label = "steam", exec = ["steam"] }] },
//!     { running = true },
//! ]
//! ```
//!
//! Each pie is a list in the [`menu`](crate::menu) vocabulary, a pie's:
//! programs, applications and submenus, and `running`, where the running
//! applications go. A running window is known by a name, as PieDock knows
//! one: the first alias that matches its app-id or title, else its app-id.
//! That name finds the entry it belongs to and the icon it is drawn with.

use std::{collections::BTreeMap, path::PathBuf};

use serde::Deserialize;

use crate::{
    Error, invalid,
    menu::{self, Item, RawItem, Vocabulary},
};

/// How big a pie is across, in logical pixels, when `size` is not written:
/// PieDock's.
pub const SIZE: u32 = 512;
const SIZE_MIN: u32 = 128;
const SIZE_MAX: u32 = 1024;

/// Every pie, and how running windows are named in them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pie {
    /// Across, in logical pixels: the ring and its icons are fitted in.
    pub size: u32,
    /// A folder of the person's own icons, named for what they stand for
    /// (`firefox.png`), looked in before the icon theme. As written: `~`
    /// and a path beside config.toml are the shell's to resolve.
    pub icons: Option<PathBuf>,
    /// What names a running window, before its app-id does.
    pub aliases: Vec<Alias>,
    /// Running windows no pie shows.
    pub ignore: Vec<Match>,
    /// Each pie, by the name a binding opens it by.
    pub menus: BTreeMap<String, Vec<Item>>,
}

/// A running window that `matches` is known as `name`: PieDock's `alias`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub matches: Match,
    pub name: String,
}

/// A running window, by what it says of itself, exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    AppId(String),
    Title(String),
}

impl Match {
    /// Whether a window with this app-id and title is the one meant.
    #[must_use]
    pub fn matches(&self, app_id: &str, title: &str) -> bool {
        match self {
            Self::AppId(written) => written == app_id,
            Self::Title(written) => written == title,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RawPie {
    size: Option<u32>,
    icons: Option<PathBuf>,
    #[serde(default)]
    aliases: Vec<RawAlias>,
    #[serde(default)]
    ignore: Vec<RawMatch>,
    #[serde(default)]
    menus: BTreeMap<String, Vec<RawItem>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawAlias {
    app_id: Option<String>,
    title: Option<String>,
    #[serde(rename = "as")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawMatch {
    app_id: Option<String>,
    title: Option<String>,
}

impl RawPie {
    pub(crate) fn apply(self) -> Result<Pie, Error> {
        let size = self.size.unwrap_or(SIZE);
        if !(SIZE_MIN..=SIZE_MAX).contains(&size) {
            return Err(invalid(
                "shell.pie.size".to_owned(),
                format!("{size} is outside {SIZE_MIN} to {SIZE_MAX} pixels"),
            ));
        }
        if self
            .icons
            .as_ref()
            .is_some_and(|icons| icons.as_os_str().is_empty())
        {
            return Err(invalid(
                "shell.pie.icons".to_owned(),
                "an empty path".to_owned(),
            ));
        }
        let aliases = self
            .aliases
            .into_iter()
            .enumerate()
            .map(|(n, alias)| {
                let key = format!("shell.pie.aliases[{}]", n + 1);
                let matches = matching(&key, alias.app_id, alias.title)?;
                let name = alias
                    .name
                    .filter(|name| !name.trim().is_empty())
                    .ok_or_else(|| invalid(key, "`as` names no application".to_owned()))?;
                Ok(Alias { matches, name })
            })
            .collect::<Result<_, Error>>()?;
        let ignore = self
            .ignore
            .into_iter()
            .enumerate()
            .map(|(n, ignored)| {
                let key = format!("shell.pie.ignore[{}]", n + 1);
                matching(&key, ignored.app_id, ignored.title)
            })
            .collect::<Result<_, Error>>()?;
        if self.menus.is_empty() {
            return Err(invalid(
                "shell.pie.menus".to_owned(),
                "name at least one pie, as `launchers = [...]`".to_owned(),
            ));
        }
        let menus = self
            .menus
            .into_iter()
            .map(|(name, items)| {
                if name.trim().is_empty() {
                    return Err(invalid(
                        "shell.pie.menus".to_owned(),
                        "a pie needs a name".to_owned(),
                    ));
                }
                let key = format!("shell.pie.menus.{name}");
                if items.is_empty() {
                    return Err(invalid(key, "a pie with nothing in it".to_owned()));
                }
                let items = menu::check(items, Vocabulary::Pie).map_err(|refused| {
                    let (first, below) = refused.at.split_first().unwrap_or((&0, &[]));
                    let place: String = below.iter().map(|n| format!(".items[{n}]")).collect();
                    invalid(format!("{key}[{first}]{place}"), refused.why.to_owned())
                })?;
                Ok((name, items))
            })
            .collect::<Result<_, Error>>()?;
        Ok(Pie {
            size,
            icons: self.icons,
            aliases,
            ignore,
            menus,
        })
    }
}

/// One of an app-id or a title, as written under `key`.
fn matching(key: &str, app_id: Option<String>, title: Option<String>) -> Result<Match, Error> {
    let refuse = |reason: &str| Err(invalid(key.to_owned(), reason.to_owned()));
    match (app_id, title) {
        (Some(written), None) | (None, Some(written)) if written.is_empty() => {
            refuse("an empty app-id or title matches nothing")
        }
        (Some(app_id), None) => Ok(Match::AppId(app_id)),
        (None, Some(title)) => Ok(Match::Title(title)),
        _ => refuse("give it one of app-id or title"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Built, ShellBuilt, parse, shell};

    fn pie(text: &str) -> Result<Pie, Error> {
        shell(text, ShellBuilt::FULL).map(|shell| shell.pie.expect("a pie"))
    }

    /// The key a refusal names, and its reason.
    fn refused(text: &str) -> (String, String) {
        match pie(text) {
            Err(Error::Invalid { key, reason }) => (key, reason),
            other => panic!("not refused by key: {other:?}"),
        }
    }

    #[test]
    fn a_pie_is_read_with_its_aliases_ignores_and_menus() {
        let text = r#"
            [shell.pie]
            size = 400
            icons = "~/.piedock/icons"
            aliases = [
                { app-id = "com.mitchellh.ghostty", as = "ghostty" },
                { title = "Steam", as = "steam" },
            ]
            ignore = [{ title = "100" }, { app-id = "zenity" }]

            [shell.pie.menus]
            launchers = [
                { label = "terminal", exec = ["term"] },
                { label = "internet", items = [{ app = "firefox" }] },
                { running = true },
            ]
            windows = [{ running = true }]
        "#;
        let read = pie(text).expect("a pie");
        assert_eq!(read.size, 400);
        assert_eq!(read.icons, Some(PathBuf::from("~/.piedock/icons")));
        assert_eq!(
            read.aliases,
            [
                Alias {
                    matches: Match::AppId("com.mitchellh.ghostty".to_owned()),
                    name: "ghostty".to_owned(),
                },
                Alias {
                    matches: Match::Title("Steam".to_owned()),
                    name: "steam".to_owned(),
                },
            ]
        );
        assert_eq!(
            read.ignore,
            [
                Match::Title("100".to_owned()),
                Match::AppId("zenity".to_owned())
            ]
        );
        assert_eq!(
            read.menus.keys().collect::<Vec<_>>(),
            ["launchers", "windows"]
        );
        assert_eq!(read.menus["launchers"].len(), 3);
        assert_eq!(read.menus["windows"], [Item::Running]);
        // The compositor reads the same table, whatever the shell has.
        assert!(parse(text, Built::default()).is_ok());
    }

    #[test]
    fn an_unwritten_size_is_piedocks_and_no_pie_is_written_by_default() {
        let read = pie("[shell.pie.menus]\nx = [{ running = true }]").unwrap();
        assert_eq!(read.size, SIZE);
        assert_eq!(read.icons, None);
        assert!(shell("", ShellBuilt::FULL).unwrap().pie.is_none());
    }

    #[test]
    fn a_window_is_matched_by_its_app_id_or_title_exactly() {
        let ghostty = Match::AppId("com.mitchellh.ghostty".to_owned());
        assert!(ghostty.matches("com.mitchellh.ghostty", "~"));
        assert!(!ghostty.matches("ghostty", "com.mitchellh.ghostty"));
        let steam = Match::Title("Steam".to_owned());
        assert!(steam.matches("steam", "Steam"));
        assert!(!steam.matches("Steam", "steam"));
    }

    #[test]
    fn what_a_pie_cannot_be_is_refused_by_its_key() {
        let menus = "\n[shell.pie.menus]\nx = [{ running = true }]";
        let with = |table: &str| format!("[shell.pie]\n{table}{menus}");
        assert_eq!(
            refused(&with("size = 64")),
            (
                "shell.pie.size".to_owned(),
                "64 is outside 128 to 1024 pixels".to_owned()
            )
        );
        assert_eq!(
            refused(&with("icons = \"\"")),
            ("shell.pie.icons".to_owned(), "an empty path".to_owned())
        );
        assert_eq!(
            refused(&with(
                "aliases = [{ app-id = \"a\", as = \"a\" }, { app-id = \"b\", title = \"b\", as = \"b\" }]"
            )),
            (
                "shell.pie.aliases[2]".to_owned(),
                "give it one of app-id or title".to_owned()
            )
        );
        assert_eq!(
            refused(&with("aliases = [{ title = \"a\" }]")),
            (
                "shell.pie.aliases[1]".to_owned(),
                "`as` names no application".to_owned()
            )
        );
        assert_eq!(
            refused(&with("ignore = [{ app-id = \"\" }]")),
            (
                "shell.pie.ignore[1]".to_owned(),
                "an empty app-id or title matches nothing".to_owned()
            )
        );
        assert_eq!(
            refused("[shell.pie]\nsize = 512"),
            (
                "shell.pie.menus".to_owned(),
                "name at least one pie, as `launchers = [...]`".to_owned()
            )
        );
        assert_eq!(
            refused("[shell.pie.menus]\nlaunchers = []"),
            (
                "shell.pie.menus.launchers".to_owned(),
                "a pie with nothing in it".to_owned()
            )
        );
        assert_eq!(
            refused(
                "[shell.pie.menus]\nlaunchers = [{ running = true }, \
                 { label = \"a\", items = [{ label = \"b\", exec = [\"b\"] }, { separator = true }] }]"
            ),
            (
                "shell.pie.menus.launchers[2].items[2]".to_owned(),
                "a pie has no separator, applications or session".to_owned()
            )
        );
        assert!(matches!(
            pie("[shell.pie]\ncolour = 1"),
            Err(Error::Parse(_))
        ));
    }

    #[test]
    fn a_shell_built_without_pies_names_the_feature() {
        let text = "[shell.pie.menus]\nx = [{ running = true }]";
        let without = ShellBuilt {
            pie: false,
            ..ShellBuilt::FULL
        };
        assert!(matches!(
            shell(text, without),
            Err(Error::ShellNotBuilt {
                key: "shell.pie",
                feature: "pie"
            })
        ));
    }

    #[test]
    fn a_changed_pie_is_a_changed_shell_so_the_shell_is_told() {
        let one = shell(
            "[shell.pie.menus]\nx = [{ running = true }]",
            ShellBuilt::FULL,
        );
        let two = shell(
            "[shell.pie]\nsize = 300\n[shell.pie.menus]\nx = [{ running = true }]",
            ShellBuilt::FULL,
        );
        assert_ne!(one.unwrap(), two.unwrap());
    }
}
