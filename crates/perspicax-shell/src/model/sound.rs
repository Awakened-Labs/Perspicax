//! The sound, as the sound server tells it: its cards, their outputs and
//! inputs, what programs play, and how loud each is.
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
//! A program's stream is named as the program names itself, else for its
//! file, else for what it plays; the shell's own are no program's.
//!
//! Choosing an output whose card plays something else is a change of the
//! card's profile first, and its sink comes after: a [`Plan`] says what to
//! do, and the thread does it as the server follows.

use super::apps::{self, App};

/// A volume of 100%, as the server counts it.
pub(crate) const NORM: u32 = 0x10000;

/// Which default a change is to: the output's, or the input's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Side {
    Output,
    Input,
}

impl Side {
    /// Its place in a pair of things, one for each side: the output's
    /// first.
    pub(crate) fn at(self) -> usize {
        usize::from(self == Self::Input)
    }
}

/// Whose volume a control is of: a side's default, or a program's stream,
/// by its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Whose {
    Side(Side),
    App(u32),
}

/// A change to a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// This many steps, up positive.
    By(i32),
    /// To this, in percent.
    To(u32),
}

/// What the server holds, as last read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The output sound goes to unless a program says otherwise, by name.
    pub(crate) default_sink: Option<String>,
    pub(crate) sinks: Vec<Sink>,
    pub(crate) cards: Vec<Card>,
    /// The input programs record from unless they say otherwise, by name.
    pub(crate) default_source: Option<String>,
    pub(crate) sources: Vec<Source>,
    /// What programs play.
    pub(crate) streams: Vec<Stream>,
    /// The shell's own number among the server's clients, once it is told.
    pub(crate) client: Option<u32>,
}

/// What a program plays: a stream into a sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stream {
    /// The server's number for it, while it lasts.
    pub(crate) index: u32,
    /// The client playing it, by the server's number for it.
    pub(crate) client: Option<u32>,
    /// The sink it plays into, by its number.
    pub(crate) sink: u32,
    /// The program's name for itself, its file's name, and the stream's own
    /// name: "Firefox", "firefox", "AudioStream".
    pub(crate) application: Option<String>,
    pub(crate) binary: Option<String>,
    pub(crate) media: String,
    /// The icon the program names, and the ID it gives, by which an icon
    /// can be found.
    pub(crate) icon: Option<String>,
    pub(crate) id: Option<String>,
    /// Each channel's volume, as the server counts it.
    pub(crate) volume: Vec<u32>,
    pub(crate) muted: bool,
}

/// An input the server records from: a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Source {
    /// The server's number for it, while it lasts.
    pub(crate) index: u32,
    /// Its name, which stays its own:
    /// `alsa_input.pci-0000_00_1f.3.analog-stereo`.
    pub(crate) name: String,
    /// What it calls itself: "Built-in Audio Analog Stereo".
    pub(crate) description: String,
    /// Each channel's volume, as the server counts it.
    pub(crate) volume: Vec<u32>,
    pub(crate) muted: bool,
    /// The number of the card it is of, if it is of one.
    pub(crate) card: Option<u32>,
    /// Its card's ports it can record from, by name, and the one it
    /// records from.
    pub(crate) ports: Vec<String>,
    pub(crate) port: Option<String>,
    /// It hears what a sink plays: a monitor, which no one chooses as an
    /// input.
    pub(crate) monitor: bool,
    /// What reads it, `device.api`: "alsa", "bluez5".
    pub(crate) api: Option<String>,
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
    /// Something plays through it now.
    pub(crate) running: bool,
    /// The source that hears what it plays, by name.
    pub(crate) monitor: Option<String>,
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
    /// What tells it apart from another of its make, as the server says
    /// it: where it is attached, its chip, its ALSA card, its Bluetooth
    /// address.
    pub(crate) details: Vec<(&'static str, String)>,
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
    /// A microphone, built in or plugged in.
    Mic,
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

/// A device a person can choose, on either side: a card's jack, or a sink
/// or a source of no card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    /// What it is known by, across reboots: `<card>/<port>`, or a sink's or
    /// a source's name.
    pub(crate) key: String,
    /// Its card, or the sink or source of no card: "E8".
    pub(crate) device: String,
    /// Its jack, and the monitor or the way it is attached, if it is a
    /// card's: "Headphones · Bluetooth".
    pub(crate) jack: Option<String>,
    /// What the panel calls it: the monitor's name, or an attached device's
    /// own, or the jack's: "DELL U2720Q", "E8", "Speakers".
    pub(crate) short: String,
    pub(crate) kind: Kind,
    /// Whether it can be used now: nothing is known to be unplugged.
    pub(crate) connected: bool,
    /// The card it is a jack of, by name, if it is one's.
    pub(crate) card: Option<String>,
    /// What tells it apart, line by line: its card's details, the profile
    /// the card plays by, and its key.
    pub(crate) details: Vec<(&'static str, String)>,
}

impl Choice {
    /// Its card and its jack, and the monitor or the way it is attached:
    /// "E8 — Headphones · Bluetooth"; or a sink's or a source's name.
    pub(crate) fn long(&self) -> String {
        long(&self.device, self.jack.as_deref())
    }
}

/// A device's whole name: `device`, and its `jack`, if it has one, after a
/// dash.
pub(crate) fn long(device: &str, jack: Option<&str>) -> String {
    match jack {
        Some(jack) => format!("{device} \u{2014} {jack}"),
        None => device.to_owned(),
    }
}

/// How to make a device the default on its side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Make this sink or source, of no card, the default on `side`.
    Device(Side, String),
    /// Set `profile` on the card, if it is given, then make the card's sink
    /// or source on `side` that plays or records through `port` the
    /// default, playing or recording through it.
    Port {
        side: Side,
        card: u32,
        card_name: String,
        profile: Option<String>,
        port: String,
    },
}

/// A sink or a source, as far as choosing it goes.
struct Device<'f> {
    name: &'f str,
    description: &'f str,
    card: Option<u32>,
    port: Option<&'f str>,
}

impl Device<'_> {
    /// It, chosen as itself, on `side`: no card's jack.
    fn loose(&self, side: Side) -> Choice {
        Choice {
            key: self.name.to_owned(),
            device: self.description.to_owned(),
            jack: None,
            short: self.description.to_owned(),
            kind: match side {
                Side::Output => Kind::Other,
                Side::Input => Kind::Mic,
            },
            connected: true,
            card: None,
            details: vec![("Key", self.name.to_owned())],
        }
    }
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

    /// The default input's source, if the server has one it can name.
    pub(crate) fn input(&self) -> Option<&Source> {
        let name = self.default_source.as_deref()?;
        self.sources.iter().find(|source| source.name == name)
    }

    /// Every output.
    pub(crate) fn outputs(&self) -> Vec<Choice> {
        self.choices(Side::Output)
    }

    /// Every device on `side`: each card's jacks that way, in the card's
    /// order, then each sink or source of no card, or of no jack of its
    /// card's; never a monitor.
    pub(crate) fn choices(&self, side: Side) -> Vec<Choice> {
        let out = side == Side::Output;
        let jacks = self.cards.iter().flat_map(|card| {
            card.ports
                .iter()
                .filter(move |port| port.output == out)
                .map(move |port| card.choice(port))
        });
        let loose = self
            .devices(side)
            .into_iter()
            .filter(|device| self.jack_of(device).is_none())
            .map(|device| device.loose(side));
        jacks.chain(loose).collect()
    }

    /// Each sink, by its number, as the output it plays to: where a
    /// program's stream can be moved.
    pub(crate) fn sink_choices(&self) -> Vec<(u32, Choice)> {
        self.sinks
            .iter()
            .zip(self.devices(Side::Output))
            .map(|(sink, device)| {
                let choice = match self.jack_of(&device) {
                    Some((card, port)) => card.choice(port),
                    None => device.loose(Side::Output),
                };
                (sink.index, choice)
            })
            .collect()
    }

    /// The sink or source on `side` playing or recording through the device
    /// keyed `key` now, by name: its card's, playing through that jack, or
    /// one of no card by that name.
    fn live(&self, side: Side, key: &str) -> Option<&str> {
        self.devices(side)
            .into_iter()
            .find(|device| match self.jack_of(device) {
                Some((card, port)) => self::key(card, port) == key,
                None => device.name == key,
            })
            .map(|device| device.name)
    }

    /// The sink playing to the output keyed `key` now, if one does.
    pub(crate) fn live_sink(&self, key: &str) -> Option<&Sink> {
        let name = self.live(Side::Output, key)?;
        self.sinks.iter().find(|sink| sink.name == name)
    }

    /// The source to meter the device keyed `key` on `side` by: an
    /// output's sink's monitor, while something plays through it; or an
    /// input's own source, if ALSA reads it. A Bluetooth headset's
    /// microphone recorded moves the headset to the poor sound of a call,
    /// so it is never.
    pub(crate) fn meter(&self, side: Side, key: &str) -> Option<String> {
        match side {
            Side::Output => {
                let sink = self.live_sink(key).filter(|sink| sink.running)?;
                sink.monitor.clone()
            }
            Side::Input => {
                let name = self.live(Side::Input, key)?;
                let source = self.sources.iter().find(|source| source.name == name)?;
                (source.api.as_deref() == Some("alsa")).then(|| source.name.clone())
            }
        }
    }

    /// What programs play, but what the shell plays itself.
    pub(crate) fn apps(&self) -> impl Iterator<Item = &Stream> {
        self.streams
            .iter()
            .filter(|stream| stream.client.is_none() || stream.client != self.client)
    }

    /// The sinks, or the sources but the monitors, as devices.
    fn devices(&self, side: Side) -> Vec<Device<'_>> {
        match side {
            Side::Output => self
                .sinks
                .iter()
                .map(|sink| Device {
                    name: &sink.name,
                    description: &sink.description,
                    card: sink.card,
                    port: sink.port.as_deref(),
                })
                .collect(),
            Side::Input => self
                .sources
                .iter()
                .filter(|source| !source.monitor)
                .map(|source| Device {
                    name: &source.name,
                    description: &source.description,
                    card: source.card,
                    port: source.port.as_deref(),
                })
                .collect(),
        }
    }

    /// The key of the output the default sink plays to.
    pub(crate) fn output_key(&self) -> Option<String> {
        self.chosen(Side::Output)
    }

    /// The key of the default on `side`.
    pub(crate) fn chosen(&self, side: Side) -> Option<String> {
        let name = match side {
            Side::Output => self.default_sink.as_deref(),
            Side::Input => self.default_source.as_deref(),
        }?;
        let device = self
            .devices(side)
            .into_iter()
            .find(|device| device.name == name)?;
        Some(match self.jack_of(&device) {
            Some((card, port)) => key(card, port),
            None => device.name.to_owned(),
        })
    }

    /// The card and the jack `device` plays or records through, if it does
    /// through one.
    fn jack_of(&self, device: &Device<'_>) -> Option<(&Card, &Port)> {
        let card = self
            .cards
            .iter()
            .find(|card| Some(card.index) == device.card)?;
        let port = device.port?;
        let port = card.ports.iter().find(|had| had.name == port)?;
        Some((card, port))
    }

    /// How to make the device of `key` the default on `side`.
    pub(crate) fn plan(&self, side: Side, key: &str) -> Option<Plan> {
        let out = side == Side::Output;
        for card in &self.cards {
            if let Some(port) = card
                .ports
                .iter()
                .find(|port| port.output == out && self::key(card, port) == key)
            {
                return Some(Plan::Port {
                    side,
                    card: card.index,
                    card_name: card.name.clone(),
                    profile: card.profile_for(port),
                    port: port.name.clone(),
                });
            }
        }
        self.devices(side)
            .into_iter()
            .find(|device| device.name == key)
            .map(|device| Plan::Device(side, device.name.to_owned()))
    }
}

impl Card {
    /// `port`, as a device of this card.
    fn choice(&self, port: &Port) -> Choice {
        let attached = match self.bus.as_deref() {
            Some("bluetooth") => Some("Bluetooth"),
            Some("usb") => Some("USB"),
            _ => None,
        };
        let extra = port.monitor.as_deref().or(attached);
        let jack = match extra {
            Some(extra) => format!("{} \u{b7} {extra}", port.description),
            None => port.description.clone(),
        };
        let short = match (&port.monitor, attached) {
            (Some(monitor), _) => monitor.clone(),
            (None, Some(_)) => self.description.clone(),
            (None, None) => port.description.clone(),
        };
        let key = key(self, port);
        let mut details = self.details.clone();
        details.extend(self.profile.clone().map(|profile| ("Profile", profile)));
        details.push(("Key", key.clone()));
        Choice {
            key,
            device: self.description.clone(),
            jack: Some(jack),
            short,
            kind: port.kind,
            connected: port.plugged != Plugged::No,
            card: Some(self.name.clone()),
            details,
        }
    }

    /// The profile to set for `port` to play or record, or `None` if the
    /// card's own does through it already. Of the profiles that do, one that
    /// can now, then one that keeps the card's other side (its microphone,
    /// choosing an output; its speakers, choosing an input), then the card's
    /// own choice.
    fn profile_for(&self, port: &Port) -> Option<String> {
        let ways = |profile: &Profile| {
            if port.output {
                (profile.sinks, profile.sources)
            } else {
                (profile.sources, profile.sinks)
            }
        };
        let plays =
            |profile: &&Profile| ways(profile).0 > 0 && port.profiles.contains(&profile.name);
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
            .max_by_key(|profile| (profile.available, ways(profile).1 > 0, profile.priority))
            .map(|profile| profile.name.clone())
    }
}

/// The key of `card`'s `port`.
fn key(card: &Card, port: &Port) -> String {
    format!("{}/{}", card.name, port.name)
}

/// The device to switch to after the one of key `now`, of `choices`: the
/// next that is connected, round to the first. `None` when there is no
/// other.
pub(crate) fn next_choice<'c>(choices: &'c [Choice], now: Option<&str>) -> Option<&'c Choice> {
    let shown: Vec<&Choice> = choices.iter().filter(|choice| choice.connected).collect();
    let at = shown
        .iter()
        .position(|choice| Some(choice.key.as_str()) == now);
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

impl Stream {
    /// What it is called: the program's name for itself, else its file's,
    /// else the stream's own.
    pub(crate) fn name(&self) -> &str {
        self.application
            .as_deref()
            .or(self.binary.as_deref())
            .unwrap_or(&self.media)
    }

    pub(crate) fn level(&self) -> Level {
        Level {
            percent: percent(&self.volume),
            muted: self.muted,
        }
    }

    /// The icon to show for it: the one its program names, else its
    /// application's among `installed`, found by its ID or its file's name
    /// as the taskbar finds a window's, else one named as its file is.
    pub(crate) fn icon_from(&self, installed: &[App]) -> Option<String> {
        if let Some(icon) = &self.icon {
            return Some(icon.clone());
        }
        [self.id.as_deref(), self.binary.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|id| apps::of_window(installed, id).and_then(|app| app.icon.clone()))
            .or_else(|| self.binary.clone())
    }
}

impl Source {
    pub(crate) fn level(&self) -> Level {
        Level {
            percent: percent(&self.volume),
            muted: self.muted,
        }
    }
}

/// How far along a meter a peak of `peak`, 0 to 1 of full scale, reaches:
/// none at 60 dB below full scale or quieter, all of it at full scale, as
/// the ear hears it, the panel's bars' range.
pub(crate) fn loudness(peak: f32) -> f32 {
    const RANGE: f32 = 60.0;
    let decibels = 20.0 * peak.max(f32::MIN_POSITIVE).log10();
    (1.0 + decibels / RANGE).clamp(0.0, 1.0)
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
            running: false,
            monitor: None,
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
            details: Vec::new(),
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
            details: Vec::new(),
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
            details: Vec::new(),
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
            default_source: None,
            sources: Vec::new(),
            ..Facts::default()
        }
    }

    /// The output of `key` among `facts`'.
    fn output(facts: &Facts, key: &str) -> Choice {
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
                speakers.long().as_str(),
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
            (e8.long().as_str(), e8.short.as_str()),
            ("E8 \u{2014} Headphones \u{b7} Bluetooth", "E8"),
            "the E8 reads as E8 \u{2014} Headphones \u{b7} Bluetooth"
        );
        let dell = output(&facts, "alsa_card.pci-0000_01_00.1/hdmi-output-1");
        assert_eq!(
            (dell.long().as_str(), dell.short.as_str(), dell.kind),
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

    fn stream(index: u32, client: Option<u32>, named: [Option<&str>; 2]) -> Stream {
        let [application, binary] = named.map(|name| name.map(str::to_owned));
        Stream {
            index,
            client,
            sink: 51,
            application,
            binary,
            media: "AudioStream".to_owned(),
            icon: None,
            id: None,
            volume: vec![NORM / 2, NORM / 2],
            muted: false,
        }
    }

    #[test]
    fn a_meter_reaches_as_far_as_the_ear_hears_a_peak() {
        assert_eq!(loudness(1.0), 1.0);
        assert_eq!(loudness(0.0), 0.0);
        assert_eq!(loudness(0.000_5), 0.0, "below 60 dB down");
        assert!((loudness(0.1) - 2.0 / 3.0).abs() < 1e-4, "20 dB down");
        assert_eq!(loudness(2.0), 1.0, "never past the end");
    }

    #[test]
    fn a_stream_is_named_by_its_program_else_its_file_else_itself() {
        let named = |names| stream(1, None, names).name().to_owned();
        assert_eq!(named([Some("Firefox"), Some("firefox")]), "Firefox");
        assert_eq!(named([None, Some("firefox")]), "firefox");
        assert_eq!(named([None, None]), "AudioStream");
        assert_eq!(
            stream(1, None, [None, None]).level(),
            Level {
                percent: 50,
                muted: false
            }
        );
    }

    #[test]
    fn a_streams_icon_is_its_own_else_its_applications_else_named_for_its_file() {
        let app = |id: &str, icon: &str| App {
            id: id.to_owned(),
            name: id.to_owned(),
            comment: None,
            icon: Some(icon.to_owned()),
            run: apps::Run {
                argv: vec![id.to_owned()],
                terminal: false,
                dir: None,
            },
            categories: Vec::new(),
            keywords: Vec::new(),
            wm_class: None,
        };
        let installed = [
            app("org.mozilla.firefox", "firefox-icon"),
            app("mpv", "mpv-icon"),
        ];
        let icon = |icon: Option<&str>, id: Option<&str>, binary: Option<&str>| {
            let stream = Stream {
                icon: icon.map(str::to_owned),
                id: id.map(str::to_owned),
                binary: binary.map(str::to_owned),
                ..stream(1, None, [None, None])
            };
            stream.icon_from(&installed)
        };
        assert_eq!(
            icon(Some("its-own"), Some("mpv"), None).as_deref(),
            Some("its-own")
        );
        assert_eq!(
            icon(None, Some("org.mozilla.firefox"), Some("mpv")).as_deref(),
            Some("firefox-icon"),
            "by its ID first"
        );
        assert_eq!(
            icon(None, None, Some("firefox")).as_deref(),
            Some("firefox-icon"),
            "by its file, as a window's app id is matched"
        );
        assert_eq!(icon(None, None, Some("paplay")).as_deref(), Some("paplay"));
        assert_eq!(icon(None, None, None), None);
    }

    #[test]
    fn a_renamed_output_keeps_its_name_across_a_reconnect() {
        let e8 = "bluez_card.FC_58_FA_A9_62_22/headset-output";
        let mut kept = crate::model::kept::Kept::default();
        kept.rename(e8, "Headset");
        let before = laptop();
        assert_eq!(kept.called(&output(&before, e8)), "Headset");
        // Reconnected, the server numbers the card and its sink afresh.
        let mut after = laptop();
        for card in &mut after.cards {
            card.index += 10;
        }
        for sink in &mut after.sinks {
            sink.index += 100;
            sink.card = sink.card.map(|card| card + 10);
        }
        assert_eq!(kept.called(&output(&after, e8)), "Headset");
    }

    #[test]
    fn the_shells_own_streams_are_not_listed() {
        let facts = Facts {
            streams: vec![
                stream(1, Some(7), [Some("Firefox"), None]),
                stream(2, Some(9), [Some("perspicax-shell"), None]),
                stream(3, None, [None, None]),
            ],
            client: Some(9),
            ..laptop()
        };
        let apps: Vec<u32> = facts.apps().map(|stream| stream.index).collect();
        assert_eq!(apps, [1, 3]);
        let unknown = Facts {
            client: None,
            ..facts
        };
        assert_eq!(unknown.apps().count(), 3, "none left out before it is told");
    }

    #[test]
    fn each_sink_is_listed_as_the_output_it_plays_to() {
        let listed: Vec<(u32, String)> = laptop()
            .sink_choices()
            .into_iter()
            .map(|(sink, choice)| (sink, choice.short))
            .collect();
        assert_eq!(
            listed,
            [(51, "Speakers".to_owned()), (566, "E8".to_owned())]
        );
        let loose = Facts {
            sinks: vec![sink(4, "one", &[NORM], false)],
            ..Facts::default()
        };
        let (sink, one) = &loose.sink_choices()[0];
        assert_eq!((*sink, one.key.as_str(), one.kind), (4, "one", Kind::Other));
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
            default_source: None,
            sources: Vec::new(),
            ..Facts::default()
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
        assert_eq!(
            facts.plan(Side::Output, "two"),
            Some(Plan::Device(Side::Output, "two".to_owned()))
        );
    }

    #[test]
    fn an_output_is_metered_by_its_sinks_monitor_and_an_input_only_if_alsa_reads_it() {
        let speakers = "alsa_card.pci-0000_00_1f.3/analog-output-speaker";
        let mut facts = laptop();
        facts.sinks[0].monitor = Some("speakers.monitor".to_owned());
        assert_eq!(facts.live_sink(speakers).map(|sink| sink.index), Some(51));
        assert_eq!(
            facts.meter(Side::Output, speakers),
            None,
            "nothing plays through it"
        );
        facts.sinks[0].running = true;
        assert_eq!(
            facts.meter(Side::Output, speakers).as_deref(),
            Some("speakers.monitor")
        );
        let headphones = "alsa_card.pci-0000_00_1f.3/analog-output-headphones";
        assert_eq!(
            facts.live_sink(headphones),
            None,
            "the sink plays elsewhere"
        );
        assert_eq!(facts.meter(Side::Output, headphones), None);
        let source = |name: &str, card, port: Option<&str>, api: &str| Source {
            index: 0,
            name: name.to_owned(),
            description: name.to_owned(),
            volume: vec![NORM],
            muted: false,
            card,
            ports: port.map(|port| vec![port.to_owned()]).unwrap_or_default(),
            port: port.map(str::to_owned),
            monitor: false,
            api: Some(api.to_owned()),
        };
        facts.sources = vec![
            source("mic", Some(0), Some("analog-input-internal-mic"), "alsa"),
            source("bluez_input.E8", None, None, "bluez5"),
        ];
        assert_eq!(
            facts
                .meter(
                    Side::Input,
                    "alsa_card.pci-0000_00_1f.3/analog-input-internal-mic"
                )
                .as_deref(),
            Some("mic")
        );
        assert_eq!(
            facts.meter(Side::Input, "bluez_input.E8"),
            None,
            "a Bluetooth input is never metered"
        );
    }

    #[test]
    fn inputs_are_a_cards_jacks_in_and_its_sources_but_never_a_monitor() {
        let source =
            |index: u32, name: &str, card: Option<u32>, port: Option<&str>, monitor| Source {
                index,
                name: name.to_owned(),
                description: name.to_owned(),
                volume: vec![NORM],
                muted: false,
                card,
                ports: port.map(|port| vec![port.to_owned()]).unwrap_or_default(),
                port: port.map(str::to_owned),
                monitor,
                api: card.map(|_| "alsa".to_owned()),
            };
        let facts = Facts {
            default_source: Some("alsa_input.pci-0000_00_1f.3.analog-stereo".to_owned()),
            sources: vec![
                source(
                    60,
                    "alsa_input.pci-0000_00_1f.3.analog-stereo",
                    Some(0),
                    Some("analog-input-internal-mic"),
                    false,
                ),
                source(61, "alsa_output.speakers.monitor", None, None, true),
                source(62, "a virtual microphone", None, None, false),
            ],
            ..laptop()
        };
        let keys: Vec<String> = facts
            .choices(Side::Input)
            .into_iter()
            .map(|choice| choice.key)
            .collect();
        assert_eq!(
            keys,
            [
                "alsa_card.pci-0000_00_1f.3/analog-input-internal-mic",
                "a virtual microphone"
            ],
            "the jack, not its source again, and no monitor"
        );
        let mic = &facts.choices(Side::Input)[0];
        assert_eq!(mic.long(), "Built-in Audio \u{2014} Internal Microphone");
        assert_eq!(
            facts.chosen(Side::Input).as_deref(),
            Some("alsa_card.pci-0000_00_1f.3/analog-input-internal-mic")
        );
        let Some(Plan::Port { side, profile, .. }) = facts.plan(
            Side::Input,
            "alsa_card.pci-0000_00_1f.3/analog-input-internal-mic",
        ) else {
            panic!("a port's plan");
        };
        assert_eq!(
            (side, profile),
            (Side::Input, None),
            "the card's own records from it"
        );
        assert_eq!(
            facts.plan(Side::Input, "a virtual microphone"),
            Some(Plan::Device(Side::Input, "a virtual microphone".to_owned()))
        );
        assert_eq!(
            facts.plan(
                Side::Input,
                "alsa_card.pci-0000_00_1f.3/analog-output-speaker"
            ),
            None,
            "an output is no input"
        );
    }

    #[test]
    fn a_port_of_the_active_profile_needs_no_profile_change() {
        let facts = laptop();
        assert_eq!(
            facts.plan(
                Side::Output,
                "alsa_card.pci-0000_00_1f.3/analog-output-speaker"
            ),
            Some(Plan::Port {
                side: Side::Output,
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
        let Some(Plan::Port { profile, .. }) = facts.plan(
            Side::Output,
            "alsa_card.pci-0000_00_1f.3/iec958-stereo-output",
        ) else {
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
            facts.plan(Side::Output, "alsa_card.pci-0000_01_00.1/hdmi-output-1")
        else {
            panic!("a port's plan");
        };
        assert_eq!(
            (card, profile.as_deref()),
            (1, Some("output:hdmi-stereo-extra1"))
        );
        assert_eq!(facts.plan(Side::Output, "nothing/at-all"), None);
    }

    #[test]
    fn the_next_output_goes_round_the_connected_ones() {
        let facts = laptop();
        let outputs = facts.outputs();
        let next = |now: Option<&str>| next_choice(&outputs, now).map(|output| output.key.as_str());
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
            next_choice(&alone, Some(alone[0].key.as_str())),
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
            default_source: None,
            sources: Vec::new(),
            ..Facts::default()
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
