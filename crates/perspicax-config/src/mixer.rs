//! `[shell.mixer]`: what the panel's `mixer` item shows of the sound.
//!
//! ```toml
//! [shell.panel]
//! items = ["start", "taskbar", "mixer", "clock"]
//!
//! [shell.mixer]
//! panel = ["mute"]
//! ```
//!
//! The item is listed among the panel's `items` as `"mixer"`, and this
//! table says what it is made of: pieces side by side, in the order
//! written, each a control of its own. With no table, it is a speaker that
//! mutes.

use serde::Deserialize;

use crate::{Error, invalid, shell::twice};

/// The mixer, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mixer {
    /// What the panel's `mixer` item shows, in order from the left. Never
    /// empty: an item of nothing is one left out of `items`.
    pub panel: Vec<Piece>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            panel: vec![Piece::Mute],
        }
    }
}

/// One piece of the panel's `mixer` item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Piece {
    /// A speaker, with as many waves as the sound is loud, and crossed out
    /// while it is muted. A click mutes the sound, or brings it back.
    Mute,
}

impl Piece {
    /// Every piece, in the order this crate lists them.
    pub const ALL: [Self; 1] = [Self::Mute];

    /// Its name as `panel` writes it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Mute => "mute",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RawMixer {
    panel: Option<Vec<Piece>>,
}

impl RawMixer {
    /// The written keys, laid over `mixer`.
    pub(crate) fn apply(self, mut mixer: Mixer) -> Result<Mixer, Error> {
        if let Some(panel) = self.panel {
            let key = || "shell.mixer.panel".to_owned();
            if panel.is_empty() {
                return Err(invalid(
                    key(),
                    "an item of nothing; to have none, leave \"mixer\" out of \
                     `[shell.panel] items`"
                        .to_owned(),
                ));
            }
            if let Some(twice) = twice(&panel) {
                return Err(invalid(
                    key(),
                    format!("\"{}\" is listed twice", twice.key()),
                ));
            }
            mixer.panel = panel;
        }
        Ok(mixer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Built, Item, Profile, Shell, ShellBuilt, parse, shell};

    fn mixer(text: &str) -> Result<Mixer, Error> {
        shell(text, ShellBuilt::FULL).map(|shell| shell.mixer)
    }

    #[test]
    fn a_mixer_is_read_with_its_pieces_and_the_compositor_reads_it_whatever_the_shell_has() {
        let text = "[shell.panel]\nitems = [\"mixer\"]\n[shell.mixer]\npanel = [\"mute\"]";
        assert_eq!(mixer(text).unwrap().panel, [Piece::Mute]);
        let panel = shell(text, ShellBuilt::FULL).unwrap().panel.unwrap();
        assert_eq!(panel.items, [Item::Mixer]);
        assert!(parse(text, Built::default()).is_ok());
    }

    #[test]
    fn with_no_table_the_mixer_is_a_speaker_that_mutes_in_either_profile() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let shell = Shell::profile(profile, ShellBuilt::FULL);
            assert_eq!(shell.mixer.panel, [Piece::Mute], "{profile:?}");
        }
        assert_eq!(mixer("").unwrap(), Mixer::default());
    }

    #[test]
    fn what_a_mixer_cannot_be_is_refused_by_its_key() {
        let refused = |text: &str| match mixer(&format!("[shell.mixer]\n{text}")) {
            Err(Error::Invalid { key, reason }) => (key, reason),
            other => panic!("{text}: not refused by key: {other:?}"),
        };
        let (key, reason) = refused("panel = []");
        assert_eq!(key, "shell.mixer.panel");
        assert!(reason.contains("leave \"mixer\" out"), "{reason}");
        let (key, reason) = refused("panel = [\"mute\", \"mute\"]");
        assert_eq!(key, "shell.mixer.panel");
        assert!(reason.contains("\"mute\" is listed twice"), "{reason}");
        for (text, word) in [("panel = [\"loud\"]", "loud"), ("colour = 1", "colour")] {
            match mixer(&format!("[shell.mixer]\n{text}")) {
                Err(Error::Parse(error)) => assert!(error.contains(word), "{error}"),
                other => panic!("{text}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_shell_built_without_audio_names_the_feature_for_the_item_and_the_table() {
        let without = ShellBuilt {
            audio: false,
            ..ShellBuilt::FULL
        };
        for (text, key) in [
            ("[shell.panel]\nitems = [\"mixer\"]", "shell.panel.items"),
            ("[shell.mixer]\npanel = [\"mute\"]", "shell.mixer"),
        ] {
            match shell(text, without) {
                Err(Error::ShellNotBuilt {
                    key: named,
                    feature: "audio",
                }) => assert_eq!(named, key),
                other => panic!("{text}: {other:?}"),
            }
        }
        let classic = Shell::profile(Profile::Classic, without).panel.unwrap();
        assert!(!classic.items.contains(&Item::Mixer));
    }

    #[test]
    fn a_changed_mixer_is_a_changed_shell_so_the_shell_is_told() {
        let one = shell("[shell.mixer]\npanel = [\"mute\"]", ShellBuilt::FULL).unwrap();
        let two = Shell {
            mixer: Mixer { panel: Vec::new() },
            ..one.clone()
        };
        assert_ne!(one, two);
    }
}
