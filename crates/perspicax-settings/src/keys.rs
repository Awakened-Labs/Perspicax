//! The Keys page's rows: every chord the session binds, and every one the
//! file writes, each marked with whose it is.
//!
//! What the profile binds is read from the file with its `[keys]` taken
//! out, so it is the profile as this file has it: with the shell off, the
//! profile's volume keys are not the profile's to give. What the file
//! writes is read from its `[keys]` entries, in the file's order and
//! spelling, each found in the bindings in force by the chord it parses as.

use perspicax_config::Trigger;
use perspicax_policy::{Action, Bindings};
use toml_edit::DocumentMut;

use crate::edit::EVERYTHING;

/// One chord, or the Logo tap: what the profile binds it to, and what the
/// file writes for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub trigger: Trigger,
    /// As the file spells it, or else as the app would write it.
    pub written_as: String,
    /// What the profile binds it to, if anything.
    pub profile: Option<Action>,
    /// What the file says for it: `None` where it says nothing, and
    /// `Some(None)` where it says `"none"`.
    pub written: Option<Option<Action>>,
}

/// Whose a binding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// The profile's, and the file says nothing of it.
    Profile,
    /// The file binds it, over what the profile binds it to.
    Overridden,
    /// The file binds it, and the profile binds it to nothing else: the
    /// person's own.
    Own,
    /// The file says `"none"`, so the key goes to the window.
    Unbound,
}

impl Binding {
    /// Whose it is.
    #[must_use]
    pub fn mark(&self) -> Mark {
        match &self.written {
            None => Mark::Profile,
            Some(None) => Mark::Unbound,
            Some(Some(action)) if self.profile.as_ref().is_some_and(|ours| ours != action) => {
                Mark::Overridden
            }
            Some(Some(_)) => Mark::Own,
        }
    }

    /// What it does now, if anything.
    #[must_use]
    pub fn action(&self) -> Option<&Action> {
        match &self.written {
            Some(written) => written.as_ref(),
            None => self.profile.as_ref(),
        }
    }
}

/// The rows for the file `text`: the Logo tap first, then the profile's
/// chords in the order it binds them, then the person's own in the file's
/// order.
///
/// # Errors
///
/// What perspicax refuses the file for: a file it refuses has no bindings
/// in force to show.
pub fn bindings(text: &str) -> Result<Vec<Binding>, perspicax_config::Error> {
    let in_force = perspicax_config::parse(text, EVERYTHING)?.bindings;
    let profile = perspicax_config::parse(&without_keys(text), EVERYTHING)?.bindings;
    let written = written(text);
    let says = |trigger: Trigger| {
        written
            .iter()
            .find(|(seen, _)| *seen == trigger)
            .map(|(_, text)| text.clone())
    };
    let row = |trigger: Trigger| Binding {
        trigger,
        written_as: says(trigger)
            .or_else(|| perspicax_config::spell_trigger(trigger))
            .unwrap_or_else(|| format!("{trigger:?}")),
        profile: bound(&profile, trigger).cloned(),
        written: says(trigger).map(|_| bound(&in_force, trigger).cloned()),
    };
    let tap =
        (profile.tap().is_some() || says(Trigger::LogoTap).is_some()).then_some(Trigger::LogoTap);
    let profiles = profile.iter().map(|(chord, _)| Trigger::Chord(*chord));
    let own = written
        .iter()
        .map(|&(trigger, _)| trigger)
        .filter(|&trigger| trigger != Trigger::LogoTap && bound(&profile, trigger).is_none());
    Ok(tap
        .into_iter()
        .chain(profiles)
        .chain(own)
        .map(row)
        .collect())
}

/// What `bindings` binds `trigger` to, chord for chord.
fn bound(bindings: &Bindings, trigger: Trigger) -> Option<&Action> {
    match trigger {
        Trigger::LogoTap => bindings.tap(),
        Trigger::Chord(chord) => bindings
            .iter()
            .find(|(bound, _)| **bound == chord)
            .map(|(_, action)| action),
    }
}

/// The file's `[keys]` entries, in its order and spelling, each with the
/// trigger it parses as. One that does not parse is left out: a file
/// perspicax takes has none.
fn written(text: &str) -> Vec<(Trigger, String)> {
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return Vec::new();
    };
    let Some(keys) = doc.get("keys").and_then(|keys| keys.as_table_like()) else {
        return Vec::new();
    };
    keys.iter()
        .filter_map(|(written, _)| {
            let trigger = perspicax_config::trigger(written).ok()?;
            Some((trigger, written.to_owned()))
        })
        .collect()
}

/// `text` with its `[keys]` gone, however it was written: what the profile
/// binds, as this file has the rest.
fn without_keys(text: &str) -> String {
    let Ok(mut doc) = text.parse::<DocumentMut>() else {
        return text.to_owned();
    };
    doc.remove("keys");
    doc.to_string()
}

#[cfg(test)]
mod tests {
    use perspicax_config::Profile;
    use perspicax_policy::{Chord, Keysym, Mods};

    use super::*;

    fn chord(mods: Mods, key: Keysym) -> Trigger {
        Trigger::Chord(Chord { mods, key })
    }

    fn logo() -> Mods {
        Mods {
            logo: true,
            ..Mods::default()
        }
    }

    fn row(rows: &[Binding], trigger: Trigger) -> &Binding {
        rows.iter()
            .find(|row| row.trigger == trigger)
            .unwrap_or_else(|| panic!("no row for {trigger:?} in {rows:#?}"))
    }

    fn spawn(program: &str) -> Action {
        Action::Spawn {
            command: vec![program.to_owned()],
            agent: false,
        }
    }

    #[test]
    fn the_keys_page_marks_the_profiles_and_the_persons_bindings() {
        let text = "[keys]\n\
                    \"Logo+t\" = { spawn = [\"foot\"] }   # mine\n\
                    \"Alt+F4\" = \"minimize\"\n\
                    \"Alt+Tab\" = \"none\"\n\
                    \"Logo+q\" = \"none\"\n";
        let rows = bindings(text).unwrap();
        let alt_f4 = row(&rows, chord(Mods::alt(), Keysym::F4));
        assert_eq!(alt_f4.mark(), Mark::Overridden);
        assert_eq!(alt_f4.profile, Some(Action::Close));
        assert_eq!(alt_f4.action(), Some(&Action::Minimize));
        let alt_tab = row(&rows, chord(Mods::alt(), Keysym::Tab));
        assert_eq!(alt_tab.mark(), Mark::Unbound);
        assert_eq!(alt_tab.action(), None);
        let logo_t = row(&rows, chord(logo(), Keysym::t));
        assert_eq!(logo_t.mark(), Mark::Own);
        assert_eq!(logo_t.action(), Some(&spawn("foot")));
        let logo_q = row(&rows, chord(logo(), Keysym::q));
        assert_eq!(
            (logo_q.mark(), logo_q.profile.as_ref()),
            (Mark::Unbound, None)
        );
        let tap = row(&rows, Trigger::LogoTap);
        assert_eq!(tap.mark(), Mark::Profile);
        assert_eq!(tap.action(), Some(&Action::StartMenu));
        assert_eq!(rows[0].trigger, Trigger::LogoTap, "the tap comes first");
        let own: Vec<&str> = rows[rows.len() - 2..]
            .iter()
            .map(|row| row.written_as.as_str())
            .collect();
        assert_eq!(
            own,
            ["Logo+t", "Logo+q"],
            "the person's own come last, as written"
        );
    }

    #[test]
    fn a_binding_written_at_the_profiles_value_is_the_persons_own() {
        let rows = bindings("[keys]\n\"Alt+F4\" = \"close\"\n").unwrap();
        assert_eq!(row(&rows, chord(Mods::alt(), Keysym::F4)).mark(), Mark::Own);
    }

    #[test]
    fn a_binding_written_with_another_spelling_is_found_by_its_chord() {
        let text = "[keys]\n\"Super+Tab\" = \"close\"\n";
        let profile = perspicax_config::Config::profile(Profile::Classic, EVERYTHING).bindings;
        let logo_tab = chord(logo(), Keysym::Tab);
        assert!(
            bound(&profile, logo_tab).is_some(),
            "classic binds Logo+Tab"
        );
        let rows = bindings(text).unwrap();
        let found = row(&rows, logo_tab);
        assert_eq!(found.written_as, "Super+Tab");
        assert_eq!(found.mark(), Mark::Overridden);
        assert_eq!(
            rows.iter().filter(|row| row.trigger == logo_tab).count(),
            1,
            "{rows:#?}"
        );
    }

    #[test]
    fn the_logo_tap_is_a_row_of_its_own() {
        let rows = bindings("[keys]\n\"Super\" = \"root-menu\"\n").unwrap();
        let taps: Vec<&Binding> = rows
            .iter()
            .filter(|row| row.trigger == Trigger::LogoTap)
            .collect();
        assert_eq!(taps.len(), 1, "{rows:#?}");
        assert_eq!(taps[0].written_as, "Super");
        assert_eq!(taps[0].profile, Some(Action::StartMenu));
        assert_eq!(taps[0].action(), Some(&Action::RootMenu));
        let minimal = bindings("profile = \"minimal\"\n").unwrap();
        assert!(
            minimal.iter().all(|row| row.trigger != Trigger::LogoTap),
            "minimal taps nothing, and the file says nothing of it"
        );
    }

    #[test]
    fn with_the_shell_off_the_profiles_volume_keys_are_not_listed() {
        let volume = |rows: &[Binding]| {
            rows.iter()
                .filter(|row| matches!(row.profile, Some(Action::Volume(_))))
                .count()
        };
        assert!(
            volume(&bindings("").unwrap()) > 0,
            "classic binds the volume keys"
        );
        let rows = bindings("[shell]\nenabled = false\n").unwrap();
        assert_eq!(volume(&rows), 0, "{rows:#?}");
    }

    #[test]
    fn every_profile_binding_has_a_spelling() {
        for profile in ["classic", "minimal"] {
            for row in bindings(&format!("profile = \"{profile}\"\n")).unwrap() {
                assert_eq!(
                    perspicax_config::spell_trigger(row.trigger).as_ref(),
                    Some(&row.written_as),
                    "{profile}: {row:?}"
                );
            }
        }
    }

    #[test]
    fn a_file_perspicax_refuses_has_no_rows() {
        assert!(bindings("[keys]\n\"Ctrl+Alt+F2\" = \"close\"\n").is_err());
        assert!(bindings("[keys\n").is_err());
    }
}
