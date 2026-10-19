//! The behaviour pages' keys: every scalar setting the file can hold, by the
//! part of the file it is in, each with the value in force, the profile's
//! value, and whether the file writes it.
//!
//! The value in force is read from the parsed [`Config`] and spelled back in
//! the file's own words, by the same `key()`s and spellers the parser's
//! words are tested against. The profile's value is what "Reset" gives: the
//! file with this one key taken out, parsed again. That is the profile as
//! this file has it, as the Keys page reads the profile's bindings, so a key
//! another one leans on is told truly: with `wallpaper = "none"` written,
//! resetting `desktop-icons` gives off, whatever the profile alone says.
//!
//! Every value is checked here on its way in, with the parser's own ranges
//! ([`perspicax_config::limits`]) and in its words; the app only draws.

use std::ops::RangeInclusive;

use perspicax_config::{
    Align, Builtin, ColorScheme, Config, Contrast, Edge, Item, Leave, Motion, PanelOutputs,
    PanelWidth, Profile, Rgba, Role, TaskbarScope, UntrustedLaunchers, WallpaperMode, limits,
};
use perspicax_policy::{Effect, Face, FocusModel, FrameButton, Mode, Switching};

use crate::{Edit, Key, Value, edit};

/// A part of the file a person thinks of as one thing, and its keys: one
/// page of the app.
pub struct Section {
    pub title: &'static str,
    pub about: &'static str,
    /// Whether the first field is the section's own switch. While it is off
    /// the rest do nothing, so a page shows only the switch; the file keeps
    /// whatever it writes for the rest.
    pub switched: bool,
    pub fields: Vec<Field>,
}

/// One key, and how to read and write it.
pub struct Field {
    pub key: Key,
    /// What it decides, in a few words.
    pub says: &'static str,
    pub kind: Kind,
    /// What nothing in force means, where nothing can be: "xkb's default".
    /// Clearing such a field resets it.
    pub unset: Option<&'static str>,
    read: Box<Reader>,
}

/// How a key's value in force is read from the parsed file.
type Reader = dyn Fn(&Config) -> Option<Value>;

/// What a key holds, and so how it is written and shown.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// `true` or `false`.
    Switch,
    /// One word of these.
    Choice(Vec<&'static str>),
    /// A whole number in `range`, of `unit`.
    Number {
        range: RangeInclusive<i64>,
        unit: &'static str,
    },
    /// A number in `range`, fractions and all.
    Fraction { range: RangeInclusive<f64> },
    /// Any words: a name, a path, a format.
    Text { example: &'static str },
    /// `"#rrggbb"`, or `"#rrggbbaa"` where the key takes one see-through.
    Colour,
    /// A program and its arguments, written as a shell would.
    Command,
    /// Some of these words, each once, in the order given.
    List(Vec<&'static str>),
    /// Two whole numbers in `range`, as `[4, 1]`.
    Pair {
        names: [&'static str; 2],
        range: RangeInclusive<i64>,
        unit: &'static str,
    },
    /// A panel's width: pixels, or a share of its monitor as `"60%"`.
    Width,
    /// The monitors a panel goes on: `"all"`, `"first"`, or their names.
    Outputs,
    /// Modifiers, as `drag` writes them: `"Alt"`, or `"none"`.
    Modifiers,
    /// The keyboard's layouts, by xkb's names: `"us,ru"`.
    Layouts,
    /// Each layout's variant, by position: `",phonetic"`.
    Variants,
}

/// One key as the file stands: what is in force, what the file says, and
/// what resetting it would give.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    /// In the file's words; `None` where nothing is in force (see
    /// [`Field::unset`]), or the section is switched off.
    pub in_force: Option<Value>,
    /// What the file writes for it, if it does.
    pub written: Option<Value>,
    /// What resetting it gives: the value in force, for a key not written.
    /// Why not, where the file without it would be refused: another key
    /// written leans on it.
    pub profile: Result<Option<Value>, String>,
    /// Where an unwritten value comes from, when not from the profile.
    pub source: Option<Source>,
}

/// Where a value not written at its own key comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A palette colour that follows another one written.
    Follows(Role),
    /// Written at another key: `[decorations] focused`, the title colour's
    /// older place.
    WrittenAt(Key),
}

impl Setting {
    /// What the field shows: what the file says, or else what is in force.
    #[must_use]
    pub fn shown(&self) -> Option<&Value> {
        self.written.as_ref().or(self.in_force.as_ref())
    }

    /// Whether what is in force is not what the file says: another key
    /// overrides it, as less motion overrides every animation.
    #[must_use]
    pub fn overridden(&self) -> bool {
        self.written
            .as_ref()
            .is_some_and(|written| self.in_force.as_ref() != Some(written))
    }
}

impl Field {
    fn new(
        table: &[&str],
        leaf: &str,
        says: &'static str,
        kind: Kind,
        read: impl Fn(&Config) -> Option<Value> + 'static,
    ) -> Self {
        Self {
            key: Key::new(table, leaf),
            says,
            kind,
            unset: None,
            read: Box::new(read),
        }
    }

    fn unset(mut self, words: &'static str) -> Self {
        self.unset = Some(words);
        self
    }

    /// The value in force, in the file's words.
    #[must_use]
    pub fn read(&self, config: &Config) -> Option<Value> {
        (self.read)(config)
    }

    /// The edit that writes `value`.
    #[must_use]
    pub fn set(&self, value: Value) -> Edit {
        Edit::Set {
            key: self.key.clone(),
            value,
        }
    }

    /// The edit that gives it back to the profile.
    #[must_use]
    pub fn reset(&self) -> Edit {
        Edit::Reset {
            key: self.key.clone(),
        }
    }

    /// The edit `text`, typed into the field, makes: nothing typed resets a
    /// key that can be unset.
    ///
    /// # Errors
    ///
    /// Why `text` is no value for this key, in the parser's words.
    pub fn edit(&self, text: &str) -> Result<Edit, String> {
        if self.unset.is_some() && text.trim().is_empty() {
            return Ok(self.reset());
        }
        self.kind.parse(text).map(|value| self.set(value))
    }
}

impl Kind {
    /// `text`, as typed, read as this kind's value.
    ///
    /// # Errors
    ///
    /// Why it is not one, in the parser's words where the parser has them.
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        let text = text.trim();
        match self {
            Self::Switch => match text {
                "on" | "true" => Ok(Value::Bool(true)),
                "off" | "false" => Ok(Value::Bool(false)),
                _ => Err(format!("`{text}` is neither on nor off")),
            },
            Self::Choice(words) => words
                .iter()
                .find(|word| **word == text)
                .map(|word| Value::String((*word).to_owned()))
                .ok_or_else(|| format!("`{text}` is not one of {}", words.join(", "))),
            Self::List(words) => {
                let chosen: Vec<String> = split_list(text);
                if let Some(wrong) = chosen.iter().find(|item| !words.contains(&item.as_str())) {
                    return Err(format!("`{wrong}` is not one of {}", words.join(", ")));
                }
                if let Some(twice) = chosen
                    .iter()
                    .enumerate()
                    .find_map(|(at, item)| chosen[..at].contains(item).then_some(item))
                {
                    return Err(format!("\"{twice}\" is listed twice"));
                }
                Ok(Value::Strings(chosen))
            }
            Self::Number { range, unit } => whole(text, range, unit).map(Value::Integer),
            Self::Fraction { range } => {
                let number: f64 = text
                    .parse()
                    .map_err(|_| format!("`{text}` is not a number"))?;
                if range.contains(&number) {
                    Ok(Value::Float(number))
                } else {
                    Err(format!(
                        "{number} is outside {} to {}",
                        range.start(),
                        range.end()
                    ))
                }
            }
            Self::Text { .. } | Self::Layouts | Self::Variants => {
                Ok(Value::String(text.to_owned()))
            }
            Self::Colour => colour(text).map(Value::String),
            Self::Command => match shlex::split(text) {
                Some(words) if !words.is_empty() => Ok(Value::Strings(words)),
                Some(_) => Err("write the program to run".to_owned()),
                None => Err("a quote in the command is never closed".to_owned()),
            },
            Self::Pair { names, range, unit } => {
                let parts: Vec<&str> = text
                    .split(|c: char| c == ',' || c == 'x' || c == '×' || c.is_whitespace())
                    .filter(|part| !part.is_empty())
                    .collect();
                let [first, second] = parts[..] else {
                    return Err(format!(
                        "write two numbers: {}, then {}",
                        names[0], names[1]
                    ));
                };
                Ok(Value::Integers(vec![
                    whole(first, range, unit)?,
                    whole(second, range, unit)?,
                ]))
            }
            Self::Width => width(text),
            Self::Outputs => match text {
                "all" | "first" => Ok(Value::String(text.to_owned())),
                _ => {
                    let names = split_list(text);
                    if names.is_empty() {
                        Err("write \"all\", \"first\", or connector names such as DP-1".to_owned())
                    } else {
                        Ok(Value::Strings(names))
                    }
                }
            },
            Self::Modifiers => {
                let mods = perspicax_config::modifiers(text)?;
                perspicax_config::spell_modifiers(mods)
                    .map(Value::String)
                    .ok_or_else(|| "a drag needs a modifier held, or \"none\"".to_owned())
            }
        }
    }

    /// `value` as it is typed into the field: the inverse of
    /// [`parse`](Self::parse).
    #[must_use]
    pub fn text(&self, value: &Value) -> String {
        match (self, value) {
            (Self::Switch, Value::Bool(on)) => (if *on { "on" } else { "off" }).to_owned(),
            (Self::Command, Value::Strings(words)) => joined(words),
            (_, Value::Bool(on)) => on.to_string(),
            (_, Value::Integer(number)) => number.to_string(),
            (_, Value::Float(number)) => number.to_string(),
            (_, Value::String(text)) => text.clone(),
            (_, Value::Strings(words)) => words.join(", "),
            (_, Value::Integers(numbers)) => numbers
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// `value` in words, units and all: `8 pixels`, `on`.
    #[must_use]
    pub fn shown(&self, value: &Value) -> String {
        let text = self.text(value);
        match self {
            Self::Number { unit, .. } | Self::Pair { unit, .. } if !unit.is_empty() => {
                format!("{text} {unit}")
            }
            _ => text,
        }
    }

    /// What the file writes, in the words the value in force is read in:
    /// `"#1E4A73"` as `#1e4a73`, `accel = 1` as 1.0, `"Super"` as `Logo`,
    /// `" 60 %"` as `60%`. Without it a value would differ from itself in
    /// spelling alone.
    #[must_use]
    pub fn normalize(&self, written: &toml::Value) -> Option<Value> {
        let as_written = plain(written)?;
        let reread = match (self, &as_written) {
            // A pointer's speed is between -1 and 1: nothing is lost.
            (Self::Fraction { .. }, Value::Integer(number)) => {
                return Some(Value::Float(*number as f64));
            }
            (Self::Colour | Self::Modifiers | Self::Width, Value::String(text)) => self.parse(text),
            (Self::Text { .. }, Value::String(text)) if text.starts_with('#') => {
                colour(text).map(Value::String)
            }
            _ => return Some(as_written),
        };
        Some(reread.unwrap_or(as_written))
    }
}

/// `text` as a whole number in `range`, refused in the parser's words.
fn whole(text: &str, range: &RangeInclusive<i64>, unit: &str) -> Result<i64, String> {
    let number: i64 = text
        .trim()
        .parse()
        .map_err(|_| format!("`{}` is not a whole number", text.trim()))?;
    if range.contains(&number) {
        return Ok(number);
    }
    let unit = if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    };
    if *range.end() >= i64::from(i32::MAX) {
        return Err(format!("{number} is less than {}{unit}", range.start()));
    }
    Err(format!(
        "{number} is outside {} to {}{unit}",
        range.start(),
        range.end()
    ))
}

/// A colour as the file reads it, in lower case.
fn colour(text: &str) -> Result<String, String> {
    Rgba::parse(text.trim())
        .map(Rgba::hex)
        .ok_or_else(|| format!("`{text}` is not a colour; write one as \"#rrggbb\""))
}

/// A panel's width: a number of pixels, or a share of its monitor.
fn width(text: &str) -> Result<Value, String> {
    if let Some(share) = text.strip_suffix('%') {
        let range = limits::PANEL_SHARE;
        let share = whole(
            share,
            &(i64::from(*range.start())..=i64::from(*range.end())),
            "%",
        )
        .map_err(|_| {
            format!(
                "{text} is not a share of the monitor: write one from {}% to {}%",
                range.start(),
                range.end()
            )
        })?;
        return Ok(Value::String(format!("{share}%")));
    }
    whole(text, &(1..=i64::from(u32::MAX)), "pixels")
        .map(Value::Integer)
        .map_err(|_| {
            format!("`{text}` is not a width; write a number of pixels, as 600, or a share, as 60%")
        })
}

/// Words a person wrote with commas, or spaces, between them.
fn split_list(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A command's words as a shell would write them, quoted where they must be.
fn joined(command: &[String]) -> String {
    shlex::try_join(command.iter().map(String::as_str)).unwrap_or_else(|_| command.join(" "))
}

/// A TOML value as one of [`Value`]'s, if it is one.
fn plain(value: &toml::Value) -> Option<Value> {
    Some(match value {
        toml::Value::Boolean(on) => Value::Bool(*on),
        toml::Value::Integer(number) => Value::Integer(*number),
        toml::Value::Float(number) => Value::Float(*number),
        toml::Value::String(text) => Value::String(text.clone()),
        toml::Value::Array(items)
            if !items.is_empty() && items.iter().all(toml::Value::is_integer) =>
        {
            Value::Integers(items.iter().filter_map(toml::Value::as_integer).collect())
        }
        toml::Value::Array(items) => Value::Strings(
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

/// The value at `key` in `root`, if the file writes one there.
fn at<'a>(root: &'a toml::Table, key: &Key) -> Option<&'a toml::Value> {
    key.table
        .iter()
        .try_fold(root, |table, seg| table.get(seg)?.as_table())?
        .get(&key.leaf)
}

/// Each of `section`'s keys, as the file `text` stands. `config` is `text`
/// parsed with every feature on, which the caller has already.
#[must_use]
pub fn settings(text: &str, config: &Config, section: &Section) -> Vec<Setting> {
    let root: toml::Table = text.parse().unwrap_or_default();
    section
        .fields
        .iter()
        .map(|field| {
            let in_force = field.read(config);
            let written = at(&root, &field.key).and_then(|value| field.kind.normalize(value));
            let profile = if written.is_some() {
                reset(text, field)
            } else {
                Ok(in_force.clone())
            };
            let source = written
                .is_none()
                .then(|| source(&root, &field.key))
                .flatten();
            Setting {
                in_force,
                written,
                profile,
                source,
            }
        })
        .collect()
}

/// What `field` would read as with its key taken out of `text`.
fn reset(text: &str, field: &Field) -> Result<Option<Value>, String> {
    let without = edit::edited(text, &[field.reset()]).map_err(|reason| reason.to_string())?;
    let config =
        perspicax_config::parse(&without, edit::EVERYTHING).map_err(|error| error.to_string())?;
    Ok(field.read(&config))
}

/// Where the unwritten palette colour at `key` comes from, if not from the
/// theme: its older place, or a colour written that it follows.
fn source(root: &toml::Table, key: &Key) -> Option<Source> {
    if key.table != ["theme", "palette"] {
        return None;
    }
    let role = Role::keyed(&key.leaf)?;
    if let Some(older) = older(role).filter(|older| at(root, older).is_some()) {
        return Some(Source::WrittenAt(older));
    }
    follows(root, role).map(Source::Follows)
}

/// The older place a palette colour can be written: the titlebars' two, in
/// `[decorations]`, from before there were themes.
fn older(role: Role) -> Option<Key> {
    let leaf = match role {
        Role::TitleFocused => "focused",
        Role::TitleUnfocused => "unfocused",
        _ => return None,
    };
    Some(Key::new(&["decorations"], leaf))
}

/// The colour `role` follows, where it or one it follows in turn is
/// written, in either place.
fn follows(root: &toml::Table, role: Role) -> Option<Role> {
    let (sources, _) = perspicax_policy::follows(role)?;
    sources.iter().copied().find(|&source| {
        at(root, &Key::new(&["theme", "palette"], source.key())).is_some()
            || older(source).is_some_and(|older| at(root, &older).is_some())
            || follows(root, source).is_some()
    })
}

/// A range of the parser's, as a field's.
fn span<T: Copy>(range: &RangeInclusive<T>) -> RangeInclusive<i64>
where
    i64: TryFrom<T>,
{
    let wide = |end: T, or| i64::try_from(end).unwrap_or(or);
    wide(*range.start(), i64::MIN)..=wide(*range.end(), i64::MAX)
}

fn number<T: Copy>(range: &RangeInclusive<T>, unit: &'static str) -> Kind
where
    i64: TryFrom<T>,
{
    Kind::Number {
        range: span(range),
        unit,
    }
}

fn on(on: bool) -> Option<Value> {
    Some(Value::Bool(on))
}

fn word(word: &str) -> Option<Value> {
    Some(Value::String(word.to_owned()))
}

fn int<T>(number: T) -> Option<Value>
where
    i64: TryFrom<T>,
{
    i64::try_from(number).ok().map(Value::Integer)
}

fn words(words: impl IntoIterator<Item = &'static str>) -> Vec<&'static str> {
    words.into_iter().collect()
}

fn text(example: &'static str) -> Kind {
    Kind::Text { example }
}

/// `text` as a field's value, where empty is nothing in force.
fn some_text(text: &str) -> Option<Value> {
    (!text.is_empty()).then(|| Value::String(text.to_owned()))
}

/// Every behaviour page's keys, in the app's order.
#[must_use]
pub fn sections() -> Vec<Section> {
    vec![
        session(),
        focus(),
        workspaces(),
        windows(),
        keyboard(),
        pointer(),
        shell(),
        panel(),
        theme(),
        decorations(),
    ]
}

fn session() -> Section {
    Section {
        title: "Session",
        about: "The profile every other page starts from, and what a session starts with.",
        switched: false,
        fields: vec![
            Field::new(
                &[],
                "profile",
                "classic is Plasma-like: click to focus, a panel. minimal is Fluxbox-like: \
                 focus follows the pointer, a wallpaper and a right-click menu.",
                Kind::Choice(words(Profile::ALL.map(Profile::key))),
                |c| word(c.profile.key()),
            ),
            Field::new(
                &[],
                "xwayland",
                "X11 applications get an Xwayland",
                Kind::Switch,
                |c| on(c.xwayland),
            ),
            Field::new(
                &[],
                "xdg-autostart",
                "Start the XDG autostart entries, as every desktop does",
                Kind::Switch,
                |c| on(c.xdg_autostart),
            ),
            Field::new(
                &["splash"],
                "enabled",
                "Show the splash while the session starts",
                Kind::Switch,
                |c| on(c.splash),
            ),
        ],
    }
}

fn focus() -> Section {
    Section {
        title: "Focus",
        about: "Which window has the keyboard, and whether pointing at it raises it.",
        switched: false,
        fields: vec![
            Field::new(
                &["focus"],
                "model",
                "click: a click focuses. sloppy: pointing focuses, and the empty desktop \
                 keeps it. strict: only what is under the pointer has it.",
                Kind::Choice(words(FocusModel::ALL.map(FocusModel::key))),
                |c| word(c.focus.model.key()),
            ),
            Field::new(
                &["focus"],
                "autoraise",
                "Raise a window when pointing at it focuses it",
                Kind::Switch,
                |c| on(c.focus.autoraise),
            ),
        ],
    }
}

fn workspaces() -> Section {
    Section {
        title: "Workspaces",
        about: "How many workspaces, and how to move between them with the pointer.",
        switched: false,
        fields: vec![
            Field::new(
                &["workspaces"],
                "mode",
                "One set spanning every monitor, or a set per monitor",
                Kind::Choice(words(Mode::ALL.map(Mode::key))),
                |c| word(c.workspaces.mode.key()),
            ),
            Field::new(
                &["workspaces"],
                "grid",
                "How many workspaces: columns, then rows",
                Kind::Pair {
                    names: ["columns", "rows"],
                    range: span(&limits::GRID_SIDE),
                    unit: "",
                },
                |c| {
                    let grid = c.workspaces.grid;
                    Some(Value::Integers(vec![
                        i64::from(grid.columns),
                        i64::from(grid.rows),
                    ]))
                },
            ),
            Field::new(
                &["workspaces"],
                "wrap",
                "Moving off one edge of the grid comes back on the other",
                Kind::Switch,
                |c| on(c.workspaces.grid.wrap),
            ),
            Field::new(
                &["workspaces", "flip"],
                "edge",
                "Rest the pointer on an edge of the desk to change workspace",
                Kind::Switch,
                |c| on(c.flipping.edge),
            ),
            Field::new(
                &["workspaces", "flip"],
                "delay-ms",
                "How long it rests there first",
                number(&limits::FLIP_DELAY_MS, "milliseconds"),
                |c| int(c.flipping.delay_ms),
            ),
            Field::new(
                &["workspaces", "flip"],
                "while-dragging",
                "Flip while dragging a window, taking it along",
                Kind::Switch,
                |c| on(c.flipping.while_dragging),
            ),
            Field::new(
                &["workspaces", "flip"],
                "scroll",
                "Scroll over the desktop to change workspace",
                Kind::Switch,
                |c| on(c.flipping.scroll),
            ),
            Field::new(
                &["workspaces", "transition"],
                "effect",
                "How a switch is drawn",
                Kind::Choice(words(Effect::ALL.map(Effect::key))),
                |c| word(c.transitioning.effect.key()),
            ),
            Field::new(
                &["workspaces", "transition"],
                "duration-ms",
                "How long a switch takes",
                number(&limits::TRANSITION_MS, "milliseconds"),
                |c| c.transitioning.duration_ms.and_then(int),
            )
            .unset("the effect's own"),
        ],
    }
}

fn windows() -> Section {
    Section {
        title: "Windows",
        about: "Moving windows against edges, dragging them, and how see-through they are.",
        switched: false,
        fields: vec![
            Field::new(
                &["snap"],
                "drag",
                "Drag a window to an edge to give it half or a quarter of a monitor",
                Kind::Switch,
                |c| on(c.snapping.drag),
            ),
            Field::new(
                &["snap"],
                "threshold",
                "How near the edge counts",
                number(&limits::SNAP_THRESHOLD, "pixels"),
                |c| int(c.snapping.threshold),
            ),
            Field::new(
                &["resistance"],
                "edges",
                "How hard a screen's or a panel's edge holds a window moved against it",
                number(&limits::RESISTANCE, "pixels"),
                |c| int(c.resistance.edges),
            ),
            Field::new(
                &["resistance"],
                "seams",
                "How hard the seam between two monitors holds it",
                number(&limits::RESISTANCE, "pixels"),
                |c| int(c.resistance.seams),
            ),
            Field::new(
                &[],
                "drag",
                "The modifiers that drag a window from anywhere on it: the left button \
                 moves it, the right resizes it",
                Kind::Modifiers,
                |c| perspicax_config::spell_modifiers(c.bindings.drag_mods()).map(Value::String),
            ),
            Field::new(
                &["placement"],
                "remember",
                "A program's next window opens where its last one closed",
                Kind::Switch,
                |c| on(c.remembering.on),
            ),
            Field::new(
                &["opacity"],
                "step",
                "How far the opacity keys move a window",
                number(&limits::OPACITY, "percent"),
                |c| int(c.opacity.step),
            ),
            Field::new(
                &["opacity"],
                "floor",
                "The faintest the keys take a window",
                number(&limits::OPACITY, "percent"),
                |c| int(c.opacity.floor),
            ),
            Field::new(
                &["opacity"],
                "unfocused",
                "How much of itself a window without the keyboard shows",
                number(&limits::OPACITY, "percent"),
                |c| int(c.opacity.unfocused),
            ),
        ],
    }
}

fn keyboard() -> Section {
    const TABLE: &[&str] = &["input", "keyboard"];
    Section {
        title: "Keyboard",
        about: "The keyboard's layouts and how a held key repeats.",
        switched: false,
        fields: vec![
            Field::new(
                TABLE,
                "layout",
                "The layouts, in order, by xkb's names",
                Kind::Layouts,
                |c| some_text(&c.keyboard.layout),
            )
            .unset("xkb's default"),
            Field::new(
                TABLE,
                "variant",
                "Each layout's variant, by position",
                Kind::Variants,
                |c| some_text(&c.keyboard.variant),
            )
            .unset("each layout's own"),
            Field::new(
                TABLE,
                "options",
                "xkb's options, such as a key to switch layouts",
                text("grp:alt_shift_toggle"),
                |c| c.keyboard.options.as_deref().and_then(some_text),
            )
            .unset("xkb's default"),
            Field::new(
                TABLE,
                "repeat-rate",
                "How fast a held key repeats",
                number(&limits::REPEAT, "keys a second"),
                |c| int(c.keyboard.repeat_rate),
            ),
            Field::new(
                TABLE,
                "repeat-delay",
                "How long before a held key starts repeating",
                number(&limits::REPEAT, "milliseconds"),
                |c| int(c.keyboard.repeat_delay),
            ),
            Field::new(
                TABLE,
                "numlock",
                "Num Lock when the session starts",
                Kind::Switch,
                |c| c.keyboard.numlock.and_then(on),
            )
            .unset("as it is"),
            Field::new(
                TABLE,
                "switching",
                "Whose layout is in use: the session's, or each window's own",
                Kind::Choice(words(Switching::ALL.map(Switching::key))),
                |c| word(c.keyboard.switching.key()),
            ),
            Field::new(TABLE, "rules", "xkb's rules", text("evdev"), |c| {
                some_text(&c.keyboard.rules)
            })
            .unset("xkb's default"),
            Field::new(TABLE, "model", "xkb's keyboard model", text("pc105"), |c| {
                some_text(&c.keyboard.model)
            })
            .unset("xkb's default"),
        ],
    }
}

fn pointer() -> Section {
    const TABLE: &[&str] = &["input", "pointer"];
    const LIBINPUT: &str = "the device's own";
    Section {
        title: "Pointer",
        about: "Mice and touchpads, and how fast a double-click is.",
        switched: false,
        fields: vec![
            Field::new(
                TABLE,
                "accel",
                "Pointer speed, from slowest to fastest",
                Kind::Fraction {
                    range: limits::ACCEL,
                },
                |c| c.pointer.accel.map(Value::Float),
            )
            .unset(LIBINPUT),
            Field::new(
                TABLE,
                "natural-scroll",
                "Scroll the content, not the view, as a touchscreen does",
                Kind::Switch,
                |c| c.pointer.natural_scroll.and_then(on),
            )
            .unset(LIBINPUT),
            Field::new(
                TABLE,
                "tap-to-click",
                "A tap on a touchpad clicks",
                Kind::Switch,
                |c| c.pointer.tap_to_click.and_then(on),
            )
            .unset(LIBINPUT),
            Field::new(
                TABLE,
                "left-handed",
                "Swap the left and right buttons",
                Kind::Switch,
                |c| c.pointer.left_handed.and_then(on),
            )
            .unset(LIBINPUT),
            Field::new(
                TABLE,
                "double-click-ms",
                "How soon a second click makes a double-click",
                number(&limits::DOUBLE_CLICK_MS, "milliseconds"),
                |c| int(c.pointer.double_click_ms),
            ),
        ],
    }
}

fn shell() -> Section {
    const TABLE: &[&str] = &["shell"];
    fn command(leaf: &str, says: &'static str, read: fn(&Config) -> &Vec<String>) -> Field {
        Field::new(TABLE, leaf, says, Kind::Command, move |c| {
            Some(Value::Strings(read(c).clone()))
        })
    }
    Section {
        title: "Shell",
        about: "perspicax-shell: the wallpaper, the desktop, the menus.",
        switched: true,
        fields: vec![
            Field::new(
                TABLE,
                "enabled",
                "Start the shell. Off leaves the desktop to what autostart starts",
                Kind::Switch,
                |c| on(c.shell.enabled),
            ),
            Field::new(
                TABLE,
                "wallpaper",
                "A colour, as #1e4a73, an image's path, or none",
                text("#1e4a73"),
                |c| match &c.shell.wallpaper {
                    None => word("none"),
                    Some(wallpaper) => match &wallpaper.image {
                        Some(image) => word(&image.to_string_lossy()),
                        None => Some(Value::String(wallpaper.colour.hex())),
                    },
                },
            ),
            Field::new(
                TABLE,
                "wallpaper-mode",
                "How an image is fitted to a monitor of another shape",
                Kind::Choice(words(WallpaperMode::ALL.map(WallpaperMode::key))),
                |c| c.shell.wallpaper.as_ref().and_then(|w| word(w.mode.key())),
            )
            .unset("no wallpaper to fit"),
            Field::new(
                TABLE,
                "root-menu",
                "A right-click on the wallpaper opens a menu of applications",
                Kind::Switch,
                |c| on(c.shell.root_menu),
            ),
            Field::new(
                TABLE,
                "menu-file",
                "A menu file that replaces the root menu or adds to it",
                text("~/.config/perspicax/menu.toml"),
                |c| {
                    c.shell
                        .menu_file
                        .as_ref()
                        .and_then(|file| word(&file.to_string_lossy()))
                },
            )
            .unset("the installed applications"),
            Field::new(
                TABLE,
                "desktop-icons",
                "The desktop folder's files, as icons on the wallpaper",
                Kind::Switch,
                |c| on(c.shell.desktop_icons),
            ),
            Field::new(
                TABLE,
                "untrusted-launchers",
                "What the desktop shows of an application's entry that may not be run",
                Kind::Choice(words(UntrustedLaunchers::ALL.map(UntrustedLaunchers::key))),
                |c| word(c.shell.untrusted_launchers.key()),
            ),
            Field::new(
                TABLE,
                "icon-theme",
                "The icon theme, by its folder's name",
                text("Adwaita"),
                |c| c.shell.icon_theme.as_deref().and_then(word),
            )
            .unset("hicolor"),
            Field::new(
                TABLE,
                "terminal",
                "What runs an application that asks for a terminal",
                Kind::Command,
                |c| c.shell.terminal.clone().map(Value::Strings),
            )
            .unset("the first terminal installed"),
            command("lock", "What the menus' Lock runs", |c| &c.shell.lock),
            command("suspend", "What Suspend runs", |c| &c.shell.suspend),
            command("reboot", "What Restart runs", |c| &c.shell.reboot),
            command("power-off", "What Shut Down runs", |c| &c.shell.power_off),
            Field::new(
                TABLE,
                "leave",
                "The ways to leave at the menus' foot, in order",
                Kind::List(words(Leave::ALL.map(Leave::key))),
                |c| {
                    Some(Value::Strings(
                        c.shell
                            .leave
                            .iter()
                            .map(|leave| leave.key().to_owned())
                            .collect(),
                    ))
                },
            ),
        ],
    }
}

fn panel() -> Section {
    const TABLE: &[&str] = &["shell", "panel"];
    let read = |read: fn(&perspicax_config::Panel) -> Option<Value>| {
        move |c: &Config| c.shell.panel.as_ref().and_then(read)
    };
    Section {
        title: "Panel",
        about: "The bar along an edge of each monitor.",
        switched: true,
        fields: vec![
            Field::new(
                TABLE,
                "enabled",
                "A panel. The minimal profile has none until it is turned on",
                Kind::Switch,
                |c| on(c.shell.panel.is_some()),
            ),
            Field::new(
                TABLE,
                "edge",
                "Which edge it runs along",
                Kind::Choice(words(Edge::ALL.map(Edge::key))),
                read(|p| word(p.edge.key())),
            ),
            Field::new(
                TABLE,
                "height",
                "How tall it is",
                number(&limits::PANEL_HEIGHT, "pixels"),
                read(|p| int(p.height)),
            ),
            Field::new(
                TABLE,
                "width",
                "How wide its bar is: pixels, as 600, or a share of the monitor, as 60%",
                Kind::Width,
                read(|p| match p.width {
                    PanelWidth::Pixels(pixels) => int(pixels),
                    PanelWidth::Percent(share) => word(&format!("{share}%")),
                }),
            ),
            Field::new(
                TABLE,
                "align",
                "Where along its edge a narrower bar sits",
                Kind::Choice(words(Align::ALL.map(Align::key))),
                read(|p| word(p.align.key())),
            ),
            Field::new(
                TABLE,
                "outputs",
                "Which monitors have one: all, first, or their names, as DP-1",
                Kind::Outputs,
                read(|p| match &p.outputs {
                    PanelOutputs::All => word("all"),
                    PanelOutputs::First => word("first"),
                    PanelOutputs::Named(names) => Some(Value::Strings(names.clone())),
                }),
            ),
            Field::new(
                TABLE,
                "taskbar",
                "Which windows its taskbar lists",
                Kind::Choice(words(TaskbarScope::ALL.map(TaskbarScope::key))),
                read(|p| word(p.taskbar.key())),
            ),
            Field::new(
                TABLE,
                "task-titles",
                "Each task shows its window's title beside its icon",
                Kind::Switch,
                read(|p| on(p.task_titles)),
            ),
            Field::new(
                TABLE,
                "items",
                "What it holds, from the left",
                Kind::List(words(Item::ALL.map(Item::key))),
                read(|p| {
                    Some(Value::Strings(
                        p.items.iter().map(|item| item.key().to_owned()).collect(),
                    ))
                }),
            ),
            Field::new(
                TABLE,
                "clock",
                "The clock's format, as strftime writes it",
                text("%H:%M"),
                read(|p| word(&p.clock)),
            ),
        ],
    }
}

fn theme() -> Section {
    const TABLE: &[&str] = &["theme"];
    const SAID: &str = "nothing said: each application decides";
    let mut fields = vec![
        Field::new(
            TABLE,
            "name",
            "The theme the colours start from",
            Kind::Choice(words(Builtin::ALL.map(Builtin::name))),
            |c| word(c.theme.builtin.name()),
        ),
        Field::new(
            TABLE,
            "font",
            "The font everything is written in: a family's name, or sans-serif, serif, \
             monospace",
            text("Inter"),
            |c| word(c.theme.font.family.key()),
        ),
        Field::new(
            TABLE,
            "font-size",
            "How big the shell's text is",
            number(&limits::FONT_SIZE, "pixels"),
            |c| int(c.theme.font.size),
        ),
        Field::new(
            TABLE,
            "cursor",
            "The pointer's theme, by its folder's name",
            text("Adwaita"),
            |c| c.theme.cursor.as_deref().and_then(word),
        )
        .unset("XCURSOR_THEME's"),
        Field::new(
            TABLE,
            "cursor-size",
            "How big the pointer is",
            number(&limits::CURSOR_SIZE, "pixels"),
            |c| c.theme.cursor_size.and_then(int),
        )
        .unset("XCURSOR_SIZE's"),
        Field::new(
            TABLE,
            "color-scheme",
            "Whether applications are asked to be dark or light",
            Kind::Choice(words(ColorScheme::ALL.map(ColorScheme::key))),
            |c| {
                c.theme
                    .apps
                    .color_scheme
                    .and_then(|scheme| word(scheme.key()))
            },
        )
        .unset(SAID),
        Field::new(
            TABLE,
            "contrast",
            "Whether applications are asked for more contrast",
            Kind::Choice(words(Contrast::ALL.map(Contrast::key))),
            |c| {
                c.theme
                    .apps
                    .contrast
                    .and_then(|contrast| word(contrast.key()))
            },
        )
        .unset(SAID),
        Field::new(
            TABLE,
            "motion",
            "reduced makes every animation instant",
            Kind::Choice(words(Motion::ALL.map(Motion::key))),
            |c| word(c.theme.motion.key()),
        ),
        Field::new(
            TABLE,
            "logo",
            "What the lock screen and the splash show: perspicax, none, or a PNG's path",
            text("~/pictures/logo.png"),
            |c| match &c.theme.logo {
                perspicax_config::Logo::Perspicax => word("perspicax"),
                perspicax_config::Logo::None => word("none"),
                perspicax_config::Logo::File(path) => word(&path.to_string_lossy()),
            },
        ),
    ];
    fields.extend(Role::ALL.iter().map(|&role| {
        Field::new(
            &["theme", "palette"],
            role.key(),
            "",
            Kind::Colour,
            move |c| Some(Value::String(c.theme.palette[role].hex())),
        )
    }));
    Section {
        title: "Theme",
        about: "What everything perspicax draws looks like, and what applications are told.",
        switched: false,
        fields,
    }
}

fn decorations() -> Section {
    const TABLE: &[&str] = &["decorations"];
    const SHADOW: &[&str] = &["decorations", "shadow"];
    const ANIMATION: &[&str] = &["decorations", "animation"];
    const AGENT: &[&str] = &["decorations", "agent"];
    const ICONS: &[&str] = &["decorations", "icons"];
    let buttons = |read: fn(&Config) -> Vec<FrameButton>| {
        move |c: &Config| {
            Some(Value::Strings(
                read(c)
                    .iter()
                    .map(|button| button.key().to_owned())
                    .collect(),
            ))
        }
    };
    let all_buttons = || Kind::List(words(FrameButton::ALL.map(FrameButton::key)));
    let mut fields = vec![
        Field::new(
            TABLE,
            "mode",
            "server: perspicax draws the frame, unless a window draws its own. client: \
             every window draws its own",
            Kind::Choice(vec!["server", "client"]),
            |c| {
                word(if c.decorations.server {
                    "server"
                } else {
                    "client"
                })
            },
        ),
        Field::new(
            TABLE,
            "title-height",
            "How tall a titlebar is",
            number(&limits::TITLE_HEIGHT, "pixels"),
            |c| int(c.decorations.title),
        ),
        Field::new(
            TABLE,
            "border",
            "How wide the border is",
            number(&limits::BORDER, "pixels"),
            |c| int(c.decorations.border),
        ),
        Field::new(
            TABLE,
            "corner-radius",
            "How round the top corners are",
            number(&limits::CORNER_RADIUS, "pixels"),
            |c| int(c.decorations.radius),
        ),
        Field::new(
            TABLE,
            "left",
            "The titlebar's buttons at the left",
            all_buttons(),
            buttons(|c| c.decorations.buttons.left().collect()),
        ),
        Field::new(
            TABLE,
            "right",
            "The titlebar's buttons at the right",
            all_buttons(),
            buttons(|c| c.decorations.buttons.right().collect()),
        ),
        Field::new(
            SHADOW,
            "size",
            "How far the shadow spreads; 0 draws none",
            number(&limits::SHADOW_SIZE, "pixels"),
            |c| int(c.decorations.shadow.size),
        ),
        Field::new(
            SHADOW,
            "offset",
            "How far it falls: across, then down",
            Kind::Pair {
                names: ["across", "down"],
                range: span(&limits::SHADOW_OFFSET),
                unit: "pixels",
            },
            |c| {
                let (x, y) = c.decorations.shadow.offset;
                Some(Value::Integers(vec![i64::from(x), i64::from(y)]))
            },
        ),
        Field::new(
            SHADOW,
            "opacity",
            "How dark it is at its darkest",
            number(&limits::SHADOW_OPACITY, "percent"),
            |c| int(c.decorations.shadow.opacity),
        ),
        Field::new(SHADOW, "colour", "Its colour", Kind::Colour, |c| {
            Some(Value::String(c.decorations.shadow.colour.hex()))
        }),
        Field::new(
            ANIMATION,
            "shade-ms",
            "How long a window takes to roll up; 0 is at once",
            number(&limits::ANIMATION_MS, "milliseconds"),
            |c| int(c.decorations.animation.shade_ms),
        ),
        Field::new(
            ANIMATION,
            "hover-ms",
            "How long a button takes to light up; 0 is at once",
            number(&limits::ANIMATION_MS, "milliseconds"),
            |c| int(c.decorations.animation.hover_ms),
        ),
        Field::new(
            AGENT,
            "outline",
            "The line around a window an agent may act on; 0 draws none",
            number(&limits::AGENT_OUTLINE, "pixels"),
            |c| int(c.decorations.agent.outline),
        ),
        Field::new(
            AGENT,
            "outline-colour",
            "The line's colour",
            Kind::Colour,
            |c| Some(Value::String(c.decorations.agent.outline_colour.hex())),
        ),
        Field::new(
            AGENT,
            "focused",
            "Its titlebar, with the keyboard",
            Kind::Colour,
            |c| Some(Value::String(c.decorations.agent.focused.hex())),
        ),
        Field::new(
            AGENT,
            "unfocused",
            "Its titlebar, without",
            Kind::Colour,
            |c| Some(Value::String(c.decorations.agent.unfocused.hex())),
        ),
    ];
    fields.extend(Face::ALL.iter().map(|&face| {
        Field::new(
            ICONS,
            face.key(),
            "",
            text("~/.config/perspicax/icons/close.png"),
            move |c| {
                c.icons
                    .file(face)
                    .and_then(|file| word(&file.to_string_lossy()))
            },
        )
        .unset("the built-in shape")
    }));
    fields.push(Field::new(
        ICONS,
        "tint",
        "Draw each icon in the bar's ink, by its shape alone",
        Kind::Switch,
        |c| on(c.icons.tint),
    ));
    Section {
        title: "Decorations",
        about: "Titlebars and borders: their size, their buttons, their shadow.",
        switched: false,
        fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_named(title: &str) -> Section {
        sections()
            .into_iter()
            .find(|section| section.title == title)
            .expect("a section")
    }

    /// The values a field is tried with: each choice, each end of a range,
    /// an example.
    fn samples(kind: &Kind) -> Vec<Value> {
        let string = |text: &str| Value::String(text.to_owned());
        let strings =
            |words: &[&str]| Value::Strings(words.iter().map(|w| (*w).to_owned()).collect());
        match kind {
            Kind::Switch => vec![Value::Bool(true), Value::Bool(false)],
            Kind::Choice(words) => words.iter().map(|word| string(word)).collect(),
            Kind::Number { range, .. } => {
                vec![Value::Integer(*range.start()), Value::Integer(*range.end())]
            }
            Kind::Fraction { range } => {
                vec![Value::Float(*range.start()), Value::Float(*range.end())]
            }
            Kind::Text { example } => vec![string(example)],
            Kind::Colour => vec![string("#123456")],
            Kind::Command => vec![strings(&["foot", "-e", "htop"])],
            Kind::List(words) => vec![strings(words), strings(&words[..1])],
            Kind::Pair { range, .. } => vec![
                Value::Integers(vec![*range.start(), *range.end()]),
                Value::Integers(vec![*range.end(), *range.start()]),
            ],
            Kind::Width => vec![Value::Integer(600), string("60%")],
            Kind::Outputs => vec![string("all"), string("first"), strings(&["DP-1"])],
            Kind::Modifiers => vec![string("Logo"), string("Ctrl+Alt"), string("none")],
            Kind::Layouts => vec![string("us,ru")],
            Kind::Variants => vec![string(",phonetic")],
        }
    }

    /// What another key must say first, for `key` to take `value`: a panel
    /// to configure, a bar narrow enough to align, a floor under the dimming.
    fn needs(key: &Key, value: &Value) -> Vec<Edit> {
        let set = |table: &[&str], leaf: &str, value: Value| Edit::Set {
            key: Key::new(table, leaf),
            value,
        };
        let mut needs = Vec::new();
        if key.table == ["shell", "panel"] && key.leaf != "enabled" {
            needs.push(set(&["shell", "panel"], "enabled", Value::Bool(true)));
        }
        match format!("{}.{}", key.table.join("."), key.leaf).as_str() {
            "shell.panel.align" => {
                needs.push(set(
                    &["shell", "panel"],
                    "width",
                    Value::String("50%".to_owned()),
                ));
            }
            "opacity.unfocused" => {
                needs.push(set(&["opacity"], "floor", Value::Integer(1)));
            }
            "opacity.floor" => {
                if let Value::Integer(floor) = value {
                    needs.push(set(&["opacity"], "step", Value::Integer(*floor)));
                    needs.push(set(&["opacity"], "unfocused", Value::Integer(100)));
                }
            }
            "input.keyboard.variant" => {
                needs.push(set(
                    &["input", "keyboard"],
                    "layout",
                    Value::String("us,ru".to_owned()),
                ));
            }
            _ => {}
        }
        needs
    }

    fn config(text: &str) -> Config {
        perspicax_config::parse(text, edit::EVERYTHING).unwrap_or_else(|e| panic!("{text}\n{e}"))
    }

    #[test]
    fn every_field_reads_back_what_is_written() {
        for profile in Profile::ALL {
            let start = format!("profile = \"{}\"\n", profile.key());
            for section in sections() {
                for field in &section.fields {
                    for value in samples(&field.kind) {
                        let mut edits = needs(&field.key, &value);
                        edits.push(field.set(value.clone()));
                        let text = edit::edited(&start, &edits).unwrap();
                        assert_eq!(
                            field.read(&config(&text)),
                            Some(value),
                            "{}.{} under {profile:?}:\n{text}",
                            field.key.table.join("."),
                            field.key.leaf
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_number_takes_its_ends_and_refuses_one_past_them() {
        for section in sections() {
            for field in &section.fields {
                let (Kind::Number { range, .. } | Kind::Pair { range, .. }) = &field.kind else {
                    continue;
                };
                let pair = matches!(field.kind, Kind::Pair { .. });
                let value = |n: i64| {
                    if pair {
                        Value::Integers(vec![n, *range.start()])
                    } else {
                        Value::Integer(n)
                    }
                };
                for n in [*range.start(), *range.end()] {
                    let mut edits = needs(&field.key, &value(n));
                    edits.push(field.set(value(n)));
                    let text = edit::edited("", &edits).unwrap();
                    assert!(
                        perspicax_config::parse(&text, edit::EVERYTHING).is_ok(),
                        "{} takes {n}",
                        field.key.leaf
                    );
                    assert!(field.kind.parse(&field.kind.text(&value(n))).is_ok());
                }
                for n in [range.start() - 1, range.end().saturating_add(1)] {
                    let mut edits = needs(&field.key, &value(n));
                    edits.push(field.set(value(n)));
                    let text = edit::edited("", &edits).unwrap();
                    assert!(
                        perspicax_config::parse(&text, edit::EVERYTHING).is_err(),
                        "{} refuses {n}:\n{text}",
                        field.key.leaf
                    );
                    assert!(
                        field.kind.parse(&field.kind.text(&value(n))).is_err(),
                        "the app refuses {n} for {} too",
                        field.key.leaf
                    );
                }
            }
        }
    }

    /// `section`'s setting for `leaf`, as `text` stands.
    fn setting(text: &str, title: &str, table: &[&str], leaf: &str) -> Setting {
        let section = section_named(title);
        let at = section
            .fields
            .iter()
            .position(|field| field.key == Key::new(table, leaf))
            .expect("a field");
        settings(text, &config(text), &section).swap_remove(at)
    }

    #[test]
    fn an_unwritten_key_is_the_profiles_and_a_written_one_names_the_profiles_value() {
        let text = "[snap]\nthreshold = 20   # mine\n";
        let threshold = setting(text, "Windows", &["snap"], "threshold");
        assert_eq!(threshold.written, Some(Value::Integer(20)));
        assert_eq!(threshold.in_force, Some(Value::Integer(20)));
        let classic = config("").snapping.threshold;
        assert_eq!(
            threshold.profile,
            Ok(Some(Value::Integer(i64::from(classic))))
        );
        let drag = setting(text, "Windows", &["snap"], "drag");
        assert_eq!(drag.written, None);
        assert_eq!(drag.profile, Ok(drag.in_force.clone()));
        assert_eq!(drag.in_force, Some(Value::Bool(true)), "classic snaps");
    }

    #[test]
    fn the_profiles_value_is_what_resetting_gives() {
        let text = "[shell]\nwallpaper = \"none\"\ndesktop-icons = false\n";
        assert!(config("").shell.desktop_icons, "classic has icons");
        let icons = setting(text, "Shell", &["shell"], "desktop-icons");
        assert_eq!(
            icons.profile,
            Ok(Some(Value::Bool(false))),
            "with no wallpaper, the profile's icons give way"
        );
    }

    #[test]
    fn a_value_another_setting_overrides_is_told_apart() {
        let text = "[theme]\nmotion = \"reduced\"\n\n[workspaces.transition]\neffect = \"slide\"\n";
        let effect = setting(text, "Workspaces", &["workspaces", "transition"], "effect");
        assert_eq!(effect.written, Some(Value::String("slide".to_owned())));
        assert_eq!(effect.in_force, Some(Value::String("none".to_owned())));
        assert!(effect.overridden());
        assert_eq!(
            effect.shown(),
            effect.written.as_ref(),
            "the field shows the file"
        );
    }

    #[test]
    fn a_value_written_in_other_words_is_not_told_apart() {
        let text = "drag = \"Super\"\n\n[theme.palette]\naccent = \"#E93D5A\"\n\n\
                    [input.pointer]\naccel = 1\n\n[shell.panel]\nwidth = \" 60 %\"\nalign = \"left\"\n";
        for (title, table, leaf, words) in [
            (
                "Theme",
                &["theme", "palette"][..],
                "accent",
                Value::String("#e93d5a".to_owned()),
            ),
            ("Windows", &[][..], "drag", Value::String("Logo".to_owned())),
            (
                "Pointer",
                &["input", "pointer"][..],
                "accel",
                Value::Float(1.0),
            ),
            (
                "Panel",
                &["shell", "panel"][..],
                "width",
                Value::String("60%".to_owned()),
            ),
        ] {
            let read = setting(text, title, table, leaf);
            assert_eq!(read.written, Some(words), "{leaf}");
            assert!(!read.overridden(), "{leaf}: {read:?}");
        }
    }

    #[test]
    fn resetting_a_key_another_one_needs_is_refused_up_front() {
        let minimal = "profile = \"minimal\"\n\n[shell.panel]\nenabled = true\nheight = 30\n";
        let enabled = setting(minimal, "Panel", &["shell", "panel"], "enabled");
        assert!(
            enabled
                .profile
                .as_ref()
                .is_err_and(|why| why.contains("shell.panel")),
            "{enabled:?}"
        );
        let narrow = "[shell.panel]\nwidth = \"50%\"\nalign = \"left\"\n";
        let width = setting(narrow, "Panel", &["shell", "panel"], "width");
        assert!(
            width
                .profile
                .as_ref()
                .is_err_and(|why| why.contains("align")),
            "{width:?}"
        );
    }

    #[test]
    fn the_panel_under_minimal_is_a_section_switched_off() {
        let text = "profile = \"minimal\"\n";
        let panel = section_named("Panel");
        assert!(panel.switched);
        let read = settings(text, &config(text), &panel);
        assert_eq!(read[0].in_force, Some(Value::Bool(false)));
        assert!(
            read[1..].iter().all(|setting| setting.in_force.is_none()),
            "{read:?}"
        );
        let classic = settings("", &config(""), &panel);
        assert_eq!(classic[0].in_force, Some(Value::Bool(true)));
    }

    #[test]
    fn a_number_outside_its_range_is_refused_in_the_parsers_words() {
        let threshold = section_named("Windows")
            .fields
            .into_iter()
            .find(|field| field.key.leaf == "threshold")
            .unwrap();
        let ours = threshold.kind.parse("99").unwrap_err();
        let theirs =
            perspicax_config::parse("[snap]\nthreshold = 99\n", edit::EVERYTHING).unwrap_err();
        assert_eq!(theirs.to_string(), format!("`snap.threshold`: {ours}"));
        assert_eq!(
            threshold.kind.parse("lots").unwrap_err(),
            "`lots` is not a whole number"
        );
    }

    #[test]
    fn a_command_is_split_and_joined_as_a_shell_would() {
        let words = Kind::Command.parse("foot -e 'htop -d 5'").unwrap();
        let expected: Vec<String> = ["foot", "-e", "htop -d 5"].map(str::to_owned).to_vec();
        assert_eq!(words, Value::Strings(expected));
        assert_eq!(Kind::Command.text(&words), "foot -e 'htop -d 5'");
        assert!(Kind::Command.parse("foot -e 'htop").is_err());
        assert!(Kind::Command.parse("  ").is_err());
    }

    #[test]
    fn a_width_is_pixels_or_a_share_of_the_monitor() {
        assert_eq!(Kind::Width.parse("600"), Ok(Value::Integer(600)));
        assert_eq!(
            Kind::Width.parse(" 60 % "),
            Ok(Value::String("60%".to_owned()))
        );
        for wrong in ["0", "0%", "101%", "wide", "-3"] {
            assert!(Kind::Width.parse(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn an_unwritten_palette_colour_says_what_it_follows() {
        let text = "[decorations]\nfocused = \"#440000\"\n\n[theme.palette]\npanel = \"#202020\"\n";
        let panel_ink = setting(text, "Theme", &["theme", "palette"], "panel-ink");
        assert_eq!(panel_ink.source, Some(Source::Follows(Role::Panel)));
        let title = setting(text, "Theme", &["theme", "palette"], "title-focused");
        assert_eq!(
            title.source,
            Some(Source::WrittenAt(Key::new(&["decorations"], "focused")))
        );
        assert_eq!(title.in_force, Some(Value::String("#440000".to_owned())));
        let ink = setting(text, "Theme", &["theme", "palette"], "title-focused-ink");
        assert_eq!(ink.source, Some(Source::Follows(Role::TitleFocused)));
        let menu = setting(text, "Theme", &["theme", "palette"], "menu");
        assert_eq!(menu.source, None);
    }

    /// dave's own file: every key it writes that a page shows is read as
    /// written, and in force as written.
    #[test]
    fn every_key_a_person_wrote_reads_as_written() {
        let text = include_str!("../tests/fixtures/commented.toml");
        let root: toml::Table = text.parse().unwrap();
        let config = config(text);
        let mut seen = 0;
        for section in sections() {
            for (field, setting) in section.fields.iter().zip(settings(text, &config, &section)) {
                if at(&root, &field.key).is_none() {
                    assert_eq!(setting.written, None);
                    continue;
                }
                seen += 1;
                assert!(setting.written.is_some(), "{:?}", field.key);
                assert!(!setting.overridden(), "{:?}: {setting:?}", field.key);
            }
        }
        assert!(seen > 20, "the file writes {seen} of the pages' keys");
    }

    #[test]
    fn clearing_a_field_that_can_be_unset_resets_it() {
        let cursor = section_named("Theme")
            .fields
            .into_iter()
            .find(|field| field.key.leaf == "cursor")
            .unwrap();
        assert_eq!(cursor.edit("  "), Ok(cursor.reset()));
        let clock = section_named("Panel")
            .fields
            .into_iter()
            .find(|field| field.key.leaf == "clock")
            .unwrap();
        assert_eq!(clock.edit(""), Ok(clock.set(Value::String(String::new()))));
    }
}
