//! The sound, as the sound server tells it: its cards, their outputs, and
//! how loud each is.
//!
//! Kept apart from the protocol it is read in, so that what the panel shows
//! of it is worked out, and tested, as plain values. The thread that talks
//! to the server fills these in (see `audio`), and the shell's loop only
//! ever sees them whole.
//!
//! An output is what a person chooses to hear the sound from: a jack of a
//! card, its port, or a sink of no card at all. Mixers list sinks, named for
//! the card and its profile ("Built-in Audio Analog Stereo"), which never
//! say which jack; here each is named for its card and its jack, and for the
//! monitor plugged into it or how it is attached, where that tells it apart:
//! "Built-in Audio — Speakers", "E8 — Headphones · Bluetooth", "TU104 HD
//! Audio Controller — HDMI / DisplayPort 2 · DELL U2720Q". Each has a key
//! that stays its own across reboots, its card's name and its port's,
//! `alsa_card.pci-0000_01_00.1/hdmi-output-1`, or a sink's name.
//!
//! Choosing an output whose card plays something else is a change of the
//! card's profile first, and its sink comes after: a [`Plan`] says what to
//! do, and the thread does it as the server follows.

/// A volume of 100%, as the server counts it.
pub(crate) const NORM: u32 = 0x10000;

/// What the server holds, as last read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The output sound goes to unless a program says otherwise, by name.
    pub(crate) default_sink: Option<String>,
    pub(crate) sinks: Vec<Sink>,
    pub(crate) cards: Vec<Card>,
}

/// An output the server plays to: a sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sink {
    /// The server's number for it, while it lasts.
    pub(crate) index: u32,
    /// Its name, which stays its own:
    /// `alsa_output.pci-0000_00_1f.3.analog-stereo`.
    pub(crate) name: String,
    /// What it calls itself: "Built-in Audio Analog Stereo".
    pub(crate) description: String,
    /// Each channel's volume, as the server counts it: [`NORM`] is 100%.
    pub(crate) volume: Vec<u32>,
    pub(crate) muted: bool,
    /// The number of the card it is of, if it is of one.
    pub(crate) card: Option<u32>,
    /// Its card's ports it can play to, by name, and the one it plays to.
    pub(crate) ports: Vec<String>,
    pub(crate) port: Option<String>,
}

/// A sound card, or a Bluetooth device: what plays, by its profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Card {
    /// The server's number for it, while it lasts.
    pub(crate) index: u32,
    /// Its name, which stays its own: `alsa_card.pci-0000_01_00.1`, or for
    /// a Bluetooth device one made of its address,
    /// `bluez_card.FC_58_FA_A9_62_22`.
    pub(crate) name: String,
    /// What it calls itself: "Built-in Audio", "TU104 HD Audio Controller".
    pub(crate) description: String,
    /// How it is attached, `device.bus`: "pci", "usb", "bluetooth".
    pub(crate) bus: Option<String>,
    pub(crate) ports: Vec<Port>,
    pub(crate) profiles: Vec<Profile>,
    /// The profile it plays by, by name.
    pub(crate) profile: Option<String>,
}

/// One of a card's jacks: a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Port {
    /// Its name, which stays its own: `hdmi-output-1`.
    pub(crate) name: String,
    /// What it calls itself: "Speakers", "HDMI / DisplayPort 2".
    pub(crate) description: String,
    pub(crate) kind: Kind,
    /// It plays sound out, rather than taking it in.
    pub(crate) output: bool,
    /// Whether something is plugged into it, as far as the card can tell.
    pub(crate) plugged: Plugged,
    /// The profiles that play through it, by name.
    pub(crate) profiles: Vec<String>,
    /// The monitor plugged into it, for an HDMI or DisplayPort jack whose
    /// monitor said its name: "DELL U2720Q".
    pub(crate) monitor: Option<String>,
}

/// Whether something is plugged into a jack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plugged {
    Yes,
    No,
    /// The card cannot tell, as of built-in speakers.
    Unknown,
}

/// What a jack is for, as its glyph shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Speaker,
    Headphones,
    /// Headphones with a microphone.
    Headset,
    /// A monitor or a television, over HDMI or DisplayPort.
    Display,
    Other,
}

/// A way a card can play: which of its jacks, with how many sinks and
/// sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Profile {
    pub(crate) name: String,
    /// Higher is the card's own choice.
    pub(crate) priority: u32,
    /// Whether it can play now: an HDMI profile with no monitor cannot.
    pub(crate) available: bool,
    pub(crate) sinks: u32,
    pub(crate) sources: u32,
}

/// An output a person can choose: a card's jack, or a sink of no card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Output {
    /// What it is known by, across reboots: `<card>/<port>`, or a sink's
    /// name.
    pub(crate) key: String,
    /// Its card and its jack, and the monitor or the way it is attached:
    /// "E8 — Headphones · Bluetooth".
    pub(crate) long: String,
    /// What the panel calls it: the monitor's name, or an attached device's
    /// own, or the jack's: "DELL U2720Q", "E8", "Speakers".
    pub(crate) short: String,
    pub(crate) kind: Kind,
    /// Whether it can be heard now: nothing is known to be unplugged.
    pub(crate) connected: bool,
}

/// How to make an output the one sound goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Make this sink, of no card, the default.
    Sink(String),
    /// Set `profile` on the card, if it is given, then make the card's sink
    /// that plays through `port` the default, playing through it.
    Port {
        card: u32,
        card_name: String,
        profile: Option<String>,
        port: String,
    },
}

/// How loud a device is, and whether it is muted: what the panel shows of
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Level {
    /// Its loudest channel, in percent of [`NORM`].
    pub(crate) percent: u32,
    pub(crate) muted: bool,
}

impl Facts {
    /// The default output's sink, if the server has one it can name.
    pub(crate) fn output(&self) -> Option<&Sink> {
        let name = self.default_sink.as_deref()?;
        self.sinks.iter().find(|sink| sink.name == name)
    }

    /// Every output, each card's jacks in the card's order, the likeliest
    /// first, then each sink of no card, or of no jack of its card's.
    pub(crate) fn outputs(&self) -> Vec<Output> {
        let jacks = self.cards.iter().flat_map(|card| {
            card.ports
                .iter()
                .filter(|port| port.output)
                .map(move |port| card.output(port))
        });
        let loose = self
            .sinks
            .iter()
            .filter(|sink| self.jack_of(sink).is_none())
            .map(|sink| Output {
                key: sink.name.clone(),
                long: sink.description.clone(),
                short: sink.description.clone(),
                kind: Kind::Other,
                connected: true,
            });
        jacks.chain(loose).collect()
    }

    /// The key of the output the default sink plays to.
    pub(crate) fn output_key(&self) -> Option<String> {
        let sink = self.output()?;
        Some(match self.jack_of(sink) {
            Some((card, port)) => key(card, port),
            None => sink.name.clone(),
        })
    }

    /// The card and the jack `sink` plays through, if it plays through one.
    fn jack_of(&self, sink: &Sink) -> Option<(&Card, &Port)> {
        let card = self
            .cards
            .iter()
            .find(|card| Some(card.index) == sink.card)?;
        let port = sink.port.as_deref()?;
        let port = card.ports.iter().find(|had| had.name == port)?;
        Some((card, port))
    }

    /// How to make the output of `key` the one sound goes to.
    pub(crate) fn plan(&self, key: &str) -> Option<Plan> {
        for card in &self.cards {
            if let Some(port) = card.ports.iter().find(|port| self::key(card, port) == key) {
                return Some(Plan::Port {
                    card: card.index,
                    card_name: card.name.clone(),
                    profile: card.profile_for(port),
                    port: port.name.clone(),
                });
            }
        }
        self.sinks
            .iter()
            .find(|sink| sink.name == key)
            .map(|sink| Plan::Sink(sink.name.clone()))
    }
}

impl Card {
    /// `port`, as an output of this card.
    fn output(&self, port: &Port) -> Output {
        let attached = match self.bus.as_deref() {
            Some("bluetooth") => Some("Bluetooth"),
            Some("usb") => Some("USB"),
            _ => None,
        };
        let extra = port.monitor.as_deref().or(attached);
        let mut long = format!("{} \u{2014} {}", self.description, port.description);
        if let Some(extra) = extra {
            long.push_str(" \u{b7} ");
            long.push_str(extra);
        }
        let short = match (&port.monitor, attached) {
            (Some(monitor), _) => monitor.clone(),
            (None, Some(_)) => self.description.clone(),
            (None, None) => port.description.clone(),
        };
        Output {
            key: key(self, port),
            long,
            short,
            kind: port.kind,
            connected: port.plugged != Plugged::No,
        }
    }

    /// The profile to set for `port` to play, or `None` if the card's own
    /// plays through it already. Of the profiles that play through it, one
    /// that can play now, then one that keeps the card's microphone, then
    /// the card's own choice.
    fn profile_for(&self, port: &Port) -> Option<String> {
        let plays = |profile: &&Profile| profile.sinks > 0 && port.profiles.contains(&profile.name);
        if self
            .profiles
            .iter()
            .filter(plays)
            .any(|profile| Some(&profile.name) == self.profile.as_ref())
        {
            return None;
        }
        self.profiles
            .iter()
            .filter(plays)
            .max_by_key(|profile| (profile.available, profile.sources > 0, profile.priority))
            .map(|profile| profile.name.clone())
    }
}

/// The key of `card`'s `port`.
fn key(card: &Card, port: &Port) -> String {
    format!("{}/{}", card.name, port.name)
}

/// The output to switch to after the one of key `now`, of `outputs`: the
/// next that is connected, round to the first. `None` when there is no
/// other.
pub(crate) fn next_output<'o>(outputs: &'o [Output], now: Option<&str>) -> Option<&'o Output> {
    let shown: Vec<&Output> = outputs.iter().filter(|output| output.connected).collect();
    let at = shown
        .iter()
        .position(|output| Some(output.key.as_str()) == now);
    match at {
        Some(at) => {
            Some(shown[(at + 1) % shown.len()]).filter(|next| Some(next.key.as_str()) != now)
        }
        None => shown.first().copied(),
    }
}

impl Sink {
    pub(crate) fn level(&self) -> Level {
        Level {
            percent: percent(&self.volume),
            muted: self.muted,
        }
    }
}

/// How loud, in percent, a device whose channels are at `volume` is: its
/// loudest channel's, rounded, which is the number pavucontrol and `pactl`
/// show.
pub(crate) fn percent(volume: &[u32]) -> u32 {
    let loudest = u64::from(volume.iter().copied().max().unwrap_or(0));
    let norm = u64::from(NORM);
    u32::try_from((loudest * 100 + norm / 2) / norm).unwrap_or(u32::MAX)
}

/// The volume, in percent, that `notches` of the wheel (up positive) take
/// `now` to, in steps of `step`: to the next multiple of it that way, so
/// that 24 goes to 25 and then 30. Going up never passes `limit`, and never
/// pulls down a volume already past it, as one set louder elsewhere; going
/// down stops at silence.
pub(crate) fn stepped(now: u32, notches: i32, step: u32, limit: u32) -> u32 {
    let step = step.max(1);
    let by = notches.unsigned_abs().saturating_mul(step);
    if notches > 0 {
        if now >= limit {
            return now;
        }
        (now / step * step).saturating_add(by).min(limit)
    } else {
        now.div_ceil(step).saturating_mul(step).saturating_sub(by)
    }
}

/// `volume`, each channel scaled so that the loudest is at `percent`,
/// keeping the balance between them. A device silent on every channel is
/// put at `percent` on each.
pub(crate) fn scaled(volume: &[u32], percent: u32) -> Vec<u32> {
    let norm = u64::from(NORM);
    let target = (u64::from(percent) * norm + 50) / 100;
    let loudest = u64::from(volume.iter().copied().max().unwrap_or(0));
    volume
        .iter()
        .map(|&channel| {
            let scaled = match loudest {
                0 => target,
                loudest => (u64::from(channel) * target + loudest / 2) / loudest,
            };
            u32::try_from(scaled).unwrap_or(u32::MAX)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink(index: u32, name: &str, volume: &[u32], muted: bool) -> Sink {
        Sink {
            index,
            name: name.to_owned(),
            description: name.to_owned(),
            volume: volume.to_vec(),
            muted,
            card: None,
            ports: Vec::new(),
            port: None,
        }
    }

    fn port(
        name: &str,
        description: &str,
        kind: Kind,
        plugged: Plugged,
        profiles: &[&str],
    ) -> Port {
        Port {
            name: name.to_owned(),
            description: description.to_owned(),
            kind,
            output: true,
            plugged,
            profiles: profiles.iter().map(|&profile| profile.to_owned()).collect(),
            monitor: None,
        }
    }

    fn profile(name: &str, priority: u32, available: bool, sinks: u32, sources: u32) -> Profile {
        Profile {
            name: name.to_owned(),
            priority,
            available,
            sinks,
            sources,
        }
    }

    /// dave's laptop, as pactl lists it: the built-in card playing to its
    /// speakers, the NVIDIA card's HDMI off, a monitor on its second jack,
    /// and the E8 headset the default.
    fn laptop() -> Facts {
        const DUPLEX: &str = "output:analog-stereo+input:analog-stereo";
        const SPDIF_DUPLEX: &str = "output:iec958-stereo+input:analog-stereo";
        let builtin = Card {
            index: 0,
            name: "alsa_card.pci-0000_00_1f.3".to_owned(),
            description: "Built-in Audio".to_owned(),
            bus: Some("pci".to_owned()),
            ports: vec![
                port(
                    "analog-output-speaker",
                    "Speakers",
                    Kind::Speaker,
                    Plugged::Unknown,
                    &[DUPLEX, "output:analog-stereo"],
                ),
                port(
                    "analog-output-headphones",
                    "Headphones",
                    Kind::Headphones,
                    Plugged::No,
                    &[DUPLEX, "output:analog-stereo"],
                ),
                port(
                    "iec958-stereo-output",
                    "Digital Output (S/PDIF)",
                    Kind::Other,
                    Plugged::Unknown,
                    &[SPDIF_DUPLEX, "output:iec958-stereo"],
                ),
                Port {
                    output: false,
                    ..port(
                        "analog-input-internal-mic",
                        "Internal Microphone",
                        Kind::Other,
                        Plugged::Unknown,
                        &[DUPLEX, SPDIF_DUPLEX],
                    )
                },
            ],
            profiles: vec![
                profile("off", 0, true, 0, 0),
                profile(DUPLEX, 6565, true, 1, 1),
                profile("output:analog-stereo", 6500, true, 1, 0),
                profile(SPDIF_DUPLEX, 5565, true, 1, 1),
                // Higher than the profile that keeps the microphone, which is
                // chosen all the same.
                profile("output:iec958-stereo", 5600, true, 1, 0),
            ],
            profile: Some(DUPLEX.to_owned()),
        };
        let nvidia = Card {
            index: 1,
            name: "alsa_card.pci-0000_01_00.1".to_owned(),
            description: "TU104 HD Audio Controller".to_owned(),
            bus: Some("pci".to_owned()),
            ports: vec![
                port(
                    "hdmi-output-0",
                    "HDMI / DisplayPort",
                    Kind::Display,
                    Plugged::No,
                    &["output:hdmi-stereo"],
                ),
                Port {
                    monitor: Some("DELL U2720Q".to_owned()),
                    ..port(
                        "hdmi-output-1",
                        "HDMI / DisplayPort 2",
                        Kind::Display,
                        Plugged::Yes,
                        &[
                            "output:hdmi-stereo-extra1",
                            "output:hdmi-surround-extra1",
                            "output:hdmi-surround71-extra1",
                        ],
                    )
                },
            ],
            profiles: vec![
                profile("off", 0, true, 0, 0),
                profile("output:hdmi-stereo", 5900, false, 1, 0),
                profile("output:hdmi-surround-extra1", 800, true, 1, 0),
                // The card's own choice, but the monitor cannot play it.
                profile("output:hdmi-surround71-extra1", 6000, false, 1, 0),
                profile("output:hdmi-stereo-extra1", 5700, true, 1, 0),
            ],
            profile: Some("off".to_owned()),
        };
        let headset = Card {
            index: 2,
            name: "bluez_card.FC_58_FA_A9_62_22".to_owned(),
            description: "E8".to_owned(),
            bus: Some("bluetooth".to_owned()),
            ports: vec![port(
                "headset-output",
                "Headphones",
                Kind::Headset,
                Plugged::Yes,
                &["a2dp-sink"],
            )],
            profiles: vec![
                profile("off", 0, true, 0, 0),
                profile("a2dp-sink", 40, true, 1, 0),
            ],
            profile: Some("a2dp-sink".to_owned()),
        };
        Facts {
            default_sink: Some("bluez_output.FC_58_FA_A9_62_22.1".to_owned()),
            sinks: vec![
                Sink {
                    card: Some(0),
                    ports: vec![
                        "analog-output-speaker".to_owned(),
                        "analog-output-headphones".to_owned(),
                    ],
                    port: Some("analog-output-speaker".to_owned()),
                    ..sink(
                        51,
                        "alsa_output.pci-0000_00_1f.3.analog-stereo",
                        &[0x3d71, 0x3d71],
                        false,
                    )
                },
                Sink {
                    card: Some(2),
                    ports: vec!["headset-output".to_owned()],
                    port: Some("headset-output".to_owned()),
                    ..sink(
                        566,
                        "bluez_output.FC_58_FA_A9_62_22.1",
                        &[NORM, NORM],
                        false,
                    )
                },
            ],
            cards: vec![builtin, nvidia, headset],
        }
    }

    /// The output of `key` among `facts`'.
    fn output(facts: &Facts, key: &str) -> Output {
        facts
            .outputs()
            .into_iter()
            .find(|output| output.key == key)
            .unwrap_or_else(|| panic!("no output {key}"))
    }

    #[test]
    fn each_output_is_named_for_its_card_and_jack_and_what_tells_it_apart() {
        let facts = laptop();
        let keys: Vec<String> = facts
            .outputs()
            .into_iter()
            .map(|output| output.key)
            .collect();
        assert_eq!(
            keys,
            [
                "alsa_card.pci-0000_00_1f.3/analog-output-speaker",
                "alsa_card.pci-0000_00_1f.3/analog-output-headphones",
                "alsa_card.pci-0000_00_1f.3/iec958-stereo-output",
                "alsa_card.pci-0000_01_00.1/hdmi-output-0",
                "alsa_card.pci-0000_01_00.1/hdmi-output-1",
                "bluez_card.FC_58_FA_A9_62_22/headset-output",
            ],
            "jacks out alone, and no sink twice"
        );
        let speakers = output(&facts, "alsa_card.pci-0000_00_1f.3/analog-output-speaker");
        assert_eq!(
            (
                speakers.long.as_str(),
                speakers.short.as_str(),
                speakers.kind
            ),
            (
                "Built-in Audio \u{2014} Speakers",
                "Speakers",
                Kind::Speaker
            )
        );
        let e8 = output(&facts, "bluez_card.FC_58_FA_A9_62_22/headset-output");
        assert_eq!(
            (e8.long.as_str(), e8.short.as_str()),
            ("E8 \u{2014} Headphones \u{b7} Bluetooth", "E8"),
            "the E8 reads as E8 \u{2014} Headphones \u{b7} Bluetooth"
        );
        let dell = output(&facts, "alsa_card.pci-0000_01_00.1/hdmi-output-1");
        assert_eq!(
            (dell.long.as_str(), dell.short.as_str(), dell.kind),
            (
                "TU104 HD Audio Controller \u{2014} HDMI / DisplayPort 2 \u{b7} DELL U2720Q",
                "DELL U2720Q",
                Kind::Display
            ),
            "an HDMI port with a monitor names the monitor"
        );
    }

    #[test]
    fn a_jack_with_nothing_in_it_is_not_connected() {
        let facts = laptop();
        let connected = |key: &str| output(&facts, key).connected;
        assert!(!connected("alsa_card.pci-0000_01_00.1/hdmi-output-0"));
        assert!(!connected(
            "alsa_card.pci-0000_00_1f.3/analog-output-headphones"
        ));
        assert!(
            connected("alsa_card.pci-0000_00_1f.3/analog-output-speaker"),
            "unknown is heard"
        );
        assert!(connected("alsa_card.pci-0000_01_00.1/hdmi-output-1"));
    }

    #[test]
    fn the_default_output_is_the_jack_its_sink_plays_through() {
        let mut facts = laptop();
        assert_eq!(
            facts.output_key().as_deref(),
            Some("bluez_card.FC_58_FA_A9_62_22/headset-output")
        );
        facts.default_sink = Some("alsa_output.pci-0000_00_1f.3.analog-stereo".to_owned());
        assert_eq!(
            facts.output_key().as_deref(),
            Some("alsa_card.pci-0000_00_1f.3/analog-output-speaker")
        );
    }

    #[test]
    fn a_sink_of_no_card_is_an_output_by_its_name() {
        let facts = Facts {
            default_sink: Some("one".to_owned()),
            sinks: vec![
                sink(1, "one", &[NORM], false),
                sink(2, "two", &[NORM], false),
            ],
            cards: Vec::new(),
        };
        let outputs = facts.outputs();
        assert_eq!(outputs.len(), 2);
        assert_eq!(
            (
                outputs[1].key.as_str(),
                outputs[1].short.as_str(),
                outputs[1].connected
            ),
            ("two", "two", true)
        );
        assert_eq!(facts.output_key().as_deref(), Some("one"));
        assert_eq!(facts.plan("two"), Some(Plan::Sink("two".to_owned())));
    }

    #[test]
    fn a_port_of_the_active_profile_needs_no_profile_change() {
        let facts = laptop();
        assert_eq!(
            facts.plan("alsa_card.pci-0000_00_1f.3/analog-output-speaker"),
            Some(Plan::Port {
                card: 0,
                card_name: "alsa_card.pci-0000_00_1f.3".to_owned(),
                profile: None,
                port: "analog-output-speaker".to_owned(),
            })
        );
    }

    #[test]
    fn the_profile_chosen_keeps_the_microphone_where_one_can() {
        let facts = laptop();
        let Some(Plan::Port { profile, .. }) =
            facts.plan("alsa_card.pci-0000_00_1f.3/iec958-stereo-output")
        else {
            panic!("a port's plan");
        };
        assert_eq!(
            profile.as_deref(),
            Some("output:iec958-stereo+input:analog-stereo")
        );
    }

    #[test]
    fn a_profile_that_can_play_is_chosen_before_the_cards_own_choice() {
        let facts = laptop();
        let Some(Plan::Port { card, profile, .. }) =
            facts.plan("alsa_card.pci-0000_01_00.1/hdmi-output-1")
        else {
            panic!("a port's plan");
        };
        assert_eq!(
            (card, profile.as_deref()),
            (1, Some("output:hdmi-stereo-extra1"))
        );
        assert_eq!(facts.plan("nothing/at-all"), None);
    }

    #[test]
    fn the_next_output_goes_round_the_connected_ones() {
        let facts = laptop();
        let outputs = facts.outputs();
        let next = |now: Option<&str>| next_output(&outputs, now).map(|output| output.key.as_str());
        assert_eq!(
            next(Some("bluez_card.FC_58_FA_A9_62_22/headset-output")),
            Some("alsa_card.pci-0000_00_1f.3/analog-output-speaker"),
            "round to the first"
        );
        assert_eq!(
            next(Some("alsa_card.pci-0000_00_1f.3/analog-output-speaker")),
            Some("alsa_card.pci-0000_00_1f.3/iec958-stereo-output"),
            "past the unplugged headphones"
        );
        assert_eq!(
            next(Some("alsa_card.pci-0000_00_1f.3/iec958-stereo-output")),
            Some("alsa_card.pci-0000_01_00.1/hdmi-output-1"),
            "past the empty HDMI jack"
        );
        assert_eq!(
            next(Some("alsa_card.pci-0000_01_00.1/hdmi-output-0")),
            Some("alsa_card.pci-0000_00_1f.3/analog-output-speaker"),
            "from one not connected, the first"
        );
        assert_eq!(
            next(None),
            Some("alsa_card.pci-0000_00_1f.3/analog-output-speaker")
        );
        let alone = [outputs[0].clone()];
        assert_eq!(
            next_output(&alone, Some(alone[0].key.as_str())),
            None,
            "nothing else to switch to"
        );
    }

    #[test]
    fn a_raw_volume_of_0x3d71_reads_as_24_percent() {
        assert_eq!(percent(&[0x3d71, 0x3d71]), 24);
        assert_eq!(percent(&[NORM]), 100);
        assert_eq!(percent(&[0]), 0);
        assert_eq!(percent(&[]), 0, "no channels is silent");
        assert_eq!(percent(&[NORM / 2, NORM]), 100, "the loudest channel");
        assert_eq!(percent(&[NORM * 3 / 2]), 150, "past 100% too");
        // pactl's own rounding, to the nearest: 327 is just under half a
        // percent, and 328 just over.
        assert_eq!(percent(&[327]), 0);
        assert_eq!(percent(&[328]), 1);
    }

    #[test]
    fn a_notch_snaps_to_the_next_step_and_stops_at_the_limit() {
        let up = |now, notches| stepped(now, notches, 5, 100);
        assert_eq!(up(24, 1), 25, "a notch snaps 24 to 25");
        assert_eq!(up(25, 1), 30);
        assert_eq!(up(24, 2), 30);
        assert_eq!(up(24, -1), 20);
        assert_eq!(up(25, -1), 20);
        assert_eq!(up(98, 1), 100, "a notch up from 98 stops at the limit");
        assert_eq!(up(98, 2), 100, "and two do too");
        assert_eq!(stepped(96, 1, 5, 98), 98, "a limit off the steps");
        assert_eq!(up(100, 1), 100);
        assert_eq!(up(120, 1), 120, "a notch up from 120 leaves it at 120");
        assert_eq!(up(120, -1), 115, "and a notch down takes it down");
        assert_eq!(up(3, -1), 0);
        assert_eq!(up(0, -3), 0, "silence is as low as it goes");
        assert_eq!(up(40, 0), 40);
        assert_eq!(stepped(24, 1, 1, 100), 25);
        assert_eq!(stepped(24, 1, 0, 100), 25, "a step of nothing is one");
    }

    #[test]
    fn scaling_keeps_the_balance_and_puts_the_loudest_where_asked() {
        assert_eq!(percent(&scaled(&[NORM, NORM / 2], 40)), 40);
        let balanced = scaled(&[NORM, NORM / 2], 50);
        assert_eq!(balanced, [NORM / 2, NORM / 4]);
        assert_eq!(scaled(&[0, 0], 30), [19661, 19661], "silent, put level");
        for percent_asked in [0, 1, 24, 99, 100, 150] {
            assert_eq!(
                percent(&scaled(&[0x3d71, 0x3d71], percent_asked)),
                percent_asked
            );
        }
        assert_eq!(scaled(&[], 50), Vec::<u32>::new());
    }

    #[test]
    fn the_output_is_the_default_sink_by_name() {
        let facts = Facts {
            default_sink: Some("two".to_owned()),
            sinks: vec![
                sink(1, "one", &[NORM], false),
                sink(2, "two", &[NORM / 4], true),
            ],
            cards: Vec::new(),
        };
        let output = facts.output().expect("the default");
        assert_eq!(output.index, 2);
        assert_eq!(
            output.level(),
            Level {
                percent: 25,
                muted: true
            }
        );
        let gone = Facts {
            default_sink: Some("three".to_owned()),
            ..facts.clone()
        };
        assert_eq!(gone.output(), None, "a default the server lists no sink of");
        assert_eq!(Facts::default().output(), None);
    }
}
