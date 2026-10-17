//! `[shell.mixer]`: what the panel's `mixer` item shows of the sound.
//!
//! ```toml
//! [shell.panel]
//! items = ["start", "taskbar", "mixer", "clock"]
//!
//! [shell.mixer]
//! panel = ["mute", "output"]
//! step = 5       # percent a turn of the wheel moves the volume, 1 to 25
//! limit = 100    # the most a turn takes it to, 1 to 150
//! ```
//!
//! The item is listed among the panel's `items` as `"mixer"`, and this
//! table says what it is made of: pieces side by side, in the order
//! written, each a control of its own. With no table, it is a speaker that
//! mutes. On any piece, the wheel moves the volume by `step`, to the next
//! multiple of it, and a middle click mutes: the output's, or on the
//! microphone, the input's.

use serde::Deserialize;

use crate::{Error, invalid, shell::twice};

/// The mixer, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mixer {
    /// What the panel's `mixer` item shows, in order from the left. Never
    /// empty: an item of nothing is one left out of `items`.
    pub panel: Vec<Piece>,
    /// How far, in percent, a notch of the wheel moves a volume: to the
    /// next multiple of it.
    pub step: u32,
    /// The loudest, in percent, a turn of the wheel takes a volume to. A
    /// volume set louder elsewhere is left there.
    pub limit: u32,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            panel: vec![Piece::Mute],
            step: 5,
            limit: 100,
        }
    }
}

/// The least and most `step` and `limit` may be: past these they are
/// typos, not settings. Half again as loud as full is as far as PulseAudio's
/// own mixer goes.
const STEP: (u32, u32) = (1, 25);
const LIMIT: (u32, u32) = (1, 150);

/// One piece of the panel's `mixer` item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Piece {
    /// A speaker, with as many waves as the sound is loud, and crossed out
    /// while it is muted. A click mutes the sound, or brings it back.
    Mute,
    /// How loud the sound is, in percent, faint while it is muted.
    Volume,
    /// What the sound plays from, by its glyph and its name: "Speakers",
    /// "E8". A click switches to the next output there is.
    Output,
    /// Bars moving with what plays, as winamp's did: how loud each band of
    /// pitch is, the deep ones at the left. Still when nothing plays.
    Levels,
    /// A microphone, crossed out while it is muted. A click mutes it, or
    /// brings it back; the wheel over it moves its volume.
    Mic,
}

impl Piece {
    /// Every piece, in the order this crate lists them.
    pub const ALL: [Self; 5] = [
        Self::Mute,
        Self::Volume,
        Self::Output,
        Self::Levels,
        Self::Mic,
    ];

    /// Its name as `panel` writes it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Mute => "mute",
            Self::Volume => "volume",
            Self::Output => "output",
            Self::Levels => "levels",
            Self::Mic => "mic",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RawMixer {
    panel: Option<Vec<Piece>>,
    step: Option<u32>,
    limit: Option<u32>,
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
        if let Some(step) = self.step {
            mixer.step = within("shell.mixer.step", step, STEP)?;
        }
        if let Some(limit) = self.limit {
            mixer.limit = within("shell.mixer.limit", limit, LIMIT)?;
        }
        Ok(mixer)
    }
}

/// `written` at `key`, if it is from `least` to `most` percent.
fn within(key: &str, written: u32, (least, most): (u32, u32)) -> Result<u32, Error> {
    if (least..=most).contains(&written) {
        Ok(written)
    } else {
        Err(invalid(
            key.to_owned(),
            format!("{written} is outside {least} to {most} percent"),
        ))
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
        let text = "[shell.panel]\nitems = [\"mixer\"]\n[shell.mixer]\npanel = [\"volume\", \"mute\", \"output\", \"levels\", \"mic\"]\nstep = 2\nlimit = 150";
        let read = mixer(text).unwrap();
        assert_eq!(
            read.panel,
            [
                Piece::Volume,
                Piece::Mute,
                Piece::Output,
                Piece::Levels,
                Piece::Mic
            ]
        );
        assert_eq!((read.step, read.limit), (2, 150));
        let panel = shell(text, ShellBuilt::FULL).unwrap().panel.unwrap();
        assert_eq!(panel.items, [Item::Mixer]);
        assert!(parse(text, Built::default()).is_ok());
    }

    #[test]
    fn with_no_table_the_mixer_is_a_speaker_that_mutes_in_either_profile() {
        for profile in [Profile::Classic, Profile::Minimal] {
            let shell = Shell::profile(profile, ShellBuilt::FULL);
            assert_eq!(shell.mixer.panel, [Piece::Mute], "{profile:?}");
            assert_eq!((shell.mixer.step, shell.mixer.limit), (5, 100));
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
        for (text, key, reason) in [
            (
                "step = 0",
                "shell.mixer.step",
                "0 is outside 1 to 25 percent",
            ),
            (
                "step = 26",
                "shell.mixer.step",
                "26 is outside 1 to 25 percent",
            ),
            (
                "limit = 0",
                "shell.mixer.limit",
                "0 is outside 1 to 150 percent",
            ),
            (
                "limit = 151",
                "shell.mixer.limit",
                "151 is outside 1 to 150 percent",
            ),
        ] {
            assert_eq!(refused(text), (key.to_owned(), reason.to_owned()));
        }
        assert_eq!(
            mixer("[shell.mixer]\nstep = 25\nlimit = 1").unwrap().step,
            25
        );
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
        let two = shell(
            "[shell.mixer]\npanel = [\"mute\", \"volume\"]",
            ShellBuilt::FULL,
        );
        let three = shell(
            "[shell.mixer]\npanel = [\"mute\"]\nstep = 2",
            ShellBuilt::FULL,
        );
        assert_ne!(one, two.unwrap());
        assert_ne!(one, three.unwrap());
    }
}
