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

mod keys;

use std::path::{Path, PathBuf};

use perspicax_policy::{
    Action, Bindings, Chord, Colour, Decorations, Direction, Flipping, Focus, FocusModel, Grid,
    Keysym, Mods, Place, Shape, Side, Snapping, Towards,
};
use serde::Deserialize;

/// Which cargo features this binary was built with, as far as config cares.
/// The binary fills it in with `cfg!`; this crate cannot see the binary's
/// features itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Built {
    pub seat: bool,
    pub xwayland: bool,
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
    /// How many workspaces, in what grid, and whether one spans every
    /// monitor or each monitor has its own.
    pub workspaces: Shape,
    /// Changing workspace with the pointer: resting it on an edge of the
    /// desk, or scrolling over the desktop.
    pub flipping: Flipping,
    /// Dragging a window to an edge of the desk to give it half or a quarter
    /// of a monitor.
    pub snapping: Snapping,
    /// Who draws a window's titlebar and border, and what they look like.
    pub decorations: Decorations,
    /// Whether X11 applications get an Xwayland. Only meaningful in a build
    /// with the `xwayland` feature; saying `true` in one without it is an
    /// error.
    pub xwayland: bool,
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
        }
    }
}

/// Pointer device settings. `None` leaves libinput's own default alone,
/// which differs by device (tap-to-click is off on most touchpads, for
/// example), so "unset" and "false" are different requests.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pointer {
    /// Acceleration, from -1 (slowest) to 1 (fastest).
    pub accel: Option<f64>,
    pub natural_scroll: Option<bool>,
    pub tap_to_click: Option<bool>,
    pub left_handed: Option<bool>,
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
        let workspaces = match profile {
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
        };
        Self {
            profile,
            focus,
            bindings,
            keyboard: Keyboard::default(),
            pointer: Pointer::default(),
            outputs: Vec::new(),
            autostart: Vec::new(),
            workspaces,
            snapping: Snapping {
                drag: profile == Profile::Classic,
                ..Snapping::default()
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
            xwayland: built.xwayland,
        }
    }
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
    keys: std::collections::BTreeMap<String, RawAction>,
    drag: Option<String>,
    input: Option<RawInput>,
    #[serde(default, rename = "output")]
    outputs: Vec<RawOutput>,
    #[serde(default)]
    autostart: Vec<Vec<String>>,
    xwayland: Option<bool>,
    workspaces: Option<RawWorkspaces>,
    snap: Option<RawSnap>,
    decorations: Option<RawDecorations>,
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

/// A binding's value: an action's name, or `{ spawn = [...] }`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawAction {
    Named(String),
    Spawn(RawSpawn),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpawn {
    spawn: Vec<String>,
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawPointer {
    accel: Option<f64>,
    natural_scroll: Option<bool>,
    tap_to_click: Option<bool>,
    left_handed: Option<bool>,
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

        for (written, action) in self.keys {
            let chord = keys::chord(&written)
                .map_err(|reason| invalid(format!("keys.{written}"), reason))?;
            config.bindings = match action_for(action)
                .map_err(|reason| invalid(format!("keys.{written}"), reason))?
            {
                Some(action) => config.bindings.bind(chord, action),
                None => config.bindings.unbind(chord),
            };
        }
        if let Some(drag) = self.drag {
            config.bindings = match keys::modifiers(&drag)
                .map_err(|reason| invalid("drag".to_owned(), reason))?
            {
                Some(mods) => config.bindings.drag_with(mods),
                None => config.bindings.no_drag(),
            };
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

        if let Some(decorations) = self.decorations {
            config.decorations = decorations.apply(config.decorations)?;
        }

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
        let colour = |key: &str, text: String| {
            Colour::parse(&text).ok_or_else(|| {
                invalid(
                    format!("decorations.{key}"),
                    format!("{text:?} is not a colour; write one as \"#rrggbb\""),
                )
            })
        };
        if let Some(focused) = self.focused {
            decorations.focused = colour("focused", focused)?;
        }
        if let Some(unfocused) = self.unfocused {
            decorations.unfocused = colour("unfocused", unfocused)?;
        }
        Ok(decorations)
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

fn action_for(action: RawAction) -> Result<Option<Action>, String> {
    Ok(Some(match action {
        RawAction::Spawn(RawSpawn { spawn }) if spawn.is_empty() => {
            return Err("`spawn` needs a program".to_owned());
        }
        RawAction::Spawn(RawSpawn { spawn }) => Action::Spawn(spawn),
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
            other => directed(other).or_else(|| numbered(other)).ok_or_else(|| {
                format!(
                    "`{other}` is not an action; use close, cycle-focus, reload, \
                         toggle-sticky, toggle-maximize, minimize, \
                         move-to-next-output, move-to-previous-output, \
                         move-to-output-<side>, workspace-<side>, workspace-<number>, \
                         send-to-workspace-<side>, carry-to-workspace-<side>, snap-<side>, \
                         none, or \
                         {{ spawn = [...] }}, where <side> is left, right, up or down"
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
fn numbered(name: &str) -> Option<Action> {
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
    use super::*;

    const SEAT: Built = Built {
        seat: true,
        xwayland: true,
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
    fn a_missing_file_is_the_classic_profile() {
        let config = load(Path::new("/nonexistent/perspicax/config.toml"), SEAT).unwrap();
        assert_eq!(config.profile, Profile::Classic);
    }
}
