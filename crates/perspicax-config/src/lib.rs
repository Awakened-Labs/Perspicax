//! `$XDG_CONFIG_HOME/perspicax/config.toml`.
//!
//! Three rules shape everything here.
//!
//! **A profile, then the person's keys on top.** `profile = "classic"` is
//! Plasma-like out of the box (click to focus) and `"minimal"` is the
//! Fluxbox/Enlightenment habit (focus follows the pointer). Every other key in
//! the file overrides the profile's value, one key at a time, so a config file
//! holds only what the person wants different.
//!
//! **Nothing is silently ignored.** Every table has `deny_unknown_fields`, so a
//! misspelled key is an error naming it rather than a setting that quietly does
//! nothing. A key for something this build left out is an error too, naming the
//! cargo feature that would provide it. A person who configures a tray in a
//! build without one should be told so, not left wondering why it never showed.
//!
//! **Pure.** Parsing and validation only: no file watching, no applying. The
//! compositor reads the file, hands the text to [`parse`], and applies the
//! [`Config`] it gets back. That keeps every rule here testable as a string in
//! and a value (or an error) out.
//!
//! Beside the file, the one other thing both processes read: [`desktop`]
//! entries, which the shell lists in its menus and the session starts in
//! [`autostart`](mod@autostart), by the same rules. And the [`menu`]
//! vocabulary, which the menu file and a pie are written in.

pub mod autostart;
pub mod desktop;
mod keys;
pub mod menu;
pub mod pie;
mod shell;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use perspicax_policy::{
    Access, Action, Bindings, Chord, Context, Decorations, Direction, Drag, Flipping, Focus,
    FocusModel, Gesture, Grid, Keysym, Mods, MouseBindings, MouseChord, Place, Program, Protocol,
    Resistance, Rule, Shape, Side, Snapping, Switching, Towards,
};
use serde::Deserialize;

use crate::shell::RawShell;
pub use crate::shell::{
    Align, Edge, Item, Leave, Panel, PanelOutputs, PanelWidth, Shell, ShellBuilt, TaskbarScope,
    UntrustedLaunchers, Wallpaper, WallpaperMode,
};
/// The double-click time, in milliseconds, of a config that does not say.
pub use perspicax_policy::DOUBLE_CLICK_MS;
/// The theme's types, as the shell reads them from [`Shell`].
pub use perspicax_policy::{
    Appearance, Builtin, ColorScheme, Contrast, Family, Font, Palette, Rgba, Role, Theme,
};

/// Which cargo features this binary was built with, as far as config cares.
/// The binary fills it in with `cfg!`; this crate cannot see the binary's
/// features itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Built {
    pub seat: bool,
    pub xwayland: bool,
    /// Rendering on demand for a screenshot, which `screencopy` needs.
    pub capture: bool,
}

/// The starting point a config file layers onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// Plasma- and Windows-like: click to focus, Alt+F4, Alt+Tab.
    #[default]
    Classic,
    /// Fluxbox- and Enlightenment-like: focus follows the pointer, and
    /// otherwise the same keys.
    Minimal,
}

/// Everything a config file decides, with the profile applied and every
/// value validated.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub profile: Profile,
    pub focus: Focus,
    pub bindings: Bindings,
    /// What a mouse button or the wheel does, by where the pointer is: the
    /// `[mouse]` tables. No profile binds any; what a press on a frame, a
    /// middle-drag of a title and the drag modifier do is built in, and a
    /// binding here comes before them.
    pub mouse: MouseBindings,
    pub keyboard: Keyboard,
    pub pointer: Pointer,
    /// Per-output settings, matched by connector name (`DP-1`, `HDMI-A-1`).
    /// An output with no rule is lit at its preferred mode, to the right of
    /// the others. Where each one goes is resolved by
    /// [`perspicax_policy::arrange`] once every monitor's size is known.
    pub outputs: Vec<OutputRule>,
    /// Programs to start once the session is up, each a program and its
    /// arguments. The person's programs: none is granted agent consent.
    pub autostart: Vec<Vec<String>>,
    /// Whether the session also starts the XDG autostart entries, as every
    /// desktop does: see [`autostart`](mod@autostart). On wherever there is a
    /// seat; saying `true` in a build without one is an error.
    pub xdg_autostart: bool,
    /// How many workspaces, in what grid, and whether one spans every
    /// monitor or each monitor has its own.
    pub workspaces: Shape,
    /// Changing workspace with the pointer: resting it on an edge of the
    /// desk, or scrolling over the desktop.
    pub flipping: Flipping,
    /// Dragging a window to an edge of the desk to give it half or a quarter
    /// of a monitor.
    pub snapping: Snapping,
    /// How hard the edges of screens and panels hold a window being moved
    /// against them.
    pub resistance: Resistance,
    /// Who draws a window's titlebar and border, and what they look like.
    /// Its colours are the theme's titlebar colours.
    pub decorations: Decorations,
    /// What everything perspicax draws looks like: the `[theme]` table.
    pub theme: Theme,
    /// Whether X11 applications get an Xwayland. Only meaningful in a build
    /// with the `xwayland` feature; saying `true` in one without it is an
    /// error.
    pub xwayland: bool,
    /// Which programs may use the protocols that reach past their own
    /// windows: taskbars, pagers, screenshot and display tools.
    pub protocols: Access,
    /// What perspicax-shell puts on the desktop, read as if the shell had
    /// every component: which it was built with is for it to check, through
    /// [`shell`].
    pub shell: Shell,
}

/// The keyboard layout and key repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyboard {
    /// xkb's RMLVO names. Empty means xkb's own default, which is what
    /// `XKB_DEFAULT_LAYOUT` and friends then decide.
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: Option<String>,
    /// Keys per second while held.
    pub repeat_rate: i32,
    /// Milliseconds before a held key starts repeating.
    pub repeat_delay: i32,
    /// Num Lock on or off when the session starts, and again whenever a save
    /// changes this. `None` leaves it as the person has it.
    pub numlock: Option<bool>,
    /// Whether the layout in use is the session's or each window's own.
    pub switching: Switching,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            rules: String::new(),
            model: String::new(),
            layout: String::new(),
            variant: String::new(),
            options: None,
            repeat_rate: 25,
            repeat_delay: 200,
            numlock: None,
            switching: Switching::Global,
        }
    }
}

/// Pointer settings: the devices' own, and the double-click time.
///
/// For a device setting, `None` leaves libinput's own default alone, which
/// differs by device (tap-to-click is off on most touchpads, for example),
/// so "unset" and "false" are different requests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pointer {
    /// Acceleration, from -1 (slowest) to 1 (fastest).
    pub accel: Option<f64>,
    pub natural_scroll: Option<bool>,
    pub tap_to_click: Option<bool>,
    pub left_handed: Option<bool>,
    /// How soon, in milliseconds, a second click must follow the first to
    /// make a double-click: on a titlebar, and on the shell's desktop icons,
    /// which [`Shell::double_click_ms`] carries to the shell. One setting,
    /// so the desk's double-clicks all ask the same of a hand.
    pub double_click_ms: u32,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            accel: None,
            natural_scroll: None,
            tap_to_click: None,
            left_handed: None,
            double_click_ms: DOUBLE_CLICK_MS,
        }
    }
}

/// The double-click times a config may ask for. Faster than the shortest, a
/// hand cannot click twice on purpose; slower than the longest, two clicks
/// a person meant as two would open what they only meant to select.
const DOUBLE_CLICK_RANGE: std::ops::RangeInclusive<u32> = 100..=2000;

/// `double-click-ms` as written, if it is in [`DOUBLE_CLICK_RANGE`].
fn double_click_ms(written: u32) -> Result<u32, Error> {
    if DOUBLE_CLICK_RANGE.contains(&written) {
        return Ok(written);
    }
    Err(invalid(
        "input.pointer.double-click-ms".to_owned(),
        format!(
            "{written} is outside {} to {} milliseconds",
            DOUBLE_CLICK_RANGE.start(),
            DOUBLE_CLICK_RANGE.end()
        ),
    ))
}

/// The most workspaces along one side of the grid. Generous: past this, a
/// grid is a typo, and a 1000x1000 one would be a million workspaces.
const GRID_SIDE: u16 = 16;

/// What to do with one output.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputRule {
    /// The connector name, as the kernel names it.
    pub name: String,
    /// `false` leaves the monitor dark.
    pub enable: bool,
    /// A mode to use instead of the preferred one.
    pub mode: Option<Mode>,
    /// Where it goes: beside another output (`right-of = "DP-1"`, with an
    /// `offset` along the shared edge), at an absolute `position`, or, with
    /// neither, to the right of the others.
    pub place: Place,
    pub scale: Option<f64>,
}

/// A display mode, as `"2560x1440"` or `"2560x1440@143.9"`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    pub width: u16,
    pub height: u16,
    /// Refresh in Hz. `None` takes the fastest the monitor offers at this
    /// size.
    pub refresh: Option<f64>,
}

/// Why a config could not be used.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Not TOML, or not this schema. The message names the key and the line.
    #[error("{0}")]
    Parse(String),
    /// A key for something this build left out.
    #[error(
        "`{key}` is not built into this perspicax; rebuild with the `{feature}` \
         cargo feature (`--features perspicax/{feature}`)"
    )]
    NotBuilt {
        key: &'static str,
        feature: &'static str,
    },
    /// A `[shell]` key for a component perspicax-shell was built without.
    #[error(
        "`{key}` is not built into this perspicax-shell; rebuild it with the \
         `{feature}` cargo feature (`--features perspicax-shell/{feature}`)"
    )]
    ShellNotBuilt {
        key: &'static str,
        feature: &'static str,
    },
    /// Well-formed TOML, and a value this schema cannot use.
    #[error("`{key}`: {reason}")]
    Invalid { key: String, reason: String },
}

impl Config {
    /// A profile's settings, with nothing layered on top.
    #[must_use]
    pub fn profile(profile: Profile, built: Built) -> Self {
        let focus = match profile {
            Profile::Classic => Focus::default(),
            Profile::Minimal => Focus {
                model: FocusModel::Sloppy,
                autoraise: false,
            },
        };
        let logo_shift = |key| Chord {
            mods: Mods {
                logo: true,
                shift: true,
                ..Mods::default()
            },
            key,
        };
        let mut bindings = Bindings::classic()
            .bind(
                logo_shift(Keysym::Right),
                Action::MoveToOutput(Towards::Next),
            )
            .bind(
                logo_shift(Keysym::Left),
                Action::MoveToOutput(Towards::Previous),
            )
            .bind(logo_shift(Keysym::r), Action::Reload);

        // Tabs, in both profiles: Logo+Tab steps through the focused
        // window's group, and Logo+G groups the focused window with the one
        // focused before it, which is the keyboard's way to do what dragging
        // one titlebar onto another does.
        let logo = |key| Chord {
            mods: Mods {
                logo: true,
                ..Mods::default()
            },
            key,
        };
        bindings = bindings
            .bind(logo(Keysym::Tab), Action::CycleTab { forward: true })
            .bind(logo_shift(Keysym::Tab), Action::CycleTab { forward: false })
            .bind(logo(Keysym::g), Action::TabWithPrevious)
            .bind(logo_shift(Keysym::g), Action::DetachTab);

        // Workspaces. Ctrl+Logo+arrows switches and, with Shift, takes the
        // focused window along, in both profiles: Plasma's keys, and Windows'
        // for left and right. Classic adds Plasma's Ctrl+F1..F4; minimal adds
        // Ctrl+Alt+arrows, the old X window managers' habit.
        let arrows = [
            (Keysym::Left, Direction::Left),
            (Keysym::Right, Direction::Right),
            (Keysym::Up, Direction::Up),
            (Keysym::Down, Direction::Down),
        ];
        let held = |ctrl, alt, shift, logo| Mods {
            ctrl,
            alt,
            shift,
            logo,
        };
        for (key, direction) in arrows {
            bindings = bindings
                .bind(
                    Chord {
                        mods: held(true, false, false, true),
                        key,
                    },
                    Action::Workspace(direction),
                )
                .bind(
                    Chord {
                        mods: held(true, false, true, true),
                        key,
                    },
                    Action::CarryToWorkspace(direction),
                );
            if profile == Profile::Minimal {
                bindings = bindings
                    .bind(
                        Chord {
                            mods: held(true, true, false, false),
                            key,
                        },
                        Action::Workspace(direction),
                    )
                    .bind(
                        Chord {
                            mods: held(true, true, true, false),
                            key,
                        },
                        Action::CarryToWorkspace(direction),
                    );
            }
        }
        if profile == Profile::Classic {
            // The start menu, opened as on Plasma and Windows: a tap of the
            // Logo key on its own.
            bindings = bindings.bind_tap(Action::StartMenu);
            // The next keyboard layout, as Windows switches it. Applications
            // no longer get Logo+Space.
            bindings = bindings.bind(logo(Keysym::space), Action::CycleLayout { forward: true });
            // Windows' snapping keys.
            for (key, direction) in arrows {
                bindings = bindings.bind(
                    Chord {
                        mods: held(false, false, false, true),
                        key,
                    },
                    Action::Snap(direction),
                );
            }
            let f_keys = [Keysym::F1, Keysym::F2, Keysym::F3, Keysym::F4];
            for (number, key) in (1..).zip(f_keys) {
                bindings = bindings.bind(
                    Chord {
                        mods: held(true, false, false, false),
                        key,
                    },
                    Action::GoToWorkspace(number),
                );
            }
        }
        let workspaces = workspaces(profile);
        Self {
            profile,
            focus,
            bindings,
            mouse: MouseBindings::default(),
            keyboard: Keyboard::default(),
            pointer: Pointer::default(),
            outputs: Vec::new(),
            autostart: Vec::new(),
            xdg_autostart: built.seat,
            workspaces,
            snapping: Snapping {
                drag: profile == Profile::Classic,
                ..Snapping::default()
            },
            resistance: match profile {
                // Plasma and Windows let a window go wherever it is dragged.
                Profile::Classic => Resistance::default(),
                // Fluxbox and Openbox hold it at the edge of a screen or a
                // panel. Between two monitors it crosses, as the pointer does.
                Profile::Minimal => Resistance {
                    edges: 20,
                    seams: 0,
                },
            },
            flipping: match profile {
                Profile::Classic => Flipping::default(),
                // The Fluxbox and Enlightenment habit: the desk is a loop the
                // pointer travels round, window in hand or not.
                Profile::Minimal => Flipping {
                    edge: true,
                    delay_ms: 300,
                    while_dragging: true,
                    scroll: true,
                },
            },
            decorations: Decorations::default(),
            theme: Theme::default(),
            xwayland: built.xwayland,
            protocols: protocols(built),
            shell: Shell::profile(profile, ShellBuilt::FULL),
        }
    }
}

/// A profile's workspaces.
fn workspaces(profile: Profile) -> Shape {
    match profile {
        // Plasma's default since 5.x when more than one is asked for, and
        // Windows' task view: a row, so left and right are all there is.
        Profile::Classic => Shape {
            mode: perspicax_policy::Mode::Spanning,
            grid: Grid {
                columns: 4,
                rows: 1,
                wrap: false,
            },
        },
        // A square to flip around, wrapping, as Fluxbox and E do.
        Profile::Minimal => Shape {
            mode: perspicax_policy::Mode::Spanning,
            grid: Grid {
                columns: 2,
                rows: 2,
                wrap: true,
            },
        },
    }
}

/// How many workspaces `grid` holds: on each monitor, when each has its
/// own.
fn count(grid: Grid) -> u32 {
    u32::from(grid.columns) * u32::from(grid.rows)
}

/// The `[protocols]` defaults, the same in both profiles: a profile is a
/// window manager's habits, not a security posture. Listing windows and
/// workspaces is open to any client, as every panel expects. Reading pixels,
/// moving monitors and speaking for the desktop shell are for the programs
/// that are known to do it, by name, which anything can claim; a person who
/// wants it tighter writes full paths.
fn protocols(built: Built) -> Access {
    let only = |names: &[&str]| Rule::Only(names.iter().map(|name| Program::parse(name)).collect());
    Access::open()
        .with(
            Protocol::Screencopy,
            if built.capture {
                only(&["grim", "wf-recorder", "xdg-desktop-portal-wlr"])
            } else {
                Rule::Off
            },
        )
        .with(
            Protocol::OutputManagement,
            only(&["wlr-randr", "kanshi", "wdisplays", "nwg-displays"]),
        )
        .with(Protocol::Shell, only(&["perspicax-shell"]))
}

/// Where the config file lives: `$XDG_CONFIG_HOME/perspicax/config.toml`,
/// falling back to `~/.config` as the XDG spec says. `None` only with neither
/// variable set.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("perspicax").join("config.toml"))
}

/// Read and parse a config file. A file that does not exist is the classic
/// profile: nobody should have to write a config to log in.
///
/// # Errors
///
/// [`Error::Read`] for a file that exists and cannot be read, and anything
/// [`parse`] refuses.
pub fn load(path: &Path, built: Built) -> Result<Config, Error> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, built),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(Config::profile(Profile::Classic, built))
        }
        Err(source) => Err(Error::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Read a config file's `[shell]` table, as a perspicax-shell built with
/// `built` does. A file that does not exist is the classic profile's shell,
/// as it is for the compositor.
///
/// # Errors
///
/// [`Error::Read`] for a file that exists and cannot be read, and anything
/// [`shell`] refuses.
pub fn load_shell(path: &Path, built: ShellBuilt) -> Result<Shell, Error> {
    match std::fs::read_to_string(path) {
        Ok(text) => shell(&text, built),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(Shell::profile(Profile::Classic, built))
        }
        Err(source) => Err(Error::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Parse a config file's `[shell]` table, with its profile, as a
/// perspicax-shell built with `built` reads it, and the double-click time
/// from `[input.pointer]`, which its icons share with the titlebars, and the
/// theme, which everything is drawn in. The rest of the file must be this
/// schema, and is otherwise the compositor's to judge.
///
/// # Errors
///
/// [`Error::Parse`] for text that is not this schema,
/// [`Error::ShellNotBuilt`] for a key the shell cannot honour,
/// [`Error::Invalid`] for a value it cannot use.
pub fn shell(text: &str, built: ShellBuilt) -> Result<Shell, Error> {
    let raw: Raw = toml::from_str(text).map_err(|error| Error::Parse(error.to_string()))?;
    let theme = theme(raw.theme, raw.decorations.as_ref())?;
    let profile = Shell::profile(raw.profile, built);
    // The grid's sides are the compositor's to judge; this only counts.
    let workspaces = match raw.workspaces.as_ref().and_then(|written| written.grid) {
        Some([columns, rows]) => u32::from(columns) * u32::from(rows),
        None => count(workspaces(raw.profile).grid),
    };
    let mut shell = match raw.shell {
        Some(shell) => shell.apply(profile, built, workspaces)?,
        None => profile,
    };
    let written = raw
        .input
        .and_then(|input| input.pointer)
        .and_then(|pointer| pointer.double_click_ms);
    if let Some(written) = written {
        shell.double_click_ms = double_click_ms(written)?;
    }
    shell.palette = theme.palette;
    shell.font = theme.font;
    Ok(shell)
}

/// Parse a config file's text.
///
/// # Errors
///
/// [`Error::Parse`] for text that is not this schema, [`Error::NotBuilt`] for
/// a key this build cannot honour, [`Error::Invalid`] for a value it cannot
/// use.
pub fn parse(text: &str, built: Built) -> Result<Config, Error> {
    let raw: Raw = toml::from_str(text).map_err(|error| Error::Parse(error.to_string()))?;
    raw.apply(built)
}

// The file, as serde reads it. Separate from `Config` so that every
// `Option` here means "not written" and the profile fills it, while `Config`
// holds only decided values.

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Raw {
    #[serde(default)]
    profile: Profile,
    focus: Option<RawFocus>,
    #[serde(default)]
    keys: BTreeMap<String, RawAction>,
    drag: Option<String>,
    mouse: Option<RawMouse>,
    input: Option<RawInput>,
    #[serde(default, rename = "output")]
    outputs: Vec<RawOutput>,
    #[serde(default)]
    autostart: Vec<Vec<String>>,
    xdg_autostart: Option<bool>,
    xwayland: Option<bool>,
    workspaces: Option<RawWorkspaces>,
    snap: Option<RawSnap>,
    resistance: Option<RawResistance>,
    decorations: Option<RawDecorations>,
    theme: Option<RawTheme>,
    protocols: Option<RawProtocols>,
    shell: Option<RawShell>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawColorScheme {
    Dark,
    Light,
    NoPreference,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawContrast {
    Normal,
    High,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawTheme {
    name: Option<String>,
    font: Option<String>,
    font_size: Option<u16>,
    cursor: Option<String>,
    cursor_size: Option<u32>,
    color_scheme: Option<RawColorScheme>,
    contrast: Option<RawContrast>,
    /// By role, checked against [`Role`] rather than by serde, so that a
    /// misspelled role is named with the table it is in.
    palette: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawProtocols {
    foreign_toplevel_list: Option<RawRule>,
    foreign_toplevel_management: Option<RawRule>,
    workspace: Option<RawRule>,
    screencopy: Option<RawRule>,
    output_management: Option<RawRule>,
    shell: Option<RawRule>,
}

/// `"any"`, `"off"`, or a list of programs.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawRule {
    Word(String),
    List(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawDecorations {
    mode: Option<RawDecorationMode>,
    title_height: Option<i32>,
    border: Option<i32>,
    focused: Option<String>,
    unfocused: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawDecorationMode {
    /// This compositor draws the frame, unless a client asks to draw its own.
    Server,
    /// Every client draws its own.
    Client,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawSnap {
    drag: Option<bool>,
    threshold: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawResistance {
    edges: Option<i32>,
    seams: Option<i32>,
}

/// More than this and an edge is a wall a person has to fight.
const RESISTANCE_MAX: i32 = 128;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawWorkspaces {
    mode: Option<RawSpread>,
    grid: Option<[u16; 2]>,
    wrap: Option<bool>,
    flip: Option<RawFlip>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawFlip {
    edge: Option<bool>,
    delay_ms: Option<u64>,
    while_dragging: Option<bool>,
    scroll: Option<bool>,
}

/// Longer than this and a person would think edge flipping was broken.
const FLIP_DELAY_MAX_MS: u64 = 5000;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawSpread {
    Spanning,
    PerOutput,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawFocus {
    model: Option<RawModel>,
    autoraise: Option<bool>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawModel {
    Click,
    Sloppy,
    Strict,
}

/// The `[mouse]` tables, one for each place a binding can apply.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMouse {
    #[serde(default)]
    desktop: BTreeMap<String, RawAction>,
    #[serde(default)]
    titlebar: BTreeMap<String, RawAction>,
    #[serde(default)]
    window: BTreeMap<String, RawAction>,
    #[serde(default)]
    anywhere: BTreeMap<String, RawAction>,
}

impl RawMouse {
    /// Put every binding in `[mouse]` in force over `config`'s, refusing one
    /// written twice in a table under two spellings, and one that would take
    /// the drag modifier away. `drag_from` says whose drag that is, when it
    /// is not the file's own; `pies` are the pies a `{ pie = "..." }` may
    /// name.
    fn apply(
        self,
        config: &mut Config,
        drag_from: &str,
        pies: &BTreeSet<String>,
    ) -> Result<(), Error> {
        let tables = [
            ("desktop", Context::Desktop, self.desktop),
            ("titlebar", Context::Titlebar, self.titlebar),
            ("window", Context::Window, self.window),
            ("anywhere", Context::Anywhere, self.anywhere),
        ];
        for (table, context, entries) in tables {
            let mut written_as = Vec::new();
            for (written, action) in entries {
                let key = format!("mouse.{table}.{written}");
                let chord = keys::mouse(&written).map_err(|reason| invalid(key.clone(), reason))?;
                if let Some((_, first)) = written_as.iter().find(|(seen, _)| *seen == chord) {
                    return Err(invalid(
                        key,
                        format!("is the same as `{first}`, already written; write it once"),
                    ));
                }
                let action =
                    action_for(action, pies).map_err(|reason| invalid(key.clone(), reason))?;
                if action.is_some() && config.bindings.takes_drag(context, &chord) {
                    return Err(invalid(
                        key,
                        drag_taken(&config.bindings, &chord, drag_from),
                    ));
                }
                let mouse = std::mem::take(&mut config.mouse);
                config.mouse = match action {
                    Some(action) => mouse.bind(context, chord, action),
                    None => mouse.unbind(context, chord),
                };
                written_as.push((chord, written));
            }
        }
        Ok(())
    }
}

/// Why a mouse binding that would take the drag away is refused, naming
/// `drag` and what it does with this press.
fn drag_taken(bindings: &Bindings, chord: &MouseChord, drag_from: &str) -> String {
    let does = match chord.gesture {
        Gesture::Press(button) => bindings.drag(chord.mods, button),
        Gesture::Double(_) | Gesture::Wheel(_) => None,
    };
    let does = match does {
        Some(Drag::Move) => "moves",
        Some(Drag::Resize) => "resizes",
        None => "drags",
    };
    format!(
        "`drag` is {}{drag_from}, which {does} a window with this press anywhere on it; a \
         binding here would always come first and the drag would never happen. Add a \
         modifier, bind it in `[mouse.titlebar]`, or change `drag`",
        keys::spelled(chord.mods)
    )
}

/// A binding's value: an action's name, `{ spawn = [...] }` or
/// `{ pie = "..." }`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawAction {
    Named(String),
    Spawn(RawSpawn),
    Pie(RawOpenPie),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpawn {
    spawn: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOpenPie {
    pie: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInput {
    keyboard: Option<RawKeyboard>,
    pointer: Option<RawPointer>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawKeyboard {
    rules: Option<String>,
    model: Option<String>,
    layout: Option<String>,
    variant: Option<String>,
    options: Option<String>,
    repeat_rate: Option<i32>,
    repeat_delay: Option<i32>,
    numlock: Option<bool>,
    switching: Option<RawSwitching>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawSwitching {
    Global,
    Window,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawPointer {
    accel: Option<f64>,
    natural_scroll: Option<bool>,
    tap_to_click: Option<bool>,
    left_handed: Option<bool>,
    double_click_ms: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawOutput {
    name: String,
    enable: Option<bool>,
    mode: Option<String>,
    position: Option<[i32; 2]>,
    left_of: Option<String>,
    right_of: Option<String>,
    above: Option<String>,
    below: Option<String>,
    offset: Option<i32>,
    scale: Option<f64>,
}

impl Raw {
    fn apply(self, built: Built) -> Result<Config, Error> {
        let mut config = Config::profile(self.profile, built);

        // The seat's sections, refused by name in a build without one.
        if !built.seat {
            if self.input.is_some() {
                return Err(Error::NotBuilt {
                    key: "input",
                    feature: "seat",
                });
            }
            if !self.outputs.is_empty() {
                return Err(Error::NotBuilt {
                    key: "output",
                    feature: "seat",
                });
            }
        }
        if let Some(xdg_autostart) = self.xdg_autostart {
            if xdg_autostart && !built.seat {
                return Err(Error::NotBuilt {
                    key: "xdg-autostart",
                    feature: "seat",
                });
            }
            config.xdg_autostart = xdg_autostart;
        }
        if let Some(xwayland) = self.xwayland {
            if xwayland && !built.xwayland {
                return Err(Error::NotBuilt {
                    key: "xwayland",
                    feature: "xwayland",
                });
            }
            config.xwayland = xwayland;
        }

        if let Some(focus) = self.focus {
            if let Some(model) = focus.model {
                config.focus.model = match model {
                    RawModel::Click => FocusModel::Click,
                    RawModel::Sloppy => FocusModel::Sloppy,
                    RawModel::Strict => FocusModel::Strict,
                };
            }
            if let Some(autoraise) = focus.autoraise {
                config.focus.autoraise = autoraise;
            }
        }

        // The pies a binding may open, before the bindings are read and
        // `[shell]` after them.
        let pies = self
            .shell
            .as_ref()
            .map(RawShell::pie_names)
            .unwrap_or_default();
        for (written, action) in self.keys {
            let trigger = keys::trigger(&written)
                .map_err(|reason| invalid(format!("keys.{written}"), reason))?;
            let action = action_for(action, &pies)
                .map_err(|reason| invalid(format!("keys.{written}"), reason))?;
            config.bindings = match (trigger, action) {
                (keys::Trigger::Chord(chord), Some(action)) => config.bindings.bind(chord, action),
                (keys::Trigger::Chord(chord), None) => config.bindings.unbind(chord),
                (keys::Trigger::LogoTap, Some(action)) => config.bindings.bind_tap(action),
                (keys::Trigger::LogoTap, None) => config.bindings.unbind_tap(),
            };
        }
        let drag_from = match (&self.drag, self.profile) {
            (Some(_), _) => "",
            (None, Profile::Classic) => ", the classic profile's",
            (None, Profile::Minimal) => ", the minimal profile's",
        };
        if let Some(drag) = self.drag {
            config.bindings = match keys::modifiers(&drag)
                .map_err(|reason| invalid("drag".to_owned(), reason))?
            {
                Some(mods) => config.bindings.drag_with(mods),
                None => config.bindings.no_drag(),
            };
        }
        // After `drag`, so a binding is checked against the drag in force.
        if let Some(mouse) = self.mouse {
            mouse.apply(&mut config, drag_from, &pies)?;
        }

        if let Some(input) = self.input {
            if let Some(keyboard) = input.keyboard {
                config.keyboard = keyboard.apply(config.keyboard)?;
            }
            if let Some(pointer) = input.pointer {
                if let Some(accel) = pointer.accel
                    && !(-1.0..=1.0).contains(&accel)
                {
                    return Err(invalid(
                        "input.pointer.accel".to_owned(),
                        format!("{accel} is outside -1 to 1"),
                    ));
                }
                config.pointer = Pointer {
                    accel: pointer.accel,
                    natural_scroll: pointer.natural_scroll,
                    tap_to_click: pointer.tap_to_click,
                    left_handed: pointer.left_handed,
                    double_click_ms: pointer
                        .double_click_ms
                        .map_or(Ok(DOUBLE_CLICK_MS), double_click_ms)?,
                };
            }
        }

        let mut seen: Vec<String> = Vec::new();
        for output in self.outputs {
            if seen.contains(&output.name) {
                return Err(invalid(
                    format!("output.{}", output.name),
                    "described twice".to_owned(),
                ));
            }
            seen.push(output.name.clone());
            config.outputs.push(output.apply()?);
        }
        if let Some(name) = cycle(&config.outputs) {
            return Err(invalid(
                format!("output.{name}"),
                "is placed beside an output that is, in the end, placed beside it".to_owned(),
            ));
        }

        if let Some(workspaces) = self.workspaces {
            if let Some(mode) = workspaces.mode {
                config.workspaces.mode = match mode {
                    RawSpread::Spanning => perspicax_policy::Mode::Spanning,
                    RawSpread::PerOutput => perspicax_policy::Mode::PerOutput,
                };
            }
            if let Some([columns, rows]) = workspaces.grid {
                if !(1..=GRID_SIDE).contains(&columns) || !(1..=GRID_SIDE).contains(&rows) {
                    return Err(invalid(
                        "workspaces.grid".to_owned(),
                        format!(
                            "[{columns}, {rows}] is not a grid; each side is 1 to {GRID_SIDE} \
                             workspaces, columns first"
                        ),
                    ));
                }
                config.workspaces.grid.columns = columns;
                config.workspaces.grid.rows = rows;
            }
            if let Some(wrap) = workspaces.wrap {
                config.workspaces.grid.wrap = wrap;
            }
            if let Some(flip) = workspaces.flip {
                if let Some(delay) = flip.delay_ms {
                    if delay > FLIP_DELAY_MAX_MS {
                        return Err(invalid(
                            "workspaces.flip.delay-ms".to_owned(),
                            format!("{delay} is more than {FLIP_DELAY_MAX_MS} milliseconds"),
                        ));
                    }
                    config.flipping.delay_ms = delay;
                }
                let flipping = &mut config.flipping;
                flipping.edge = flip.edge.unwrap_or(flipping.edge);
                flipping.while_dragging = flip.while_dragging.unwrap_or(flipping.while_dragging);
                flipping.scroll = flip.scroll.unwrap_or(flipping.scroll);
            }
        }

        if let Some(snap) = self.snap {
            if let Some(threshold) = snap.threshold {
                if !(1..=64).contains(&threshold) {
                    return Err(invalid(
                        "snap.threshold".to_owned(),
                        format!("{threshold} is outside 1 to 64 pixels"),
                    ));
                }
                config.snapping.threshold = threshold;
            }
            config.snapping.drag = snap.drag.unwrap_or(config.snapping.drag);
        }

        if let Some(resistance) = self.resistance {
            for (key, value, into) in [
                ("edges", resistance.edges, &mut config.resistance.edges),
                ("seams", resistance.seams, &mut config.resistance.seams),
            ] {
                let Some(value) = value else {
                    continue;
                };
                if !(0..=RESISTANCE_MAX).contains(&value) {
                    return Err(invalid(
                        format!("resistance.{key}"),
                        format!("{value} is outside 0 to {RESISTANCE_MAX} pixels"),
                    ));
                }
                *into = value;
            }
        }

        config.theme = theme(self.theme, self.decorations.as_ref())?;
        if let Some(decorations) = self.decorations {
            config.decorations = decorations.apply(config.decorations)?;
        }
        let palette = &config.theme.palette;
        config.decorations.focused = palette[Role::TitleFocused].colour();
        config.decorations.focused_ink = palette[Role::TitleFocusedInk].colour();
        config.decorations.unfocused = palette[Role::TitleUnfocused].colour();
        config.decorations.unfocused_ink = palette[Role::TitleUnfocusedInk].colour();

        if let Some(protocols) = self.protocols {
            config.protocols = protocols.apply(config.protocols, built)?;
        }

        // Whatever the shell was built with: see `shell`.
        if let Some(shell) = self.shell {
            config.shell = shell.apply(
                config.shell,
                ShellBuilt::FULL,
                count(config.workspaces.grid),
            )?;
        }
        config.shell.double_click_ms = config.pointer.double_click_ms;
        config.shell.palette = config.theme.palette;
        config.shell.font = config.theme.font.clone();

        if let Some(empty) = self.autostart.iter().position(Vec::is_empty) {
            return Err(invalid(
                format!("autostart[{empty}]"),
                "an empty command".to_owned(),
            ));
        }
        config.autostart = self.autostart;
        Ok(config)
    }
}

impl RawDecorations {
    fn apply(self, mut decorations: Decorations) -> Result<Decorations, Error> {
        if let Some(mode) = self.mode {
            decorations.server = matches!(mode, RawDecorationMode::Server);
        }
        let pixels = |key: &str, value: i32, range: std::ops::RangeInclusive<i32>| {
            if range.contains(&value) {
                Ok(value)
            } else {
                Err(invalid(
                    format!("decorations.{key}"),
                    format!(
                        "{value} is outside {} to {} pixels",
                        range.start(),
                        range.end()
                    ),
                ))
            }
        };
        if let Some(title) = self.title_height {
            decorations.title = pixels("title-height", title, 8..=96)?;
        }
        if let Some(border) = self.border {
            decorations.border = pixels("border", border, 0..=32)?;
        }
        // Its colours are the theme's: see `theme`.
        Ok(decorations)
    }
}

/// The `[theme]` table, decided: a theme by name, the colours a person wrote
/// over it, and the font.
///
/// `[decorations] focused` and `unfocused` were the titlebars' colours before
/// there were themes, and still are: the same two roles, written in the older
/// place. Writing one in both places is refused rather than one quietly
/// winning.
fn theme(written: Option<RawTheme>, decorations: Option<&RawDecorations>) -> Result<Theme, Error> {
    let written = written.unwrap_or_default();
    let builtin = match written.name {
        Some(name) => Builtin::named(&name).ok_or_else(|| {
            let known: Vec<&str> = Builtin::ALL.iter().map(|theme| theme.name()).collect();
            invalid(
                "theme.name".to_owned(),
                format!("{name:?} is not a theme; there are {}", known.join(", ")),
            )
        })?,
        None => Builtin::default(),
    };

    let mut colours = std::collections::BTreeMap::new();
    for (key, text) in written.palette.unwrap_or_default() {
        let at = format!("theme.palette.{key}");
        let role = Role::keyed(&key)
            .ok_or_else(|| invalid(at.clone(), "is not one of the palette's colours".to_owned()))?;
        colours.insert(role, palette_colour(&at, role, &text)?);
    }
    if let Some(decorations) = decorations {
        let older = [
            ("focused", &decorations.focused, Role::TitleFocused),
            ("unfocused", &decorations.unfocused, Role::TitleUnfocused),
        ];
        for (key, text, role) in older {
            let Some(text) = text else { continue };
            let at = format!("decorations.{key}");
            if colours.contains_key(&role) {
                return Err(invalid(
                    at,
                    format!(
                        "is [theme.palette] {} as well; write it in one place",
                        role.key()
                    ),
                ));
            }
            colours.insert(role, palette_colour(&at, role, text)?);
        }
    }

    let mut font = Font::default();
    if let Some(family) = written.font {
        if family.trim().is_empty() {
            return Err(invalid(
                "theme.font".to_owned(),
                "an empty name; leave it out for the system's sans-serif".to_owned(),
            ));
        }
        font.family = Family::parse(&family);
    }
    if let Some(size) = written.font_size {
        if !(10..=20).contains(&size) {
            return Err(invalid(
                "theme.font-size".to_owned(),
                format!("{size} is outside 10 to 20 pixels"),
            ));
        }
        font.size = size;
    }

    if written
        .cursor
        .as_deref()
        .is_some_and(|name| name.trim().is_empty())
    {
        return Err(invalid(
            "theme.cursor".to_owned(),
            "an empty name; leave it out for XCURSOR_THEME's".to_owned(),
        ));
    }
    if let Some(size) = written.cursor_size
        && !(8..=128).contains(&size)
    {
        return Err(invalid(
            "theme.cursor-size".to_owned(),
            format!("{size} is outside 8 to 128 pixels"),
        ));
    }

    // What applications are told: the theme's own, then what was written.
    // An accent written for perspicax's drawing is the applications' too.
    let mut apps = builtin.appearance();
    if let Some(scheme) = written.color_scheme {
        apps.color_scheme = Some(match scheme {
            RawColorScheme::Dark => ColorScheme::Dark,
            RawColorScheme::Light => ColorScheme::Light,
            RawColorScheme::NoPreference => ColorScheme::NoPreference,
        });
    }
    if let Some(contrast) = written.contrast {
        apps.contrast = Some(match contrast {
            RawContrast::Normal => Contrast::Normal,
            RawContrast::High => Contrast::High,
        });
    }
    if let Some(accent) = colours.get(&Role::Accent) {
        apps.accent = Some(accent.colour());
    }

    Ok(Theme {
        builtin,
        palette: builtin.palette().written_over(&colours),
        font,
        cursor: written.cursor,
        cursor_size: written.cursor_size,
        apps,
    })
}

/// A colour written for `role` at `at`: opaque, unless the role is one of the
/// few drawn see-through.
fn palette_colour(at: &str, role: Role, text: &str) -> Result<Rgba, Error> {
    let colour = Rgba::parse(text).ok_or_else(|| {
        let forms = if role.takes_alpha() {
            "\"#rrggbb\" or \"#rrggbbaa\""
        } else {
            "\"#rrggbb\""
        };
        invalid(
            at.to_owned(),
            format!("{text:?} is not a colour; write one as {forms}"),
        )
    })?;
    if !colour.is_opaque() && !role.takes_alpha() {
        return Err(invalid(
            at.to_owned(),
            format!(
                "{text:?} is see-through, and this is drawn opaque so the compositor can \
                 say what it covers; write it as \"#rrggbb\""
            ),
        ));
    }
    Ok(colour)
}

impl RawProtocols {
    fn apply(self, mut access: Access, built: Built) -> Result<Access, Error> {
        let written = [
            (Protocol::ForeignToplevelList, self.foreign_toplevel_list),
            (
                Protocol::ForeignToplevelManagement,
                self.foreign_toplevel_management,
            ),
            (Protocol::Workspace, self.workspace),
            (Protocol::Screencopy, self.screencopy),
            (Protocol::OutputManagement, self.output_management),
            (Protocol::Shell, self.shell),
        ];
        for (protocol, raw) in written {
            let Some(raw) = raw else { continue };
            let rule = raw.rule(protocol)?;
            if protocol == Protocol::Screencopy && rule != Rule::Off && !built.capture {
                return Err(Error::NotBuilt {
                    key: "protocols.screencopy",
                    feature: "capture",
                });
            }
            access = access.with(protocol, rule);
        }
        Ok(access)
    }
}

impl RawRule {
    fn rule(self, protocol: Protocol) -> Result<Rule, Error> {
        let key = || format!("protocols.{}", protocol.key());
        match self {
            Self::Word(word) => match word.as_str() {
                "any" => Ok(Rule::Any),
                "off" => Ok(Rule::Off),
                other => Err(invalid(
                    key(),
                    format!(
                        "`{other}` is not a rule; write \"any\", \"off\", or a list of \
                         programs such as [\"grim\", \"/usr/bin/kanshi\"]"
                    ),
                )),
            },
            Self::List(programs) if programs.is_empty() => Err(invalid(
                key(),
                "an empty list admits nobody; write \"off\"".to_owned(),
            )),
            Self::List(programs) => match programs.iter().position(|entry| entry.trim().is_empty())
            {
                Some(blank) => Err(invalid(
                    format!("{}[{blank}]", key()),
                    "an empty program name".to_owned(),
                )),
                None => Ok(Rule::Only(
                    programs.iter().map(|entry| Program::parse(entry)).collect(),
                )),
            },
        }
    }
}

impl RawKeyboard {
    fn apply(self, mut keyboard: Keyboard) -> Result<Keyboard, Error> {
        let positive = |key: &str, value: i32| {
            if value > 0 {
                Ok(value)
            } else {
                Err(invalid(
                    format!("input.keyboard.{key}"),
                    format!("{value} is not positive"),
                ))
            }
        };
        if let Some(rate) = self.repeat_rate {
            keyboard.repeat_rate = positive("repeat-rate", rate)?;
        }
        if let Some(delay) = self.repeat_delay {
            keyboard.repeat_delay = positive("repeat-delay", delay)?;
        }
        keyboard.rules = self.rules.unwrap_or(keyboard.rules);
        keyboard.model = self.model.unwrap_or(keyboard.model);
        keyboard.layout = self.layout.unwrap_or(keyboard.layout);
        keyboard.variant = self.variant.unwrap_or(keyboard.variant);
        keyboard.options = self.options.or(keyboard.options);
        keyboard.numlock = self.numlock.or(keyboard.numlock);
        if let Some(switching) = self.switching {
            keyboard.switching = match switching {
                RawSwitching::Global => Switching::Global,
                RawSwitching::Window => Switching::Window,
            };
        }
        Ok(keyboard)
    }
}

impl RawOutput {
    fn apply(self) -> Result<OutputRule, Error> {
        let key = format!("output.{}", self.name);
        let mode = self
            .mode
            .as_deref()
            .map(mode)
            .transpose()
            .map_err(|reason| invalid(format!("{key}.mode"), reason))?;
        if let Some(scale) = self.scale
            && !(0.5..=4.0).contains(&scale)
        {
            return Err(invalid(
                format!("{key}.scale"),
                format!("{scale} is outside 0.5 to 4"),
            ));
        }
        let place = self.place(&key)?;
        Ok(OutputRule {
            name: self.name,
            enable: self.enable.unwrap_or(true),
            mode,
            place,
            scale: self.scale,
        })
    }

    /// At most one of `position`, `left-of`, `right-of`, `above` and `below`,
    /// and an `offset` only with a side to be offset along.
    fn place(&self, key: &str) -> Result<Place, Error> {
        let sides = [
            ("left-of", Side::LeftOf, &self.left_of),
            ("right-of", Side::RightOf, &self.right_of),
            ("above", Side::Above, &self.above),
            ("below", Side::Below, &self.below),
        ];
        let mut written = sides
            .iter()
            .filter_map(|(word, side, of)| of.as_ref().map(|of| (*word, *side, of)));
        let beside = written.next();
        if let Some((second, _, _)) = written.next() {
            return Err(invalid(
                format!("{key}.{second}"),
                format!(
                    "an output goes on one side of another; `{}` is already written",
                    beside.map_or("", |(word, _, _)| word)
                ),
            ));
        }
        match (self.position, beside) {
            (Some(_), Some((word, _, _))) => Err(invalid(
                format!("{key}.{word}"),
                "`position` already says where this output goes; write one or the other".to_owned(),
            )),
            (Some(_), None) | (None, None) if self.offset.is_some() => Err(invalid(
                format!("{key}.offset"),
                "an offset is along the edge shared with another output; say which, with \
                 left-of, right-of, above or below"
                    .to_owned(),
            )),
            (Some([x, y]), None) => Ok(Place::At(x, y)),
            (None, Some((word, _, of))) if *of == self.name => Err(invalid(
                format!("{key}.{word}"),
                "an output cannot be placed beside itself".to_owned(),
            )),
            (None, Some((_, side, of))) => Ok(Place::Beside {
                side,
                of: of.clone(),
                offset: self.offset.unwrap_or(0),
            }),
            (None, None) => Ok(Place::Auto),
        }
    }
}

/// The first output whose chain of `beside`s comes back to it, if any.
/// Following a chain at most as many steps as there are rules is enough: a
/// longer chain has visited some rule twice.
fn cycle(outputs: &[OutputRule]) -> Option<&str> {
    let of = |name: &str| {
        outputs
            .iter()
            .find(|rule| rule.name == name)
            .and_then(|rule| match &rule.place {
                Place::Beside { of, .. } => Some(of.as_str()),
                _ => None,
            })
    };
    outputs
        .iter()
        .map(|rule| rule.name.as_str())
        .find(|&start| {
            let mut at = of(start);
            for _ in 0..outputs.len() {
                match at {
                    Some(name) if name == start => return true,
                    Some(name) => at = of(name),
                    None => return false,
                }
            }
            false
        })
}

/// The action a binding's value names. `pies` are the pies
/// `[shell.pie.menus]` names, which a `{ pie = "..." }` must be one of.
fn action_for(action: RawAction, pies: &BTreeSet<String>) -> Result<Option<Action>, String> {
    Ok(Some(match action {
        RawAction::Spawn(RawSpawn { spawn }) if spawn.is_empty() => {
            return Err("`spawn` needs a program".to_owned());
        }
        RawAction::Spawn(RawSpawn { spawn }) => Action::Spawn(spawn),
        RawAction::Pie(RawOpenPie { pie }) if pies.contains(&pie) => Action::Pie(pie),
        RawAction::Pie(RawOpenPie { pie }) if pies.is_empty() => {
            return Err(format!(
                "there is no pie `{pie}`: write it in `[shell.pie.menus]`"
            ));
        }
        RawAction::Pie(RawOpenPie { pie }) => {
            let named: Vec<&str> = pies.iter().map(String::as_str).collect();
            return Err(format!(
                "there is no pie `{pie}`; `[shell.pie.menus]` names {}",
                named.join(", ")
            ));
        }
        RawAction::Named(name) => match name.as_str() {
            "none" => return Ok(None),
            "close" => Action::Close,
            "cycle-focus" => Action::CycleFocus,
            "move-to-next-output" => Action::MoveToOutput(Towards::Next),
            "move-to-previous-output" => Action::MoveToOutput(Towards::Previous),
            "reload" => Action::Reload,
            "toggle-sticky" => Action::ToggleSticky,
            "toggle-maximize" => Action::ToggleMaximize,
            "minimize" => Action::Minimize,
            "next-tab" => Action::CycleTab { forward: true },
            "previous-tab" => Action::CycleTab { forward: false },
            "tab-with-previous" => Action::TabWithPrevious,
            "detach-tab" => Action::DetachTab,
            "start-menu" => Action::StartMenu,
            "root-menu" => Action::RootMenu,
            "next-layout" => Action::CycleLayout { forward: true },
            "previous-layout" => Action::CycleLayout { forward: false },
            "next-workspace" => Action::CycleWorkspace { forward: true },
            "previous-workspace" => Action::CycleWorkspace { forward: false },
            other => directed(other).or_else(|| numbered(other)).ok_or_else(|| {
                format!(
                    "`{other}` is not an action; use close, cycle-focus, reload, \
                         toggle-sticky, toggle-maximize, minimize, next-tab, previous-tab, \
                         tab-with-previous, detach-tab, start-menu, root-menu, \
                         next-layout, previous-layout, layout-<1-4>, \
                         next-workspace, previous-workspace, \
                         move-to-next-output, move-to-previous-output, \
                         move-to-output-<side>, workspace-<side>, workspace-<number>, \
                         send-to-workspace-<side>, carry-to-workspace-<side>, snap-<side>, \
                         none, {{ spawn = [...] }}, or \
                         {{ pie = \"<name>\" }}, where <side> is left, right, up or down"
                )
            })?,
        },
    }))
}

/// An action that takes a side: `workspace-left`, `move-to-output-down`.
fn directed(name: &str) -> Option<Action> {
    let (verb, side) = name.rsplit_once('-')?;
    let direction = match side {
        "left" => Direction::Left,
        "right" => Direction::Right,
        "up" => Direction::Up,
        "down" => Direction::Down,
        _ => return None,
    };
    Some(match verb {
        "workspace" => Action::Workspace(direction),
        "send-to-workspace" => Action::SendToWorkspace(direction),
        "carry-to-workspace" => Action::CarryToWorkspace(direction),
        "move-to-output" => Action::MoveToOutput(Towards::Side(direction)),
        "snap" => Action::Snap(direction),
        _ => return None,
    })
}

/// `workspace-3`. Any positive number is accepted here: whether the grid has
/// that many is a question for the grid, which a later reload may change.
/// `layout-2`, from 1 to 4: xkb holds four layouts at most, and whether the
/// keymap has that many is the keymap's question.
fn numbered(name: &str) -> Option<Action> {
    if let Some(layout) = name.strip_prefix("layout-") {
        let number: u8 = layout.parse().ok()?;
        return (1..=4).contains(&number).then_some(Action::Layout(number));
    }
    let number: u16 = name.strip_prefix("workspace-")?.parse().ok()?;
    (number > 0).then_some(Action::GoToWorkspace(number))
}

/// `"WxH"` or `"WxH@Hz"`.
fn mode(text: &str) -> Result<Mode, String> {
    let bad = || format!("`{text}` is not a mode; write it as 2560x1440 or 2560x1440@144");
    let (size, refresh) = match text.split_once('@') {
        Some((size, refresh)) => (size, Some(refresh.parse::<f64>().map_err(|_| bad())?)),
        None => (text, None),
    };
    let (width, height) = size.split_once('x').ok_or_else(bad)?;
    Ok(Mode {
        width: width.trim().parse().map_err(|_| bad())?,
        height: height.trim().parse().map_err(|_| bad())?,
        refresh,
    })
}

fn invalid(key: String, reason: String) -> Error {
    Error::Invalid { key, reason }
}

#[cfg(test)]
mod tests {
    use perspicax_policy::{Colour, Palette};

    use super::*;

    const SEAT: Built = Built {
        seat: true,
        xwayland: true,
        capture: true,
    };

    fn alt(key: Keysym) -> (Mods, [Keysym; 1]) {
        (Mods::alt(), [key])
    }

    #[test]
    fn an_empty_file_is_the_classic_profile() {
        assert_eq!(
            parse("", SEAT).unwrap(),
            Config::profile(Profile::Classic, SEAT)
        );
    }

    #[test]
    fn the_minimal_profile_focuses_under_the_pointer() {
        let config = parse(r#"profile = "minimal""#, SEAT).unwrap();
        assert_eq!(config.focus.model, FocusModel::Sloppy);
    }

    #[test]
    fn a_users_key_overrides_one_setting_and_keeps_the_rest_of_the_profile() {
        let config = parse(
            r#"
            profile = "minimal"
            [focus]
            autoraise = true
            "#,
            SEAT,
        )
        .unwrap();
        assert_eq!(config.focus.model, FocusModel::Sloppy, "from the profile");
        assert!(config.focus.autoraise, "from the file");
    }

    #[test]
    fn a_misspelled_key_is_an_error_naming_it() {
        let error = parse("[focus]\nautorasie = true", SEAT).unwrap_err();
        assert!(error.to_string().contains("autorasie"), "{error}");
    }

    #[test]
    fn a_key_for_an_uncompiled_feature_is_refused() {
        let built = Built {
            seat: true,
            xwayland: false,
            capture: true,
        };
        let error = parse("xwayland = true", built).unwrap_err();
        assert!(
            error.to_string().contains("`xwayland` cargo feature"),
            "the message has to say how to get it: {error}"
        );
    }

    #[test]
    fn turning_off_what_is_not_built_is_not_an_error() {
        let built = Built {
            seat: true,
            xwayland: false,
            capture: true,
        };
        assert!(!parse("xwayland = false", built).unwrap().xwayland);
    }

    #[test]
    fn input_settings_in_a_build_without_a_seat_are_refused() {
        let error = parse("[input.keyboard]\nlayout = \"de\"", Built::default()).unwrap_err();
        assert!(matches!(
            error,
            Error::NotBuilt {
                key: "input",
                feature: "seat"
            }
        ));
    }

    #[test]
    fn a_spawn_binding_is_added_and_none_takes_a_profile_binding_away() {
        let config = parse(
            r#"
            [keys]
            "Logo+Return" = { spawn = ["foot"] }
            "Alt+F4" = "none"
            "#,
            SEAT,
        )
        .unwrap();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::Return]),
            Some(&Action::Spawn(vec!["foot".to_owned()]))
        );
        let (mods, f4) = alt(Keysym::F4);
        assert_eq!(config.bindings.resolve(mods, &f4), None);
    }

    #[test]
    fn an_unknown_action_is_named() {
        let error = parse("[keys]\n\"Alt+x\" = \"explode\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("`explode`"), "{error}");
    }

    const SIDE: Gesture = Gesture::Press(perspicax_policy::Button::Side);

    #[test]
    fn each_mouse_table_binds_where_the_pointer_is() {
        let config = parse(
            r#"
            [mouse.desktop]
            "Mouse8" = "workspace-left"
            "WheelDown" = "next-workspace"

            [mouse.titlebar]
            "Double+Mouse1" = "minimize"

            [mouse.window]
            "Alt+Mouse9" = "toggle-sticky"

            [mouse.anywhere]
            "Logo+Mouse8" = { spawn = ["foot"] }
            "#,
            SEAT,
        )
        .unwrap();
        let none = Mods::default();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        let mouse = &config.mouse;
        assert_eq!(
            mouse.resolve(Context::Desktop, none, SIDE),
            Some(&Action::Workspace(Direction::Left))
        );
        assert_eq!(
            mouse.resolve(Context::Window, none, SIDE),
            None,
            "the desktop's thumb button is not taken from a window"
        );
        assert_eq!(
            mouse.resolve(
                Context::Desktop,
                none,
                Gesture::Wheel(perspicax_policy::Wheel::Down)
            ),
            Some(&Action::CycleWorkspace { forward: true })
        );
        assert_eq!(
            mouse.resolve(
                Context::Titlebar,
                none,
                Gesture::Double(perspicax_policy::Button::Left)
            ),
            Some(&Action::Minimize)
        );
        assert_eq!(
            mouse.resolve(
                Context::Titlebar,
                Mods::alt(),
                Gesture::Press(perspicax_policy::Button::Extra)
            ),
            Some(&Action::ToggleSticky),
            "a titlebar is part of its window"
        );
        assert_eq!(
            mouse.resolve(Context::Window, logo, SIDE),
            Some(&Action::Spawn(vec!["foot".to_owned()]))
        );
    }

    #[test]
    fn no_profile_binds_a_mouse_button() {
        for profile in [Profile::Classic, Profile::Minimal] {
            assert_eq!(
                Config::profile(profile, SEAT).mouse,
                MouseBindings::default()
            );
        }
    }

    #[test]
    fn a_misspelled_mouse_table_or_button_is_an_error_naming_it() {
        let error = parse("[mouse.panel]\n\"Mouse8\" = \"close\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("`panel`"), "{error}");
        let error = parse("[mouse.desktop]\n\"thumb\" = \"close\"", SEAT).unwrap_err();
        assert!(
            error.to_string().contains("`mouse.desktop.thumb`"),
            "{error}"
        );
        let error = parse("[mouse.desktop]\n\"Mouse8\" = \"explode\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("`explode`"), "{error}");
    }

    #[test]
    fn one_button_written_twice_in_a_table_is_refused_naming_both() {
        let text = "[mouse.desktop]\n\"Mouse8\" = \"close\"\n\"side\" = \"minimize\"";
        let error = parse(text, SEAT).unwrap_err().to_string();
        assert!(error.contains("`mouse.desktop.side`"), "{error}");
        assert!(error.contains("`Mouse8`"), "{error}");
        // In two tables it is two bindings.
        let text = "[mouse.desktop]\n\"Mouse8\" = \"close\"\n[mouse.window]\n\"side\" = \"none\"";
        assert!(parse(text, SEAT).is_ok());
    }

    #[test]
    fn a_window_binding_that_would_take_the_drag_away_is_refused_naming_drag() {
        let error = parse(
            "drag = \"Alt\"\n[mouse.window]\n\"Alt+Mouse1\" = \"close\"",
            SEAT,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("`mouse.window.Alt+Mouse1`"), "{error}");
        assert!(error.contains("`drag` is Alt,"), "{error}");
        assert!(error.contains("moves"), "{error}");
        // The classic profile drags with Alt without a word in the file.
        let error = parse("[mouse.anywhere]\n\"Alt+Mouse3\" = \"close\"", SEAT)
            .unwrap_err()
            .to_string();
        assert!(error.contains("the classic profile's"), "{error}");
        assert!(error.contains("resizes"), "{error}");
    }

    #[test]
    fn a_binding_that_leaves_the_drag_some_press_is_accepted() {
        for text in [
            "drag = \"Logo\"\n[mouse.window]\n\"Alt+Mouse1\" = \"close\"",
            "drag = \"none\"\n[mouse.window]\n\"Alt+Mouse1\" = \"close\"",
            "[mouse.titlebar]\n\"Alt+Mouse1\" = \"close\"",
            "[mouse.desktop]\n\"Alt+Mouse1\" = \"close\"",
            "[mouse.window]\n\"Double+Alt+Mouse1\" = \"close\"",
            "[mouse.window]\n\"Alt+Mouse2\" = \"close\"",
            "[mouse.window]\n\"Alt+Mouse1\" = \"none\"",
        ] {
            assert!(parse(text, SEAT).is_ok(), "{text}");
        }
    }

    #[test]
    fn none_in_a_mouse_table_hands_the_button_back_there() {
        let config = parse(
            "[mouse.anywhere]\n\"Mouse8\" = \"workspace-left\"\n\
             [mouse.window]\n\"Mouse8\" = \"none\"",
            SEAT,
        )
        .unwrap();
        let none = Mods::default();
        assert_eq!(config.mouse.resolve(Context::Window, none, SIDE), None);
        assert_eq!(
            config.mouse.resolve(Context::Desktop, none, SIDE),
            Some(&Action::Workspace(Direction::Left))
        );
    }

    #[test]
    fn the_next_and_previous_workspace_are_actions_for_keys_too() {
        let config = parse(
            "[keys]\n\"Logo+n\" = \"next-workspace\"\n\"Logo+p\" = \"previous-workspace\"",
            SEAT,
        )
        .unwrap();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::n]),
            Some(&Action::CycleWorkspace { forward: true })
        );
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::p]),
            Some(&Action::CycleWorkspace { forward: false })
        );
    }

    #[test]
    fn the_shell_reads_a_file_with_mouse_bindings() {
        assert!(shell("[mouse.desktop]\n\"Mouse8\" = \"close\"", ShellBuilt::FULL).is_ok());
    }

    #[test]
    fn reload_is_bound_in_both_profiles() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let config = Config::profile(profile, SEAT);
            let held = Mods {
                logo: true,
                shift: true,
                ..Mods::default()
            };
            assert_eq!(
                config.bindings.resolve(held, &[Keysym::r]),
                Some(&Action::Reload),
                "{profile:?}"
            );
        }
    }

    #[test]
    fn drag_can_be_moved_to_logo_or_turned_off() {
        let logo = parse(r#"drag = "Logo""#, SEAT).unwrap();
        let held = Mods {
            logo: true,
            ..Mods::default()
        };
        assert!(
            logo.bindings
                .drag(held, perspicax_policy::Button::Left)
                .is_some()
        );
        let off = parse(r#"drag = "none""#, SEAT).unwrap();
        assert!(
            off.bindings
                .drag(Mods::alt(), perspicax_policy::Button::Left)
                .is_none()
        );
    }

    #[test]
    fn outputs_parse_mode_position_and_scale() {
        let config = parse(
            r#"
            [[output]]
            name = "DP-1"
            mode = "2560x1440@143.9"
            position = [1920, 0]
            scale = 1.5

            [[output]]
            name = "eDP-1"
            enable = false
            "#,
            SEAT,
        )
        .unwrap();
        assert_eq!(
            config.outputs[0].mode,
            Some(Mode {
                width: 2560,
                height: 1440,
                refresh: Some(143.9)
            })
        );
        assert_eq!(config.outputs[0].place, Place::At(1920, 0));
        assert!(!config.outputs[1].enable);
    }

    #[test]
    fn an_output_is_placed_beside_another_with_an_offset() {
        let config = parse(
            r#"
            [[output]]
            name = "DP-1"

            [[output]]
            name = "HDMI-A-1"
            right-of = "DP-1"
            offset = -180
            "#,
            SEAT,
        )
        .unwrap();
        assert_eq!(config.outputs[0].place, Place::Auto);
        assert_eq!(
            config.outputs[1].place,
            Place::Beside {
                side: Side::RightOf,
                of: "DP-1".to_owned(),
                offset: -180
            }
        );
    }

    #[test]
    fn an_output_beside_one_with_no_rule_is_allowed() {
        // The anchor may simply be a monitor that needs no settings.
        let config = parse("[[output]]\nname = \"eDP-1\"\nbelow = \"DP-1\"", SEAT).unwrap();
        assert!(matches!(
            config.outputs[0].place,
            Place::Beside {
                side: Side::Below,
                ..
            }
        ));
    }

    #[test]
    fn two_sides_for_one_output_are_refused() {
        let text = "[[output]]\nname = \"A\"\nleft-of = \"B\"\nabove = \"C\"";
        let error = parse(text, SEAT).unwrap_err();
        assert!(error.to_string().contains("output.A.above"), "{error}");
    }

    #[test]
    fn a_position_and_a_side_together_are_refused() {
        let text = "[[output]]\nname = \"A\"\nposition = [0, 0]\nright-of = \"B\"";
        let error = parse(text, SEAT).unwrap_err();
        assert!(error.to_string().contains("output.A.right-of"), "{error}");
    }

    #[test]
    fn an_offset_with_no_side_is_refused() {
        let error = parse("[[output]]\nname = \"A\"\noffset = 10", SEAT).unwrap_err();
        assert!(error.to_string().contains("output.A.offset"), "{error}");
    }

    #[test]
    fn an_output_beside_itself_is_refused() {
        assert!(parse("[[output]]\nname = \"A\"\nbelow = \"A\"", SEAT).is_err());
    }

    #[test]
    fn outputs_placed_beside_each_other_in_a_circle_are_refused() {
        let text = "[[output]]\nname = \"A\"\nright-of = \"B\"\n\
                    [[output]]\nname = \"B\"\nbelow = \"C\"\n\
                    [[output]]\nname = \"C\"\nleft-of = \"A\"";
        let error = parse(text, SEAT).unwrap_err();
        assert!(error.to_string().contains("output.A"), "{error}");
    }

    #[test]
    fn a_malformed_mode_is_named_with_its_key() {
        let error = parse("[[output]]\nname = \"DP-1\"\nmode = \"big\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("output.DP-1.mode"), "{error}");
    }

    #[test]
    fn one_output_described_twice_is_refused() {
        let text = "[[output]]\nname = \"DP-1\"\n[[output]]\nname = \"DP-1\"";
        assert!(parse(text, SEAT).is_err());
    }

    #[test]
    fn keyboard_settings_layer_on_the_defaults() {
        let config = parse("[input.keyboard]\nlayout = \"de\"\nrepeat-rate = 40", SEAT).unwrap();
        assert_eq!(config.keyboard.layout, "de");
        assert_eq!(config.keyboard.repeat_rate, 40);
        assert_eq!(config.keyboard.repeat_delay, 200, "the default, untouched");
    }

    #[test]
    fn num_lock_is_left_alone_unless_the_config_says() {
        assert_eq!(parse("", SEAT).unwrap().keyboard.numlock, None);
        let on = parse("[input.keyboard]\nnumlock = true", SEAT).unwrap();
        assert_eq!(on.keyboard.numlock, Some(true));
        let off = parse("[input.keyboard]\nnumlock = false", SEAT).unwrap();
        assert_eq!(off.keyboard.numlock, Some(false));
        assert!(parse("[input.keyboard]\nnumlock = \"on\"", SEAT).is_err());
    }

    #[test]
    fn the_layout_is_the_sessions_unless_the_config_gives_each_window_its_own() {
        assert_eq!(
            parse("", SEAT).unwrap().keyboard.switching,
            Switching::Global
        );
        let window = parse("[input.keyboard]\nswitching = \"window\"", SEAT).unwrap();
        assert_eq!(window.keyboard.switching, Switching::Window);
        let error = parse("[input.keyboard]\nswitching = \"app\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("switching"), "{error}");
    }

    #[test]
    fn layout_actions_are_named_and_numbered_from_one_to_four() {
        let text = "[keys]\n\"Ctrl+1\" = \"layout-1\"\n\
                    \"Ctrl+4\" = \"layout-4\"\n\"Ctrl+n\" = \"next-layout\"\n\
                    \"Ctrl+p\" = \"previous-layout\"";
        let bindings = parse(text, SEAT).unwrap().bindings;
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        assert_eq!(
            bindings.resolve(ctrl, &[Keysym::_1]),
            Some(&Action::Layout(1))
        );
        assert_eq!(
            bindings.resolve(ctrl, &[Keysym::_4]),
            Some(&Action::Layout(4))
        );
        assert_eq!(
            bindings.resolve(ctrl, &[Keysym::n]),
            Some(&Action::CycleLayout { forward: true })
        );
        assert_eq!(
            bindings.resolve(ctrl, &[Keysym::p]),
            Some(&Action::CycleLayout { forward: false })
        );
        for refused in ["layout-0", "layout-5", "layout-x"] {
            let text = format!("[keys]\n\"Ctrl+1\" = \"{refused}\"");
            assert!(parse(&text, SEAT).is_err(), "{refused}");
        }
    }

    #[test]
    fn classic_switches_layout_with_logo_space_and_minimal_does_not() {
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        let classic = Config::profile(Profile::Classic, SEAT).bindings;
        assert_eq!(
            classic.resolve(logo, &[Keysym::space]),
            Some(&Action::CycleLayout { forward: true })
        );
        let minimal = Config::profile(Profile::Minimal, SEAT).bindings;
        assert_eq!(minimal.resolve(logo, &[Keysym::space]), None);
    }

    #[test]
    fn a_zero_repeat_rate_is_refused() {
        assert!(parse("[input.keyboard]\nrepeat-rate = 0", SEAT).is_err());
    }

    #[test]
    fn pointer_acceleration_outside_its_range_is_refused() {
        assert!(parse("[input.pointer]\naccel = 2.0", SEAT).is_err());
    }

    #[test]
    fn an_empty_autostart_command_is_refused() {
        assert!(parse("autostart = [[]]", SEAT).is_err());
    }

    #[test]
    fn xdg_autostart_is_on_in_both_profiles_and_can_be_turned_off() {
        for profile in [Profile::Classic, Profile::Minimal] {
            assert!(Config::profile(profile, SEAT).xdg_autostart, "{profile:?}");
        }
        assert!(!parse("xdg-autostart = false", SEAT).unwrap().xdg_autostart);
    }

    #[test]
    fn xdg_autostart_in_a_build_without_a_seat_is_refused() {
        let error = parse("xdg-autostart = true", Built::default()).unwrap_err();
        assert!(matches!(
            error,
            Error::NotBuilt {
                key: "xdg-autostart",
                feature: "seat"
            }
        ));
        assert!(
            !parse("xdg-autostart = false", Built::default())
                .unwrap()
                .xdg_autostart,
            "turning off what is not built is not an error"
        );
    }

    #[test]
    fn both_profiles_span_every_monitor_and_differ_in_their_grid() {
        let classic = Config::profile(Profile::Classic, SEAT).workspaces;
        let minimal = Config::profile(Profile::Minimal, SEAT).workspaces;
        assert_eq!(classic.mode, perspicax_policy::Mode::Spanning);
        assert_eq!((classic.grid.columns, classic.grid.rows), (4, 1));
        assert_eq!(minimal.mode, perspicax_policy::Mode::Spanning);
        assert_eq!((minimal.grid.columns, minimal.grid.rows), (2, 2));
        assert!(minimal.grid.wrap);
    }

    #[test]
    fn workspaces_can_be_per_output_on_a_grid_of_their_own() {
        let config = parse("[workspaces]\nmode = \"per-output\"\ngrid = [3, 2]", SEAT).unwrap();
        assert_eq!(config.workspaces.mode, perspicax_policy::Mode::PerOutput);
        assert_eq!(config.workspaces.grid.len(), 6);
        assert!(
            !config.workspaces.grid.wrap,
            "the classic profile's, untouched"
        );
    }

    #[test]
    fn the_minimal_profile_flips_at_the_edge_and_classic_does_not() {
        assert!(!Config::profile(Profile::Classic, SEAT).flipping.edge);
        let minimal = Config::profile(Profile::Minimal, SEAT).flipping;
        assert!(minimal.edge && minimal.while_dragging && minimal.scroll);
    }

    #[test]
    fn edge_flipping_is_turned_on_with_its_own_delay() {
        let config = parse("[workspaces.flip]\nedge = true\ndelay-ms = 500", SEAT).unwrap();
        assert!(config.flipping.edge);
        assert_eq!(config.flipping.delay_ms, 500);
        assert!(!config.flipping.scroll, "the profile's, untouched");
    }

    #[test]
    fn the_wheel_flips_the_other_way_with_a_fluxbox_keys_files_own_lines() {
        // `OnDesktop Mouse4 :NextWorkspace`, and so on round the wheel.
        let config = parse(
            r#"
            profile = "minimal"

            [mouse.desktop]
            "Mouse4" = "next-workspace"
            "Mouse5" = "previous-workspace"
            "WheelLeft" = "next-workspace"
            "WheelRight" = "previous-workspace"
            "#,
            SEAT,
        )
        .unwrap();
        let turned = |wheel| {
            config
                .mouse
                .resolve(Context::Desktop, Mods::default(), Gesture::Wheel(wheel))
        };
        let next = Some(&Action::CycleWorkspace { forward: true });
        let previous = Some(&Action::CycleWorkspace { forward: false });
        assert_eq!(turned(perspicax_policy::Wheel::Up), next);
        assert_eq!(turned(perspicax_policy::Wheel::Left), next);
        assert_eq!(turned(perspicax_policy::Wheel::Down), previous);
        assert_eq!(turned(perspicax_policy::Wheel::Right), previous);
        assert!(
            config.flipping.scroll,
            "the profile's, untouched: the bindings come before it"
        );
    }

    #[test]
    fn a_flip_delay_of_minutes_is_refused() {
        let error = parse("[workspaces.flip]\ndelay-ms = 600000", SEAT).unwrap_err();
        assert!(
            error.to_string().contains("workspaces.flip.delay-ms"),
            "{error}"
        );
    }

    #[test]
    fn classic_snaps_by_drag_and_by_logo_arrows_and_minimal_does_not() {
        let classic = Config::profile(Profile::Classic, SEAT);
        assert!(classic.snapping.drag);
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            classic.bindings.resolve(logo, &[Keysym::Left]),
            Some(&Action::Snap(Direction::Left))
        );
        let minimal = Config::profile(Profile::Minimal, SEAT);
        assert!(!minimal.snapping.drag);
        assert_eq!(minimal.bindings.resolve(logo, &[Keysym::Left]), None);
    }

    #[test]
    fn snapping_is_configured_in_its_own_table() {
        let config = parse("[snap]\ndrag = false\nthreshold = 12", SEAT).unwrap();
        assert!(!config.snapping.drag);
        assert_eq!(config.snapping.threshold, 12);
        assert!(parse("[snap]\nthreshold = 0", SEAT).is_err());
    }

    #[test]
    fn minimal_holds_a_moving_window_at_screen_edges_and_classic_does_not() {
        assert_eq!(
            Config::profile(Profile::Classic, SEAT).resistance,
            Resistance::default()
        );
        assert_eq!(
            Config::profile(Profile::Minimal, SEAT).resistance,
            Resistance {
                edges: 20,
                seams: 0,
            }
        );
    }

    #[test]
    fn resistance_is_configured_in_its_own_table() {
        let config = parse("[resistance]\nedges = 8\nseams = 16", SEAT).unwrap();
        assert_eq!(
            config.resistance,
            Resistance {
                edges: 8,
                seams: 16,
            }
        );
        let seams = parse("profile = \"minimal\"\n[resistance]\nseams = 4", SEAT).unwrap();
        assert_eq!(seams.resistance.edges, 20, "the profile's, kept");
        assert_eq!(seams.resistance.seams, 4);
        let bounds = parse("[resistance]\nedges = 0\nseams = 128", SEAT).unwrap();
        assert_eq!(
            bounds.resistance,
            Resistance {
                edges: 0,
                seams: 128,
            }
        );
    }

    #[test]
    fn a_resistance_out_of_range_is_refused_by_name() {
        for (text, key) in [
            ("[resistance]\nedges = -1", "resistance.edges"),
            ("[resistance]\nedges = 129", "resistance.edges"),
            ("[resistance]\nseams = -1", "resistance.seams"),
            ("[resistance]\nseams = 129", "resistance.seams"),
        ] {
            let error = parse(text, SEAT).unwrap_err();
            assert!(error.to_string().contains(key), "{text}: {error}");
        }
        assert!(
            parse("[resistance]\nwindows = 4", SEAT).is_err(),
            "resisting other windows is not a key yet"
        );
    }

    #[test]
    fn decorations_are_configured_in_their_own_table() {
        assert_eq!(
            Config::profile(Profile::Minimal, SEAT).decorations,
            Decorations::default()
        );
        let config = parse(
            "[decorations]\nmode = \"client\"\ntitle-height = 30\nborder = 0\n\
             focused = \"#ff8800\"",
            SEAT,
        )
        .unwrap();
        let decorations = config.decorations;
        assert!(!decorations.server);
        assert_eq!((decorations.title, decorations.border), (30, 0));
        assert_eq!(decorations.focused, Colour::rgb(0xff, 0x88, 0x00));
        assert_eq!(decorations.unfocused, Decorations::default().unfocused);
    }

    #[test]
    fn a_decoration_that_cannot_be_drawn_is_refused_by_name() {
        for (text, key) in [
            ("title-height = 2", "decorations.title-height"),
            ("border = -1", "decorations.border"),
            ("focused = \"orange\"", "decorations.focused"),
        ] {
            let error = parse(&format!("[decorations]\n{text}"), SEAT).unwrap_err();
            assert!(error.to_string().contains(key), "{error}");
        }
        assert!(parse("[decorations]\nmode = \"both\"", SEAT).is_err());
    }

    /// The shell draws in the theme, and reads it as the compositor does,
    /// so a theme change reaches it as a change to `[shell]` would.
    #[test]
    fn the_shell_reads_the_theme_and_the_compositor_hands_it_on() {
        let text = "[theme]\nname = \"breeze-light\"\nfont-size = 16\n\
                    [theme.palette]\naccent = \"#e93d5a\"";
        let shell = shell(text, ShellBuilt::FULL).unwrap();
        assert_eq!(shell.font.size, 16);
        assert_eq!(
            shell.palette[Role::Accent],
            Rgba::new(0xe9, 0x3d, 0x5a, 0xff)
        );
        assert_eq!(
            shell.palette[Role::Panel],
            Builtin::BreezeLight.palette()[Role::Panel]
        );
        assert_eq!(parse(text, SEAT).unwrap().shell, shell);
        assert_ne!(
            shell.palette,
            Shell::profile(Profile::Classic, ShellBuilt::FULL).palette
        );
    }

    #[test]
    fn with_no_theme_the_desk_looks_as_it_did_before_themes() {
        let config = parse("", SEAT).unwrap();
        assert_eq!(config.theme, Theme::default());
        assert_eq!(config.decorations, Decorations::default());
    }

    #[test]
    fn a_theme_is_named_and_written_over_one_colour_at_a_time() {
        let config = parse(
            "[theme]\nname = \"breeze-dark\"\nfont = \"Noto Sans\"\nfont-size = 12\n\
             [theme.palette]\npanel = \"#102030\"\nselected = \"#ff000080\"\n",
            SEAT,
        )
        .unwrap();
        let theme = config.theme;
        assert_eq!(theme.builtin, Builtin::BreezeDark);
        assert_eq!((theme.cursor, theme.cursor_size), (None, None));
        assert_eq!(theme.font.family, Family::Named("Noto Sans".to_owned()));
        assert_eq!(theme.font.size, 12);
        assert_eq!(
            theme.palette[Role::Panel],
            Rgba::new(0x10, 0x20, 0x30, 0xff)
        );
        assert_eq!(theme.palette[Role::Selected], Rgba::new(0xff, 0, 0, 0x80));
        // The rest is the theme's own.
        let dark = Builtin::BreezeDark.palette();
        assert_eq!(theme.palette[Role::Menu], dark[Role::Menu]);
        // And the titlebars wear the theme's colours.
        assert_eq!(
            config.decorations.focused,
            dark[Role::TitleFocused].colour()
        );
        assert_eq!(
            config.decorations.unfocused_ink,
            dark[Role::TitleUnfocusedInk].colour()
        );
    }

    /// Applications are told nothing by the default theme, a Breeze's scheme
    /// and accent by a Breeze, and whatever is written over either.
    #[test]
    fn applications_are_told_what_the_theme_and_the_file_say() {
        assert_eq!(parse("", SEAT).unwrap().theme.apps, Appearance::default());

        let dark = parse("[theme]\nname = \"breeze-dark\"", SEAT)
            .unwrap()
            .theme
            .apps;
        assert_eq!(dark.color_scheme, Some(ColorScheme::Dark));
        assert_eq!(dark.accent, Some(Colour::rgb(0x3d, 0xae, 0xe9)));

        let written = parse(
            "[theme]\ncolor-scheme = \"light\"\ncontrast = \"high\"\n\
             [theme.palette]\naccent = \"#e93d5a\"",
            SEAT,
        )
        .unwrap()
        .theme
        .apps;
        assert_eq!(
            written,
            Appearance {
                color_scheme: Some(ColorScheme::Light),
                accent: Some(Colour::rgb(0xe9, 0x3d, 0x5a)),
                contrast: Some(Contrast::High),
            }
        );
        assert!(matches!(
            parse("[theme]\ncolor-scheme = \"grey\"", SEAT),
            Err(Error::Parse(_))
        ));
    }

    #[test]
    fn the_pointer_is_named_and_sized_in_the_theme() {
        let theme = parse("[theme]\ncursor = \"Adwaita\"\ncursor-size = 32", SEAT)
            .unwrap()
            .theme;
        assert_eq!(theme.cursor.as_deref(), Some("Adwaita"));
        assert_eq!(theme.cursor_size, Some(32));
    }

    /// `[decorations] focused` predates themes. It still says the focused
    /// titlebar's colour, its ink follows as it always did, and writing the
    /// colour in both places is refused rather than one quietly winning.
    #[test]
    fn the_older_titlebar_colours_are_the_palettes_and_not_twice() {
        let config = parse("[decorations]\nfocused = \"#ffffff\"", SEAT).unwrap();
        assert_eq!(config.decorations.focused, Colour::rgb(0xff, 0xff, 0xff));
        assert_eq!(config.decorations.focused_ink, Colour::rgb(0, 0, 0));
        assert_eq!(
            config.theme.palette[Role::TitleFocused],
            Rgba::new(0xff, 0xff, 0xff, 0xff)
        );

        let error = parse(
            "[decorations]\nfocused = \"#ffffff\"\n[theme.palette]\ntitle-focused = \"#000000\"",
            SEAT,
        )
        .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid { key, .. } if key == "decorations.focused"),
            "{error}"
        );
    }

    #[test]
    fn a_theme_that_cannot_be_drawn_is_refused_by_its_key() {
        for (text, key) in [
            ("name = \"Breeze\"", "theme.name"),
            ("font = \" \"", "theme.font"),
            ("font-size = 40", "theme.font-size"),
            ("cursor = \"\"", "theme.cursor"),
            ("cursor-size = 4", "theme.cursor-size"),
            (
                "[theme.palette]\nbackground = \"#000000\"",
                "theme.palette.background",
            ),
            ("[theme.palette]\npanel = \"dark\"", "theme.palette.panel"),
            // Opaque, so the compositor can prove what a panel covers.
            (
                "[theme.palette]\npanel = \"#00000080\"",
                "theme.palette.panel",
            ),
        ] {
            let error = parse(&format!("[theme]\n{text}"), SEAT).unwrap_err();
            assert!(
                matches!(&error, Error::Invalid { key: at, .. } if at == key),
                "{text}: {error}"
            );
        }
        assert!(matches!(
            parse("[theme]\ncolour = \"#000000\"", SEAT),
            Err(Error::Parse(_))
        ));
        assert_eq!(Palette::default(), Builtin::Perspicax.palette());
    }

    #[test]
    fn an_empty_or_huge_grid_is_refused() {
        for grid in ["[0, 2]", "[2, 99]"] {
            let error = parse(&format!("[workspaces]\ngrid = {grid}"), SEAT).unwrap_err();
            assert!(error.to_string().contains("workspaces.grid"), "{error}");
        }
    }

    #[test]
    fn ctrl_logo_arrows_switch_workspace_and_with_shift_carry_the_window() {
        let config = Config::profile(Profile::Classic, SEAT);
        let held = Mods {
            ctrl: true,
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(held, &[Keysym::Right]),
            Some(&Action::Workspace(Direction::Right))
        );
        let shifted = Mods {
            shift: true,
            ..held
        };
        assert_eq!(
            config.bindings.resolve(shifted, &[Keysym::Left]),
            Some(&Action::CarryToWorkspace(Direction::Left))
        );
    }

    #[test]
    fn workspace_actions_are_named_by_side_and_by_number() {
        let config = parse(
            r#"
            [keys]
            "Logo+1" = "workspace-1"
            "Logo+Shift+Up" = "send-to-workspace-up"
            "Logo+Ctrl+Down" = "move-to-output-down"
            "Logo+s" = "toggle-sticky"
            "#,
            SEAT,
        )
        .unwrap();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::_1]),
            Some(&Action::GoToWorkspace(1))
        );
        assert_eq!(
            config.bindings.resolve(
                Mods {
                    shift: true,
                    ..logo
                },
                &[Keysym::Up]
            ),
            Some(&Action::SendToWorkspace(Direction::Up))
        );
        assert_eq!(
            config
                .bindings
                .resolve(Mods { ctrl: true, ..logo }, &[Keysym::Down]),
            Some(&Action::MoveToOutput(Towards::Side(Direction::Down)))
        );
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::s]),
            Some(&Action::ToggleSticky)
        );
    }

    #[test]
    fn logo_tab_steps_through_tabs_and_logo_g_groups_in_both_profiles() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let bindings = Config::profile(profile, SEAT).bindings;
            let logo = Mods {
                logo: true,
                ..Mods::default()
            };
            let shift = Mods {
                shift: true,
                ..logo
            };
            assert_eq!(
                bindings.resolve(logo, &[Keysym::Tab]),
                Some(&Action::CycleTab { forward: true })
            );
            assert_eq!(
                bindings.resolve(shift, &[Keysym::Tab]),
                Some(&Action::CycleTab { forward: false })
            );
            assert_eq!(
                bindings.resolve(logo, &[Keysym::g]),
                Some(&Action::TabWithPrevious)
            );
            assert_eq!(
                bindings.resolve(shift, &[Keysym::g]),
                Some(&Action::DetachTab)
            );
        }
        let config = parse("[keys]\n\"Alt+t\" = \"next-tab\"", SEAT).unwrap();
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(alt, &[Keysym::t]),
            Some(&Action::CycleTab { forward: true })
        );
    }

    #[test]
    fn the_titlebar_buttons_are_actions_a_key_can_have_too() {
        let config = parse(
            "[keys]\n\"Logo+m\" = \"toggle-maximize\"\n\"Logo+n\" = \"minimize\"",
            SEAT,
        )
        .unwrap();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::m]),
            Some(&Action::ToggleMaximize)
        );
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::n]),
            Some(&Action::Minimize)
        );
    }

    #[test]
    fn workspace_zero_and_a_side_that_is_not_one_are_refused() {
        assert!(parse("[keys]\n\"Logo+0\" = \"workspace-0\"", SEAT).is_err());
        assert!(parse("[keys]\n\"Logo+0\" = \"workspace-sideways\"", SEAT).is_err());
    }

    #[test]
    fn logo_on_its_own_binds_a_tap() {
        let config = parse("[keys]\n\"Super\" = \"root-menu\"", SEAT).unwrap();
        assert_eq!(config.bindings.tap(), Some(&Action::RootMenu));
        let config = parse("[keys]\n\"Logo\" = \"none\"", SEAT).unwrap();
        assert_eq!(
            config.bindings.tap(),
            None,
            "none takes the profile's tap away"
        );
    }

    #[test]
    fn only_logo_can_be_bound_on_its_own() {
        let error = parse("[keys]\n\"Alt\" = \"start-menu\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("keys.Alt"), "{error}");
        assert!(error.to_string().contains("only Logo"), "{error}");
    }

    #[test]
    fn classic_taps_logo_for_the_start_menu_and_minimal_does_not() {
        assert_eq!(
            Config::profile(Profile::Classic, SEAT).bindings.tap(),
            Some(&Action::StartMenu)
        );
        assert_eq!(Config::profile(Profile::Minimal, SEAT).bindings.tap(), None);
    }

    #[test]
    fn the_menus_are_actions_a_chord_can_have() {
        let config = parse(
            "[keys]\n\"Alt+F1\" = \"root-menu\"\n\"Logo+Space\" = \"start-menu\"",
            SEAT,
        )
        .unwrap();
        assert_eq!(
            config.bindings.resolve(Mods::alt(), &[Keysym::F1]),
            Some(&Action::RootMenu)
        );
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::space]),
            Some(&Action::StartMenu)
        );
    }

    #[test]
    fn a_pie_is_opened_by_a_key_a_button_or_a_tap_of_logo_by_its_name() {
        let pies = "[shell.pie.menus]\nlaunchers = [{ running = true }]\n";
        let config = parse(
            &format!(
                "[keys]\n\"Logo+p\" = {{ pie = \"launchers\" }}\n\
                 [mouse.anywhere]\n\"Mouse8\" = {{ pie = \"launchers\" }}\n{pies}"
            ),
            SEAT,
        )
        .unwrap();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        let launchers = Action::Pie("launchers".to_owned());
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::p]),
            Some(&launchers)
        );
        assert_eq!(
            config.mouse.resolve(Context::Window, Mods::default(), SIDE),
            Some(&launchers)
        );
        let tapped = parse(
            &format!("[keys]\n\"Logo\" = {{ pie = \"launchers\" }}\n{pies}"),
            SEAT,
        )
        .unwrap();
        assert_eq!(tapped.bindings.tap(), Some(&launchers));
    }

    #[test]
    fn a_pie_no_shell_pie_names_is_refused_by_its_binding() {
        let reason = |text: &str| match parse(text, SEAT) {
            Err(Error::Invalid { key, reason }) => (key, reason),
            other => panic!("not refused: {other:?}"),
        };
        assert_eq!(
            reason(
                "[mouse.anywhere]\n\"Mouse8\" = { pie = \"launcher\" }\n\
                 [shell.pie.menus]\nlaunchers = [{ running = true }]\nwindows = [{ running = true }]"
            ),
            (
                "mouse.anywhere.Mouse8".to_owned(),
                "there is no pie `launcher`; `[shell.pie.menus]` names launchers, windows"
                    .to_owned()
            )
        );
        assert_eq!(
            reason("[keys]\n\"Logo+p\" = { pie = \"launchers\" }"),
            (
                "keys.Logo+p".to_owned(),
                "there is no pie `launchers`: write it in `[shell.pie.menus]`".to_owned()
            )
        );
        let error = parse("[keys]\n\"Logo+p\" = \"explode\"", SEAT).unwrap_err();
        assert!(
            error.to_string().contains("{ pie = \"<name>\" }"),
            "{error}"
        );
    }

    #[test]
    fn the_shell_protocol_defaults_to_perspicax_shell_in_both_profiles() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let access = Config::profile(profile, SEAT).protocols;
            assert!(access.admits(Protocol::Shell, Some("/usr/bin/perspicax-shell")));
            assert!(!access.admits(Protocol::Shell, Some("/usr/bin/waybar")));
            assert!(!access.admits(Protocol::Shell, None));
        }
        let config = parse("[protocols]\nshell = \"off\"", SEAT).unwrap();
        assert_eq!(config.protocols.rule(Protocol::Shell), &Rule::Off);
    }

    #[test]
    fn a_missing_file_is_the_classic_profile() {
        let config = load(Path::new("/nonexistent/perspicax/config.toml"), SEAT).unwrap();
        assert_eq!(config.profile, Profile::Classic);
    }

    #[test]
    fn protocols_default_is_the_same_in_both_profiles() {
        let classic = Config::profile(Profile::Classic, SEAT).protocols;
        assert_eq!(classic, Config::profile(Profile::Minimal, SEAT).protocols);
        assert!(classic.admits(Protocol::ForeignToplevelManagement, None));
        assert!(classic.admits(Protocol::Screencopy, Some("/usr/bin/grim")));
        assert!(!classic.admits(Protocol::Screencopy, Some("/usr/bin/firefox")));
        assert!(classic.admits(Protocol::OutputManagement, Some("/usr/bin/kanshi")));
    }

    #[test]
    fn a_protocol_key_layers_on_the_profile() {
        let config = parse(
            r#"
            [protocols]
            workspace = "off"
            screencopy = ["/usr/bin/grim", "flameshot"]
            "#,
            SEAT,
        )
        .unwrap();
        let access = config.protocols;
        assert!(!access.admits(Protocol::Workspace, Some("/usr/bin/waybar")));
        assert!(access.admits(Protocol::Screencopy, Some("/usr/bin/grim")));
        assert!(!access.admits(Protocol::Screencopy, Some("/tmp/grim")));
        assert!(access.admits(Protocol::Screencopy, Some("/opt/x/flameshot")));
        assert!(
            access.admits(Protocol::ForeignToplevelList, None),
            "from the profile"
        );
    }

    #[test]
    fn screencopy_without_capture_names_the_feature() {
        let built = Built {
            capture: false,
            ..SEAT
        };
        assert_eq!(
            Config::profile(Profile::Classic, built)
                .protocols
                .rule(Protocol::Screencopy),
            &Rule::Off
        );
        let error = parse("[protocols]\nscreencopy = \"any\"", built).unwrap_err();
        assert!(
            matches!(
                error,
                Error::NotBuilt {
                    key: "protocols.screencopy",
                    feature: "capture"
                }
            ),
            "{error}"
        );
        assert!(parse("[protocols]\nscreencopy = \"off\"", built).is_ok());
    }

    #[test]
    fn an_unknown_protocol_key_is_named() {
        let error = parse("[protocols]\nscreenshot = \"any\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("screenshot"), "{error}");
    }

    #[test]
    fn an_empty_allowlist_is_refused() {
        let error = parse("[protocols]\noutput-management = []", SEAT).unwrap_err();
        assert!(
            error.to_string().contains("protocols.output-management"),
            "{error}"
        );
        assert!(parse("[protocols]\nworkspace = [\"\"]", SEAT).is_err());
    }

    #[test]
    fn a_word_other_than_any_or_off_is_refused() {
        let error = parse("[protocols]\nworkspace = \"all\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("`all` is not a rule"), "{error}");
    }
}
