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

use perspicax_policy::{Action, Bindings, Chord, Focus, FocusModel, Keysym, Mods, Towards};
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
    /// the others.
    pub outputs: Vec<OutputRule>,
    /// Programs to start once the session is up, each a program and its
    /// arguments. The person's programs: none is granted agent consent.
    pub autostart: Vec<Vec<String>>,
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

/// What to do with one output.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputRule {
    /// The connector name, as the kernel names it.
    pub name: String,
    /// `false` leaves the monitor dark.
    pub enable: bool,
    /// A mode to use instead of the preferred one.
    pub mode: Option<Mode>,
    /// Where its top-left corner goes in the global space, instead of to the
    /// right of the outputs already lit.
    pub position: Option<(i32, i32)>,
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
        let bindings = Bindings::classic()
            .bind(
                logo_shift(Keysym::Right),
                Action::MoveToOutput(Towards::Next),
            )
            .bind(
                logo_shift(Keysym::Left),
                Action::MoveToOutput(Towards::Previous),
            )
            .bind(logo_shift(Keysym::r), Action::Reload);
        Self {
            profile,
            focus,
            bindings,
            keyboard: Keyboard::default(),
            pointer: Pointer::default(),
            outputs: Vec::new(),
            autostart: Vec::new(),
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
#[serde(deny_unknown_fields)]
struct RawOutput {
    name: String,
    enable: Option<bool>,
    mode: Option<String>,
    position: Option<[i32; 2]>,
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
        Ok(OutputRule {
            name: self.name,
            enable: self.enable.unwrap_or(true),
            mode,
            position: self.position.map(|[x, y]| (x, y)),
            scale: self.scale,
        })
    }
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
            other => {
                return Err(format!(
                    "`{other}` is not an action; use close, cycle-focus, \
                     move-to-next-output, move-to-previous-output, reload, none, \
                     or {{ spawn = [...] }}"
                ));
            }
        },
    }))
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
        assert_eq!(config.outputs[0].position, Some((1920, 0)));
        assert!(!config.outputs[1].enable);
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
    fn a_missing_file_is_the_classic_profile() {
        let config = load(Path::new("/nonexistent/perspicax/config.toml"), SEAT).unwrap();
        assert_eq!(config.profile, Profile::Classic);
    }
}
