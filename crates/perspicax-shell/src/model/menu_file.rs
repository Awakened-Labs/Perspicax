//! The menu file: a person's own root menu, in TOML, named by `[shell]
//! menu-file`.
//!
//! ```toml
//! mode = "extend"         # above the applications; "replace" for instead
//!
//! [[items]]
//! label = "Terminal"
//! exec = ["foot"]         # a program and its arguments, run without a shell
//! icon = "utilities-terminal"
//!
//! [[items]]
//! app = "firefox"         # an installed application, by its desktop file
//!
//! [[items]]
//! separator = true
//!
//! [[items]]
//! label = "Projects"      # a submenu
//! items = [{ label = "perspicax", exec = ["foot", "-D", "~/code/perspicax"] }]
//! ```
//!
//! `applications = true` stands for the applications by group, and
//! `session = true` for the ways to leave, as `[shell] leave` lists them, so
//! a menu that replaces the root menu can still hold them wherever it likes. Each item is exactly one of
//! these. `~` at the start of an argument is the home folder, as a shell
//! would have it.
//!
//! The words, and the checking of them, are perspicax-config's
//! [`menu`](perspicax_config::menu) vocabulary, which a pie is written in
//! too. What is left here is reading the file and the `~`.

use std::path::{Path, PathBuf};

pub(crate) use perspicax_config::menu::Mode;
use perspicax_config::menu::{self, Item, RawItem, Vocabulary};
use serde::Deserialize;

use super::apps::Run;

/// A menu file, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuFile {
    pub(crate) mode: Mode,
    pub(crate) items: Vec<FileItem>,
}

/// One item of a menu file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileItem {
    Run {
        label: String,
        icon: Option<String>,
        run: Run,
    },
    App {
        id: String,
        label: Option<String>,
        icon: Option<String>,
    },
    Open {
        label: String,
        icon: Option<String>,
        items: Vec<FileItem>,
    },
    Separator,
    Applications,
    Session,
}

/// Why a menu file cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum MenuFileError {
    #[error("{0}")]
    Read(String),
    #[error("{0}")]
    Parse(String),
    /// An item, numbered from one, `2.1` for the first of the second's.
    #[error("item {at}: {why}")]
    Item { at: String, why: &'static str },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    items: Vec<RawItem>,
}

/// Read the menu file at `path`.
pub(crate) fn read(path: &Path, home: Option<&Path>) -> Result<MenuFile, MenuFileError> {
    let text =
        std::fs::read_to_string(path).map_err(|error| MenuFileError::Read(error.to_string()))?;
    parse(&text, home)
}

/// Read a menu file's text. `home` is what `~` stands for.
pub(crate) fn parse(text: &str, home: Option<&Path>) -> Result<MenuFile, MenuFileError> {
    let raw: RawFile =
        toml::from_str(text).map_err(|error| MenuFileError::Parse(error.to_string()))?;
    let items = menu::check(raw.items, Vocabulary::Menu).map_err(|refused| {
        let at: Vec<String> = refused.at.iter().map(usize::to_string).collect();
        MenuFileError::Item {
            at: at.join("."),
            why: refused.why,
        }
    })?;
    Ok(MenuFile {
        mode: raw.mode,
        items: file_items(items, home),
    })
}

/// The checked items, with `~` in each argument as the home folder. The
/// menu vocabulary has no `running`, so there is none to leave out.
fn file_items(items: Vec<Item>, home: Option<&Path>) -> Vec<FileItem> {
    items
        .into_iter()
        .filter_map(|item| {
            Some(match item {
                Item::Run {
                    label,
                    icon,
                    exec,
                    terminal,
                } => FileItem::Run {
                    label,
                    icon,
                    run: Run {
                        argv: exec.iter().map(|word| tilde(word, home)).collect(),
                        terminal,
                        dir: None,
                    },
                },
                Item::App { app, label, icon } => FileItem::App {
                    id: app,
                    label,
                    icon,
                },
                Item::Open { label, icon, items } => FileItem::Open {
                    label,
                    icon,
                    items: file_items(items, home),
                },
                Item::Separator => FileItem::Separator,
                Item::Applications => FileItem::Applications,
                Item::Session => FileItem::Session,
                Item::Running => return None,
            })
        })
        .collect()
}

/// `word` with a leading `~` as the home folder.
fn tilde(word: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return word.to_owned();
    };
    match word.strip_prefix('~') {
        Some("") => home.display().to_string(),
        Some(rest) if rest.starts_with('/') => {
            PathBuf::from(home).join(&rest[1..]).display().to_string()
        }
        _ => word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(text: &str) -> MenuFileError {
        parse(text, None).expect_err("refused")
    }

    #[test]
    fn a_menu_file_reads_programs_apps_submenus_and_separators() {
        let file = parse(
            r#"
            mode = "replace"
            items = [
                { label = "Projects", items = [
                    { label = "Code", exec = ["foot", "-D", "~/code"], terminal = false },
                ] },
                { app = "firefox" },
                { separator = true },
                { applications = true },
                { session = true },
            ]
            "#,
            Some(Path::new("/home/ada")),
        )
        .expect("a menu file");
        assert_eq!(file.mode, Mode::Replace);
        assert_eq!(
            file.items,
            [
                FileItem::Open {
                    label: "Projects".to_owned(),
                    icon: None,
                    items: vec![FileItem::Run {
                        label: "Code".to_owned(),
                        icon: None,
                        run: Run {
                            argv: vec!["foot".into(), "-D".into(), "/home/ada/code".into()],
                            terminal: false,
                            dir: None,
                        },
                    }],
                },
                FileItem::App {
                    id: "firefox".to_owned(),
                    label: None,
                    icon: None,
                },
                FileItem::Separator,
                FileItem::Applications,
                FileItem::Session,
            ]
        );
        assert_eq!(parse("", None).expect("empty").mode, Mode::Extend);
    }

    #[test]
    fn a_menu_file_item_that_is_not_one_thing_is_refused_by_its_place() {
        assert_eq!(
            refused("[[items]]\nlabel = \"x\"\nexec = [\"x\"]\napp = \"x\""),
            MenuFileError::Item {
                at: "1".to_owned(),
                why: "give it exactly one of exec, items, app, separator, applications or session"
            }
        );
        assert_eq!(
            refused("[[items]]\nlabel = \"a\"\n[[items.items]]\nexec = [\"x\"]"),
            MenuFileError::Item {
                at: "1.1".to_owned(),
                why: "it needs a label"
            }
        );
        assert_eq!(
            refused("[[items]]\nlabel = \"x\"\nexec = []"),
            MenuFileError::Item {
                at: "1".to_owned(),
                why: "exec names no program"
            }
        );
        assert!(matches!(
            refused("mode = \"sideways\""),
            MenuFileError::Parse(_)
        ));
        assert!(matches!(
            refused("[[items]]\ncolour = 1"),
            MenuFileError::Parse(_)
        ));
    }
}
