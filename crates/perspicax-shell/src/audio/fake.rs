//! A sound server for the client's tests: the protocol's side a server
//! speaks, as the `pulseaudio` crate encodes it, over a socket of a test's
//! own.
//!
//! It holds two cards and two sinks: the built-in card, playing to its
//! speakers through `alsa_output.speakers`, the default; a Bluetooth
//! headset's sink, of no card here; and an HDMI card whose profile is off,
//! with a monitor on its second jack, whose sink comes only once the
//! profile that plays through that jack is set, as a server's does.
//!
//! It answers what the client asks, one connection at a time, and keeps a
//! line for each thing asked, to be counted. A change, made from a test or
//! by what the client asked, is told the way a server tells it, as a
//! subscription's event.

use std::{
    ffi::{CStr, CString},
    io::{BufReader, Write as _},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use pulseaudio::protocol::{
    self, AuthReply, CardInfo, CardPortInfo, CardProfileInfo, ChannelMap, ChannelVolume, Command,
    MAX_VERSION, Props, PulseError, SampleSpec, ServerInfo, SetClientNameReply, SinkInfo,
    SubscriptionEvent, SubscriptionEventFacility, SubscriptionEventType, SubscriptionMask, Volume,
    port_info::{PortAvailable, PortDirection, PortInfo, PortType},
};

use super::client::PLAYS;
use crate::model::sound::NORM;

/// The built-in card's sink, the default.
pub(super) const SPEAKERS: &str = "alsa_output.speakers";
/// The HDMI card's, the profile that plays through its second jack, the
/// jack, and the sink that profile brings.
pub(super) const HDMI: &str = "alsa_card.pci-0000_01_00.1";
pub(super) const EXTRA1: &str = "output:hdmi-stereo-extra1";
pub(super) const JACK: &str = "hdmi-output-1";
pub(super) const HDMI_SINK: &str = "alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1";

/// The server, serving on a thread until it is dropped.
pub(super) struct Fake {
    path: PathBuf,
    shared: Arc<Mutex<Shared>>,
}

struct Shared {
    sinks: Vec<SinkInfo>,
    cards: Vec<CardInfo>,
    default: CString,
    /// What was asked, in order: one line each.
    asked: Vec<String>,
    /// The connection being served, to write on.
    writer: Option<UnixStream>,
    version: u16,
    /// What the client subscribed to: it is told of nothing else.
    mask: SubscriptionMask,
}

impl Fake {
    /// A server at a socket named for `name`.
    pub(super) fn start(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("perspicax-sound-{name}-{}", std::process::id()));
        std::fs::remove_file(&path).ok();
        let listener = UnixListener::bind(&path).expect("a socket for the fake server");
        let shared = Arc::new(Mutex::new(Shared {
            sinks: vec![
                SinkInfo {
                    card_index: Some(0),
                    ports: vec![
                        sink_port("analog-output-speaker", PortType::Speaker),
                        sink_port("analog-output-headphones", PortType::Headphones),
                    ],
                    ..sink(0, SPEAKERS, 0x3d71)
                },
                sink(1, "bluez_output.E8", NORM),
            ],
            cards: vec![built_in(), hdmi()],
            default: CString::new(SPEAKERS).expect("a name"),
            asked: Vec::new(),
            writer: None,
            version: MAX_VERSION,
            mask: SubscriptionMask::empty(),
        }));
        let serving = Arc::clone(&shared);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    return;
                };
                serve(&serving, stream);
            }
        });
        Self { path, shared }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// What was asked so far.
    pub(super) fn asked(&self) -> Vec<String> {
        self.lock().asked.clone()
    }

    /// Change the sink of `index` with `change`, and tell it `times` over.
    pub(super) fn change_sink(&self, index: u32, times: usize, change: impl FnOnce(&mut Fixture)) {
        let mut shared = self.lock();
        let sink = shared
            .sinks
            .iter_mut()
            .find(|sink| sink.index == index)
            .expect("a sink of that index");
        let mut fixture = Fixture { muted: sink.muted };
        change(&mut fixture);
        sink.muted = fixture.muted;
        for _ in 0..times {
            shared.event(SubscriptionEventFacility::Sink, Some(index));
        }
    }

    /// Hang up on the client, as a server stopping does.
    pub(super) fn hang_up(&self) {
        if let Some(writer) = self.lock().writer.take() {
            writer.shutdown(Shutdown::Both).ok();
        }
    }

    /// Send a message of a command no protocol has: its number, 255, then
    /// its sequence number.
    pub(super) fn send_unknown(&self) {
        let message = [b'L', 0, 0, 0, 255, b'L', 0xff, 0xff, 0xff, 0xff];
        let mut shared = self.lock();
        let writer = shared.writer.as_mut().expect("a client");
        let mut descriptor = [0; protocol::DESCRIPTOR_SIZE];
        protocol::encode_descriptor(
            &mut descriptor,
            &protocol::Descriptor {
                length: message.len() as u32,
                channel: u32::MAX,
                offset: 0,
                flags: protocol::DescriptorFlags::empty(),
            },
        );
        writer.write_all(&descriptor).expect("written");
        writer.write_all(&message).expect("written");
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().expect("the fake server's state")
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.hang_up();
        std::fs::remove_file(&self.path).ok();
    }
}

/// What a test may change of a sink.
pub(super) struct Fixture {
    pub(super) muted: bool,
}

impl Shared {
    /// Tell the client the object of `index` changed.
    fn event(&mut self, facility: SubscriptionEventFacility, index: Option<u32>) {
        self.told(facility, SubscriptionEventType::Changed, index);
    }

    /// Tell the client of `kind` of change to the object of `index`.
    fn told(
        &mut self,
        facility: SubscriptionEventFacility,
        kind: SubscriptionEventType,
        index: Option<u32>,
    ) {
        let version = self.version;
        let subscribed = match facility {
            SubscriptionEventFacility::Sink => SubscriptionMask::SINK,
            SubscriptionEventFacility::Card => SubscriptionMask::CARD,
            SubscriptionEventFacility::Server => SubscriptionMask::SERVER,
            _ => SubscriptionMask::ALL,
        };
        if !self.mask.contains(subscribed) {
            return;
        }
        if let Some(writer) = self.writer.as_mut() {
            let event = SubscriptionEvent {
                event_facility: facility,
                event_type: kind,
                index,
            };
            protocol::write_command_message(
                writer,
                u32::MAX,
                &Command::SubscribeEvent(event),
                version,
            )
            .ok();
        }
    }

    /// Set card `index`'s profile to `profile`: the HDMI card's brings its
    /// sink with the profile that plays through its jack, and takes it with
    /// any other.
    fn set_profile(&mut self, index: u32, profile: &CStr) {
        let Some(card) = self.cards.iter_mut().find(|card| card.index == index) else {
            return;
        };
        card.active_profile = Some(profile.to_owned());
        let hdmi = card.name.to_bytes() == HDMI.as_bytes();
        self.event(SubscriptionEventFacility::Card, Some(index));
        if hdmi && profile.to_bytes() == EXTRA1.as_bytes() {
            self.sinks.push(SinkInfo {
                card_index: Some(index),
                ports: vec![sink_port(JACK, PortType::Hdmi)],
                ..sink(2, HDMI_SINK, NORM)
            });
            self.told(
                SubscriptionEventFacility::Sink,
                SubscriptionEventType::New,
                Some(2),
            );
        } else if hdmi && self.sinks.iter().any(|sink| sink.index == 2) {
            self.sinks.retain(|sink| sink.index != 2);
            self.told(
                SubscriptionEventFacility::Sink,
                SubscriptionEventType::Removed,
                Some(2),
            );
        }
    }
}

/// Serve one client until it goes.
fn serve(shared: &Mutex<Shared>, stream: UnixStream) {
    let mut reader = BufReader::new(stream.try_clone().expect("a second handle"));
    {
        let mut shared = shared.lock().expect("the state");
        shared.writer = Some(stream);
        shared.version = MAX_VERSION;
        shared.mask = SubscriptionMask::empty();
    }
    loop {
        let version = shared.lock().expect("the state").version;
        let Ok((seq, command)) = protocol::read_command_message(&mut reader, version) else {
            return;
        };
        let mut shared = shared.lock().expect("the state");
        let version = shared.version;
        let (mut changed, mut negotiated, mut profile, mut defaulted) = (None, None, None, false);
        let mut subscribed = None;
        let Shared {
            sinks,
            cards,
            default,
            asked,
            writer,
            ..
        } = &mut *shared;
        let Some(writer) = writer.as_mut() else {
            return;
        };
        let found = |sinks: &[SinkInfo], name: Option<&CString>| {
            sinks.iter().position(|sink| Some(&sink.name) == name)
        };
        let written = match command {
            Command::Auth(auth) => {
                let version = auth.version.min(MAX_VERSION);
                negotiated = Some(version);
                let reply = AuthReply {
                    version,
                    ..AuthReply::default()
                };
                protocol::write_reply_message(writer, seq, &reply, MAX_VERSION)
            }
            Command::SetClientName(_) => protocol::write_reply_message(
                writer,
                seq,
                &SetClientNameReply { client_id: 7 },
                version,
            ),
            Command::Subscribe(mask) => {
                asked.push("subscribe".to_owned());
                subscribed = Some(mask);
                protocol::write_ack_message(writer, seq)
            }
            Command::GetServerInfo => {
                asked.push("server".to_owned());
                let info = ServerInfo {
                    server_name: Some(c"PulseAudio (on PipeWire 1.6.8)".to_owned()),
                    default_sink_name: Some(default.clone()),
                    ..ServerInfo::default()
                };
                protocol::write_reply_message(writer, seq, &info, version)
            }
            Command::GetSinkInfoList => {
                asked.push("sinks".to_owned());
                protocol::write_reply_message(writer, seq, &sinks.clone(), version)
            }
            Command::GetSinkInfo(get) => {
                asked.push(format!("sink {}", get.index.unwrap_or(u32::MAX)));
                match sinks.iter().find(|sink| Some(sink.index) == get.index) {
                    Some(sink) => protocol::write_reply_message(writer, seq, sink, version),
                    None => protocol::write_error(writer, seq, &PulseError::NoEntity),
                }
            }
            Command::GetCardInfoList => {
                asked.push("cards".to_owned());
                protocol::write_reply_message(writer, seq, &cards.clone(), version)
            }
            Command::GetCardInfo(get) => {
                asked.push(format!("card {}", get.index.unwrap_or(u32::MAX)));
                match cards.iter().find(|card| Some(card.index) == get.index) {
                    Some(card) => protocol::write_reply_message(writer, seq, card, version),
                    None => protocol::write_error(writer, seq, &PulseError::NoEntity),
                }
            }
            Command::SetSinkVolume(set) => {
                let volume: Vec<u32> = set.volume.channels().iter().map(Volume::as_u32).collect();
                asked.push(format!("volume {:?} {volume:?}", set.device_name));
                match found(sinks, set.device_name.as_ref()) {
                    Some(at) => {
                        sinks[at].cvolume = set.volume;
                        changed = Some(sinks[at].index);
                        protocol::write_ack_message(writer, seq)
                    }
                    None => protocol::write_error(writer, seq, &PulseError::NoEntity),
                }
            }
            Command::SetSinkMute(mute) => {
                asked.push(format!("mute {:?}", mute.device_name));
                match found(sinks, mute.device_name.as_ref()) {
                    Some(at) => {
                        sinks[at].muted = mute.mute;
                        changed = Some(sinks[at].index);
                        protocol::write_ack_message(writer, seq)
                    }
                    None => protocol::write_error(writer, seq, &PulseError::NoEntity),
                }
            }
            Command::SetCardProfile(set) => {
                asked.push(format!(
                    "profile {} {}",
                    set.card_index.unwrap_or(u32::MAX),
                    set.profile_name.to_string_lossy()
                ));
                profile = set.card_index.map(|index| (index, set.profile_name));
                protocol::write_ack_message(writer, seq)
            }
            Command::SetSinkPort(set) => {
                asked.push(format!("port {}", set.port_name.to_string_lossy()));
                let port = found(sinks, set.name.as_ref()).and_then(|at| {
                    let at_port = sinks[at]
                        .ports
                        .iter()
                        .position(|port| port.name == set.port_name)?;
                    Some((at, at_port))
                });
                match port {
                    Some((at, at_port)) => {
                        sinks[at].active_port = at_port;
                        changed = Some(sinks[at].index);
                        protocol::write_ack_message(writer, seq)
                    }
                    None => protocol::write_error(writer, seq, &PulseError::NoEntity),
                }
            }
            Command::SetDefaultSink(name) => {
                asked.push(format!("default {}", name.to_string_lossy()));
                *default = name;
                defaulted = true;
                protocol::write_ack_message(writer, seq)
            }
            other => {
                asked.push(format!("{other:?}"));
                protocol::write_error(writer, seq, &PulseError::Command)
            }
        };
        if written.is_err() {
            return;
        }
        if let Some(version) = negotiated {
            shared.version = version;
        }
        if let Some(mask) = subscribed {
            shared.mask = mask;
        }
        if let Some(index) = changed {
            shared.event(SubscriptionEventFacility::Sink, Some(index));
        }
        if let Some((index, profile)) = profile {
            shared.set_profile(index, &profile);
        }
        if defaulted {
            shared.event(SubscriptionEventFacility::Server, None);
        }
    }
}

/// A stereo sink of `index`, called `name`, at `volume` on both channels.
fn sink(index: u32, name: &str, volume: u32) -> SinkInfo {
    let mut cvolume = ChannelVolume::empty();
    cvolume.push(Volume::from_u32_clamped(volume));
    cvolume.push(Volume::from_u32_clamped(volume));
    SinkInfo {
        index,
        name: CString::new(name).expect("a name"),
        description: Some(CString::new(name).expect("a name")),
        sample_spec: SampleSpec {
            channels: 2,
            ..SampleSpec::default()
        },
        channel_map: ChannelMap::stereo(),
        cvolume,
        ..SinkInfo::default()
    }
}

/// A sink's port called `name`, of `kind`.
fn sink_port(name: &str, kind: PortType) -> PortInfo {
    PortInfo {
        name: CString::new(name).expect("a name"),
        port_type: kind,
        description: Some(CString::new(name).expect("a name")),
        dir: PLAYS,
        priority: 0,
        available: PortAvailable::Unknown,
        availability_group: None,
    }
}

/// A card's jack, out, called `name`, of `kind`, played through by
/// `profiles`.
fn card_port(
    name: &str,
    kind: PortType,
    available: PortAvailable,
    profiles: &[&str],
) -> CardPortInfo {
    CardPortInfo {
        name: CString::new(name).expect("a name"),
        port_type: kind,
        description: Some(CString::new(name).expect("a name")),
        props: Props::new(),
        dir: PLAYS,
        priority: 0,
        available,
        availability_group: None,
        profiles: profiles
            .iter()
            .map(|profile| CString::new(*profile).expect("a name"))
            .collect(),
        latency_offset: 0,
    }
}

/// A profile called `name`, with `sinks` sinks.
fn profile(name: &str, sinks: u32) -> CardProfileInfo {
    CardProfileInfo {
        name: CString::new(name).expect("a name"),
        description: None,
        priority: 0,
        available: 1,
        num_sinks: sinks,
        num_sources: 0,
    }
}

/// A card of `index` called `name`, describing itself as `description`,
/// with its `ports` and `profiles`, playing by `active`.
fn card(
    index: u32,
    name: &str,
    description: &CStr,
    ports: Vec<CardPortInfo>,
    profiles: Vec<CardProfileInfo>,
    active: &str,
) -> CardInfo {
    let mut props = Props::new();
    props.set_bytes(c"device.description", description.to_bytes_with_nul());
    props.set_bytes(c"device.bus", c"pci".to_bytes_with_nul());
    CardInfo {
        index,
        name: CString::new(name).expect("a name"),
        props,
        owner_module_index: None,
        driver: None,
        ports,
        profiles,
        active_profile: Some(CString::new(active).expect("a name")),
    }
}

fn built_in() -> CardInfo {
    const DUPLEX: &str = "output:analog-stereo+input:analog-stereo";
    card(
        0,
        "alsa_card.pci-0000_00_1f.3",
        c"Built-in Audio",
        vec![
            card_port(
                "analog-output-speaker",
                PortType::Speaker,
                PortAvailable::Unknown,
                &[DUPLEX],
            ),
            card_port(
                "analog-output-headphones",
                PortType::Headphones,
                PortAvailable::No,
                &[DUPLEX],
            ),
            CardPortInfo {
                // A source's jack, which the crate calls its output.
                dir: PortDirection::Output,
                ..card_port(
                    "analog-input-internal-mic",
                    PortType::Mic,
                    PortAvailable::Unknown,
                    &[DUPLEX],
                )
            },
        ],
        vec![profile("off", 0), profile(DUPLEX, 1)],
        DUPLEX,
    )
}

fn hdmi() -> CardInfo {
    let mut jack = card_port(JACK, PortType::Hdmi, PortAvailable::Yes, &[EXTRA1]);
    jack.props
        .set_bytes(c"device.product.name", c"DELL U2720Q".to_bytes_with_nul());
    card(
        1,
        HDMI,
        c"TU104 HD Audio Controller",
        vec![jack],
        vec![profile("off", 0), profile(EXTRA1, 1)],
        "off",
    )
}
