//! `[shell.start-menu]`: what the start menu holds, what typing in it
//! finds, and how its button on the panel looks.
//!
//! ```toml
//! [shell.start-menu]
//! icon = "start-here"            # an icon theme's name, or an image file
//! mode = "replace"               # in place of every application
//! search = "all"                 # and still find any of them by typing
//! items = [
//!     { app = "firefox" },
//!     { label = "Terminal", exec = ["foot"] },
//!     { separator = true },
//!     { session = true },
//! ]
//! ```
//!
//! The items are a menu's, in the [`menu`](crate::menu) vocabulary, as the
//! menu file's are. With no table, the start menu is every application by
//! group, then the ways to leave, and its button is Perspicax's mark.

use serde::Deserialize;

use crate::{
    Error, invalid,
    menu::{self, Item, Mode, RawItem, Vocabulary},
};

/// The start menu, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartMenu {
    /// The start button's icon: a name in the icon theme, or, with a `/` in
    /// it, an image file, as written: `~` and a path beside config.toml are
    /// the shell's to resolve. `None` is Perspicax's mark, and so is an icon
    /// that cannot be found or read.
    pub icon: Option<String>,
    /// Whether `items` go above the applications, or in place of them and
    /// the ways to leave.
    pub mode: Mode,
    /// What typing in the menu finds.
    pub search: Search,
    pub items: Vec<Item>,
}

/// What typing in the start menu finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Search {
    /// What the menu holds.
    #[default]
    Menu,
    /// What the menu holds and every installed application, so that a
    /// short menu can still start anything.
    All,
    /// Nothing: the menu has no search line.
    #[serde(rename = "none")]
    Nothing,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RawStartMenu {
    icon: Option<String>,
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    search: Search,
    #[serde(default)]
    items: Vec<RawItem>,
}

impl RawStartMenu {
    /// Whether it names the start button's icon, which only a panel draws.
    pub(crate) fn names_icon(&self) -> bool {
        self.icon.is_some()
    }

    pub(crate) fn apply(self) -> Result<StartMenu, Error> {
        if self
            .icon
            .as_ref()
            .is_some_and(|icon| icon.trim().is_empty())
        {
            return Err(invalid(
                "shell.start-menu.icon".to_owned(),
                "an empty name; leave it out for Perspicax's mark".to_owned(),
            ));
        }
        if self.mode == Mode::Replace && self.items.is_empty() {
            return Err(invalid(
                "shell.start-menu.items".to_owned(),
                "a menu in place of everything, with nothing in it, never opens; for only \
                 the ways to leave, write `[{ session = true }]`"
                    .to_owned(),
            ));
        }
        let items = menu::check(self.items, Vocabulary::Menu).map_err(|refused| {
            invalid(
                refused.key("shell.start-menu.items"),
                refused.why.to_owned(),
            )
        })?;
        Ok(StartMenu {
            icon: self.icon,
            mode: self.mode,
            search: self.search,
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Built, Profile, Shell, ShellBuilt, parse, shell};

    fn start_menu(text: &str) -> Result<StartMenu, Error> {
        shell(text, ShellBuilt::FULL).map(|shell| shell.start_menu.expect("a start menu"))
    }

    /// The key a refusal names, and its reason.
    fn refused(text: &str) -> (String, String) {
        match start_menu(text) {
            Err(Error::Invalid { key, reason }) => (key, reason),
            other => panic!("not refused by key: {other:?}"),
        }
    }

    #[test]
    fn a_start_menu_is_read_with_its_icon_mode_search_and_items() {
        let text = r#"
            [shell.start-menu]
            icon = "start-here"
            mode = "replace"
            search = "all"
            items = [
                { app = "firefox" },
                { app = "org.kde.dolphin", label = "Files" },
                { label = "Terminal", exec = ["foot"], icon = "utilities-terminal" },
                { label = "Projects", items = [
                    { label = "perspicax", exec = ["foot", "-D", "~/code/perspicax"] },
                ] },
                { separator = true },
                { session = true },
            ]
        "#;
        let read = start_menu(text).expect("a start menu");
        assert_eq!(read.icon.as_deref(), Some("start-here"));
        assert_eq!(read.mode, Mode::Replace);
        assert_eq!(read.search, Search::All);
        assert_eq!(read.items.len(), 6);
        assert_eq!(
            read.items[0],
            Item::App {
                app: "firefox".to_owned(),
                label: None,
                icon: None,
            }
        );
        assert_eq!(read.items[5], Item::Session);
        // The compositor reads the same table, whatever the shell has.
        assert!(parse(text, Built::default()).is_ok());

        let only_leaving = r#"
            [shell.start-menu]
            mode = "replace"
            search = "none"
            items = [{ session = true }]
        "#;
        let read = start_menu(only_leaving).expect("a start menu");
        assert_eq!(read.search, Search::Nothing);
        assert_eq!(read.items, [Item::Session]);
    }

    #[test]
    fn a_start_menu_of_items_alone_extends_and_finds_what_it_holds() {
        let read = start_menu("[shell.start-menu]\nitems = [{ app = \"firefox\" }]").unwrap();
        assert_eq!(read.icon, None);
        assert_eq!(read.mode, Mode::Extend);
        assert_eq!(read.search, Search::Menu);
        let read = start_menu("[shell.start-menu]\nicon = \"~/logo.png\"").unwrap();
        assert_eq!(read.icon.as_deref(), Some("~/logo.png"), "as written");
        assert_eq!(read.items, []);
    }

    #[test]
    fn no_start_menu_is_written_by_default_in_either_profile() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let shell = Shell::profile(profile, ShellBuilt::FULL);
            assert_eq!(shell.start_menu, None, "{profile:?}");
        }
        assert_eq!(shell("", ShellBuilt::FULL).unwrap().start_menu, None);
    }

    #[test]
    fn what_a_start_menu_cannot_be_is_refused_by_its_key() {
        let at = |items: &str| refused(&format!("[shell.start-menu]\nitems = {items}"));
        assert_eq!(
            at("[{ session = true, label = \"x\" }]").0,
            "shell.start-menu.items[1]"
        );
        assert_eq!(
            at("[{ app = \"a\" }, { label = \"b\", items = [{ separator = true, exec = [\"c\"] }] }]")
                .0,
            "shell.start-menu.items[2].items[1]"
        );
        assert_eq!(
            at("[{ running = true }]").0,
            "shell.start-menu.items[1]",
            "a pie's word, not a menu's"
        );
        assert_eq!(
            refused("[shell.start-menu]\nicon = \" \"").0,
            "shell.start-menu.icon"
        );
        let (key, reason) = refused("[shell.start-menu]\nmode = \"replace\"");
        assert_eq!(key, "shell.start-menu.items");
        assert!(reason.contains("session = true"), "{reason}");
        for (text, word) in [
            ("mode = \"sideways\"", "sideways"),
            ("search = \"some\"", "some"),
            ("colour = 1", "colour"),
        ] {
            match start_menu(&format!("[shell.start-menu]\n{text}")) {
                Err(Error::Parse(error)) => assert!(error.contains(word), "{error}"),
                other => panic!("{text}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_shell_built_without_menus_or_a_panel_names_the_feature() {
        let without_menus = ShellBuilt {
            menus: false,
            ..ShellBuilt::FULL
        };
        assert!(matches!(
            shell("[shell.start-menu]\nsearch = \"all\"", without_menus),
            Err(Error::ShellNotBuilt {
                key: "shell.start-menu",
                feature: "menus"
            })
        ));
        let without_panel = ShellBuilt {
            panel: false,
            ..ShellBuilt::FULL
        };
        assert!(
            shell("[shell.start-menu]\nsearch = \"all\"", without_panel).is_ok(),
            "Logo opens it with no panel"
        );
        assert!(matches!(
            shell("[shell.start-menu]\nicon = \"start-here\"", without_panel),
            Err(Error::ShellNotBuilt {
                key: "shell.start-menu.icon",
                feature: "panel"
            })
        ));
    }

    #[test]
    fn a_changed_start_menu_is_a_changed_shell_so_the_shell_is_told() {
        let one = shell(
            "[shell.start-menu]\nitems = [{ app = \"firefox\" }]",
            ShellBuilt::FULL,
        );
        let two = shell(
            "[shell.start-menu]\nitems = [{ app = \"firefox\" }]\nsearch = \"none\"",
            ShellBuilt::FULL,
        );
        assert_ne!(one.unwrap(), two.unwrap());
    }
}
