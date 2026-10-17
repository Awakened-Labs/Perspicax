//! What a person keeps of the sound's devices between sessions: which they
//! hid, and what they named them.
//!
//! Each is kept by key: a jack's `<card>/<port>`, a sink's or a source's
//! name, or for a whole card, the card's name, which hides each of its
//! jacks at once, those it gains later too. A hidden device is listed in
//! the mixer only under "Hidden", and the panel's output piece never moves
//! to it; hiding the one the sound is on leaves the sound there. State, not
//! config: the person sets it in the mixer, never in a file, and the shell
//! keeps it as `mixer.json` in perspicax's state folder (see `wl::kept`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::sound::Choice;

/// What the file says first, so that one a later perspicax writes some
/// other way is not guessed at.
const VERSION: u32 = 1;

/// What is kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Kept {
    /// The keys hidden: devices', and cards'.
    pub(crate) hidden: BTreeSet<String>,
    /// The person's own names, by key.
    pub(crate) names: BTreeMap<String, String>,
}

/// The file.
#[derive(Debug, Serialize, Deserialize)]
struct Written {
    version: u32,
    #[serde(default)]
    hidden: BTreeSet<String>,
    #[serde(default)]
    names: BTreeMap<String, String>,
}

/// Why a file was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unread {
    /// It is not what this perspicax writes, and why.
    Garbled(String),
    /// It is another perspicax's, of this version.
    Version(u32),
}

impl Kept {
    /// What `text`, a file this perspicax wrote, keeps.
    pub(crate) fn read(text: &str) -> Result<Self, Unread> {
        let written: Written =
            serde_json::from_str(text).map_err(|error| Unread::Garbled(error.to_string()))?;
        if written.version != VERSION {
            return Err(Unread::Version(written.version));
        }
        Ok(Self {
            hidden: written.hidden,
            names: written.names,
        })
    }

    /// It, as the file says it.
    pub(crate) fn written(&self) -> String {
        let written = Written {
            version: VERSION,
            hidden: self.hidden.clone(),
            names: self.names.clone(),
        };
        let mut text = serde_json::to_string_pretty(&written).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Whether the device keyed `key`, of the card named `card` if it is a
    /// card's, is hidden: it, or its whole card.
    pub(crate) fn hides(&self, key: &str, card: Option<&str>) -> bool {
        self.hidden.contains(key) || card.is_some_and(|card| self.hidden.contains(card))
    }

    /// What the panel calls `choice`: the person's name for it, else its
    /// own short one.
    pub(crate) fn called(&self, choice: &Choice) -> String {
        self.name(&choice.key).unwrap_or(&choice.short).to_owned()
    }

    /// Whether `choice` is hidden, itself or with its card.
    pub(crate) fn hides_choice(&self, choice: &Choice) -> bool {
        self.hides(&choice.key, choice.card.as_deref())
    }

    /// The name the person gave what is keyed `key`, if they gave one.
    pub(crate) fn name(&self, key: &str) -> Option<&str> {
        self.names.get(key).map(String::as_str)
    }

    /// Hide what is keyed `key`, a device or a whole card, or bring it
    /// back. Whether that changed anything.
    pub(crate) fn hide(&mut self, key: &str, hidden: bool) -> bool {
        if hidden {
            self.hidden.insert(key.to_owned())
        } else {
            self.hidden.remove(key)
        }
    }

    /// Name what is keyed `key` `name`; a blank name forgets the one it
    /// had, and it goes by its own again. Whether that changed anything.
    pub(crate) fn rename(&mut self, key: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            self.names.remove(key).is_some()
        } else {
            self.names
                .insert(key.to_owned(), name.to_owned())
                .as_deref()
                != Some(name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HDMI: &str = "alsa_card.pci-0000_01_00.1";

    #[test]
    fn hiding_a_card_hides_each_of_its_outputs_and_a_device_only_itself() {
        let mut kept = Kept::default();
        assert!(kept.hide(HDMI, true));
        assert!(!kept.hide(HDMI, true), "hidden already");
        let jack = |port: &str| format!("{HDMI}/{port}");
        assert!(kept.hides(&jack("hdmi-output-0"), Some(HDMI)));
        assert!(kept.hides(&jack("hdmi-output-3"), Some(HDMI)), "every one");
        assert!(!kept.hides(
            "alsa_card.pci-0000_00_1f.3/analog-output-speaker",
            Some("alsa_card.pci-0000_00_1f.3")
        ));
        kept.hide("two", true);
        assert!(kept.hides("two", None), "a sink of no card");
        assert!(!kept.hides("one", None));
        assert!(kept.hide(HDMI, false));
        assert!(
            !kept.hides(&jack("hdmi-output-0"), Some(HDMI)),
            "brought back"
        );
    }

    #[test]
    fn a_choice_is_called_by_the_persons_name_and_hidden_with_its_card() {
        let choice = Choice {
            key: format!("{HDMI}/hdmi-output-1"),
            device: "TU104 HD Audio Controller".to_owned(),
            jack: Some("HDMI / DisplayPort 2".to_owned()),
            short: "DELL U2720Q".to_owned(),
            kind: crate::model::sound::Kind::Display,
            connected: true,
            card: Some(HDMI.to_owned()),
            details: Vec::new(),
        };
        let mut kept = Kept::default();
        assert_eq!(kept.called(&choice), "DELL U2720Q", "its own");
        kept.rename(&choice.key, "Desk");
        assert_eq!(kept.called(&choice), "Desk");
        assert!(!kept.hides_choice(&choice));
        kept.hide(HDMI, true);
        assert!(kept.hides_choice(&choice), "with its card");
    }

    #[test]
    fn a_name_is_kept_trimmed_and_a_blank_one_forgets_it() {
        let mut kept = Kept::default();
        let key = "bluez_card.FC_58_FA_A9_62_22/headset-output";
        assert!(kept.rename(key, "  Headset "));
        assert_eq!(kept.name(key), Some("Headset"));
        assert!(!kept.rename(key, "Headset"), "the same");
        assert!(kept.rename(key, "   "));
        assert_eq!(kept.name(key), None, "its own again");
        assert!(!kept.rename(key, ""), "nothing to forget");
    }

    #[test]
    fn what_is_written_is_read_back_and_another_version_or_garble_is_not() {
        let mut kept = Kept::default();
        kept.hide(HDMI, true);
        kept.rename("bluez_card.FC_58_FA_A9_62_22/headset-output", "Headset");
        let text = kept.written();
        assert!(text.starts_with("{\n  \"version\": 1,"), "{text}");
        assert!(text.ends_with('\n'));
        assert_eq!(Kept::read(&text), Ok(kept));
        assert_eq!(
            Kept::read(r#"{"version": 2, "hidden": ["x"]}"#),
            Err(Unread::Version(2))
        );
        assert!(matches!(
            Kept::read("{\"version\""),
            Err(Unread::Garbled(_))
        ));
        assert_eq!(
            Kept::read(r#"{"version": 1}"#),
            Ok(Kept::default()),
            "nothing kept yet"
        );
    }
}
