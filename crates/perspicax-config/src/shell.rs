//! `[shell]`: what perspicax-shell puts on the desktop.
//!
//! The compositor and the shell read the same file, each for its own part.
//! The compositor takes any `[shell]` table, whatever the shell was built
//! with: it only starts the shell and tells it when the table changed. So a
//! key for a component the shell was built without is the shell's to refuse,
//! not the compositor's. The shell reads the file through [`crate::shell`],
//! which refuses such a key by name and names the cargo feature that would
//! provide it, as [`crate::parse`] does for the compositor's own keys.

use std::path::PathBuf;

use perspicax_policy::Colour;
use serde::Deserialize;

use crate::{Error, Profile, invalid};

/// Which of perspicax-shell's components it was built with: its cargo
/// features, as far as config cares. The shell fills it in with `cfg!`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellBuilt {
    pub wallpaper: bool,
    pub panel: bool,
    pub menus: bool,
    pub tray: bool,
    /// Desktop icons.
    pub icons: bool,
}

impl ShellBuilt {
    /// Every component. The compositor reads `[shell]` as if the shell had
    /// them all, since what the shell was built with is the shell's to check.
    pub const FULL: Self = Self {
        wallpaper: true,
        panel: true,
        menus: true,
        tray: true,
        icons: true,
    };
}

/// Everything `[shell]` decides, with the profile applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
    /// Whether perspicax starts the shell at all. `false` leaves the desktop
    /// to whatever the person starts from `autostart`.
    pub enabled: bool,
    /// What every monitor shows behind the windows. `None` is no wallpaper,
    /// for a person who brings their own.
    pub wallpaper: Option<Wallpaper>,
    /// A right-click on the wallpaper opens a menu of the installed
    /// applications.
    pub root_menu: bool,
    /// A TOML menu file that replaces the root menu or extends it.
    pub menu_file: Option<PathBuf>,
    /// The desktop folder's files, as icons on the wallpaper. Never with no
    /// wallpaper: they are drawn on it.
    pub desktop_icons: bool,
    /// What the desktop shows of an application's entry that may not be
    /// run.
    pub untrusted_launchers: UntrustedLaunchers,
    /// How soon, in milliseconds, a second click on a desktop icon must
    /// follow the first to open it: `[input.pointer] double-click-ms`, read
    /// here so that a change to it reaches the shell as one to `[shell]`
    /// does.
    pub double_click_ms: u32,
    /// The icon theme, by its folder name. `None` is hicolor, the theme every
    /// application installs into, and the one any theme missing an icon
    /// falls back to.
    pub icon_theme: Option<String>,
    /// What an application that asks for a terminal is run in, as a program
    /// and its arguments. `None` takes the first terminal installed.
    pub terminal: Option<Vec<String>>,
    /// What the start menu's Lock runs.
    pub lock: Vec<String>,
    /// The panel, or `None` for none.
    pub panel: Option<Panel>,
}

/// What the desktop shows of an application's entry that may not be run.
/// Anyone's download can land on the desktop, and an entry there could call
/// itself a document and run anything at all, so an application's entry is
/// trusted only when it may be run, as Plasma trusts one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UntrustedLaunchers {
    /// Shown as the file it is, and opened as one.
    #[default]
    AsFiles,
    /// Not shown, until it is made executable.
    Hidden,
}

/// A wallpaper: a colour, and an image over it.
#[derive(Debug, Clone, PartialEq)]
pub struct Wallpaper {
    /// Shown wherever the image is not: everywhere when there is no image,
    /// beside one that fits rather than fills, and in place of one that
    /// cannot be read.
    pub colour: Colour,
    /// As written in the file. The shell finds it from there: `~/` is the
    /// home folder, and a relative path is beside the config file.
    pub image: Option<PathBuf>,
    pub mode: WallpaperMode,
}

/// How an image is fitted to a monitor of a different shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperMode {
    /// Scaled to cover the monitor, cropped at the long sides.
    #[default]
    Fill,
    /// Scaled to show whole, with the colour at the short sides.
    Fit,
    /// At its own size, in the middle.
    Center,
    /// At its own size, repeated from the top-left corner.
    Tile,
}

/// The panel: a bar along one edge of a monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub edge: Edge,
    /// In logical pixels. Windows are kept out of it.
    pub height: u32,
    /// Which monitors have one.
    pub outputs: PanelOutputs,
    /// Which windows each panel's taskbar lists.
    pub taskbar: TaskbarScope,
    /// What it holds, in order from the left.
    pub items: Vec<Item>,
    /// The clock's format, as strftime writes it.
    pub clock: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Edge {
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelOutputs {
    /// Every monitor.
    All,
    /// The first monitor, the one at the left of the desk.
    First,
    /// The monitors of these connector names.
    Named(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskbarScope {
    /// The windows on the panel's own monitor.
    ThisOutput,
    /// Every window, on every panel.
    All,
}

/// One thing a panel holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Item {
    /// The start button, which opens the start menu.
    Start,
    /// The open windows, one button each.
    Taskbar,
    /// The workspaces.
    Pager,
    /// Status icons from other programs.
    Tray,
    Clock,
}

impl Item {
    /// Its name as `items` writes it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Taskbar => "taskbar",
            Self::Pager => "pager",
            Self::Tray => "tray",
            Self::Clock => "clock",
        }
    }
}

/// What `wallpaper` is set to for no wallpaper.
const NONE: &str = "none";

/// The tallest panel: past this it is a typo, not a panel.
const PANEL_MAX: u32 = 128;
/// The shortest panel that still holds a line of text.
const PANEL_MIN: u32 = 16;

impl Shell {
    /// A profile's shell, holding only what `built` has.
    #[must_use]
    pub fn profile(profile: Profile, built: ShellBuilt) -> Self {
        let (colour, panel, icons, theme) = match profile {
            // Plasma's: a panel along the bottom and icons on the desktop.
            // Adwaita, for an icon on most applications and every menu
            // group: hicolor holds only what applications install, and
            // Plasma's own Breeze is drawn mostly in SVG, which a shell
            // built without `svg` cannot draw.
            Profile::Classic => (
                Colour::rgb(0x1e, 0x4a, 0x73),
                Some(Panel::classic(built)),
                true,
                Some("Adwaita"),
            ),
            // Fluxbox's: the desktop is a wallpaper and a right-click menu.
            Profile::Minimal => (Colour::rgb(0x3c, 0x40, 0x48), None, false, None),
        };
        Self {
            enabled: true,
            wallpaper: built.wallpaper.then_some(Wallpaper {
                colour,
                image: None,
                mode: WallpaperMode::Fill,
            }),
            root_menu: built.menus,
            menu_file: None,
            desktop_icons: icons && built.icons,
            untrusted_launchers: UntrustedLaunchers::AsFiles,
            double_click_ms: crate::DOUBLE_CLICK_MS,
            icon_theme: theme
                .filter(|_| built.menus || built.panel || built.icons)
                .map(str::to_owned),
            terminal: None,
            lock: vec!["swaylock".to_owned()],
            panel: panel.filter(|_| built.panel),
        }
    }
}

impl Panel {
    /// Plasma's panel, holding only what `built` has.
    fn classic(built: ShellBuilt) -> Self {
        let items = [
            Item::Start,
            Item::Taskbar,
            Item::Pager,
            Item::Tray,
            Item::Clock,
        ];
        Self {
            edge: Edge::Bottom,
            height: 40,
            outputs: PanelOutputs::All,
            taskbar: TaskbarScope::ThisOutput,
            items: items
                .into_iter()
                .filter(|item| match item {
                    Item::Start => built.menus,
                    Item::Tray => built.tray,
                    _ => true,
                })
                .collect(),
            clock: "%H:%M".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RawShell {
    enabled: Option<bool>,
    wallpaper: Option<String>,
    wallpaper_mode: Option<WallpaperMode>,
    root_menu: Option<bool>,
    menu_file: Option<PathBuf>,
    desktop_icons: Option<bool>,
    untrusted_launchers: Option<UntrustedLaunchers>,
    icon_theme: Option<String>,
    terminal: Option<Vec<String>>,
    lock: Option<Vec<String>>,
    panel: Option<RawPanel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawPanel {
    enabled: Option<bool>,
    edge: Option<Edge>,
    height: Option<u32>,
    outputs: Option<RawOutputs>,
    taskbar: Option<TaskbarScope>,
    items: Option<Vec<Item>>,
    clock: Option<String>,
}

/// `"all"`, `"first"`, or a list of connector names.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawOutputs {
    Word(String),
    List(Vec<String>),
}

impl RawShell {
    /// Layer the written keys on `shell`, refusing any for a component
    /// `built` lacks.
    pub(crate) fn apply(self, mut shell: Shell, built: ShellBuilt) -> Result<Shell, Error> {
        self.check(built)?;
        if let Some(enabled) = self.enabled {
            shell.enabled = enabled;
        }
        if let Some(written) = self.wallpaper {
            shell.wallpaper = wallpaper(written, shell.wallpaper)?;
        }
        if let (Some(mode), Some(wallpaper)) = (self.wallpaper_mode, &mut shell.wallpaper) {
            wallpaper.mode = mode;
        }
        if let Some(root_menu) = self.root_menu {
            shell.root_menu = root_menu;
        }
        if let Some(file) = self.menu_file {
            if file.as_os_str().is_empty() {
                return Err(invalid(
                    "shell.menu-file".to_owned(),
                    "an empty path".to_owned(),
                ));
            }
            shell.menu_file = Some(file);
        }
        if let Some(icons) = self.desktop_icons {
            shell.desktop_icons = icons;
        }
        if shell.wallpaper.is_none() && shell.desktop_icons {
            // The icons are drawn on the wallpaper. A profile's icons give
            // way to a person bringing their own; asking for both is a
            // mistake to point out.
            if self.desktop_icons == Some(true) {
                return Err(invalid(
                    "shell.desktop-icons".to_owned(),
                    format!(
                        "the icons are drawn on the shell's wallpaper, and \
                         `wallpaper = \"{NONE}\"` leaves it out"
                    ),
                ));
            }
            shell.desktop_icons = false;
        }
        if let Some(untrusted) = self.untrusted_launchers {
            shell.untrusted_launchers = untrusted;
        }
        if let Some(theme) = self.icon_theme {
            if theme.trim().is_empty() {
                return Err(invalid(
                    "shell.icon-theme".to_owned(),
                    "an empty theme name".to_owned(),
                ));
            }
            shell.icon_theme = Some(theme);
        }
        if let Some(terminal) = self.terminal {
            shell.terminal = Some(command("shell.terminal", terminal)?);
        }
        if let Some(lock) = self.lock {
            shell.lock = command("shell.lock", lock)?;
        }
        if let Some(panel) = self.panel {
            shell.panel = panel.apply(shell.panel, built)?;
        }
        Ok(shell)
    }

    /// Refuse a key that turns on, or configures, a component `built`
    /// lacks. Turning one off is never refused: a person may say "no tray"
    /// to a shell that has none.
    fn check(&self, built: ShellBuilt) -> Result<(), Error> {
        let panel = self.panel.as_ref();
        let items = panel
            .and_then(|panel| panel.items.as_deref())
            .unwrap_or(&[]);
        let checks = [
            (
                self.wallpaper
                    .as_deref()
                    .is_some_and(|written| written != NONE),
                "shell.wallpaper",
                built.wallpaper,
                "wallpaper",
            ),
            (
                self.wallpaper_mode.is_some(),
                "shell.wallpaper-mode",
                built.wallpaper,
                "wallpaper",
            ),
            (
                self.root_menu == Some(true),
                "shell.root-menu",
                built.menus,
                "menus",
            ),
            (
                self.menu_file.is_some(),
                "shell.menu-file",
                built.menus,
                "menus",
            ),
            (self.lock.is_some(), "shell.lock", built.menus, "menus"),
            (
                self.desktop_icons == Some(true),
                "shell.desktop-icons",
                built.icons,
                "icons",
            ),
            (
                self.untrusted_launchers.is_some(),
                "shell.untrusted-launchers",
                built.icons,
                "icons",
            ),
            (
                self.terminal.is_some(),
                "shell.terminal",
                built.menus || built.icons,
                "menus",
            ),
            (
                self.icon_theme.is_some(),
                "shell.icon-theme",
                built.menus || built.panel || built.icons,
                "menus",
            ),
            (
                panel.is_some_and(|panel| panel.enabled != Some(false)),
                "shell.panel",
                built.panel,
                "panel",
            ),
            (
                items.contains(&Item::Start),
                "shell.panel.items",
                built.menus,
                "menus",
            ),
            (
                items.contains(&Item::Tray),
                "shell.panel.items",
                built.tray,
                "tray",
            ),
        ];
        match checks
            .into_iter()
            .find(|&(written, _, present, _)| written && !present)
        {
            Some((_, key, _, feature)) => Err(Error::ShellNotBuilt { key, feature }),
            None => Ok(()),
        }
    }
}

impl RawPanel {
    /// The panel these keys describe, given the one the profile has. Every
    /// key is checked even on a panel turned off, so that turning it back on
    /// cannot be what finds a mistake.
    fn apply(self, profile: Option<Panel>, built: ShellBuilt) -> Result<Option<Panel>, Error> {
        let written = self.edge.is_some()
            || self.height.is_some()
            || self.outputs.is_some()
            || self.taskbar.is_some()
            || self.items.is_some()
            || self.clock.is_some();
        let had = profile.is_some();
        let mut panel = profile.unwrap_or_else(|| Panel::classic(built));
        if let Some(edge) = self.edge {
            panel.edge = edge;
        }
        if let Some(height) = self.height {
            if !(PANEL_MIN..=PANEL_MAX).contains(&height) {
                return Err(invalid(
                    "shell.panel.height".to_owned(),
                    format!("{height} is outside {PANEL_MIN} to {PANEL_MAX} pixels"),
                ));
            }
            panel.height = height;
        }
        if let Some(outputs) = self.outputs {
            panel.outputs = outputs.apply()?;
        }
        if let Some(taskbar) = self.taskbar {
            panel.taskbar = taskbar;
        }
        if let Some(items) = self.items {
            if let Some(twice) = items
                .iter()
                .enumerate()
                .find_map(|(at, item)| items[..at].contains(item).then_some(item))
            {
                return Err(invalid(
                    "shell.panel.items".to_owned(),
                    format!("\"{}\" is listed twice", twice.key()),
                ));
            }
            panel.items = items;
        }
        if let Some(clock) = self.clock {
            if clock.trim().is_empty() {
                return Err(invalid(
                    "shell.panel.clock".to_owned(),
                    "an empty format; to have no clock, leave \"clock\" out of `items`".to_owned(),
                ));
            }
            panel.clock = clock;
        }
        match self.enabled {
            Some(true) => Ok(Some(panel)),
            Some(false) => Ok(None),
            None if had => Ok(Some(panel)),
            None if !written => Ok(None),
            None => Err(invalid(
                "shell.panel".to_owned(),
                "this profile has no panel to configure; write `enabled = true` to have one"
                    .to_owned(),
            )),
        }
    }
}

impl RawOutputs {
    fn apply(self) -> Result<PanelOutputs, Error> {
        let key = || "shell.panel.outputs".to_owned();
        match self {
            Self::Word(word) => match word.as_str() {
                "all" => Ok(PanelOutputs::All),
                "first" => Ok(PanelOutputs::First),
                other => Err(invalid(
                    key(),
                    format!(
                        "`{other}` is not a choice of monitors; write \"all\", \"first\", or a \
                         list of connector names such as [\"DP-1\"]"
                    ),
                )),
            },
            Self::List(names) if names.is_empty() => Err(invalid(
                key(),
                "an empty list puts the panel nowhere; write `enabled = false`".to_owned(),
            )),
            Self::List(names) => match names.iter().position(|name| name.trim().is_empty()) {
                Some(blank) => Err(invalid(
                    format!("{}[{blank}]", key()),
                    "an empty connector name".to_owned(),
                )),
                None => Ok(PanelOutputs::Named(names)),
            },
        }
    }
}

/// `wallpaper = ...` laid over what the profile had: `"none"`, a colour, or
/// the path of an image shown over the colour.
fn wallpaper(written: String, had: Option<Wallpaper>) -> Result<Option<Wallpaper>, Error> {
    let key = || "shell.wallpaper".to_owned();
    let had = had.unwrap_or(Wallpaper {
        colour: Colour::rgb(0, 0, 0),
        image: None,
        mode: WallpaperMode::Fill,
    });
    if written == NONE {
        return Ok(None);
    }
    if written.trim().is_empty() {
        return Err(invalid(
            key(),
            "an empty path; write \"none\" for no wallpaper".to_owned(),
        ));
    }
    if written.starts_with('#') {
        let colour = Colour::parse(&written).ok_or_else(|| {
            invalid(
                key(),
                format!("{written:?} is not a colour; write one as \"#rrggbb\""),
            )
        })?;
        return Ok(Some(Wallpaper {
            colour,
            image: None,
            ..had
        }));
    }
    Ok(Some(Wallpaper {
        image: Some(PathBuf::from(written)),
        ..had
    }))
}

/// A program and its arguments, of which there must be at least the program.
fn command(key: &str, command: Vec<String>) -> Result<Vec<String>, Error> {
    if command
        .first()
        .is_none_or(|program| program.trim().is_empty())
    {
        return Err(invalid(key.to_owned(), "an empty command".to_owned()));
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Built, Config, parse, shell};

    /// A shell built with nothing but a wallpaper.
    const WALLPAPER_ONLY: ShellBuilt = ShellBuilt {
        wallpaper: true,
        panel: false,
        menus: false,
        tray: false,
        icons: false,
    };

    #[test]
    fn classic_has_a_panel_and_minimal_has_only_a_wallpaper_and_root_menu() {
        let classic = shell("", ShellBuilt::FULL).unwrap();
        let panel = classic.panel.expect("a panel");
        assert_eq!(panel.edge, Edge::Bottom);
        assert_eq!(
            panel.items,
            [
                Item::Start,
                Item::Taskbar,
                Item::Pager,
                Item::Tray,
                Item::Clock
            ]
        );
        assert_eq!(panel.outputs, PanelOutputs::All);
        assert_eq!(panel.taskbar, TaskbarScope::ThisOutput);
        assert!(classic.wallpaper.is_some() && classic.root_menu && classic.desktop_icons);
        assert_eq!(classic.icon_theme.as_deref(), Some("Adwaita"));

        let minimal = shell("profile = \"minimal\"", ShellBuilt::FULL).unwrap();
        assert!(minimal.wallpaper.is_some() && minimal.root_menu);
        assert_eq!(minimal.panel, None);
        assert!(!minimal.desktop_icons);
        assert_eq!(minimal.icon_theme, None, "hicolor");
        assert_ne!(
            minimal.wallpaper.unwrap().colour,
            classic.wallpaper.unwrap().colour
        );
    }

    #[test]
    fn the_compositor_accepts_a_shell_section_whatever_the_shell_was_built_with() {
        let text = r#"
            [shell]
            wallpaper = "~/Pictures/hills.png"
            desktop-icons = true
            [shell.panel]
            items = ["start", "tray", "clock"]
        "#;
        let config = parse(text, Built::default()).unwrap();
        assert_eq!(
            config.shell.wallpaper.unwrap().image,
            Some(PathBuf::from("~/Pictures/hills.png"))
        );
        assert!(shell(text, ShellBuilt::FULL).is_ok());
        assert!(
            shell(text, WALLPAPER_ONLY).is_err(),
            "the shell's to refuse"
        );
    }

    #[test]
    fn a_shell_key_left_out_of_the_build_names_its_feature() {
        let error = shell("[shell.panel]\nheight = 30", WALLPAPER_ONLY).unwrap_err();
        assert!(
            matches!(
                error,
                Error::ShellNotBuilt {
                    key: "shell.panel",
                    feature: "panel"
                }
            ),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("--features perspicax-shell/panel"),
            "the message has to say how to get it: {error}"
        );

        let tray = ShellBuilt {
            tray: false,
            ..ShellBuilt::FULL
        };
        let error = shell("[shell.panel]\nitems = [\"tray\"]", tray).unwrap_err();
        assert!(
            matches!(
                error,
                Error::ShellNotBuilt {
                    feature: "tray",
                    ..
                }
            ),
            "{error}"
        );
        assert_eq!(
            Shell::profile(Profile::Classic, tray).panel.unwrap().items,
            [Item::Start, Item::Taskbar, Item::Pager, Item::Clock],
            "the profile's panel leaves the tray out instead"
        );
    }

    #[test]
    fn turning_off_what_the_shell_was_not_built_with_is_not_an_error() {
        let shell = shell(
            "[shell]\nroot-menu = false\nwallpaper = \"none\"\n[shell.panel]\nenabled = false",
            ShellBuilt::default(),
        )
        .unwrap();
        assert_eq!(shell.wallpaper, None);
        assert_eq!(shell.panel, None);
        assert!(!shell.root_menu);
    }

    #[test]
    fn a_wallpaper_is_a_colour_or_an_image_over_the_profiles_colour() {
        let classic = Shell::profile(Profile::Classic, ShellBuilt::FULL)
            .wallpaper
            .unwrap();
        let colour = shell("[shell]\nwallpaper = \"#102030\"", ShellBuilt::FULL)
            .unwrap()
            .wallpaper
            .unwrap();
        assert_eq!(colour.colour, Colour::rgb(0x10, 0x20, 0x30));
        assert_eq!(colour.image, None);

        let image = shell(
            "[shell]\nwallpaper = \"hills.jpg\"\nwallpaper-mode = \"tile\"",
            ShellBuilt::FULL,
        )
        .unwrap()
        .wallpaper
        .unwrap();
        assert_eq!(image.image, Some(PathBuf::from("hills.jpg")));
        assert_eq!(image.colour, classic.colour, "shown beside the image");
        assert_eq!(image.mode, WallpaperMode::Tile);

        let error = shell("[shell]\nwallpaper = \"#12345\"", ShellBuilt::FULL).unwrap_err();
        assert!(error.to_string().contains("shell.wallpaper"), "{error}");
    }

    #[test]
    fn a_panel_goes_on_the_monitors_named_or_the_first() {
        let outputs = |written: &str| {
            shell(
                &format!("[shell.panel]\noutputs = {written}"),
                ShellBuilt::FULL,
            )
            .map(|shell| shell.panel.unwrap().outputs)
        };
        assert_eq!(outputs("\"first\"").unwrap(), PanelOutputs::First);
        assert_eq!(
            outputs("[\"DP-1\", \"HDMI-A-1\"]").unwrap(),
            PanelOutputs::Named(vec!["DP-1".to_owned(), "HDMI-A-1".to_owned()])
        );
        assert!(outputs("\"second\"").is_err());
        assert!(outputs("[]").is_err());
    }

    #[test]
    fn a_profile_with_no_panel_has_one_only_when_asked() {
        let minimal = |panel: &str| {
            shell(
                &format!("profile = \"minimal\"\n[shell.panel]\n{panel}"),
                ShellBuilt::FULL,
            )
        };
        let error = minimal("edge = \"top\"").unwrap_err();
        assert!(error.to_string().contains("enabled = true"), "{error}");
        let panel = minimal("enabled = true\nedge = \"top\"")
            .unwrap()
            .panel
            .unwrap();
        assert_eq!(panel.edge, Edge::Top);
        assert_eq!(panel.height, 40, "the rest is classic's");
    }

    #[test]
    fn a_panel_item_listed_twice_or_unknown_is_refused() {
        let error = shell(
            "[shell.panel]\nitems = [\"clock\", \"clock\"]",
            ShellBuilt::FULL,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("\"clock\" is listed twice"),
            "{error}"
        );
        assert!(shell("[shell.panel]\nitems = [\"weather\"]", ShellBuilt::FULL).is_err());
        assert!(shell("[shell.panel]\nheight = 4", ShellBuilt::FULL).is_err());
    }

    #[test]
    fn the_shell_reads_only_its_own_table_and_the_profile() {
        // `[input]` needs a seat, which the compositor would refuse in a
        // build without one; the shell does not care.
        let text = "profile = \"minimal\"\n[input.keyboard]\nlayout = \"de\"";
        assert!(shell(text, ShellBuilt::FULL).is_ok());
        assert_eq!(
            Config::profile(Profile::Minimal, Built::default()).shell,
            Shell::profile(Profile::Minimal, ShellBuilt::FULL)
        );
    }

    #[test]
    fn desktop_icons_need_the_wallpaper_they_are_drawn_on() {
        let own = shell("[shell]\nwallpaper = \"none\"", ShellBuilt::FULL).unwrap();
        assert!(
            !own.desktop_icons,
            "classic's icons give way to a wallpaper of the person's own"
        );
        let both = "[shell]\nwallpaper = \"none\"\ndesktop-icons = true";
        let error = shell(both, ShellBuilt::FULL).unwrap_err();
        assert!(
            error.to_string().contains("shell.desktop-icons")
                && error.to_string().contains("drawn on the shell's wallpaper"),
            "{error}"
        );
        assert!(
            parse(both, Built::default()).is_err(),
            "the compositor refuses it too"
        );
        let plain = shell("[shell]\nwallpaper = \"#102030\"", ShellBuilt::FULL).unwrap();
        assert!(
            plain.desktop_icons,
            "any wallpaper of the shell's holds them"
        );
    }

    #[test]
    fn untrusted_launchers_are_shown_as_files_or_hidden() {
        assert_eq!(
            shell("", ShellBuilt::FULL).unwrap().untrusted_launchers,
            UntrustedLaunchers::AsFiles
        );
        let hidden = "[shell]\nuntrusted-launchers = \"hidden\"";
        assert_eq!(
            shell(hidden, ShellBuilt::FULL).unwrap().untrusted_launchers,
            UntrustedLaunchers::Hidden
        );
        assert!(
            matches!(
                shell(hidden, WALLPAPER_ONLY),
                Err(Error::ShellNotBuilt {
                    key: "shell.untrusted-launchers",
                    feature: "icons"
                })
            ),
            "a shell without icons has no launchers to hide"
        );
        assert!(shell("[shell]\nuntrusted-launchers = \"run\"", ShellBuilt::FULL).is_err());
    }

    #[test]
    fn the_double_click_time_is_the_pointers_for_the_compositor_and_the_shell() {
        let seat = Built {
            seat: true,
            ..Built::default()
        };
        let untouched = parse("", seat).unwrap();
        assert_eq!(untouched.pointer.double_click_ms, crate::DOUBLE_CLICK_MS);
        assert_eq!(untouched.shell.double_click_ms, crate::DOUBLE_CLICK_MS);

        let slow = "[input.pointer]\ndouble-click-ms = 650";
        let config = parse(slow, seat).unwrap();
        assert_eq!(config.pointer.double_click_ms, 650, "the titlebars'");
        assert_eq!(
            config.shell.double_click_ms, 650,
            "what the compositor compares to tell the shell of a change"
        );
        assert_eq!(
            shell(slow, WALLPAPER_ONLY).unwrap().double_click_ms,
            650,
            "the icons'"
        );

        for written in [50, 5000] {
            let text = format!("[input.pointer]\ndouble-click-ms = {written}");
            for error in [
                parse(&text, seat).unwrap_err(),
                shell(&text, ShellBuilt::FULL).unwrap_err(),
            ] {
                assert!(
                    error.to_string().contains("input.pointer.double-click-ms")
                        && error.to_string().contains("100 to 2000"),
                    "{error}"
                );
            }
        }
    }
}
