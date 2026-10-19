//! The ways out of a refusal, each checked before it is offered.
//!
//! A refusal names a key, and most often the edit refused is not about that
//! key at all: a switch to `profile = "minimal"` is refused for a
//! `[shell.panel]` the profile has no panel for. So the key named is offered
//! back to its profile, and where perspicax says what it wants instead, that
//! too. Each is offered only when the file it makes would be taken.

use crate::{Edit, Fix, Key, Value, edit};

/// The tables whose keys a person names, not the schema: a chord, an
/// application's id, a pie, a workspace. Such a key can hold a `.`, so a key
/// perspicax names is split after its table, never on its dots.
const NAMED_BY_THE_PERSON: &[&[&str]] = &[
    &["keys"],
    &["opacity", "apps"],
    &["shell", "pie", "menus"],
    &["shell", "wallpapers"],
];

/// The fixes for `error`, which perspicax gave for `edits` made on `text`.
/// Each fix's edits are `edits` and its own, so applying them is the whole
/// of taking it; one that leaves the file as it was is no fix.
pub(crate) fn fixes(text: &str, edits: &[Edit], error: &perspicax_config::Error) -> Vec<Fix> {
    let perspicax_config::Error::Invalid { key, .. } = error else {
        return Vec::new();
    };
    let mut offered = Vec::new();
    if key == "shell.panel" {
        offered.push((
            "Turn the panel on: `enabled = true` in `[shell.panel]`".to_owned(),
            Edit::Set {
                key: Key::new(&["shell", "panel"], "enabled"),
                value: Value::Bool(true),
            },
        ));
    }
    if let Some(named) = named(key) {
        offered.push((
            format!("Reset `{key}` to the profile's"),
            Edit::Reset { key: named },
        ));
    }
    offered
        .into_iter()
        .filter_map(|(says, fix)| {
            let edits: Vec<Edit> = edits.iter().cloned().chain([fix]).collect();
            let fixed = edit::edited(text, &edits).ok()?;
            let taken = perspicax_config::parse(&fixed, edit::EVERYTHING).is_ok();
            (taken && fixed != text).then_some(Fix { says, edits })
        })
        .collect()
}

/// The key perspicax's `key` names: `keys.Logo+.` is `Logo+.` in `[keys]`,
/// and `mouse.desktop.Mouse4` is `Mouse4` in `[mouse.desktop]`.
fn named(key: &str) -> Option<Key> {
    for table in NAMED_BY_THE_PERSON {
        if let Some(leaf) = key.strip_prefix(&format!("{}.", table.join("."))) {
            return Some(Key::new(table, leaf));
        }
    }
    if let Some(rest) = key.strip_prefix("mouse.") {
        let (context, leaf) = rest.split_once('.')?;
        return Some(Key::new(&["mouse", context], leaf));
    }
    let path: Vec<&str> = key.split('.').collect();
    let (leaf, table) = path.split_last()?;
    Some(Key::new(table, leaf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_named_by_the_person_is_split_after_its_table_never_on_its_dots() {
        assert_eq!(named("keys.Logo+."), Some(Key::new(&["keys"], "Logo+.")));
        assert_eq!(
            named("opacity.apps.org.gnome.Nautilus"),
            Some(Key::new(&["opacity", "apps"], "org.gnome.Nautilus"))
        );
        assert_eq!(
            named("mouse.desktop.Logo+Mouse4"),
            Some(Key::new(&["mouse", "desktop"], "Logo+Mouse4"))
        );
        assert_eq!(named("shell.panel"), Some(Key::new(&["shell"], "panel")));
        assert_eq!(named("profile"), Some(Key::new(&[], "profile")));
    }

    #[test]
    fn a_reset_that_undoes_the_refused_edit_is_not_offered() {
        let edits = [Edit::Set {
            key: Key::new(&["decorations"], "border"),
            value: Value::Integer(99),
        }];
        let error = perspicax_config::Error::Invalid {
            key: "decorations.border".to_owned(),
            reason: "99 is outside 0 to 8 pixels".to_owned(),
        };
        assert_eq!(fixes("", &edits, &error), []);
    }

    #[test]
    fn a_fix_the_file_would_still_be_refused_with_is_not_offered() {
        // Resetting `mode` leaves the border perspicax refuses next.
        let text = "[decorations]\nmode = \"server\"\nborder = 99\n";
        let error = perspicax_config::Error::Invalid {
            key: "decorations.mode".to_owned(),
            reason: "a stand-in".to_owned(),
        };
        assert_eq!(fixes(text, &[], &error), []);
        let error = perspicax_config::Error::Invalid {
            key: "decorations.border".to_owned(),
            reason: "99 is outside 0 to 8 pixels".to_owned(),
        };
        let offered = fixes(text, &[], &error);
        assert_eq!(
            offered,
            [Fix {
                says: "Reset `decorations.border` to the profile's".to_owned(),
                edits: vec![Edit::Reset {
                    key: Key::new(&["decorations"], "border"),
                }],
            }]
        );
    }
}
