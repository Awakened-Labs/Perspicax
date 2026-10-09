//! The words a hand-written menu is made of, wherever it is written.
//!
//! ```toml
//! items = [
//!     { label = "Terminal", exec = ["foot"], icon = "utilities-terminal" },
//!     { app = "firefox" },                       # an installed application
//!     { label = "Projects", items = [ ... ] },   # a submenu
//!     { separator = true },
//! ]
//! ```
//!
//! The menu file (`[shell] menu-file`) and a pie (`[shell.pie]`) share them,
//! so a menu is written one way whichever menu it is. Each item is exactly
//! one kind of thing, and each [`Vocabulary`] admits its own kinds: a menu
//! also has `separator`, `applications` (the applications by group) and
//! `session` (the ways to leave); a pie has `running`, where the running
//! applications go.
//!
//! Checking is all this does. `~` in an argument is the home folder, and
//! that is the shell's to expand: the compositor reads the same file with
//! no home of the shell's to go by.

use serde::Deserialize;

/// Where a menu file's items go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Above the applications.
    #[default]
    Extend,
    /// In place of everything else.
    Replace,
}

/// Which menu the items are for, and so which kinds of item it admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vocabulary {
    /// The menu file's: separators, the applications, the ways to leave.
    Menu,
    /// A pie's: `running`, once, in its first ring.
    Pie,
}

/// One item, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A program and its arguments, run without a shell.
    Run {
        label: String,
        icon: Option<String>,
        exec: Vec<String>,
        terminal: bool,
    },
    /// An installed application, by its desktop file's id. A pie also takes
    /// a path to a desktop file here, one with a `/` in it.
    App {
        app: String,
        label: Option<String>,
        icon: Option<String>,
    },
    /// A submenu.
    Open {
        label: String,
        icon: Option<String>,
        items: Vec<Item>,
    },
    Separator,
    Applications,
    Session,
    /// Where a pie's running applications go.
    Running,
}

/// Why an item was refused, and which: `at` numbers it from one, a
/// submenu's items after their submenu's, so `[2, 1]` is the first item of
/// the second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub at: Vec<usize>,
    pub why: &'static str,
}

/// One item as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawItem {
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
    #[serde(default)]
    running: bool,
}

/// Check the items of one menu, as `vocabulary` has them.
///
/// # Errors
///
/// [`Refused`] for the first item that is not exactly one thing the
/// vocabulary knows, or is not written as that thing must be.
pub fn check(raw: Vec<RawItem>, vocabulary: Vocabulary) -> Result<Vec<Item>, Refused> {
    level(raw, vocabulary, &[], &mut 0)
}

fn level(
    raw: Vec<RawItem>,
    vocabulary: Vocabulary,
    above: &[usize],
    running: &mut usize,
) -> Result<Vec<Item>, Refused> {
    raw.into_iter()
        .enumerate()
        .map(|(n, item)| {
            let mut at = above.to_vec();
            at.push(n + 1);
            item.check(vocabulary, at, running)
        })
        .collect()
}

impl RawItem {
    fn check(
        self,
        vocabulary: Vocabulary,
        at: Vec<usize>,
        running: &mut usize,
    ) -> Result<Item, Refused> {
        let refuse = |why| Refused {
            at: at.clone(),
            why,
        };
        let decorated = self.label.is_some() || self.icon.is_some();
        match vocabulary {
            Vocabulary::Menu => {
                if self.running {
                    return Err(refuse(
                        "running is a pie's: where its running applications go",
                    ));
                }
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
                if (self.separator || self.applications || self.session) && decorated {
                    return Err(refuse(
                        "a separator, applications or session has no label or icon",
                    ));
                }
            }
            Vocabulary::Pie => {
                if self.separator || self.applications || self.session {
                    return Err(refuse("a pie has no separator, applications or session"));
                }
                let kinds = [
                    self.exec.is_some(),
                    self.items.is_some(),
                    self.app.is_some(),
                    self.running,
                ];
                if kinds.into_iter().filter(|&kind| kind).count() != 1 {
                    return Err(refuse("give it exactly one of exec, items, app or running"));
                }
                if self.running {
                    if decorated || self.terminal {
                        return Err(refuse("running has no label or icon"));
                    }
                    *running += 1;
                    if at.len() > 1 || *running > 1 {
                        return Err(refuse("running goes in a pie's first ring, once"));
                    }
                    return Ok(Item::Running);
                }
                if self.app.as_deref().is_some_and(|app| app.trim().is_empty()) {
                    return Err(refuse("app names no application"));
                }
                if self.items.as_ref().is_some_and(Vec::is_empty) {
                    return Err(refuse("a submenu with nothing in it"));
                }
            }
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
        Ok(if let Some(exec) = &self.exec {
            if exec.first().is_none_or(String::is_empty) {
                return Err(refuse("exec names no program"));
            }
            Item::Run {
                label: label()?,
                icon: self.icon.clone(),
                exec: exec.clone(),
                terminal: self.terminal,
            }
        } else if let Some(below) = self.items {
            Item::Open {
                label: label()?,
                icon: self.icon.clone(),
                items: level(below, vocabulary, &at, running)?,
            }
        } else if let Some(app) = self.app {
            Item::App {
                app,
                label: self.label,
                icon: self.icon,
            }
        } else if self.separator {
            Item::Separator
        } else if self.applications {
            Item::Applications
        } else {
            Item::Session
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(text: &str) -> Vec<RawItem> {
        #[derive(Deserialize)]
        struct Raw {
            items: Vec<RawItem>,
        }
        toml::from_str::<Raw>(text).expect("items").items
    }

    fn pie(text: &str) -> Result<Vec<Item>, Refused> {
        check(items(text), Vocabulary::Pie)
    }

    fn menu(text: &str) -> Result<Vec<Item>, Refused> {
        check(items(text), Vocabulary::Menu)
    }

    fn refused(at: &[usize], why: &'static str) -> Result<Vec<Item>, Refused> {
        Err(Refused {
            at: at.to_vec(),
            why,
        })
    }

    #[test]
    fn a_pie_reads_programs_apps_submenus_and_where_the_running_go() {
        let read = pie(r#"
            items = [
                { label = "terminal", exec = ["term"] },
                { app = "vivaldi-stable" },
                { app = "~/.local/share/applications/x.desktop", icon = "x" },
                { label = "internet", icon = "web", items = [
                    { label = "firefox", exec = ["firefox", "-P", "work"] },
                ] },
                { running = true },
            ]
        "#)
        .expect("a pie");
        assert_eq!(
            read,
            [
                Item::Run {
                    label: "terminal".to_owned(),
                    icon: None,
                    exec: vec!["term".to_owned()],
                    terminal: false,
                },
                Item::App {
                    app: "vivaldi-stable".to_owned(),
                    label: None,
                    icon: None,
                },
                Item::App {
                    app: "~/.local/share/applications/x.desktop".to_owned(),
                    label: None,
                    icon: Some("x".to_owned()),
                },
                Item::Open {
                    label: "internet".to_owned(),
                    icon: Some("web".to_owned()),
                    items: vec![Item::Run {
                        label: "firefox".to_owned(),
                        icon: None,
                        exec: vec!["firefox".into(), "-P".into(), "work".into()],
                        terminal: false,
                    }],
                },
                Item::Running,
            ]
        );
    }

    #[test]
    fn a_pie_refuses_what_only_a_menu_has_and_running_anywhere_but_once_in_its_first_ring() {
        assert_eq!(
            pie("items = [{ separator = true }]"),
            refused(&[1], "a pie has no separator, applications or session")
        );
        assert_eq!(
            pie("items = [{ session = true }]"),
            refused(&[1], "a pie has no separator, applications or session")
        );
        assert_eq!(
            pie(r#"items = [{ label = "x", exec = ["x"], running = true }]"#),
            refused(&[1], "give it exactly one of exec, items, app or running")
        );
        assert_eq!(
            pie(r#"items = [{ label = "x" }]"#),
            refused(&[1], "give it exactly one of exec, items, app or running")
        );
        assert_eq!(
            pie(r#"items = [{ running = true, label = "x" }]"#),
            refused(&[1], "running has no label or icon")
        );
        assert_eq!(
            pie("items = [{ running = true }, { running = true }]"),
            refused(&[2], "running goes in a pie's first ring, once")
        );
        assert_eq!(
            pie(r#"items = [{ label = "a", items = [{ running = true }] }]"#),
            refused(&[1, 1], "running goes in a pie's first ring, once")
        );
        assert_eq!(
            pie(r#"items = [{ app = " " }]"#),
            refused(&[1], "app names no application")
        );
        assert_eq!(
            pie(r#"items = [{ label = "a", items = [] }]"#),
            refused(&[1], "a submenu with nothing in it")
        );
        assert_eq!(
            pie(r#"items = [{ label = "a", items = [{ label = "b", exec = [] }] }]"#),
            refused(&[1, 1], "exec names no program")
        );
        assert_eq!(
            pie(r#"items = [{ exec = ["x"] }]"#),
            refused(&[1], "it needs a label")
        );
        assert_eq!(
            pie(r#"items = [{ app = "x", terminal = true }]"#),
            refused(&[1], "terminal goes with exec")
        );
    }

    #[test]
    fn a_menu_refuses_running_and_keeps_its_own_words() {
        assert_eq!(
            menu("items = [{ running = true }]"),
            refused(
                &[1],
                "running is a pie's: where its running applications go"
            )
        );
        assert_eq!(
            menu(r#"items = [{ label = "x", exec = ["x"], app = "x" }]"#),
            refused(
                &[1],
                "give it exactly one of exec, items, app, separator, applications or session"
            )
        );
        assert_eq!(
            menu(r#"items = [{ separator = true, label = "x" }]"#),
            refused(
                &[1],
                "a separator, applications or session has no label or icon"
            )
        );
        assert_eq!(
            menu(r#"items = [{ separator = true }, { applications = true }, { session = true }]"#),
            Ok(vec![Item::Separator, Item::Applications, Item::Session])
        );
        // A menu's empty submenu and empty app are left to the shell, as
        // they always were.
        assert!(menu(r#"items = [{ label = "a", items = [] }, { app = "" }]"#).is_ok());
    }
}
