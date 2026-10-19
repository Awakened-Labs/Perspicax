//! The monitors' rules, `[[output]]`, as the Outputs page stages and writes
//! them: each entry as the file writes it, the edits that make the file say
//! another set, and what the compositor would make of a set.
//!
//! A rule is kept in the person's words: a mode written `2560x1440@144`
//! stays so until it is changed, though the monitor's own is 143.912 Hz.
//! What a rule does to a monitor is read by the compositor's own rules,
//! [`pick_mode`] and [`arrange`], so the page shows what a session would.

use perspicax_config::Mode;
use perspicax_policy::{Head, HeadMode, Place, Screen, Side, arrange, pick_mode};

use crate::{Edit, Key, Value};

/// One `[[output]]` entry, as the file writes it: each key's value, or
/// nothing where the entry leaves it to the compositor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rule {
    /// The connector: `DP-1`, `eDP-1`.
    pub name: String,
    /// `false` leaves the monitor dark; nothing is on.
    pub enable: Option<bool>,
    /// As written; nothing is the monitor's preferred mode.
    pub mode: Option<String>,
    pub place: Placed,
    /// Nothing is 1.
    pub scale: Option<f64>,
}

/// Where a rule puts its monitor, as the file writes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Placed {
    /// None of the keys: to the right of the others.
    #[default]
    Auto,
    /// `position = [x, y]`.
    At(i32, i32),
    /// `right-of = "DP-1"`, and an `offset` along the shared edge where one
    /// is written.
    Beside {
        side: Side,
        of: String,
        offset: Option<i32>,
    },
}

/// Where one monitor goes on the desk, in logical pixels, as the compositor
/// would put it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arranged {
    pub name: String,
    pub at: (i32, i32),
    pub size: (i32, i32),
}

impl Rule {
    /// A rule for `name` that says nothing: its monitor on, at its preferred
    /// mode, to the right of the others, at scale 1.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    /// Whether it leaves its monitor on.
    #[must_use]
    pub fn on(&self) -> bool {
        self.enable != Some(false)
    }

    /// Where it puts its monitor, as the compositor reads it.
    #[must_use]
    pub fn place(&self) -> Place {
        match &self.place {
            Placed::Auto => Place::Auto,
            Placed::At(x, y) => Place::At(*x, *y),
            Placed::Beside { side, of, offset } => Place::Beside {
                side: *side,
                of: of.clone(),
                offset: offset.unwrap_or(0),
            },
        }
    }

    /// Its keys, as the file writes them, in the order a person writes them:
    /// whether it is on, its mode, where it goes, and its scale.
    fn keys(&self) -> Vec<(&'static str, Value)> {
        let mut keys = Vec::new();
        if let Some(enable) = self.enable {
            keys.push(("enable", Value::Bool(enable)));
        }
        if let Some(mode) = &self.mode {
            keys.push(("mode", Value::String(mode.clone())));
        }
        match &self.place {
            Placed::Auto => {}
            Placed::At(x, y) => {
                keys.push(("position", Value::Integers(vec![(*x).into(), (*y).into()])));
            }
            Placed::Beside { side, of, offset } => {
                keys.push((side.key(), Value::String(of.clone())));
                if let Some(offset) = offset {
                    keys.push(("offset", Value::Integer((*offset).into())));
                }
            }
        }
        if let Some(scale) = self.scale {
            keys.push(("scale", Value::Float(scale)));
        }
        keys
    }
}

/// The file's `[[output]]` entries, in its order, each read as far as it
/// can be: an entry with no name is no rule. A file perspicax refuses is
/// the session's to say so of; this reads what it says.
#[must_use]
pub fn rules(text: &str) -> Vec<Rule> {
    let Ok(root) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(entries) = root.get("output").and_then(toml::Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(toml::Value::as_table)
        .filter_map(rule)
        .collect()
}

fn rule(entry: &toml::Table) -> Option<Rule> {
    let int = |key: &str| {
        entry
            .get(key)
            .and_then(toml::Value::as_integer)
            .and_then(|n| i32::try_from(n).ok())
    };
    let string = |key: &str| {
        entry
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    let position = entry
        .get("position")
        .and_then(toml::Value::as_array)
        .and_then(|xy| match xy.as_slice() {
            [x, y] => Some((
                i32::try_from(x.as_integer()?).ok()?,
                i32::try_from(y.as_integer()?).ok()?,
            )),
            _ => None,
        });
    let beside = Side::ALL
        .into_iter()
        .find_map(|side| Some((side, string(side.key())?)));
    let place = match (position, beside) {
        (Some((x, y)), _) => Placed::At(x, y),
        (None, Some((side, of))) => Placed::Beside {
            side,
            of,
            offset: int("offset"),
        },
        (None, None) => Placed::Auto,
    };
    Some(Rule {
        name: string(crate::layout::NAME)?,
        enable: entry.get("enable").and_then(toml::Value::as_bool),
        mode: string("mode"),
        place,
        scale: entry.get("scale").and_then(|scale| {
            scale
                .as_float()
                .or_else(|| scale.as_integer().map(|whole| whole as f64))
        }),
    })
}

/// The edits that make a file whose rules are `from` say `to`, key by key:
/// a line the person wrote is changed where it stands and keeps its
/// comment, and a key `to` no longer says is taken out. A rule `to` lacks
/// goes whole. Within a rule, each key that goes is taken out before any is
/// written, so a monitor moved from a position to beside another is never
/// written in both places.
#[must_use]
pub fn edits(from: &[Rule], to: &[Rule]) -> Vec<Edit> {
    let key = |name: &str, leaf: &str| Key::new(&["output", name], leaf);
    let mut edits = Vec::new();
    for rule in to {
        let old = from
            .iter()
            .find(|old| old.name == rule.name)
            .map(Rule::keys)
            .unwrap_or_default();
        let new = rule.keys();
        for (leaf, _) in &old {
            if !new.iter().any(|(kept, _)| kept == leaf) {
                edits.push(Edit::Reset {
                    key: key(&rule.name, leaf),
                });
            }
        }
        for (leaf, value) in new {
            if !old.contains(&(leaf, value.clone())) {
                edits.push(Edit::Set {
                    key: key(&rule.name, leaf),
                    value,
                });
            }
        }
    }
    for gone in from
        .iter()
        .filter(|old| !to.iter().any(|rule| rule.name == old.name))
    {
        edits.push(Edit::Reset {
            key: Key::new(&["output"], &gone.name),
        });
    }
    edits
}

/// `mode` as `[[output]] mode` writes it, its refresh in Hz with as many
/// digits as it needs: `2560x1440@143.912`, `1920x1080@60`. A mode of no
/// known refresh is its size alone.
#[must_use]
pub fn mode_word(mode: &HeadMode) -> String {
    let size = format!("{}x{}", mode.width, mode.height);
    if mode.refresh <= 0 {
        return size;
    }
    let (whole, part) = (mode.refresh / 1000, mode.refresh % 1000);
    if part == 0 {
        format!("{size}@{whole}")
    } else {
        let part = format!("{part:03}");
        format!("{size}@{whole}.{}", part.trim_end_matches('0'))
    }
}

/// `mode` as a person reads it: `2560 × 1440 at 143.91 Hz`.
#[must_use]
pub fn mode_shown(mode: &HeadMode) -> String {
    let size = format!("{} × {}", mode.width, mode.height);
    if mode.refresh <= 0 {
        return size;
    }
    let hertz = format!("{:.2}", mode.hertz());
    let hertz = hertz.trim_end_matches('0').trim_end_matches('.');
    format!("{size} at {hertz} Hz")
}

/// Which of `modes` a monitor with `rule` shows, by index, and whether that
/// is the mode the rule asks for: one the monitor lacks, or one that is no
/// mode at all, falls back to its preferred. `None` for a monitor offering
/// none.
#[must_use]
pub fn picked(rule: Option<&Rule>, modes: &[HeadMode]) -> Option<(usize, bool)> {
    match rule.and_then(|rule| rule.mode.as_deref()) {
        None => pick_mode(None, modes),
        Some(written) => match written.parse::<Mode>() {
            Ok(mode) => pick_mode(Some((mode.width, mode.height, mode.refresh)), modes),
            Err(_) => pick_mode(None, modes).map(|(at, _)| (at, false)),
        },
    }
}

/// Whether `rules` leave one of the `connected` monitors on. A monitor with
/// no rule is on.
#[must_use]
pub fn leaves_one_on(rules: &[Rule], connected: &[&str]) -> bool {
    connected.iter().any(|name| {
        rules
            .iter()
            .find(|rule| rule.name == *name)
            .is_none_or(Rule::on)
    })
}

/// Where each of `heads` would go under `rules`, as the compositor places
/// them: every monitor the rules leave on, in the order they are listed, at
/// its picked mode's size over its scale, through [`arrange`].
#[must_use]
pub fn arranged(rules: &[Rule], heads: &[Head]) -> Vec<Arranged> {
    let lit: Vec<(&str, (i32, i32), Place)> = heads
        .iter()
        .filter_map(|head| {
            let rule = rules.iter().find(|rule| rule.name == head.name);
            if !rule.is_none_or(Rule::on) {
                return None;
            }
            let (at, _) = picked(rule, &head.modes)?;
            let mode = head.modes[at];
            let scale = rule.and_then(|rule| rule.scale).unwrap_or(1.0);
            // A monitor's size over a scale of at least 0.5 fits an i32.
            let logical = |pixels: i32| (f64::from(pixels) / scale).ceil() as i32;
            let size = (logical(mode.width), logical(mode.height));
            Some((
                head.name.as_str(),
                size,
                rule.map(Rule::place).unwrap_or_default(),
            ))
        })
        .collect();
    let screens: Vec<Screen<'_>> = lit
        .iter()
        .map(|(name, size, place)| Screen {
            name,
            size: *size,
            place,
        })
        .collect();
    lit.iter()
        .zip(arrange(&screens))
        .map(|((name, size, _), at)| Arranged {
            name: (*name).to_owned(),
            at,
            size: *size,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: i32, height: i32, refresh: i32, preferred: bool) -> HeadMode {
        HeadMode {
            width,
            height,
            refresh,
            preferred,
        }
    }

    fn head(name: &str, modes: Vec<HeadMode>) -> Head {
        Head {
            name: name.to_owned(),
            enabled: true,
            modes,
            current: Some(0),
            position: (0, 0),
            scale: 1.0,
        }
    }

    #[test]
    fn rules_read_what_the_file_writes() {
        let text = "[[output]]\nname = \"DP-1\"\nmode = \"2560x1440@144\"\n\n\
                    [[output]]\nname = \"HDMI-A-1\"\nright-of = \"DP-1\"\noffset = 180\nscale = 1.25\n\n\
                    [[output]]\nname = \"eDP-1\"\nenable = false\nposition = [0, 1440]\nscale = 2\n";
        assert_eq!(
            rules(text),
            [
                Rule {
                    mode: Some("2560x1440@144".to_owned()),
                    ..Rule::new("DP-1")
                },
                Rule {
                    place: Placed::Beside {
                        side: Side::RightOf,
                        of: "DP-1".to_owned(),
                        offset: Some(180),
                    },
                    scale: Some(1.25),
                    ..Rule::new("HDMI-A-1")
                },
                Rule {
                    enable: Some(false),
                    place: Placed::At(0, 1440),
                    scale: Some(2.0),
                    ..Rule::new("eDP-1")
                },
            ]
        );
    }

    #[test]
    fn an_unchanged_rule_makes_no_edit() {
        let text = "[[output]]\nname = \"DP-1\"\nenable = true\nbelow = \"eDP-1\"\noffset = 0\n";
        let rules = rules(text);
        assert_eq!(edits(&rules, &rules), []);
        assert_eq!(
            edits(&[], &[Rule::new("DP-1")]),
            [],
            "a rule saying nothing"
        );
    }

    #[test]
    fn a_rule_gone_is_taken_out_whole_and_a_key_gone_alone() {
        let from = [
            Rule {
                scale: Some(2.0),
                mode: Some("1920x1080".to_owned()),
                ..Rule::new("DP-1")
            },
            Rule {
                enable: Some(false),
                ..Rule::new("eDP-1")
            },
        ];
        let to = [Rule {
            mode: Some("1920x1080".to_owned()),
            ..Rule::new("DP-1")
        }];
        assert_eq!(
            edits(&from, &to),
            [
                Edit::Reset {
                    key: Key::new(&["output", "DP-1"], "scale")
                },
                Edit::Reset {
                    key: Key::new(&["output"], "eDP-1")
                },
            ]
        );
    }

    #[test]
    fn a_mode_word_reads_back_as_the_mode_offered() {
        let modes = [
            mode(2560, 1440, 59_951, true),
            mode(2560, 1440, 143_912, false),
            mode(2560, 1440, 120_000, false),
            mode(1920, 1080, 60_000, false),
            mode(1920, 1080, 50_000, false),
        ];
        assert_eq!(mode_word(&modes[1]), "2560x1440@143.912");
        assert_eq!(mode_word(&modes[3]), "1920x1080@60");
        assert_eq!(mode_word(&mode(800, 600, 0, false)), "800x600");
        for (at, offered) in modes.iter().enumerate() {
            let rule = Rule {
                mode: Some(mode_word(offered)),
                ..Rule::new("DP-1")
            };
            assert_eq!(picked(Some(&rule), &modes), Some((at, true)), "{rule:?}");
        }
    }

    #[test]
    fn a_mode_is_shown_with_its_refresh_rounded() {
        assert_eq!(
            mode_shown(&mode(2560, 1440, 143_912, false)),
            "2560 × 1440 at 143.91 Hz"
        );
        assert_eq!(
            mode_shown(&mode(1920, 1080, 60_000, false)),
            "1920 × 1080 at 60 Hz"
        );
    }

    #[test]
    fn a_mode_the_monitor_lacks_shows_its_preferred_and_says_so() {
        let modes = [
            mode(1920, 1080, 60_000, false),
            mode(2560, 1440, 59_951, true),
        ];
        let lacking = Rule {
            mode: Some("800x600".to_owned()),
            ..Rule::new("DP-1")
        };
        assert_eq!(picked(Some(&lacking), &modes), Some((1, false)));
        let garbled = Rule {
            mode: Some("big".to_owned()),
            ..Rule::new("DP-1")
        };
        assert_eq!(picked(Some(&garbled), &modes), Some((1, false)));
        assert_eq!(picked(None, &modes), Some((1, true)));
    }

    #[test]
    fn turning_every_connected_monitor_off_is_told() {
        let off = |name: &str| Rule {
            enable: Some(false),
            ..Rule::new(name)
        };
        let connected = ["DP-1", "eDP-1"];
        assert!(leaves_one_on(&[off("DP-1")], &connected));
        assert!(leaves_one_on(&[off("DP-1"), off("HDMI-A-1")], &connected));
        assert!(!leaves_one_on(&[off("DP-1"), off("eDP-1")], &connected));
    }

    #[test]
    fn the_preview_places_monitors_as_the_compositor_would() {
        let big = head("DP-1", vec![mode(2560, 1440, 144_000, true)]);
        let laptop = head("eDP-1", vec![mode(1920, 1200, 60_000, true)]);
        let side = head("HDMI-A-1", vec![mode(1920, 1080, 60_000, true)]);
        let rules = [
            Rule {
                place: Placed::Beside {
                    side: Side::Below,
                    of: "DP-1".to_owned(),
                    offset: Some(300),
                },
                scale: Some(1.5),
                ..Rule::new("eDP-1")
            },
            Rule {
                enable: Some(false),
                ..Rule::new("HDMI-A-1")
            },
        ];
        assert_eq!(
            arranged(&rules, &[big, laptop, side]),
            [
                Arranged {
                    name: "DP-1".to_owned(),
                    at: (0, 0),
                    size: (2560, 1440),
                },
                Arranged {
                    name: "eDP-1".to_owned(),
                    at: (300, 1440),
                    size: (1280, 800),
                },
            ]
        );
    }
}
