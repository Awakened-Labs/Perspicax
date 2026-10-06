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

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::apps::Run;

/// A menu file, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuFile {
    pub(crate) mode: Mode,
    pub(crate) items: Vec<FileItem>,
}

/// Where a menu file's items go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Mode {
    /// Above the applications.
    #[default]
    Extend,
    /// In place of everything else.
    Replace,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItem {
    label: Option<String>,
    icon: Option<String>,
    exec: Option<Vec<String>>,
    #[serde(default)]
    terminal: bool,
    items: Option<Vec<RawItem>>,
    app: Option<String>,
    #[serde(default)]
    separator: bool,
    #[serde(default)]
    applications: bool,
    #[serde(default)]
    session: bool,
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
    Ok(MenuFile {
        mode: raw.mode,
        items: items(raw.items, "", home)?,
    })
}

fn items(
    raw: Vec<RawItem>,
    above: &str,
    home: Option<&Path>,
) -> Result<Vec<FileItem>, MenuFileError> {
    raw.into_iter()
        .enumerate()
        .map(|(n, item)| {
            let at = format!("{above}{}", n + 1);
            item.check(&at, home)
        })
        .collect()
}

impl RawItem {
    fn check(self, at: &str, home: Option<&Path>) -> Result<FileItem, MenuFileError> {
        let refuse = |why| MenuFileError::Item {
            at: at.to_owned(),
            why,
        };
        let kinds = [
            self.exec.is_some(),
            self.items.is_some(),
            self.app.is_some(),
            self.separator,
            self.applications,
            self.session,
        ];
        if kinds.into_iter().filter(|&kind| kind).count() != 1 {
            return Err(refuse(
                "give it exactly one of exec, items, app, separator, applications or session",
            ));
        }
        let decorated = self.label.is_some() || self.icon.is_some();
        if (self.separator || self.applications || self.session) && decorated {
            return Err(refuse(
                "a separator, applications or session has no label or icon",
            ));
        }
        if self.terminal && self.exec.is_none() {
            return Err(refuse("terminal goes with exec"));
        }
        let label = || {
            self.label
                .clone()
                .filter(|label| !label.trim().is_empty())
                .ok_or_else(|| refuse("it needs a label"))
        };
        Ok(if let Some(argv) = &self.exec {
            if argv.first().is_none_or(String::is_empty) {
                return Err(refuse("exec names no program"));
            }
            FileItem::Run {
                label: label()?,
                icon: self.icon.clone(),
                run: Run {
                    argv: argv.iter().map(|word| tilde(word, home)).collect(),
                    terminal: self.terminal,
                    dir: None,
                },
            }
        } else if let Some(below) = self.items {
            FileItem::Open {
                label: label()?,
                icon: self.icon.clone(),
                items: items(below, &format!("{at}."), home)?,
            }
        } else if let Some(id) = self.app {
            FileItem::App {
                id,
                label: self.label,
                icon: self.icon,
            }
        } else if self.separator {
            FileItem::Separator
        } else if self.applications {
            FileItem::Applications
        } else {
            FileItem::Session
        })
    }
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
