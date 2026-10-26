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
//! vocabulary, which the menu file, the start menu and a pie are written in.
//! And [`locate`]: where a path written in the file points. And
//! [`find_program`]: where perspicax's own programs are, so that the session
//! and `perspicax config check` find the same shell; and [`which`], where
//! anybody else's is.

mod actions;
pub mod autostart;
pub mod desktop;
mod keys;
pub mod limits;
pub mod menu;
pub mod mixer;
pub mod pie;
mod shell;
pub mod start_menu;
mod swaylock;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Path, PathBuf},
};

use perspicax_policy::{
    Access, Action, Animation, Bindings, Buttons, Chord, Colour, Context, Decorations, Direction,
    Drag, Effect, Face, Flipping, Focus, FocusModel, FrameButton, Gesture, Grid, Hatch, Keysym,
    Mods, MouseBindings, MouseChord, Never, Opacity, Place, Program, Protocol, Remembering,
    Resistance, Rule, Shadow, Shape, Side, Snapping, Switching, Towards, Transitioning, VolumeKey,
};
use serde::Deserialize;

pub use crate::actions::{Spelled, action_names, spell_action};
pub use crate::keys::{
    Trigger, modifiers, spell_chord, spell_modifiers, spell_press, spell_trigger, trigger,
};
use crate::shell::RawShell;
pub use crate::shell::{
    Align, Edge, Item, Leave, Panel, PanelOutputs, PanelWidth, Shell, ShellBuilt, TaskbarScope,
    UntrustedLaunchers, Wallpaper, WallpaperMode,
};
/// The double-click time, in milliseconds, of a config that does not say.
pub use perspicax_policy::DOUBLE_CLICK_MS;
/// The theme's types, as the shell reads them from [`Shell`].
pub use perspicax_policy::{
    Appearance, Builtin, ColorScheme, Contrast, Family, Font, Logo, Motion, Palette, Rgba, Role,
    Theme,
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
    /// Drawing SVG, which an SVG icon for a titlebar button needs.
    pub svg: bool,
}

/// A person's own pictures for the titlebar's buttons: the
/// `[decorations.icons]` table. A face with no file is drawn as the
/// built-in shape, and so is one whose file cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icons {
    /// A file for each face that has one, as written: a seat finds a
    /// relative path beside the config and `~` in the home folder, as it
    /// finds the theme's logo ([`locate`]).
    pub files: Vec<(Face, PathBuf)>,
    /// Whether a file is a mask, drawn in the bar's ink by its alpha, so it
    /// reads on every bar and in every theme. Otherwise it is drawn in its
    /// own colours, the same on every bar.
    pub tint: bool,
}

impl Default for Icons {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            tint: true,
        }
    }
}

impl Icons {
    /// The file `face` is drawn from, if it has one.
    #[must_use]
    pub fn file(&self, face: Face) -> Option<&Path> {
        self.files
            .iter()
            .find(|(drawn, _)| *drawn == face)
            .map(|(_, path)| path.as_path())
    }
}

/// What an agent may do on the seat, beyond acting on what it was granted:
/// the `[agent]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// Which programs an agent may start for itself, with the `launch`
    /// tool, and act on: `"off"`, `"any"`, or a list of programs, each a
    /// bare name the session's `PATH` finds or a whole path (see
    /// [`Rule::launches`]). What a listed program starts is the agent's too,
    /// so a terminal, a shell or an interpreter listed here admits anything.
    /// Off in both profiles: the first thing an agent can put on the
    /// person's screen, so the person names it.
    pub launch: Rule,
    /// Where the session serves the agent interface, for an agent inside it.
    /// Read once, before anything starts: a change waits for the next login.
    pub socket: AgentSocket,
    /// The programs no agent is ever given, whatever grants them: those
    /// that exist so the person can oversee an agent, which an agent driving
    /// them would oversee itself. Each a bare name, matched wherever the
    /// program is, or a whole path. ennius in both profiles, whose review
    /// queue holds back the agent's own writes; `[]` gives none. The
    /// settings app is never an agent's whatever this says (see
    /// [`SETTINGS_APP`]), so it is not written here.
    pub never: Never,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            launch: Rule::Off,
            socket: AgentSocket::Unsaid,
            never: Never::new(vec![Program::parse("ennius")]),
        }
    }
}

/// `[agent] socket`: whether the session serves the agent interface on a
/// socket, and where. What it says goes over `--mcp-socket`, so a session a
/// display manager starts, which no flag reaches, can have one, and a person
/// can be sure theirs has none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentSocket {
    /// Not written: `--mcp-socket` decides.
    #[default]
    Unsaid,
    /// `false`: none, though `--mcp-socket` names one.
    Off,
    /// `true`: where `--mcp-socket` says, or else `perspicax-mcp` in
    /// `$XDG_RUNTIME_DIR`.
    On,
    /// A whole path: there, wherever `--mcp-socket` says.
    At(PathBuf),
}

/// A program to start once the session is up: an `autostart` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The program and its arguments.
    pub command: Vec<String>,
    /// Whether an agent may act on it, as `{ spawn = [...], agent = true }`
    /// says: its process is then granted until it exits. Otherwise it is the
    /// person's, as anything they start is.
    pub agent: bool,
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

impl Profile {
    pub const ALL: [Self; 2] = [Self::Classic, Self::Minimal];

    /// Its name as `profile` writes it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Minimal => "minimal",
        }
    }
}

/// Everything a config file decides, with the profile applied and every
/// value validated.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub profile: Profile,
    pub focus: Focus,
    pub bindings: Bindings,
    /// What a mouse button or the wheel does, by where the pointer is: the
    /// `[mouse]` tables, over what the profile binds. Minimal binds the
    /// wheel over a titlebar, up to roll the window up and down to unroll
    /// it, as Openbox does; classic binds nothing. What a press on a frame,
    /// a middle-drag of a title and the drag modifier do is built in, and a
    /// binding here comes before them.
    pub mouse: MouseBindings,
    pub keyboard: Keyboard,
    pub pointer: Pointer,
    /// Per-output settings, matched by connector name (`DP-1`, `HDMI-A-1`).
    /// An output with no rule is lit at its preferred mode, to the right of
    /// the others. Where each one goes is resolved by
    /// [`perspicax_policy::arrange`] once every monitor's size is known.
    pub outputs: Vec<OutputRule>,
    /// Programs to start once the session is up, in order. Each is the
    /// person's, unless the entry says an agent may act on it.
    pub autostart: Vec<Start>,
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
    /// How a workspace switch is drawn while it happens: a slide, a
    /// crossfade, a wipe, a carousel, or nothing. Not part of `workspaces`,
    /// whose every change reshapes them.
    pub transitioning: Transitioning,
    /// Dragging a window to an edge of the desk to give it half or a quarter
    /// of a monitor.
    pub snapping: Snapping,
    /// How hard the edges of screens and panels hold a window being moved
    /// against them.
    pub resistance: Resistance,
    /// How see-through windows are drawn: each application's, how far the
    /// keys move one, and how far a window without the keyboard is dimmed.
    /// For the person's eyes only: a see-through window still covers what is
    /// behind it, as far as an agent is told.
    pub opacity: Opacity,
    /// Whether a program's next window opens where its last one closed, and
    /// the programs whose never does: the `[placement]` table. Where they
    /// closed is the person's history, kept apart from this file, as state.
    pub remembering: Remembering,
    /// Who draws a window's titlebar and border, and what they look like.
    /// Its colours are the theme's titlebar colours.
    pub decorations: Decorations,
    /// The pictures the titlebar's buttons are drawn from, where a person
    /// has their own: kept apart from `decorations`, which is `Copy`.
    pub icons: Icons,
    /// What everything perspicax draws looks like: the `[theme]` table.
    pub theme: Theme,
    /// Whether a session shows the splash as it starts -- the `splash`
    /// colour and the theme's logo, where the desktop will be -- until the
    /// desktop is drawn over it: the `[splash]` table. On wherever there is a
    /// seat; saying `true` in a build without one is an error.
    pub splash: bool,
    /// Whether X11 applications get an Xwayland. Only meaningful in a build
    /// with the `xwayland` feature; saying `true` in one without it is an
    /// error.
    pub xwayland: bool,
    /// Which programs may use the protocols that reach past their own
    /// windows: taskbars, pagers, screenshot and display tools.
    pub protocols: Access,
    /// What an agent may do on the seat: the `[agent]` table.
    pub agent: Agent,
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

/// `double-click-ms` as written, if it is in [`limits::DOUBLE_CLICK_MS`].
fn double_click_ms(written: u32) -> Result<u32, Error> {
    if limits::DOUBLE_CLICK_MS.contains(&written) {
        return Ok(written);
    }
    Err(invalid(
        "input.pointer.double-click-ms".to_owned(),
        format!(
            "{written} is outside {} to {} milliseconds",
            limits::DOUBLE_CLICK_MS.start(),
            limits::DOUBLE_CLICK_MS.end()
        ),
    ))
}

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
    /// Not TOML, or not this schema, at a line and column of the file where
    /// toml can say where. Shown as toml shows it: where, the line, a mark
    /// under what is wrong, and what is.
    #[error("{shown}")]
    Parse {
        /// What toml says, whole, as it shows it.
        shown: String,
        /// What is wrong, in a line.
        message: String,
        /// The line and the column, each counted from 1, where toml can
        /// say.
        at: Option<(usize, usize)>,
    },
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

        // An agent, in both profiles: Logo+A lets one act on the focused
        // window's program, or stops it, and Logo+Shift+A takes back
        // everything it was let act on.
        bindings = bindings
            .bind(logo(Keysym::a), Action::ToggleAgent)
            .bind(logo_shift(Keysym::a), Action::RevokeAgents);

        // The volume keys, in both profiles: the shell's to carry out, and
        // dropped, with nothing said, when `[shell] enabled = false`.
        for (key, volume) in [
            (Keysym::XF86_AudioRaiseVolume, VolumeKey::Up),
            (Keysym::XF86_AudioLowerVolume, VolumeKey::Down),
            (Keysym::XF86_AudioMute, VolumeKey::Mute),
            (Keysym::XF86_AudioMicMute, VolumeKey::MicMute),
        ] {
            let chord = Chord {
                mods: Mods::default(),
                key,
            };
            bindings = bindings.bind(chord, Action::Volume(volume));
        }

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
            mouse: mouse(profile),
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
            // Nothing see-through in either: it is a taste, and asked for.
            opacity: Opacity::default(),
            // Both: a Wayland window cannot ask where to open, so it opens
            // where its program's last one was left.
            remembering: Remembering::default(),
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
            // Both slide: a switch that shows which way it went is the more
            // use where minimal's grid wraps and its edges flip.
            transitioning: Transitioning::default(),
            decorations: decorations(profile),
            icons: Icons::default(),
            theme: Theme::default(),
            splash: built.seat,
            xwayland: built.xwayland,
            protocols: protocols(built),
            agent: Agent::default(),
            shell: Shell::profile(profile, ShellBuilt::FULL),
        }
    }
}

/// How a profile decorates windows, before the `[decorations]` table: the
/// same frame in both, and their titlebars' buttons.
fn decorations(profile: Profile) -> Decorations {
    use FrameButton::{Close, Maximize, Minimize, OnTop, Shade, Sticky};
    match profile {
        // Plasma's rounded corners, and a shadow under every window that
        // shows which is over which.
        Profile::Classic => Decorations {
            radius: 5,
            shadow: Shadow {
                size: 18,
                offset: (0, 4),
                opacity: 50,
                colour: Colour::rgb(0, 0, 0),
            },
            // A roll the eye can follow, and a light that comes up rather
            // than snapping on.
            animation: Animation {
                shade_ms: 150,
                hover_ms: 80,
            },
            ..Decorations::default()
        },
        // Fluxbox's own titlebar, Stick at the left and the rest at the
        // right, with the shade button the wheel over the title also works;
        // and beside Stick, keeping the window on top (#64).
        Profile::Minimal => Decorations {
            buttons: Buttons::new(&[Sticky, OnTop], &[Shade, Minimize, Maximize, Close])
                .expect("six buttons, once each"),
            ..Decorations::default()
        },
    }
}

/// What a profile binds the mouse to, before the `[mouse]` tables.
fn mouse(profile: Profile) -> MouseBindings {
    let wheel = |wheel| MouseChord {
        mods: Mods::default(),
        gesture: Gesture::Wheel(wheel),
    };
    match profile {
        // Plasma and Windows do nothing with the wheel over a titlebar.
        Profile::Classic => MouseBindings::default(),
        // Openbox rolls a window up with the wheel over its titlebar, and
        // down again. A double-click on the title still maximizes, which is
        // built in, not bound.
        Profile::Minimal => MouseBindings::default()
            .bind(
                Context::Titlebar,
                wheel(perspicax_policy::Wheel::Up),
                Action::Shade,
            )
            .bind(
                Context::Titlebar,
                wheel(perspicax_policy::Wheel::Down),
                Action::Unshade,
            ),
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

/// The settings app's program. It edits this file, and the file is what
/// holds every agent back, so no agent is ever handed it: the config
/// refuses to mark it for one, and `perspicax-index` refuses to act on its
/// windows whatever was granted.
pub use perspicax_policy::SETTINGS_APP;
pub(crate) use perspicax_policy::is_settings_app;

/// Why `perspicax-settings` may not be an agent's, and what to do instead.
fn no_agent_settings(remedy: &str) -> String {
    format!(
        "`{SETTINGS_APP}` edits this file, which is what holds an agent back, so no agent may \
         be given it; {remedy}"
    )
}

/// Why a program `[agent] never` names, as `program` there, may not be an
/// agent's, and what to do instead.
fn never_an_agents(program: &str, remedy: &str) -> String {
    format!("`{program}` is in `[agent] never`, so no agent may be given it; {remedy}")
}

/// The `[protocols]` defaults, the same in both profiles: a profile is a
/// window manager's habits, not a security posture. Listing windows and
/// workspaces is open to any client, as every panel expects. Reading pixels,
/// moving monitors, speaking for the desktop shell and hearing the chord a
/// person presses to bind are for the programs that are known to do it, by
/// name, which anything can claim; a person who wants it tighter writes full
/// paths. The settings app reads the monitors it lists on its Outputs page:
/// it writes `[[output]]` already, so that grants it nothing new.
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
            only(&[
                "wlr-randr",
                "kanshi",
                "wdisplays",
                "nwg-displays",
                SETTINGS_APP,
            ]),
        )
        .with(Protocol::Shell, only(&["perspicax-shell"]))
        .with(Protocol::Settings, only(&[SETTINGS_APP]))
}

/// Set in the environment of every program a session on a seat starts: the
/// config file the session read. Empty for a session that read none.
pub const SESSION_CONFIG: &str = "PERSPICAX_CONFIG";

/// The config a program in the session reads: the session's own
/// ([`SESSION_CONFIG`]), or else the one a session would read by default.
/// `perspicax lock`, `perspicax config check` and the settings app each find
/// it so.
///
/// `var` reads the environment.
#[must_use]
pub fn session_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    var(SESSION_CONFIG)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(default_path)
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

/// Where perspicax keeps what it remembers between sessions, which the
/// person never writes: `$XDG_STATE_HOME/perspicax`, or
/// `~/.local/state/perspicax` with no state folder set. `None` with neither
/// variable set, or only relative ones. Both processes keep files there:
/// the session's log and where windows last closed, and the mixer's.
///
/// `var` reads the environment.
#[must_use]
pub fn state_folder(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    // The XDG spec's rule: a relative folder is no folder at all.
    let folder = |name: &str| var(name).map(PathBuf::from).filter(|dir| dir.is_absolute());
    let state = folder("XDG_STATE_HOME")
        .or_else(|| folder("HOME").map(|home| home.join(".local/state")))?;
    Some(state.join("perspicax"))
}

/// Where a file `written` in the config file is: an image, a menu's icon
/// folder. `~/` is the home folder, and a relative path is beside the config
/// file, so a person can keep a wallpaper next to the config that names it.
/// Read by both processes, so a path means the same thing to each.
#[must_use]
pub fn locate(written: &Path, config: Option<&Path>, home: Option<&Path>) -> PathBuf {
    if let (Ok(rest), Some(home)) = (written.strip_prefix("~"), home) {
        return home.join(rest);
    }
    match config.and_then(Path::parent) {
        Some(beside) if written.is_relative() => beside.join(written),
        _ => written.to_owned(),
    }
}

/// Where one of perspicax's own programs is: `name` in `beside`, the folder
/// the running binary is in, where `cargo build` and a package both put the
/// pair; then in the first folder of `path` that has one. Only a file that
/// may be run counts.
#[must_use]
pub fn find_program(name: &str, beside: Option<&Path>, path: Option<&OsStr>) -> Option<PathBuf> {
    let on_path = path.into_iter().flat_map(std::env::split_paths);
    beside
        .map(Path::to_owned)
        .into_iter()
        .chain(on_path)
        .map(|folder| folder.join(name))
        .find(|candidate| runnable(candidate))
}

/// Where the program a command names would be run from, as an exec finds it:
/// a name with a `/` in it as written, and a bare name in the first folder of
/// `path` that holds one. Only a file that may be run counts, and only a
/// folder `path` names by a whole path: a relative one is somewhere else for
/// every program that reads it.
///
/// For a program somebody else names -- a lock program, an MCP server
/// another client starts -- where [`find_program`] is for one of perspicax's
/// own.
#[must_use]
pub fn which(command: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if command.contains('/') {
        let named = PathBuf::from(command);
        return runnable(&named).then_some(named);
    }
    if command.is_empty() {
        return None;
    }
    path.into_iter()
        .flat_map(std::env::split_paths)
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(command))
        .find(|candidate| runnable(candidate))
}

/// Whether `path` is a file that may be run.
fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
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
    let raw: Raw = toml::from_str(text).map_err(|error| unparsed(text, &error))?;
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
    let raw: Raw = toml::from_str(text).map_err(|error| unparsed(text, &error))?;
    raw.apply(built)
}

impl Error {
    /// Why, in a line, for a notice the person reads at a glance; the log
    /// has room for the whole of it. Of a file that is not TOML or not this
    /// schema, the line and the column, then what is wrong there; of the
    /// rest, what they show, which is already a line.
    #[must_use]
    pub fn in_a_line(&self) -> String {
        match self {
            Self::Parse {
                message,
                at: Some((line, column)),
                ..
            } => format!("line {line}, column {column}: {message}"),
            Self::Parse { message, .. } => message.clone(),
            error => error.to_string(),
        }
    }
}

/// `error`, from reading `text` as this schema, as an [`Error::Parse`]: with
/// the line and the column its span starts at, counted as toml counts them
/// when it shows where.
fn unparsed(text: &str, error: &toml::de::Error) -> Error {
    let at = error.span().map(|span| {
        let before = text.get(..span.start).unwrap_or(text);
        let line = before.matches('\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count()
            + 1;
        (line, column)
    });
    let message = error
        .message()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Error::Parse {
        shown: error.to_string(),
        message,
        at,
    }
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
    autostart: Vec<RawStart>,
    xdg_autostart: Option<bool>,
    xwayland: Option<bool>,
    workspaces: Option<RawWorkspaces>,
    snap: Option<RawSnap>,
    resistance: Option<RawResistance>,
    opacity: Option<RawOpacity>,
    placement: Option<RawPlacement>,
    decorations: Option<RawDecorations>,
    theme: Option<RawTheme>,
    splash: Option<RawSplash>,
    protocols: Option<RawProtocols>,
    agent: Option<RawAgent>,
    shell: Option<RawShell>,
}

/// The `[agent]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawAgent {
    launch: Option<RawRule>,
    socket: Option<RawSocket>,
    never: Option<Vec<String>>,
}

/// `true`, `false`, or a path.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawSocket {
    Switch(bool),
    At(String),
}

impl RawAgent {
    fn apply(self, mut agent: Agent) -> Result<Agent, Error> {
        // Before `launch`, which may not name what this withholds.
        if let Some(never) = self.never {
            for (at, program) in never.iter().enumerate() {
                if program.is_empty() {
                    return Err(invalid(
                        format!("agent.never[{at}]"),
                        "an empty program name".to_owned(),
                    ));
                }
                if program.contains('/') && !program.starts_with('/') {
                    return Err(invalid(
                        format!("agent.never[{at}]"),
                        format!(
                            "`{program}` is a path from nowhere in particular; write the whole \
                             path, from /, or a bare name to match wherever it is"
                        ),
                    ));
                }
            }
            agent.never = Never::new(
                never
                    .iter()
                    .map(|program| Program::parse(program))
                    .collect(),
            );
        }
        if let Some(launch) = self.launch {
            let rule = launch.rule("agent.launch")?;
            for (at, program) in rule.named().into_iter().enumerate() {
                if is_settings_app(program) {
                    return Err(invalid(
                        format!("agent.launch[{at}]"),
                        no_agent_settings("take it out of the list"),
                    ));
                }
                if program.contains('/') && !program.starts_with('/') {
                    return Err(invalid(
                        format!("agent.launch[{at}]"),
                        format!(
                            "`{program}` is a path from nowhere in particular; write the whole \
                             path, from /, or a bare name for the session's PATH to find"
                        ),
                    ));
                }
                if let Some(withheld) = agent.never.naming(program) {
                    return Err(invalid(
                        format!("agent.launch[{at}]"),
                        never_an_agents(withheld, "take it out of one list or the other"),
                    ));
                }
            }
            agent.launch = rule;
        }
        if let Some(socket) = self.socket {
            agent.socket = match socket {
                RawSocket::Switch(true) => AgentSocket::On,
                RawSocket::Switch(false) => AgentSocket::Off,
                RawSocket::At(path) if path.starts_with('/') => AgentSocket::At(path.into()),
                RawSocket::At(path) if path.is_empty() => {
                    return Err(invalid(
                        "agent.socket".to_owned(),
                        "an empty path; write true, false, or a whole path".to_owned(),
                    ));
                }
                RawSocket::At(path) => {
                    return Err(invalid(
                        "agent.socket".to_owned(),
                        format!(
                            "`{path}` is a path from nowhere in particular; write the whole \
                             path, from /, or true for $XDG_RUNTIME_DIR/perspicax-mcp"
                        ),
                    ));
                }
            };
        }
        Ok(agent)
    }
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

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawMotion {
    Normal,
    Reduced,
}

/// The `[splash]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawSplash {
    enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawTheme {
    name: Option<String>,
    logo: Option<String>,
    font: Option<String>,
    font_size: Option<u16>,
    cursor: Option<String>,
    cursor_size: Option<u32>,
    color_scheme: Option<RawColorScheme>,
    contrast: Option<RawContrast>,
    motion: Option<RawMotion>,
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
    settings: Option<RawRule>,
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
    left: Option<Vec<RawButton>>,
    right: Option<Vec<RawButton>>,
    corner_radius: Option<i32>,
    agent: Option<RawAgentLook>,
    icons: Option<RawIcons>,
    shadow: Option<RawShadow>,
    animation: Option<RawAnimation>,
}

/// The `[decorations.animation]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawAnimation {
    shade_ms: Option<u32>,
    hover_ms: Option<u32>,
}

/// The `[decorations.shadow]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawShadow {
    size: Option<i32>,
    offset: Option<[i32; 2]>,
    opacity: Option<i32>,
    colour: Option<String>,
}

/// The `[decorations.icons]` table: a file for each face, by what a press
/// of the button then does.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawIcons {
    close: Option<String>,
    minimize: Option<String>,
    maximize: Option<String>,
    restore: Option<String>,
    shade: Option<String>,
    unshade: Option<String>,
    stick: Option<String>,
    unstick: Option<String>,
    on_top: Option<String>,
    off_top: Option<String>,
    tint: Option<bool>,
}

impl RawIcons {
    fn apply(self, built: Built) -> Result<Icons, Error> {
        let written = [
            (Face::Close, self.close),
            (Face::Minimize, self.minimize),
            (Face::Maximize, self.maximize),
            (Face::Restore, self.restore),
            (Face::Shade, self.shade),
            (Face::Unshade, self.unshade),
            (Face::Stick, self.stick),
            (Face::Unstick, self.unstick),
            (Face::OnTop, self.on_top),
            (Face::OffTop, self.off_top),
        ];
        let mut files = Vec::new();
        for (face, path) in written {
            match path {
                None => {}
                Some(path) if path.trim().is_empty() => {
                    return Err(invalid(
                        format!("decorations.icons.{}", face.key()),
                        "an empty path; leave it out for the built-in shape".to_owned(),
                    ));
                }
                Some(path) if !built.svg && is_svg(Path::new(&path)) => {
                    return Err(Error::NotBuilt {
                        key: icon_key(face),
                        feature: "svg",
                    });
                }
                Some(path) => files.push((face, PathBuf::from(path))),
            }
        }
        Ok(Icons {
            files,
            tint: self.tint.unwrap_or(true),
        })
    }
}

/// A titlebar button, as `[decorations] left` and `right` name it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawButton {
    Shade,
    Sticky,
    OnTop,
    Minimize,
    Maximize,
    Close,
}

impl From<RawButton> for FrameButton {
    fn from(button: RawButton) -> Self {
        match button {
            RawButton::Shade => Self::Shade,
            RawButton::Sticky => Self::Sticky,
            RawButton::OnTop => Self::OnTop,
            RawButton::Minimize => Self::Minimize,
            RawButton::Maximize => Self::Maximize,
            RawButton::Close => Self::Close,
        }
    }
}

/// The `[decorations.agent]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawAgentLook {
    focused: Option<String>,
    unfocused: Option<String>,
    outline: Option<i32>,
    outline_colour: Option<String>,
    refused_colour: Option<String>,
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

/// Read wider than a percent, so that a value out of range is refused by its
/// key rather than by serde.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawOpacity {
    step: Option<i64>,
    floor: Option<i64>,
    unfocused: Option<i64>,
    /// By app id or `WM_CLASS` class, which no schema could list.
    apps: Option<BTreeMap<String, i64>>,
}

/// The `[placement]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawPlacement {
    remember: Option<bool>,
    /// By app id or `WM_CLASS` class, which no schema could list.
    forget: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawWorkspaces {
    mode: Option<RawSpread>,
    grid: Option<[u16; 2]>,
    wrap: Option<bool>,
    flip: Option<RawFlip>,
    transition: Option<RawTransition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawFlip {
    edge: Option<bool>,
    delay_ms: Option<u64>,
    while_dragging: Option<bool>,
    scroll: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawTransition {
    effect: Option<RawEffect>,
    duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawEffect {
    None,
    Slide,
    Crossfade,
    Wipe,
    Carousel,
}

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
                let action = action_for(action, pies, &config.agent.never)
                    .map_err(|reason| invalid(key.clone(), reason))?;
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

/// Why a `[keys]` entry for an escape hatch is refused. The compositor takes
/// those chords before it looks at a binding, so one written here could
/// never fire, and a `"none"` could never hand the chord to the client.
fn hatch_taken(trigger: keys::Trigger) -> Option<String> {
    let keys::Trigger::Chord(chord) = trigger else {
        return None;
    };
    let does = match perspicax_policy::hatch(chord.mods, [chord.key])? {
        Hatch::Exit => "ends the session".to_owned(),
        Hatch::Vt(vt) => format!("switches to VT {vt}"),
    };
    Some(format!(
        "{does} before any binding is looked at, so no config can bind or unbind it; choose \
         another chord"
    ))
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
    /// Whether an agent may act on what it starts.
    #[serde(default)]
    agent: bool,
}

/// An `autostart` entry: a program and its arguments, or the same as
/// `{ spawn = [...] }`, which may say `agent = true`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawStart {
    Command(Vec<String>),
    Spawn(RawSpawn),
}

impl From<RawStart> for Start {
    fn from(start: RawStart) -> Self {
        match start {
            RawStart::Command(command) => Self {
                command,
                agent: false,
            },
            RawStart::Spawn(RawSpawn { spawn, agent }) => Self {
                command: spawn,
                agent,
            },
        }
    }
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
            if self.agent.is_some() {
                return Err(Error::NotBuilt {
                    key: "agent",
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
        if let Some(enabled) = self.splash.and_then(|splash| splash.enabled) {
            if enabled && !built.seat {
                return Err(Error::NotBuilt {
                    key: "splash.enabled",
                    feature: "seat",
                });
            }
            config.splash = enabled;
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

        // Before the bindings and `autostart`, which may not give an agent
        // what `[agent] never` names.
        if let Some(agent) = self.agent {
            config.agent = agent.apply(config.agent)?;
        }

        // The pies a binding may open, before the bindings are read and
        // `[shell]` after them.
        let pies = self
            .shell
            .as_ref()
            .map(RawShell::pie_names)
            .unwrap_or_default();
        // The first key written to change the sound, to refuse with the
        // shell off.
        let mut volume_written = None;
        let mut written_as = Vec::new();
        for (written, action) in self.keys {
            let key = format!("keys.{written}");
            let trigger = keys::trigger(&written).map_err(|reason| invalid(key.clone(), reason))?;
            if let Some(reason) = hatch_taken(trigger) {
                return Err(invalid(key, reason));
            }
            if let Some((_, first)) = written_as.iter().find(|(seen, _)| *seen == trigger) {
                return Err(invalid(
                    key,
                    format!("is the same as `{first}`, already written; write it once"),
                ));
            }
            let action = action_for(action, &pies, &config.agent.never)
                .map_err(|reason| invalid(key.clone(), reason))?;
            if matches!(action, Some(Action::Volume(_))) && volume_written.is_none() {
                volume_written = Some(key);
            }
            written_as.push((trigger, written));
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
                    && !limits::ACCEL.contains(&accel)
                {
                    return Err(invalid(
                        "input.pointer.accel".to_owned(),
                        format!(
                            "{accel} is outside {} to {}",
                            limits::ACCEL.start(),
                            limits::ACCEL.end()
                        ),
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
                let side = limits::GRID_SIDE;
                if !side.contains(&columns) || !side.contains(&rows) {
                    return Err(invalid(
                        "workspaces.grid".to_owned(),
                        format!(
                            "[{columns}, {rows}] is not a grid; each side is {} to {} \
                             workspaces, columns first",
                            side.start(),
                            side.end()
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
                    if !limits::FLIP_DELAY_MS.contains(&delay) {
                        return Err(invalid(
                            "workspaces.flip.delay-ms".to_owned(),
                            format!(
                                "{delay} is more than {} milliseconds",
                                limits::FLIP_DELAY_MS.end()
                            ),
                        ));
                    }
                    config.flipping.delay_ms = delay;
                }
                let flipping = &mut config.flipping;
                flipping.edge = flip.edge.unwrap_or(flipping.edge);
                flipping.while_dragging = flip.while_dragging.unwrap_or(flipping.while_dragging);
                flipping.scroll = flip.scroll.unwrap_or(flipping.scroll);
            }
            if let Some(transition) = workspaces.transition {
                if let Some(effect) = transition.effect {
                    config.transitioning.effect = match effect {
                        RawEffect::None => perspicax_policy::Effect::None,
                        RawEffect::Slide => perspicax_policy::Effect::Slide,
                        RawEffect::Crossfade => perspicax_policy::Effect::Crossfade,
                        RawEffect::Wipe => perspicax_policy::Effect::Wipe,
                        RawEffect::Carousel => perspicax_policy::Effect::Carousel,
                    };
                }
                if let Some(length) = transition.duration_ms {
                    let key = "workspaces.transition.duration-ms".to_owned();
                    if length == 0 {
                        return Err(invalid(
                            key,
                            "0 is no transition: write effect = \"none\"".to_owned(),
                        ));
                    }
                    if !limits::TRANSITION_MS.contains(&length) {
                        return Err(invalid(
                            key,
                            format!(
                                "{length} is more than {} milliseconds",
                                limits::TRANSITION_MS.end()
                            ),
                        ));
                    }
                    config.transitioning.duration_ms = Some(length);
                }
            }
        }

        if let Some(snap) = self.snap {
            if let Some(threshold) = snap.threshold {
                let range = limits::SNAP_THRESHOLD;
                if !range.contains(&threshold) {
                    return Err(invalid(
                        "snap.threshold".to_owned(),
                        format!(
                            "{threshold} is outside {} to {} pixels",
                            range.start(),
                            range.end()
                        ),
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
                let range = limits::RESISTANCE;
                if !range.contains(&value) {
                    return Err(invalid(
                        format!("resistance.{key}"),
                        format!(
                            "{value} is outside {} to {} pixels",
                            range.start(),
                            range.end()
                        ),
                    ));
                }
                *into = value;
            }
        }

        if let Some(opacity) = self.opacity {
            config.opacity = opacity.apply(config.opacity)?;
        }

        if let Some(placement) = self.placement {
            config.remembering = placement.apply(config.remembering)?;
        }

        config.theme = theme(self.theme, self.decorations.as_ref())?;
        if let Some(mut decorations) = self.decorations {
            if let Some(icons) = decorations.icons.take() {
                config.icons = icons.apply(built)?;
            }
            config.decorations = decorations.apply(config.decorations)?;
        }
        let palette = &config.theme.palette;
        config.decorations.focused = palette[Role::TitleFocused].colour();
        config.decorations.focused_ink = palette[Role::TitleFocusedInk].colour();
        config.decorations.unfocused = palette[Role::TitleUnfocused].colour();
        config.decorations.unfocused_ink = palette[Role::TitleUnfocusedInk].colour();
        config.decorations.close_hover = palette[Role::TitleCloseHover].colour();
        // Less motion is every animation at once, whatever each one's own
        // table says: a person troubled by motion should not have to find
        // them all.
        if config.theme.motion == Motion::Reduced {
            config.transitioning.effect = Effect::None;
            config.decorations.animation = Animation::default();
        }

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
        // The volume is the shell's to change: with it off, a key written to
        // change it is a mistake, and the profile's are given back to the
        // programs.
        if !config.shell.enabled {
            if let Some(written) = volume_written {
                return Err(invalid(
                    written,
                    "the volume is the shell's to change, and `[shell] enabled = false`".to_owned(),
                ));
            }
            config.bindings = config
                .bindings
                .without(|action| matches!(action, Action::Volume(_)));
        }
        config.shell.double_click_ms = config.pointer.double_click_ms;
        config.shell.palette = config.theme.palette;
        config.shell.font = config.theme.font.clone();

        let starts: Vec<Start> = self.autostart.into_iter().map(Start::from).collect();
        if let Some(empty) = starts.iter().position(|start| start.command.is_empty()) {
            return Err(invalid(
                format!("autostart[{empty}]"),
                "an empty command".to_owned(),
            ));
        }
        if let Some(settings) = starts
            .iter()
            .position(|start| start.agent && is_settings_app(&start.command[0]))
        {
            return Err(invalid(
                format!("autostart[{settings}]"),
                no_agent_settings("drop `agent = true`"),
            ));
        }
        for (at, start) in starts.iter().enumerate().filter(|(_, start)| start.agent) {
            if let Some(withheld) = config.agent.never.naming(&start.command[0]) {
                return Err(invalid(
                    format!("autostart[{at}]"),
                    never_an_agents(withheld, "drop `agent = true`, or take it out of the list"),
                ));
            }
        }
        config.autostart = starts;
        Ok(config)
    }
}

impl RawOpacity {
    fn apply(self, mut opacity: Opacity) -> Result<Opacity, Error> {
        let range = limits::OPACITY;
        let percent = |key: String, value: i64| {
            u8::try_from(value)
                .ok()
                .filter(|percent| range.contains(percent))
                .ok_or_else(|| {
                    invalid(
                        key,
                        format!(
                            "{value} is outside {} to {} percent",
                            range.start(),
                            range.end()
                        ),
                    )
                })
        };
        for (key, value, into) in [
            ("step", self.step, &mut opacity.step),
            ("floor", self.floor, &mut opacity.floor),
            ("unfocused", self.unfocused, &mut opacity.unfocused),
        ] {
            if let Some(value) = value {
                *into = percent(format!("opacity.{key}"), value)?;
            }
        }
        // The floor keeps a window from vanishing under the keys. Dimming
        // multiplies, so a window could still go fainter, but not by less
        // than the floor allows the keys.
        if opacity.unfocused < opacity.floor {
            return Err(invalid(
                "opacity.unfocused".to_owned(),
                format!(
                    "{} is below the floor, {}; dim no further than the keys can go",
                    opacity.unfocused, opacity.floor
                ),
            ));
        }
        for (app, value) in self.apps.unwrap_or_default() {
            if app.is_empty() {
                return Err(invalid(
                    "opacity.apps".to_owned(),
                    "an application is named \"\"; name it by its app id or WM_CLASS".to_owned(),
                ));
            }
            let key = format!("opacity.apps.{app}");
            let value = percent(key.clone(), value)?;
            // Below it, opacity-down would take the window up to the floor.
            if value < opacity.floor {
                return Err(invalid(
                    key,
                    format!("{value} is below the floor, {}", opacity.floor),
                ));
            }
            opacity.apps.insert(app, value);
        }
        Ok(opacity)
    }
}

impl RawPlacement {
    fn apply(self, mut remembering: Remembering) -> Result<Remembering, Error> {
        remembering.on = self.remember.unwrap_or(remembering.on);
        for app in self.forget.unwrap_or_default() {
            if app.is_empty() {
                return Err(invalid(
                    "placement.forget".to_owned(),
                    "an application is named \"\"; name it by its app id or WM_CLASS".to_owned(),
                ));
            }
            remembering.forget.insert(app);
        }
        Ok(remembering)
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
            decorations.title = pixels("title-height", title, limits::TITLE_HEIGHT)?;
        }
        if let Some(border) = self.border {
            decorations.border = pixels("border", border, limits::BORDER)?;
        }
        if let Some(radius) = self.corner_radius {
            decorations.radius = pixels("corner-radius", radius, limits::CORNER_RADIUS)?;
        }
        if let Some(written) = self.animation {
            let length = |key: &str, ms: u32| {
                if limits::ANIMATION_MS.contains(&ms) {
                    Ok(ms)
                } else {
                    Err(invalid(
                        format!("decorations.animation.{key}"),
                        format!(
                            "{ms} is more than {} milliseconds; 0 is none",
                            limits::ANIMATION_MS.end()
                        ),
                    ))
                }
            };
            if let Some(ms) = written.shade_ms {
                decorations.animation.shade_ms = length("shade-ms", ms)?;
            }
            if let Some(ms) = written.hover_ms {
                decorations.animation.hover_ms = length("hover-ms", ms)?;
            }
        }
        if let Some(written) = self.shadow {
            let shadow = &mut decorations.shadow;
            if let Some(size) = written.size {
                shadow.size = pixels("shadow.size", size, limits::SHADOW_SIZE)?;
            }
            if let Some([x, y]) = written.offset {
                shadow.offset = (
                    pixels("shadow.offset", x, limits::SHADOW_OFFSET)?,
                    pixels("shadow.offset", y, limits::SHADOW_OFFSET)?,
                );
            }
            if let Some(opacity) = written.opacity {
                let range = limits::SHADOW_OPACITY;
                shadow.opacity = u8::try_from(opacity)
                    .ok()
                    .filter(|_| range.contains(&opacity))
                    .ok_or_else(|| {
                        invalid(
                            "decorations.shadow.opacity".to_owned(),
                            format!(
                                "{opacity} is outside {} to {} percent",
                                range.start(),
                                range.end()
                            ),
                        )
                    })?;
            }
            if let Some(text) = written.colour {
                shadow.colour = opaque_colour("decorations.shadow.colour", &text)?;
            }
        }
        if self.left.is_some() || self.right.is_some() {
            decorations.buttons = titlebar_buttons(self.left, self.right, decorations.buttons)?;
        }
        // Its colours are the theme's: see `theme`.
        if let Some(agent) = self.agent {
            let look = &mut decorations.agent;
            if let Some(outline) = agent.outline {
                look.outline = pixels("agent.outline", outline, limits::AGENT_OUTLINE)?;
            }
            let written = [
                ("focused", agent.focused, &mut look.focused),
                ("unfocused", agent.unfocused, &mut look.unfocused),
                (
                    "outline-colour",
                    agent.outline_colour,
                    &mut look.outline_colour,
                ),
                (
                    "refused-colour",
                    agent.refused_colour,
                    &mut look.refused_colour,
                ),
            ];
            for (key, text, colour) in written {
                if let Some(text) = text {
                    *colour = opaque_colour(&format!("decorations.agent.{key}"), &text)?;
                }
            }
        }
        Ok(decorations)
    }
}

/// Whether `path` names an SVG, by its extension: how a seat tells which
/// reader an icon needs.
#[must_use]
pub fn is_svg(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

/// The key an icon for `face` is written at, whole, as an error names it.
const fn icon_key(face: Face) -> &'static str {
    match face {
        Face::Close => "decorations.icons.close",
        Face::Minimize => "decorations.icons.minimize",
        Face::Maximize => "decorations.icons.maximize",
        Face::Restore => "decorations.icons.restore",
        Face::Shade => "decorations.icons.shade",
        Face::Unshade => "decorations.icons.unshade",
        Face::Stick => "decorations.icons.stick",
        Face::Unstick => "decorations.icons.unstick",
        Face::OnTop => "decorations.icons.on-top",
        Face::OffTop => "decorations.icons.off-top",
    }
}

/// The titlebar's two ends, as `[decorations] left` and `right` write them
/// over `had`, the profile's.
///
/// A button listed twice in one list is refused at that list's key, and
/// one in both lists at `right`'s. An end left unwritten keeps the
/// profile's buttons there, but for any the written end has taken: under
/// minimal, whose sticky is at the left, `right = ["sticky", "close"]`
/// moves sticky to the right rather than being refused for a sticky the
/// person never wrote at the left.
fn titlebar_buttons(
    left: Option<Vec<RawButton>>,
    right: Option<Vec<RawButton>>,
    had: Buttons,
) -> Result<Buttons, Error> {
    let written = |key: &str, list: Option<Vec<RawButton>>| {
        let Some(list) = list else {
            return Ok(None);
        };
        let list: Vec<FrameButton> = list.into_iter().map(FrameButton::from).collect();
        match shell::twice(&list) {
            Some(twice) => Err(invalid(
                format!("decorations.{key}"),
                format!("\"{}\" is listed twice", twice.key()),
            )),
            None => Ok(Some(list)),
        }
    };
    let (left, right) = (written("left", left)?, written("right", right)?);
    let untaken = |had: Vec<FrameButton>, taken: &[FrameButton]| -> Vec<FrameButton> {
        had.into_iter()
            .filter(|button| !taken.contains(button))
            .collect()
    };
    let left =
        left.unwrap_or_else(|| untaken(had.left().collect(), right.as_deref().unwrap_or(&[])));
    let right = right.unwrap_or_else(|| untaken(had.right().collect(), &left));
    // Each list has each button once, and an end left unwritten has none
    // the other has: a button twice over is in both written lists.
    Buttons::new(&left, &right).map_err(|both| {
        invalid(
            "decorations.right".to_owned(),
            format!("\"{}\" is in decorations.left as well", both.key()),
        )
    })
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
        let range = limits::FONT_SIZE;
        if !range.contains(&size) {
            return Err(invalid(
                "theme.font-size".to_owned(),
                format!(
                    "{size} is outside {} to {} pixels",
                    range.start(),
                    range.end()
                ),
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
        && !limits::CURSOR_SIZE.contains(&size)
    {
        return Err(invalid(
            "theme.cursor-size".to_owned(),
            format!(
                "{size} is outside {} to {} pixels",
                limits::CURSOR_SIZE.start(),
                limits::CURSOR_SIZE.end()
            ),
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

    let logo = match written.logo.as_deref() {
        None | Some("perspicax") => Logo::Perspicax,
        Some("none") => Logo::None,
        Some(path) if path.trim().is_empty() => {
            return Err(invalid(
                "theme.logo".to_owned(),
                "an empty path; leave it out for Perspicax's lockup, or write \"none\" for the \
                 colour alone"
                    .to_owned(),
            ));
        }
        Some(path) => Logo::File(path.into()),
    };

    Ok(Theme {
        builtin,
        palette: builtin.palette().written_over(&colours),
        font,
        cursor: written.cursor,
        cursor_size: written.cursor_size,
        apps,
        logo,
        motion: match written.motion {
            None | Some(RawMotion::Normal) => Motion::Normal,
            Some(RawMotion::Reduced) => Motion::Reduced,
        },
    })
}

/// A colour written for `role` at `at`: opaque, unless the role is one of the
/// few drawn see-through.
/// A colour that is drawn opaque, at `at`: one of the agent's marks, which
/// the compositor says cover what is under them.
fn opaque_colour(at: &str, text: &str) -> Result<Colour, Error> {
    match Rgba::parse(text) {
        Some(colour) if colour.is_opaque() => Ok(colour.colour()),
        Some(_) => Err(invalid(
            at.to_owned(),
            format!(
                "{text:?} is see-through, and this is drawn opaque so the compositor can say \
                 what it covers; write it as \"#rrggbb\""
            ),
        )),
        None => Err(invalid(
            at.to_owned(),
            format!("{text:?} is not a colour; write one as \"#rrggbb\""),
        )),
    }
}

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
            (Protocol::Settings, self.settings),
        ];
        for (protocol, raw) in written {
            let Some(raw) = raw else { continue };
            let rule = raw.rule(&format!("protocols.{}", protocol.key()))?;
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
    /// The rule this is, at `key`: a protocol's, or `agent.launch`.
    fn rule(self, key: &str) -> Result<Rule, Error> {
        let key = || key.to_owned();
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
            if limits::REPEAT.contains(&value) {
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
            .map(str::parse)
            .transpose()
            .map_err(|reason| invalid(format!("{key}.mode"), reason))?;
        if let Some(scale) = self.scale
            && !limits::SCALE.contains(&scale)
        {
            return Err(invalid(
                format!("{key}.scale"),
                format!(
                    "{scale} is outside {} to {}",
                    limits::SCALE.start(),
                    limits::SCALE.end()
                ),
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
    /// and an `offset` only with a side to be offset along: each number within
    /// [`limits::OUTPUT_PLACE`].
    fn place(&self, key: &str) -> Result<Place, Error> {
        let sides = [
            (Side::LeftOf, &self.left_of),
            (Side::RightOf, &self.right_of),
            (Side::Above, &self.above),
            (Side::Below, &self.below),
        ];
        let mut written = sides
            .iter()
            .filter_map(|(side, of)| of.as_ref().map(|of| (side.key(), *side, of)));
        let beside = written.next();
        let range = limits::OUTPUT_PLACE;
        let outside = |what: String, word: &str| {
            invalid(
                format!("{key}.{word}"),
                format!(
                    "{what} is outside {} to {} pixels",
                    range.start(),
                    range.end()
                ),
            )
        };
        if let Some([x, y]) = self.position
            && !(range.contains(&x) && range.contains(&y))
        {
            return Err(outside(format!("[{x}, {y}]"), "position"));
        }
        if let Some(offset) = self.offset
            && !range.contains(&offset)
        {
            return Err(outside(offset.to_string(), "offset"));
        }
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
fn action_for(
    action: RawAction,
    pies: &BTreeSet<String>,
    never: &Never,
) -> Result<Option<Action>, String> {
    Spelled::from(action).read(pies, never)
}

impl From<RawAction> for Spelled {
    fn from(action: RawAction) -> Self {
        match action {
            RawAction::Named(name) => Self::Name(name),
            RawAction::Spawn(RawSpawn { spawn, agent }) => Self::Spawn {
                command: spawn,
                agent,
            },
            RawAction::Pie(RawOpenPie { pie }) => Self::Pie(pie),
        }
    }
}

/// `"WxH"` or `"WxH@Hz"`.
/// A mode as `[[output]] mode` writes it: `"2560x1440"`, or with a refresh
/// in Hz, `"2560x1440@143.912"`.
impl std::str::FromStr for Mode {
    /// Why it is not a mode, in the parser's words.
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || format!("`{text}` is not a mode; write it as 2560x1440 or 2560x1440@144");
        let (size, refresh) = match text.split_once('@') {
            Some((size, refresh)) => (size, Some(refresh.parse::<f64>().map_err(|_| bad())?)),
            None => (text, None),
        };
        let (width, height) = size.split_once('x').ok_or_else(bad)?;
        Ok(Self {
            width: width.trim().parse().map_err(|_| bad())?,
            height: height.trim().parse().map_err(|_| bad())?,
            refresh,
        })
    }
}

fn invalid(key: String, reason: String) -> Error {
    Error::Invalid { key, reason }
}

#[cfg(test)]
mod tests {
    use perspicax_policy::{AgentLook, Palette};

    use super::*;

    const SEAT: Built = Built {
        seat: true,
        xwayland: true,
        capture: true,
        svg: true,
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

    /// The settings app writes each choice it offers with the value's
    /// `key()`: every word must be one this parser reads back as that value.
    #[test]
    fn every_word_a_page_offers_reads_back_as_its_value() {
        let read = |text: String| parse(&text, SEAT).unwrap_or_else(|e| panic!("{text}: {e}"));
        let panel = |text: String| read(text).shell.panel.expect("a panel");
        for profile in Profile::ALL {
            let text = format!("profile = \"{}\"", profile.key());
            assert_eq!(read(text).profile, profile);
        }
        for model in FocusModel::ALL {
            let text = format!("[focus]\nmodel = \"{}\"", model.key());
            assert_eq!(read(text).focus.model, model);
        }
        for mode in perspicax_policy::Mode::ALL {
            let text = format!("[workspaces]\nmode = \"{}\"", mode.key());
            assert_eq!(read(text).workspaces.mode, mode);
        }
        for effect in Effect::ALL {
            let text = format!("[workspaces.transition]\neffect = \"{}\"", effect.key());
            assert_eq!(read(text).transitioning.effect, effect);
        }
        for switching in Switching::ALL {
            let text = format!("[input.keyboard]\nswitching = \"{}\"", switching.key());
            assert_eq!(read(text).keyboard.switching, switching);
        }
        for scheme in ColorScheme::ALL {
            let text = format!("[theme]\ncolor-scheme = \"{}\"", scheme.key());
            assert_eq!(read(text).theme.apps.color_scheme, Some(scheme));
        }
        for contrast in Contrast::ALL {
            let text = format!("[theme]\ncontrast = \"{}\"", contrast.key());
            assert_eq!(read(text).theme.apps.contrast, Some(contrast));
        }
        for motion in Motion::ALL {
            let text = format!("[theme]\nmotion = \"{}\"", motion.key());
            assert_eq!(read(text).theme.motion, motion);
        }
        for mode in WallpaperMode::ALL {
            let text = format!("[shell]\nwallpaper-mode = \"{}\"", mode.key());
            assert_eq!(read(text).shell.wallpaper.expect("a wallpaper").mode, mode);
        }
        for untrusted in UntrustedLaunchers::ALL {
            let text = format!("[shell]\nuntrusted-launchers = \"{}\"", untrusted.key());
            assert_eq!(read(text).shell.untrusted_launchers, untrusted);
        }
        for edge in Edge::ALL {
            let text = format!("[shell.panel]\nedge = \"{}\"", edge.key());
            assert_eq!(panel(text).edge, edge);
        }
        for align in Align::ALL {
            let text = format!(
                "[shell.panel]\nwidth = \"50%\"\nalign = \"{}\"",
                align.key()
            );
            assert_eq!(panel(text).align, align);
        }
        for taskbar in TaskbarScope::ALL {
            let text = format!("[shell.panel]\ntaskbar = \"{}\"", taskbar.key());
            assert_eq!(panel(text).taskbar, taskbar);
        }
        for item in Item::ALL {
            let text = format!("[shell.panel]\nitems = [\"{}\"]", item.key());
            assert_eq!(panel(text).items, [item]);
        }
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
            svg: true,
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
            svg: true,
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
            Some(&Action::Spawn {
                command: vec!["foot".to_owned()],
                agent: false
            })
        );
        let (mods, f4) = alt(Keysym::F4);
        assert_eq!(config.bindings.resolve(mods, &f4), None);
    }

    #[test]
    fn an_unknown_action_is_named() {
        let error = parse("[keys]\n\"Alt+x\" = \"explode\"", SEAT).unwrap_err();
        assert!(error.to_string().contains("`explode`"), "{error}");
    }

    /// A fresh folder for one test, emptied when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("perspicax-config-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("a scratch folder");
            Self(dir)
        }

        /// A folder in it holding a program called `name`, with `mode`.
        fn program(&self, folder: &str, name: &str, mode: u32) -> PathBuf {
            use std::os::unix::fs::PermissionsExt as _;
            let folder = self.0.join(folder);
            std::fs::create_dir_all(&folder).expect("a folder");
            let program = folder.join(name);
            std::fs::write(&program, "").expect("a program");
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(mode))
                .expect("its mode");
            folder
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn the_program_beside_perspicax_comes_before_the_one_on_path() {
        let scratch = Scratch::new("beside");
        let beside = scratch.program("bin", "perspicax-shell", 0o755);
        let installed = scratch.program("usr-bin", "perspicax-shell", 0o755);
        let path = std::env::join_paths([&installed]).unwrap();
        assert_eq!(
            find_program("perspicax-shell", Some(&beside), Some(&path)),
            Some(beside.join("perspicax-shell"))
        );
        assert_eq!(
            find_program("perspicax-shell", Some(&scratch.0), Some(&path)),
            Some(installed.join("perspicax-shell")),
            "and with none beside, the one on PATH"
        );
    }

    #[test]
    fn a_file_that_may_not_be_run_is_not_the_program() {
        let scratch = Scratch::new("mode");
        let beside = scratch.program("bin", "perspicax-shell", 0o644);
        assert_eq!(find_program("perspicax-shell", Some(&beside), None), None);
    }

    #[test]
    fn a_command_with_a_slash_is_where_it_says_and_a_bare_one_is_on_path() {
        let scratch = Scratch::new("which");
        let first = scratch.program("first", "swaylock", 0o755);
        let second = scratch.program("second", "swaylock", 0o755);
        let path = std::env::join_paths([&first, &second]).unwrap();
        assert_eq!(which("swaylock", Some(&path)), Some(first.join("swaylock")));
        let named = second.join("swaylock");
        assert_eq!(
            which(named.to_str().unwrap(), None),
            Some(named.clone()),
            "a path needs no PATH"
        );
        let missing = scratch.0.join("nowhere/swaylock");
        assert_eq!(which(missing.to_str().unwrap(), Some(&path)), None);
        assert_eq!(which("", Some(&path)), None);
    }

    #[test]
    fn which_skips_a_folder_path_names_relatively() {
        let scratch = Scratch::new("relative");
        scratch.program("here", "swaylock", 0o755);
        let path = std::env::join_paths(["here"]).unwrap();
        assert_eq!(which("swaylock", Some(&path)), None);
    }

    /// The key and the reason a `[keys]` table is refused for.
    fn keys_refused(table: &str) -> (String, String) {
        match parse(&format!("[keys]\n{table}"), SEAT) {
            Err(Error::Invalid { key, reason }) => (key, reason),
            other => panic!("{table}: {other:?}"),
        }
    }

    #[test]
    fn an_escape_hatch_chord_is_refused_naming_what_it_does() {
        let (key, reason) = keys_refused("\"Ctrl+Alt+F2\" = \"close\"");
        assert_eq!(key, "keys.Ctrl+Alt+F2");
        assert!(
            reason.starts_with("switches to VT 2 before any binding"),
            "{reason}"
        );
        let (key, reason) = keys_refused("\"Ctrl+Alt+BackSpace\" = \"close\"");
        assert_eq!(key, "keys.Ctrl+Alt+BackSpace");
        assert!(
            reason.starts_with("ends the session before any binding"),
            "{reason}"
        );
    }

    #[test]
    fn an_escape_hatch_chord_cannot_be_unbound_either() {
        let (_, reason) = keys_refused("\"Ctrl+Alt+F3\" = \"none\"");
        assert!(
            reason.contains("no config can bind or unbind it"),
            "{reason}"
        );
    }

    #[test]
    fn a_hatch_with_more_modifiers_held_is_still_a_hatch() {
        let (_, reason) = keys_refused("\"Ctrl+Alt+Shift+F12\" = \"close\"");
        assert!(reason.starts_with("switches to VT 12"), "{reason}");
    }

    #[test]
    fn ctrl_alt_with_another_key_is_still_bindable() {
        let text = "[keys]\n\"Ctrl+Alt+Left\" = \"workspace-left\"\n\
                    \"Ctrl+F2\" = \"close\"\n\"Alt+F2\" = \"minimize\"\n\
                    \"Ctrl+Logo+BackSpace\" = \"reload\"";
        let config = parse(text, SEAT).unwrap();
        let ctrl_f2 = Mods {
            ctrl: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(ctrl_f2, &[Keysym::F2]),
            Some(&Action::Close)
        );
    }

    #[test]
    fn two_spellings_of_one_chord_in_keys_are_refused() {
        let (key, reason) = keys_refused("\"Logo+a\" = \"close\"\n\"Super+A\" = \"minimize\"");
        assert_eq!(key, "keys.Super+A");
        assert_eq!(
            reason,
            "is the same as `Logo+a`, already written; write it once"
        );
    }

    #[test]
    fn two_spellings_of_the_logo_tap_are_refused() {
        let (key, reason) = keys_refused("\"Logo\" = \"start-menu\"\n\"Super\" = \"none\"");
        assert_eq!(key, "keys.Super");
        assert!(reason.contains("`Logo`"), "{reason}");
    }

    #[test]
    fn no_profile_binds_an_escape_hatch() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let config = Config::profile(profile, SEAT);
            for (chord, action) in config.bindings.iter() {
                assert_eq!(
                    perspicax_policy::hatch(chord.mods, [chord.key]),
                    None,
                    "{profile:?} binds {chord:?} to {action:?}"
                );
            }
        }
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
            Some(&Action::Spawn {
                command: vec!["foot".to_owned()],
                agent: false
            })
        );
    }

    /// As Openbox does, and over the titlebar alone: over the window the
    /// wheel is its client's, and over the desktop the person's to bind. A
    /// double-click on the title is left unbound, so it still maximizes,
    /// which is built in. Classic binds nothing.
    #[test]
    fn minimal_rolls_a_window_up_and_down_with_the_wheel_over_its_titlebar_and_classic_does_not() {
        let none = Mods::default();
        let up = Gesture::Wheel(perspicax_policy::Wheel::Up);
        let down = Gesture::Wheel(perspicax_policy::Wheel::Down);
        let minimal = Config::profile(Profile::Minimal, SEAT).mouse;
        assert_eq!(
            minimal.resolve(Context::Titlebar, none, up),
            Some(&Action::Shade)
        );
        assert_eq!(
            minimal.resolve(Context::Titlebar, none, down),
            Some(&Action::Unshade)
        );
        for context in [Context::Window, Context::Desktop, Context::Anywhere] {
            assert_eq!(minimal.resolve(context, none, up), None, "{context:?}");
            assert_eq!(minimal.resolve(context, none, down), None, "{context:?}");
        }
        assert_eq!(
            minimal.resolve(Context::Titlebar, Mods::alt(), up),
            None,
            "the wheel alone, with nothing held"
        );
        assert_eq!(
            minimal.resolve(
                Context::Titlebar,
                none,
                Gesture::Double(perspicax_policy::Button::Left)
            ),
            None,
            "a double-click is maximize's"
        );
        assert_eq!(
            Config::profile(Profile::Classic, SEAT).mouse,
            MouseBindings::default()
        );
    }

    #[test]
    fn minimals_wheel_over_a_titlebar_can_be_handed_back_or_bound_otherwise() {
        let config = parse(
            "profile = \"minimal\"\n[mouse.titlebar]\n\
             \"WheelUp\" = \"none\"\n\"WheelDown\" = \"toggle-shade\"",
            SEAT,
        )
        .unwrap();
        let none = Mods::default();
        assert_eq!(
            config.mouse.resolve(
                Context::Titlebar,
                none,
                Gesture::Wheel(perspicax_policy::Wheel::Up)
            ),
            None
        );
        assert_eq!(
            config.mouse.resolve(
                Context::Titlebar,
                none,
                Gesture::Wheel(perspicax_policy::Wheel::Down)
            ),
            Some(&Action::ToggleShade)
        );
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
    fn an_output_position_past_the_world_is_refused_with_its_ends() {
        let text = "[[output]]\nname = \"A\"\nposition = [2147483647, 0]";
        let error = parse(text, SEAT).unwrap_err().to_string();
        assert!(error.contains("output.A.position"), "{error}");
        assert!(error.contains("-65536 to 65536"), "{error}");
        let edge = "[[output]]\nname = \"A\"\nposition = [-65536, 65536]";
        assert_eq!(
            parse(edge, SEAT).unwrap().outputs[0].place,
            Place::At(-65536, 65536)
        );
    }

    #[test]
    fn an_output_offset_past_the_world_is_refused_with_its_ends() {
        let text = "[[output]]\nname = \"A\"\nright-of = \"B\"\noffset = -65537";
        let error = parse(text, SEAT).unwrap_err().to_string();
        assert!(error.contains("output.A.offset"), "{error}");
        assert!(error.contains("-65536 to 65536"), "{error}");
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
    fn an_autostart_entry_may_be_a_command_or_a_table_marked_agent() {
        let config = parse(
            r#"autostart = [["waybar"], { spawn = ["foot", "-e", "htop"], agent = true }, { spawn = ["mako"] }]"#,
            SEAT,
        )
        .expect("a valid config");
        let start = |command: &[&str], agent| Start {
            command: command.iter().map(|word| (*word).to_owned()).collect(),
            agent,
        };
        assert_eq!(
            config.autostart,
            [
                start(&["waybar"], false),
                start(&["foot", "-e", "htop"], true),
                start(&["mako"], false),
            ]
        );
    }

    #[test]
    fn an_empty_command_in_either_form_is_refused() {
        for text in [
            "autostart = [[]]",
            "autostart = [{ spawn = [], agent = true }]",
        ] {
            let error = parse(text, SEAT).unwrap_err();
            assert!(
                error.to_string().contains("autostart[0]"),
                "{text}: {error}"
            );
        }
    }

    #[test]
    fn a_binding_may_spawn_for_the_agent() {
        let config = parse(
            "[keys]\n\"Logo+Shift+Return\" = { spawn = [\"foot\"], agent = true }",
            SEAT,
        )
        .expect("a valid config");
        let logo_shift = Mods {
            logo: true,
            shift: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(logo_shift, &[Keysym::Return]),
            Some(&Action::Spawn {
                command: vec!["foot".to_owned()],
                agent: true
            })
        );
    }

    #[test]
    fn the_settings_app_is_never_given_to_an_agent() {
        let refused = |toml: &str| match parse(toml, SEAT) {
            Err(Error::Invalid { key, reason }) => {
                assert!(
                    reason.contains("`perspicax-settings` edits this file"),
                    "{reason}"
                );
                key
            }
            other => panic!("{toml}: {other:?}"),
        };
        assert_eq!(
            refused("[keys]\n\"Logo+,\" = { spawn = [\"perspicax-settings\"], agent = true }"),
            "keys.Logo+,"
        );
        assert_eq!(
            refused(
                "[mouse.desktop]\n\"Mouse8\" = { spawn = [\"/usr/bin/perspicax-settings\"], agent = true }"
            ),
            "mouse.desktop.Mouse8"
        );
        assert_eq!(
            refused("autostart = [[\"foot\"], { spawn = [\"perspicax-settings\"], agent = true }]"),
            "autostart[1]"
        );
        assert_eq!(
            refused("[agent]\nlaunch = [\"foot\", \"/usr/local/bin/perspicax-settings\"]"),
            "agent.launch[1]"
        );
        let persons = parse(
            "autostart = [[\"perspicax-settings\"]]\n[keys]\n\"Logo+,\" = { spawn = [\"perspicax-settings\"] }",
            SEAT,
        );
        assert!(persons.is_ok(), "the person's own: {persons:?}");
    }

    /// `[agent] never` is read before the bindings, wherever the file
    /// writes it, so a binding above it is held to it too.
    #[test]
    fn what_never_names_is_given_to_no_agent() {
        let refused = |toml: &str| match parse(toml, SEAT) {
            Err(Error::Invalid { key, reason }) => {
                assert!(
                    reason.contains("`ennius` is in `[agent] never`, so no agent may be given it"),
                    "{reason}"
                );
                key
            }
            other => panic!("{toml}: {other:?}"),
        };
        assert_eq!(
            refused("[keys]\n\"Logo+e\" = { spawn = [\"ennius\", \"gui\"], agent = true }"),
            "keys.Logo+e"
        );
        assert_eq!(
            refused(
                "[mouse.desktop]\n\"Mouse8\" = { spawn = [\"/usr/local/bin/ennius\"], agent = true }"
            ),
            "mouse.desktop.Mouse8"
        );
        assert_eq!(
            refused("autostart = [[\"foot\"], { spawn = [\"ennius\", \"gui\"], agent = true }]"),
            "autostart[1]"
        );
        assert_eq!(
            refused("[agent]\nlaunch = [\"foot\", \"ennius\"]"),
            "agent.launch[1]"
        );
        match parse(
            "[keys]\n\"Logo+w\" = { spawn = [\"wallet\"], agent = true }\n\
             [agent]\nnever = [\"wallet\"]",
            SEAT,
        ) {
            Err(Error::Invalid { key, reason }) => {
                assert_eq!(key, "keys.Logo+w", "the list read before the keys above it");
                assert!(
                    reason.contains("`wallet` is in `[agent] never`"),
                    "{reason}"
                );
            }
            other => panic!("{other:?}"),
        }

        let persons = parse(
            "autostart = [[\"ennius\", \"gui\"]]\n[keys]\n\"Logo+e\" = { spawn = [\"ennius\", \"gui\"] }",
            SEAT,
        );
        assert!(persons.is_ok(), "the person's own: {persons:?}");
        let cleared = parse(
            "[keys]\n\"Logo+e\" = { spawn = [\"ennius\", \"gui\"], agent = true }\n\
             [agent]\nnever = []\nlaunch = [\"ennius\"]",
            SEAT,
        );
        assert!(cleared.is_ok(), "the person took it off: {cleared:?}");
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
    fn both_profiles_slide_workspaces_by_default() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let transitioning = Config::profile(profile, SEAT).transitioning;
            assert_eq!(
                transitioning.effect,
                perspicax_policy::Effect::Slide,
                "{profile:?}"
            );
            assert_eq!(transitioning.length_ms(), 200, "{profile:?}");
        }
    }

    #[test]
    fn a_transition_takes_its_effect_and_its_length() {
        let config = parse(
            "[workspaces.transition]\neffect = \"carousel\"\nduration-ms = 650",
            SEAT,
        )
        .unwrap();
        assert_eq!(
            config.transitioning.effect,
            perspicax_policy::Effect::Carousel
        );
        assert_eq!(config.transitioning.length_ms(), 650);
        for (word, effect) in [
            ("slide", perspicax_policy::Effect::Slide),
            ("crossfade", perspicax_policy::Effect::Crossfade),
            ("wipe", perspicax_policy::Effect::Wipe),
            ("none", perspicax_policy::Effect::None),
        ] {
            let text = format!("[workspaces.transition]\neffect = \"{word}\"");
            assert_eq!(parse(&text, SEAT).unwrap().transitioning.effect, effect);
        }
    }

    #[test]
    fn an_effect_takes_its_own_length_unless_one_is_written() {
        let config = parse("[workspaces.transition]\neffect = \"wipe\"", SEAT).unwrap();
        assert_eq!(config.transitioning.duration_ms, None);
        assert_eq!(config.transitioning.length_ms(), 300, "a wipe's own");
        let config = parse("[workspaces.transition]\nduration-ms = 120", SEAT).unwrap();
        assert_eq!(
            config.transitioning.effect,
            perspicax_policy::Effect::Slide,
            "the profile's, untouched"
        );
        assert_eq!(config.transitioning.length_ms(), 120);
    }

    #[test]
    fn a_transition_of_no_time_or_of_seconds_is_refused_by_name() {
        for text in [
            "[workspaces.transition]\nduration-ms = 0",
            "[workspaces.transition]\nduration-ms = 2001",
        ] {
            let error = parse(text, SEAT).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("workspaces.transition.duration-ms"),
                "{text}: {error}"
            );
        }
        let error = parse("[workspaces.transition]\nduration-ms = 0", SEAT).unwrap_err();
        assert!(error.to_string().contains("effect = \"none\""), "{error}");
        assert!(parse("[workspaces.transition]\nduration-ms = 2000", SEAT).is_ok());
    }

    #[test]
    fn an_effect_not_known_is_refused() {
        assert!(matches!(
            parse("[workspaces.transition]\neffect = \"dissolve\"", SEAT),
            Err(Error::Parse { .. })
        ));
        assert!(
            parse("[workspaces.transition]\nspeed = 2", SEAT).is_err(),
            "nor a key it does not have"
        );
    }

    #[test]
    fn no_effect_keeps_its_length() {
        let config = parse(
            "[workspaces.transition]\neffect = \"none\"\nduration-ms = 500",
            SEAT,
        )
        .unwrap();
        assert_eq!(config.transitioning.effect, perspicax_policy::Effect::None);
        assert_eq!(
            config.transitioning.duration_ms,
            Some(500),
            "kept, for when an effect is turned on again"
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
    fn no_profile_draws_anything_see_through() {
        for profile in [Profile::Classic, Profile::Minimal] {
            assert_eq!(
                Config::profile(profile, SEAT).opacity,
                Opacity::default(),
                "{profile:?}"
            );
        }
        assert_eq!(parse("", SEAT).unwrap().opacity, Opacity::default());
    }

    #[test]
    fn opacity_is_configured_in_its_own_table_with_each_applications_beneath() {
        let config = parse(
            "[opacity]\nstep = 5\nfloor = 30\nunfocused = 85\n\
             [opacity.apps]\nfoot = 90\n\"org.gnome.Nautilus\" = 95",
            SEAT,
        )
        .unwrap();
        assert_eq!(
            config.opacity,
            Opacity {
                step: 5,
                floor: 30,
                unfocused: 85,
                apps: BTreeMap::from([
                    ("foot".to_owned(), 90),
                    ("org.gnome.Nautilus".to_owned(), 95),
                ]),
            }
        );
        let apps_alone = parse("[opacity.apps]\nfoot = 90", SEAT).unwrap();
        assert_eq!(apps_alone.opacity.step, 10, "the default, kept");
        assert_eq!(apps_alone.opacity.ruled(Some("foot")), 90);
        let bounds = parse("[opacity]\nstep = 100\nfloor = 1\nunfocused = 1", SEAT).unwrap();
        assert_eq!(
            (
                bounds.opacity.step,
                bounds.opacity.floor,
                bounds.opacity.unfocused
            ),
            (100, 1, 1)
        );
    }

    #[test]
    fn an_opacity_out_of_range_is_refused_by_name() {
        for (text, key) in [
            ("[opacity]\nstep = 0", "opacity.step"),
            ("[opacity]\nstep = 101", "opacity.step"),
            ("[opacity]\nfloor = 0", "opacity.floor"),
            ("[opacity]\nfloor = -20", "opacity.floor"),
            ("[opacity]\nunfocused = 150", "opacity.unfocused"),
            ("[opacity.apps]\nfoot = 0", "opacity.apps.foot"),
            ("[opacity.apps]\nfoot = 101", "opacity.apps.foot"),
            ("[opacity.apps]\nfoot = 300", "opacity.apps.foot"),
        ] {
            let error = parse(text, SEAT).unwrap_err();
            assert!(
                matches!(&error, Error::Invalid { key: at, .. } if at == key),
                "{text}: {error}"
            );
        }
    }

    #[test]
    fn nothing_may_be_set_below_the_floor_but_by_dimming() {
        for (text, key) in [
            ("[opacity]\nunfocused = 15", "opacity.unfocused"),
            ("[opacity]\nfloor = 50\nunfocused = 40", "opacity.unfocused"),
            (
                "[opacity]\nfloor = 50\n[opacity.apps]\nfoot = 40",
                "opacity.apps.foot",
            ),
            ("[opacity.apps]\nfoot = 10", "opacity.apps.foot"),
        ] {
            let error = parse(text, SEAT).unwrap_err();
            assert!(
                matches!(&error, Error::Invalid { key: at, .. } if at == key),
                "{text}: {error}"
            );
        }
        assert!(parse("[opacity]\nfloor = 50\nunfocused = 50", SEAT).is_ok());
    }

    #[test]
    fn an_opacity_key_misspelled_or_an_application_unnamed_is_refused() {
        assert!(parse("[opacity]\nfocused = 80", SEAT).is_err());
        assert!(parse("[opacity]\nstep = 7.5", SEAT).is_err());
        let error = parse("[opacity.apps]\n\"\" = 90", SEAT).unwrap_err();
        assert!(
            matches!(&error, Error::Invalid { key, .. } if key == "opacity.apps"),
            "{error}"
        );
    }

    #[test]
    fn both_profiles_open_a_programs_window_where_its_last_one_closed() {
        for profile in [Profile::Classic, Profile::Minimal] {
            assert_eq!(
                Config::profile(profile, SEAT).remembering,
                Remembering::default(),
                "{profile:?}"
            );
        }
        let config = parse("", SEAT).unwrap();
        assert!(config.remembering.on && config.remembering.forget.is_empty());
    }

    #[test]
    fn placement_is_configured_in_its_own_table_with_the_programs_to_forget() {
        let config = parse(
            "[placement]\nremember = false\n\
             forget = [\"org.keepassxc.KeePassXC\", \"Firefox\"]",
            SEAT,
        )
        .unwrap();
        assert_eq!(
            config.remembering,
            Remembering {
                on: false,
                forget: BTreeSet::from([
                    "org.keepassxc.KeePassXC".to_owned(),
                    "Firefox".to_owned()
                ]),
            }
        );
        let forget_alone = parse("[placement]\nforget = [\"foot\"]", SEAT).unwrap();
        assert!(forget_alone.remembering.on, "the default, kept");
        assert!(!forget_alone.remembering.remembers("foot"));
        assert!(forget_alone.remembering.remembers("ghostty"));
        assert!(
            shell("[placement]\nremember = false", ShellBuilt::FULL).is_ok(),
            "the shell reads the same file"
        );
    }

    #[test]
    fn a_placement_key_misspelled_or_an_application_unnamed_is_refused() {
        assert!(parse("[placement]\nremember-size = true", SEAT).is_err());
        assert!(parse("[placement]\nremember = \"yes\"", SEAT).is_err());
        assert!(
            parse("[placement]\nforget = [{ title = \"Inbox\" }]", SEAT).is_err(),
            "never by title"
        );
        let error = parse("[placement]\nforget = [\"foot\", \"\"]", SEAT).unwrap_err();
        assert!(
            matches!(&error, Error::Invalid { key, .. } if key == "placement.forget"),
            "{error}"
        );
    }

    #[test]
    fn decorations_are_configured_in_their_own_table() {
        assert_eq!(
            Config::profile(Profile::Minimal, SEAT).decorations,
            Decorations {
                buttons: Config::profile(Profile::Minimal, SEAT).decorations.buttons,
                ..Decorations::default()
            },
            "minimal's frame is flat, the one there always was, but for its buttons"
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

    fn ends(buttons: Buttons) -> (Vec<FrameButton>, Vec<FrameButton>) {
        (buttons.left().collect(), buttons.right().collect())
    }

    #[test]
    fn minimal_puts_sticky_on_the_left_as_fluxbox_does_and_classic_keeps_todays_bar() {
        use FrameButton::*;
        assert_eq!(
            ends(Config::profile(Profile::Minimal, SEAT).decorations.buttons),
            (vec![Sticky, OnTop], vec![Shade, Minimize, Maximize, Close])
        );
        assert_eq!(
            ends(Config::profile(Profile::Classic, SEAT).decorations.buttons),
            (vec![], vec![Shade, Minimize, Maximize, Close])
        );
        assert_eq!(
            Config::profile(Profile::Classic, SEAT).decorations.buttons,
            Decorations::default().buttons,
            "classic's buttons are the ones there always were"
        );
        let config = parse(
            "[decorations]\nleft = [\"close\", \"maximize\"]\nright = [\"sticky\"]",
            SEAT,
        )
        .unwrap();
        assert_eq!(
            ends(config.decorations.buttons),
            (vec![Close, Maximize], vec![Sticky])
        );
    }

    #[test]
    fn a_titlebar_button_listed_twice_or_unknown_is_refused_by_name() {
        for (text, key, says) in [
            (
                "[decorations]\nleft = [\"close\", \"close\"]",
                "decorations.left",
                "\"close\" is listed twice",
            ),
            (
                "[decorations]\nright = [\"shade\", \"close\", \"shade\"]",
                "decorations.right",
                "\"shade\" is listed twice",
            ),
            (
                "[decorations]\nleft = [\"sticky\"]\nright = [\"close\", \"sticky\"]",
                "decorations.right",
                "\"sticky\" is in decorations.left as well",
            ),
        ] {
            match parse(text, SEAT) {
                Err(Error::Invalid { key: at, reason }) => {
                    assert_eq!(at, key, "{text}");
                    assert!(reason.contains(says), "{text}: {reason}");
                }
                other => panic!("{text}: {other:?}"),
            }
        }
        let unknown = parse("[decorations]\nleft = [\"pin\"]", SEAT).unwrap_err();
        let said = unknown.to_string();
        assert!(said.contains("pin") && said.contains("left"), "{said}");
    }

    #[test]
    fn an_empty_list_is_a_bare_end() {
        let config = parse("profile = \"minimal\"\n[decorations]\nleft = []", SEAT).unwrap();
        assert_eq!(config.decorations.buttons.left().count(), 0, "no sticky");
        assert_eq!(config.decorations.buttons.right().count(), 4);
        let config = parse("[decorations]\nleft = []\nright = []", SEAT).unwrap();
        assert_eq!(
            ends(config.decorations.buttons),
            (vec![], vec![]),
            "a titlebar with no buttons at all, close included, is a person's to choose"
        );
    }

    /// An end left unwritten is the profile's, less what the written end
    /// took: a button moves rather than being refused for a place the
    /// person never wrote it in.
    #[test]
    fn writing_one_end_moves_a_button_off_the_other() {
        use FrameButton::*;
        let config = parse(
            "profile = \"minimal\"\n[decorations]\nright = [\"sticky\", \"close\"]",
            SEAT,
        )
        .unwrap();
        assert_eq!(
            ends(config.decorations.buttons),
            (vec![OnTop], vec![Sticky, Close])
        );
        let config = parse("[decorations]\nleft = [\"close\"]", SEAT).unwrap();
        assert_eq!(
            ends(config.decorations.buttons),
            (vec![Close], vec![Shade, Minimize, Maximize]),
            "classic's close taken to the left"
        );
    }

    /// A person's pictures for the buttons, by what a press then does, and
    /// masks unless told otherwise; with none, the built-in shapes.
    #[test]
    fn a_buttons_icons_are_files_by_what_a_press_does() {
        assert_eq!(
            Config::profile(Profile::Minimal, SEAT).icons,
            Icons::default()
        );
        assert!(Icons::default().files.is_empty() && Icons::default().tint);
        let config = parse(
            "[decorations.icons]\nclose = \"~/.config/perspicax/buttons/close.png\"\n\
             restore = \"buttons/restore.png\"\ntint = false",
            SEAT,
        )
        .unwrap();
        let icons = &config.icons;
        assert_eq!(
            icons.file(Face::Close),
            Some(Path::new("~/.config/perspicax/buttons/close.png")),
            "as written: the seat finds it"
        );
        assert_eq!(
            icons.file(Face::Restore),
            Some(Path::new("buttons/restore.png"))
        );
        assert_eq!(icons.file(Face::Maximize), None, "restore is not maximize");
        assert!(!icons.tint);
        let config = parse("[decorations.icons]\nstick = \"pin.png\"", SEAT).unwrap();
        assert!(config.icons.tint, "a mask by default");
    }

    /// Keeping a window on top is a button like the rest (#64): either end
    /// may carry it, by its own name, and both its faces' pictures are read.
    #[test]
    fn on_top_is_a_button_either_end_may_carry_and_its_icons_are_read() {
        use FrameButton::*;
        let config = parse(
            "[decorations]\nleft = [\"on-top\"]\nright = [\"close\"]\n\
             [decorations.icons]\non-top = \"up.png\"\noff-top = \"down.png\"",
            SEAT,
        )
        .unwrap();
        assert_eq!(ends(config.decorations.buttons), (vec![OnTop], vec![Close]));
        assert_eq!(config.icons.file(Face::OnTop), Some(Path::new("up.png")));
        assert_eq!(config.icons.file(Face::OffTop), Some(Path::new("down.png")));
        let config = parse("[decorations]\nright = [\"on-top\", \"close\"]", SEAT).unwrap();
        assert_eq!(
            ends(config.decorations.buttons),
            (vec![], vec![OnTop, Close])
        );
        match parse("[decorations.icons]\noff-top = \" \"", SEAT) {
            Err(Error::Invalid { key, .. }) => assert_eq!(key, "decorations.icons.off-top"),
            other => panic!("{other:?}"),
        }
    }

    /// An SVG needs a build that draws SVG; one without it says which
    /// feature would, at the icon's own key. A PNG needs nothing more.
    #[test]
    fn an_svg_in_a_build_without_svg_is_refused_naming_the_feature() {
        let built = Built { svg: false, ..SEAT };
        let error = parse("[decorations.icons]\nrestore = \"restore.SVG\"", built).unwrap_err();
        assert!(
            matches!(
                error,
                Error::NotBuilt {
                    key: "decorations.icons.restore",
                    feature: "svg"
                }
            ),
            "{error}"
        );
        assert!(parse("[decorations.icons]\nrestore = \"restore.png\"", built).is_ok());
        let config = parse("[decorations.icons]\nrestore = \"restore.svg\"", SEAT).unwrap();
        assert!(is_svg(config.icons.file(Face::Restore).unwrap()));
    }

    #[test]
    fn an_icon_that_cannot_be_named_is_refused_by_its_key() {
        match parse("[decorations.icons]\nunshade = \" \"", SEAT) {
            Err(Error::Invalid { key, reason }) => {
                assert_eq!(key, "decorations.icons.unshade");
                assert!(reason.contains("empty"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        let unknown = parse("[decorations.icons]\nsticky = \"pin.png\"", SEAT).unwrap_err();
        let said = unknown.to_string();
        assert!(said.contains("sticky"), "the button, not a face: {said}");
    }

    /// classic rounds its titlebars' top corners, as Plasma does, and
    /// minimal keeps them square, as Fluxbox does; either is a key away.
    #[test]
    fn classic_rounds_its_corners_and_minimal_keeps_them_square() {
        assert_eq!(
            Config::profile(Profile::Classic, SEAT).decorations.radius,
            5
        );
        assert_eq!(
            Config::profile(Profile::Minimal, SEAT).decorations.radius,
            0
        );
        let config = parse("[decorations]\ncorner-radius = 0", SEAT).unwrap();
        assert_eq!(config.decorations.radius, 0, "square, in classic");
        let config = parse(
            "profile = \"minimal\"\n[decorations]\ncorner-radius = 8",
            SEAT,
        )
        .unwrap();
        assert_eq!(config.decorations.radius, 8);
        match parse("[decorations]\ncorner-radius = 17", SEAT) {
            Err(Error::Invalid { key, .. }) => assert_eq!(key, "decorations.corner-radius"),
            other => panic!("{other:?}"),
        }
    }

    /// classic casts a shadow under every window it frames, and minimal,
    /// flat, none; either is a table away.
    #[test]
    fn classic_casts_a_shadow_and_minimal_none() {
        let classic = Config::profile(Profile::Classic, SEAT).decorations.shadow;
        assert_eq!(
            (classic.size, classic.offset, classic.opacity),
            (18, (0, 4), 50)
        );
        assert_eq!(
            Config::profile(Profile::Minimal, SEAT)
                .decorations
                .shadow
                .size,
            0
        );
        let config = parse(
            "profile = \"minimal\"\n[decorations.shadow]\nsize = 20\noffset = [2, -4]\n\
             opacity = 60\ncolour = \"#102030\"",
            SEAT,
        )
        .unwrap();
        let shadow = config.decorations.shadow;
        assert_eq!(
            (shadow.size, shadow.offset, shadow.opacity),
            (20, (2, -4), 60)
        );
        assert_eq!(shadow.colour, Colour::rgb(0x10, 0x20, 0x30));
        let config = parse("[decorations.shadow]\nsize = 0", SEAT).unwrap();
        assert_eq!(config.decorations.shadow.size, 0, "none, in classic");
        for (text, key) in [
            ("size = 65", "decorations.shadow.size"),
            ("offset = [0, 40]", "decorations.shadow.offset"),
            ("opacity = 101", "decorations.shadow.opacity"),
            ("opacity = -1", "decorations.shadow.opacity"),
            ("colour = \"grey\"", "decorations.shadow.colour"),
        ] {
            match parse(&format!("[decorations.shadow]\n{text}"), SEAT) {
                Err(Error::Invalid { key: at, .. }) => assert_eq!(at, key, "{text}"),
                other => panic!("{text}: {other:?}"),
            }
        }
    }

    /// classic rolls a window up as the eye can follow and brings a
    /// button's light up; minimal, flat, does both at once.
    #[test]
    fn classic_animates_its_frames_and_minimal_does_not() {
        let classic = Config::profile(Profile::Classic, SEAT)
            .decorations
            .animation;
        assert_eq!((classic.shade_ms, classic.hover_ms), (150, 80));
        assert_eq!(
            Config::profile(Profile::Minimal, SEAT)
                .decorations
                .animation,
            Animation::default()
        );
        let config = parse(
            "profile = \"minimal\"\n[decorations.animation]\nshade-ms = 200",
            SEAT,
        )
        .unwrap();
        assert_eq!(config.decorations.animation.shade_ms, 200);
        assert_eq!(config.decorations.animation.hover_ms, 0);
        match parse("[decorations.animation]\nhover-ms = 1001", SEAT) {
            Err(Error::Invalid { key, .. }) => assert_eq!(key, "decorations.animation.hover-ms"),
            other => panic!("{other:?}"),
        }
    }

    /// Less motion is every animation at once, a workspace switch's among
    /// them, whatever their own tables say.
    #[test]
    fn reduced_motion_makes_every_switch_and_roll_instant() {
        let config = parse(
            "[theme]\nmotion = \"reduced\"\n[workspaces.transition]\neffect = \"carousel\"\n\
             [decorations.animation]\nshade-ms = 400",
            SEAT,
        )
        .unwrap();
        assert_eq!(config.theme.motion, Motion::Reduced);
        assert_eq!(config.transitioning.effect, Effect::None);
        assert_eq!(config.decorations.animation, Animation::default());
        let config = parse("[theme]\nmotion = \"normal\"", SEAT).unwrap();
        assert_ne!(config.transitioning.effect, Effect::None, "classic's slide");
        assert_eq!(config.decorations.animation.shade_ms, 150);
        assert!(parse("[theme]\nmotion = \"none\"", SEAT).is_err());
    }

    #[test]
    fn the_agents_marks_are_configured_under_decorations() {
        let look = Config::profile(Profile::Classic, SEAT).decorations.agent;
        assert_eq!(look, AgentLook::default());
        assert_eq!(look.outline, 3, "on by default, for the windows it marks");
        let config = parse(
            "[decorations.agent]\nfocused = \"#102030\"\noutline = 0\n\
             outline-colour = \"#ff0000\"\nrefused-colour = \"#ffaa00\"",
            SEAT,
        )
        .unwrap();
        let look = config.decorations.agent;
        assert_eq!(look.focused, Colour::rgb(0x10, 0x20, 0x30));
        assert_eq!(look.unfocused, AgentLook::default().unfocused);
        assert_eq!(look.outline, 0, "0 is none");
        assert_eq!(look.outline_colour, Colour::rgb(0xff, 0, 0));
        assert_eq!(look.refused_colour, Colour::rgb(0xff, 0xaa, 0));
    }

    #[test]
    fn an_agent_mark_that_cannot_be_drawn_is_refused_by_name() {
        for (text, key) in [
            ("outline = 9", "decorations.agent.outline"),
            ("outline = -1", "decorations.agent.outline"),
            ("focused = \"purple\"", "decorations.agent.focused"),
            (
                "outline-colour = \"#b37feb80\"",
                "decorations.agent.outline-colour",
            ),
            (
                "refused-colour = \"red\"",
                "decorations.agent.refused-colour",
            ),
        ] {
            match parse(&format!("[decorations.agent]\n{text}"), SEAT).unwrap_err() {
                Error::Invalid { key: at, .. } => assert_eq!(at, key, "{text}"),
                other => panic!("{text}: {other}"),
            }
        }
        assert!(parse("[decorations.agent]\nglow = 1", SEAT).is_err());
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
        let colours = |decorations: Decorations| {
            (
                decorations.focused,
                decorations.focused_ink,
                decorations.unfocused,
                decorations.unfocused_ink,
                decorations.close_hover,
            )
        };
        assert_eq!(
            colours(config.decorations),
            colours(Decorations::default()),
            "the titlebars' colours"
        );
    }

    #[test]
    fn the_logo_is_perspicaxs_unless_another_or_none_is_written() {
        let logo = |text: &str| parse(&format!("[theme]\n{text}"), SEAT).unwrap().theme.logo;
        assert_eq!(logo(""), Logo::Perspicax);
        assert_eq!(logo("logo = \"perspicax\""), Logo::Perspicax);
        assert_eq!(logo("logo = \"none\""), Logo::None);
        assert_eq!(
            logo("logo = \"~/Pictures/logo.png\""),
            Logo::File("~/Pictures/logo.png".into()),
            "as written: the compositor finds it"
        );
    }

    #[test]
    fn the_splash_is_on_wherever_there_is_a_seat_and_can_be_turned_off() {
        for profile in [Profile::Classic, Profile::Minimal] {
            assert!(Config::profile(profile, SEAT).splash, "{profile:?}");
        }
        assert!(!parse("[splash]\nenabled = false", SEAT).unwrap().splash);
        assert!(parse("[splash]", SEAT).unwrap().splash, "unsaid is on");
        let error = parse("[splash]\nenabled = true", Built::default()).unwrap_err();
        assert!(matches!(
            error,
            Error::NotBuilt {
                key: "splash.enabled",
                feature: "seat"
            }
        ));
        assert!(
            !parse("[splash]\nenabled = false", Built::default())
                .unwrap()
                .splash
        );
        assert!(matches!(
            parse("[splash]\ncolour = \"#000000\"", SEAT),
            Err(Error::Parse { .. })
        ));
    }

    #[test]
    fn a_path_is_found_from_home_or_beside_the_config() {
        let config = Path::new("/etc/xdg/perspicax/config.toml");
        let home = Path::new("/home/ada");
        assert_eq!(
            locate(Path::new("~/Pictures/a.png"), Some(config), Some(home)),
            Path::new("/home/ada/Pictures/a.png")
        );
        assert_eq!(
            locate(Path::new("a.png"), Some(config), Some(home)),
            Path::new("/etc/xdg/perspicax/a.png")
        );
        assert_eq!(
            locate(Path::new("/srv/a.png"), Some(config), Some(home)),
            Path::new("/srv/a.png")
        );
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
            Err(Error::Parse { .. })
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

    /// The close button's light is a palette colour like any, and reaches
    /// the titlebars as their own colours do.
    #[test]
    fn the_close_buttons_light_is_written_in_the_palette() {
        assert_eq!(
            Config::profile(Profile::Classic, SEAT)
                .decorations
                .close_hover,
            Colour::rgb(0xda, 0x44, 0x53),
            "with no file at all"
        );
        let config = parse("[theme.palette]\ntitle-close-hover = \"#e81123\"", SEAT).unwrap();
        assert_eq!(
            config.decorations.close_hover,
            Colour::rgb(0xe8, 0x11, 0x23)
        );
        let config = parse("[theme.palette]\nalert = \"#ff0000\"", SEAT).unwrap();
        assert_eq!(
            config.decorations.close_hover,
            Colour::rgb(0xff, 0, 0),
            "following the alert"
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
            ("logo = \" \"", "theme.logo"),
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
            Err(Error::Parse { .. })
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
    fn the_agent_actions_are_named_and_bound_to_logo_a_in_both_profiles() {
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        let shift = Mods {
            shift: true,
            ..logo
        };
        for profile in [Profile::Classic, Profile::Minimal] {
            let bindings = Config::profile(profile, SEAT).bindings;
            assert_eq!(
                bindings.resolve(logo, &[Keysym::a]),
                Some(&Action::ToggleAgent)
            );
            assert_eq!(
                bindings.resolve(shift, &[Keysym::a]),
                Some(&Action::RevokeAgents)
            );
        }
        let config = parse(
            "[keys]\n\"Logo+Y\" = \"toggle-agent\"\n\"Logo+Shift+Y\" = \"revoke-agent-all\"\n\
             \"Logo+A\" = \"none\"",
            SEAT,
        )
        .expect("a valid config");
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::y]),
            Some(&Action::ToggleAgent)
        );
        assert_eq!(
            config.bindings.resolve(shift, &[Keysym::y]),
            Some(&Action::RevokeAgents)
        );
        assert_eq!(config.bindings.resolve(logo, &[Keysym::a]), None, "unbound");
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
    fn shading_is_three_actions_a_key_can_have_and_neither_profile_binds_a_key_to() {
        let config = parse(
            "[keys]\n\"Logo+u\" = \"shade\"\n\
             \"Logo+d\" = \"unshade\"\n\"Logo+s\" = \"toggle-shade\"",
            SEAT,
        )
        .unwrap();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        for (key, action) in [
            (Keysym::u, Action::Shade),
            (Keysym::d, Action::Unshade),
            (Keysym::s, Action::ToggleShade),
        ] {
            assert_eq!(config.bindings.resolve(logo, &[key]), Some(&action));
        }
        let shading = |action: &Action| {
            matches!(
                action,
                Action::Shade | Action::Unshade | Action::ToggleShade
            )
        };
        for profile in [Profile::Classic, Profile::Minimal] {
            let bindings = Config::profile(profile, SEAT).bindings;
            assert!(
                !bindings.iter().any(|(_, action)| shading(action)),
                "{profile:?} binds one"
            );
            assert!(!bindings.tap().is_some_and(shading), "{profile:?}'s tap");
        }
        let error = parse("[keys]\n\"Logo+x\" = \"roll-up\"", SEAT).unwrap_err();
        assert!(
            error.to_string().contains("shade, unshade, toggle-shade"),
            "the refusal lists them: {error}"
        );
    }

    #[test]
    fn toggle_on_top_is_an_action_neither_profile_binds() {
        let config = parse("[keys]\n\"Logo+t\" = \"toggle-on-top\"", SEAT).unwrap();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            config.bindings.resolve(logo, &[Keysym::t]),
            Some(&Action::ToggleOnTop)
        );
        let on_top = |action: &Action| *action == Action::ToggleOnTop;
        for profile in [Profile::Classic, Profile::Minimal] {
            let config = Config::profile(profile, SEAT);
            assert!(
                !config.bindings.iter().any(|(_, action)| on_top(action)),
                "{profile:?} binds a key to it"
            );
            assert!(
                !config.bindings.tap().is_some_and(on_top),
                "{profile:?}'s tap"
            );
            assert!(
                !config
                    .mouse
                    .iter()
                    .any(|(_, _, action)| action.is_some_and(on_top)),
                "{profile:?} binds a button to it"
            );
        }
        let error = parse("[keys]\n\"Logo+x\" = \"keep-above\"", SEAT).unwrap_err();
        assert!(
            error.to_string().contains("toggle-sticky, toggle-on-top"),
            "the refusal lists it beside sticky: {error}"
        );
    }

    #[test]
    fn opacity_is_three_actions_a_chord_can_have_and_neither_profile_binds() {
        let config = parse(
            "[keys]\n\"Logo+Page_Up\" = \"opacity-up\"\n\
             \"Logo+Page_Down\" = \"opacity-down\"\n\"Logo+End\" = \"opacity-reset\"",
            SEAT,
        )
        .unwrap();
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        for (key, action) in [
            (Keysym::Page_Up, Action::OpacityUp),
            (Keysym::Page_Down, Action::OpacityDown),
            (Keysym::End, Action::OpacityReset),
        ] {
            assert_eq!(config.bindings.resolve(logo, &[key]), Some(&action));
        }
        let opacity = |action: &Action| {
            matches!(
                action,
                Action::OpacityUp | Action::OpacityDown | Action::OpacityReset
            )
        };
        for profile in [Profile::Classic, Profile::Minimal] {
            let bindings = Config::profile(profile, SEAT).bindings;
            assert!(
                !bindings.iter().any(|(_, action)| opacity(action)),
                "{profile:?} binds one"
            );
            assert!(!bindings.tap().is_some_and(opacity), "{profile:?}'s tap");
        }
        let error = parse("[keys]\n\"Logo+x\" = \"opacity\"", SEAT).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("opacity-up, opacity-down, opacity-reset"),
            "the refusal lists them: {error}"
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

    /// What the key `key`, pressed alone, does under `config`.
    fn alone(config: &Config, key: Keysym) -> Option<&Action> {
        config.bindings.resolve(Mods::default(), &[key])
    }

    #[test]
    fn the_volume_keys_change_the_sound_in_both_profiles_and_a_persons_own_wins() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let config = Config::profile(profile, SEAT);
            for (key, volume) in [
                (Keysym::XF86_AudioRaiseVolume, VolumeKey::Up),
                (Keysym::XF86_AudioLowerVolume, VolumeKey::Down),
                (Keysym::XF86_AudioMute, VolumeKey::Mute),
                (Keysym::XF86_AudioMicMute, VolumeKey::MicMute),
            ] {
                assert_eq!(
                    alone(&config, key),
                    Some(&Action::Volume(volume)),
                    "{profile:?} {key:?}"
                );
            }
        }
        let own = parse(
            "[keys]\nXF86AudioMute = { spawn = [\"wpctl\", \"set-mute\", \"@DEFAULT_SINK@\", \"toggle\"] }\n\"Logo+F9\" = \"mic-mute\"\n\"Logo+F10\" = \"volume-down\"\nXF86AudioMicMute = \"none\"",
            SEAT,
        )
        .unwrap();
        assert!(matches!(
            alone(&own, Keysym::XF86_AudioMute),
            Some(Action::Spawn { .. })
        ));
        assert_eq!(
            alone(&own, Keysym::XF86_AudioMicMute),
            None,
            "given back, by name"
        );
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            own.bindings.resolve(logo, &[Keysym::F9]),
            Some(&Action::Volume(VolumeKey::MicMute))
        );
        assert_eq!(
            own.bindings.resolve(logo, &[Keysym::F10]),
            Some(&Action::Volume(VolumeKey::Down))
        );
    }

    #[test]
    fn with_the_shell_off_a_volume_key_written_is_refused_by_its_key_and_the_profiles_dropped() {
        match parse(
            "[shell]\nenabled = false\n[keys]\nXF86AudioRaiseVolume = \"volume-up\"",
            SEAT,
        ) {
            Err(Error::Invalid { key, reason }) => {
                assert_eq!(key, "keys.XF86AudioRaiseVolume");
                assert!(reason.contains("enabled = false"), "{reason}");
            }
            other => panic!("not refused by key: {other:?}"),
        }
        let off = parse("[shell]\nenabled = false", SEAT).unwrap();
        assert_eq!(
            alone(&off, Keysym::XF86_AudioRaiseVolume),
            None,
            "given back"
        );
        assert_eq!(
            off.bindings.resolve(Mods::alt(), &[Keysym::F4]),
            Some(&Action::Close),
            "the rest kept"
        );
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
    fn the_settings_protocol_defaults_to_perspicax_settings_in_both_profiles() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let access = Config::profile(profile, SEAT).protocols;
            assert!(access.admits(Protocol::Settings, Some("/usr/bin/perspicax-settings")));
            assert!(!access.admits(Protocol::Settings, Some("/usr/bin/perspicax-shell")));
            assert!(!access.admits(Protocol::Settings, None));
        }
        let config = parse("[protocols]\nsettings = \"off\"", SEAT).unwrap();
        assert_eq!(config.protocols.rule(Protocol::Settings), &Rule::Off);
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
    fn the_settings_app_may_read_the_monitors_by_default() {
        let protocols = Config::profile(Profile::Classic, SEAT).protocols;
        let app = format!("/usr/local/bin/{SETTINGS_APP}");
        assert!(protocols.admits(Protocol::OutputManagement, Some(&app)));
        assert!(!protocols.admits(Protocol::OutputManagement, Some("/usr/bin/firefox")));
    }

    #[test]
    fn every_side_word_reads_back() {
        for side in Side::ALL {
            let text = format!("[[output]]\nname = \"A\"\n{} = \"B\"", side.key());
            let config = parse(&text, SEAT).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(
                config.outputs[0].place,
                Place::Beside {
                    side,
                    of: "B".to_owned(),
                    offset: 0
                }
            );
        }
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

    #[test]
    fn ennius_is_never_an_agents_in_both_profiles_until_the_person_says() {
        for profile in [Profile::Classic, Profile::Minimal] {
            assert_eq!(
                Config::profile(profile, SEAT).agent.never.named(),
                ["ennius"],
                "{profile:?}"
            );
        }
        let never = |toml: &str| parse(toml, SEAT).unwrap().agent.never;
        assert!(never("[agent]\nnever = []").named().is_empty());
        assert_eq!(
            never("[agent]\nnever = [\"ennius\", \"/opt/wallet/bin/wallet\"]").named(),
            ["ennius", "/opt/wallet/bin/wallet"]
        );
        assert_eq!(
            never("[agent]\nnever = []").naming("/usr/bin/perspicax-settings"),
            Some(SETTINGS_APP),
            "the settings app, which no list takes away"
        );
        for (text, key) in [
            ("never = [\"\"]", "agent.never[0]"),
            ("never = [\"ennius\", \"bin/wallet\"]", "agent.never[1]"),
        ] {
            match parse(&format!("[agent]\n{text}"), SEAT).unwrap_err() {
                Error::Invalid { key: at, .. } => assert_eq!(at, key, "{text}"),
                other => panic!("{text}: {other}"),
            }
        }
        assert!(
            parse("[agent]\nnever = \"ennius\"", SEAT).is_err(),
            "a list"
        );
    }

    #[test]
    fn agent_launch_is_off_in_both_profiles() {
        for profile in [Profile::Classic, Profile::Minimal] {
            assert_eq!(Config::profile(profile, SEAT).agent.launch, Rule::Off);
        }
        let config = parse("[agent]\nlaunch = [\"flutter\", \"/usr/bin/zenity\"]", SEAT);
        assert_eq!(
            config.unwrap().agent.launch.named(),
            ["flutter", "/usr/bin/zenity"]
        );
    }

    #[test]
    fn a_bad_launch_rule_is_named_by_its_key() {
        let key = |toml: &str| match parse(toml, SEAT).unwrap_err() {
            Error::Invalid { key, .. } => key,
            other => panic!("{other}"),
        };
        assert_eq!(key("[agent]\nlaunch = \"sometimes\""), "agent.launch");
        assert_eq!(key("[agent]\nlaunch = []"), "agent.launch");
        assert_eq!(key("[agent]\nlaunch = [\"foot\", \"\"]"), "agent.launch[1]");
        assert_eq!(
            key("[agent]\nlaunch = [\"foot\", \"bin/x\"]"),
            "agent.launch[1]",
            "a path from wherever the compositor stands"
        );
        let error = parse("[agent]\nlaunch = \"any\"", Built::default()).unwrap_err();
        assert!(
            matches!(
                error,
                Error::NotBuilt {
                    key: "agent",
                    feature: "seat"
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn the_agent_socket_is_a_switch_or_a_whole_path_and_unsaid_by_default() {
        for profile in [Profile::Classic, Profile::Minimal] {
            assert_eq!(
                Config::profile(profile, SEAT).agent.socket,
                AgentSocket::Unsaid,
                "so --mcp-socket decides, as before the file could say"
            );
        }
        let socket = |toml: &str| parse(toml, SEAT).map(|config| config.agent.socket);
        assert_eq!(socket("[agent]\nsocket = true").unwrap(), AgentSocket::On);
        assert_eq!(socket("[agent]\nsocket = false").unwrap(), AgentSocket::Off);
        assert_eq!(
            socket("[agent]\nsocket = \"/run/user/1000/agent\"").unwrap(),
            AgentSocket::At("/run/user/1000/agent".into())
        );
        for path in ["run/agent", ""] {
            match socket(&format!("[agent]\nsocket = \"{path}\"")).unwrap_err() {
                Error::Invalid { key, .. } => assert_eq!(key, "agent.socket"),
                other => panic!("{path:?}: {other}"),
            }
        }
        assert!(matches!(
            socket("[agent]\nsocket = 1").unwrap_err(),
            Error::Parse { .. }
        ));
    }

    /// A file that is not TOML, or not this schema, is refused at the line
    /// and the column toml shows, counted in letters, not bytes, and said in
    /// a line from there.
    #[test]
    fn a_file_that_is_not_toml_is_refused_where_toml_shows() {
        for (text, line, column) in [
            ("[shell]\nosd = \n", 2, 7),
            (
                "profile = \"classic\"\n[theme]\ncolour = \"#000000\"\n",
                3,
                1,
            ),
            ("# café\n[agent]\nsocket = 1\n", 3, 10),
            ("[agent]\nlaunch = [\"é\" \"x\"]\n", 2, 15),
        ] {
            let error = parse(text, SEAT).unwrap_err();
            let Error::Parse { shown, message, at } = &error else {
                panic!("{text:?}: {error:?}");
            };
            assert_eq!(*at, Some((line, column)), "{text:?}: {shown}");
            assert!(
                shown.contains(&format!("line {line}, column {column}")),
                "{shown}"
            );
            assert_eq!(
                error.in_a_line(),
                format!("line {line}, column {column}: {message}")
            );
            assert!(!error.in_a_line().contains('\n'), "{}", error.in_a_line());
        }
    }

    /// Every refusal can be said in a line, for a notice; the log keeps
    /// toml's whole picture of where.
    #[test]
    fn every_refusal_is_said_in_a_line() {
        let invalid = Error::Invalid {
            key: "shell.mixer.step".to_owned(),
            reason: "99 is outside 1 to 25 percent".to_owned(),
        };
        assert_eq!(
            invalid.in_a_line(),
            "`shell.mixer.step`: 99 is outside 1 to 25 percent"
        );
        let refused = [
            invalid,
            Error::NotBuilt {
                key: "xwayland",
                feature: "xwayland",
            },
            Error::ShellNotBuilt {
                key: "shell.tray",
                feature: "tray",
            },
            Error::Read {
                path: "/nowhere/config.toml".into(),
                source: std::io::ErrorKind::PermissionDenied.into(),
            },
            parse("[agent]\nsocket = 1", SEAT).unwrap_err(),
            // Where toml cannot say where: what is wrong, alone.
            Error::Parse {
                shown: "TOML parse error\nmissing field `x`".to_owned(),
                message: "missing field `x`".to_owned(),
                at: None,
            },
        ];
        for error in &refused {
            assert!(!error.in_a_line().contains('\n'), "{error:?}");
            assert!(!error.in_a_line().is_empty(), "{error:?}");
        }
        assert!(
            refused[4].to_string().contains('\n'),
            "the log keeps the whole"
        );
        assert_eq!(refused[5].in_a_line(), "missing field `x`");
    }
}
