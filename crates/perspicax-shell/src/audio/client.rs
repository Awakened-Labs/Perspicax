//! The client: connected, it reads what the server holds and follows it;
//! cut off, it says so and tries again after a pause.
//!
//! Connecting is the protocol's handshake, the version both ends speak and
//! the shell's name, then a subscription to the server's changes, then a
//! read of everything there is to show. Nothing is told until that read is
//! whole. pipewire-pulse answers a new client's first reads before it has
//! found what it holds, with no sinks and no cards, then tells each as new;
//! so the first facts told may be empty, and the rest follow at once. From then on, a change the server tells is read again: the object
//! it names, once however often it is told while that read is out. A change
//! told before the reply to a read was sent before that reply, so the reply
//! already holds it. An object removed is dropped.
//!
//! A server that cannot be reached, or goes away, is told once, and tried
//! again after a pause that doubles each time, from [`PAUSE_FIRST`] to
//! [`PAUSE_MOST`], so a server that is not coming back costs next to
//! nothing; the pause starts afresh once one is reached.
//!
//! Choosing an output is done as the server follows: a card's profile set
//! first, if its own plays elsewhere; then, once a sink of the card plays
//! through the output's port, that port set on it, if it plays through
//! another, and the sink made the default. Under PipeWire, WirePlumber
//! moves the streams that follow the default; moving them here would pin
//! them. A card that has no such sink within [`CHOOSE_WITHIN`] is given up
//! on, and that is told.

use std::{
    collections::HashSet,
    ffi::CString,
    io,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use pulseaudio::protocol::{
    AuthParams, CardInfo, ChannelVolume, Command, GetCardInfo, GetSinkInfo, MAX_VERSION, Prop,
    Props, PulseError, SetCardProfileParams, SetDeviceMuteParams, SetDeviceVolumeParams,
    SetPortParams, SinkInfo, SubscriptionEvent, SubscriptionEventFacility, SubscriptionEventType,
    SubscriptionMask, Volume,
    port_info::{PortAvailable, PortDirection, PortType},
};
use smithay_client_toolkit::reexports::calloop::{
    Interest, LoopHandle, LoopSignal, Mode, PostAction, RegistrationToken,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};

use super::{
    Ask, News, Target,
    wire::{Expect, Incoming, Reply, Wire},
};
use crate::model::sound::{Card, Facts, Kind, Plan, Plugged, Port, Profile, Sink};

/// The first pause before trying a server again, and the longest.
pub(super) const PAUSE_FIRST: Duration = Duration::from_millis(500);
pub(super) const PAUSE_MOST: Duration = Duration::from_secs(10);
/// How long a server has to answer the handshake.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);
/// How long a card has to give the sink an output chosen plays through.
const CHOOSE_WITHIN: Duration = Duration::from_secs(3);

/// What the thread holds.
pub(super) struct Client {
    /// The server's socket, or `None` with none named, nor any runtime
    /// folder to find one in.
    server: Option<PathBuf>,
    tell: Box<dyn Fn(News) -> bool + Send>,
    handle: LoopHandle<'static, Client>,
    signal: LoopSignal,
    /// The connection, while there is one.
    link: Option<Link>,
    /// How long to wait before trying again, the next time it is lost.
    pause: Duration,
    /// That it is gone has been told since it was last there.
    told_gone: bool,
}

/// A connection, and what it has read.
struct Link {
    wire: Wire,
    /// The socket's source in the loop.
    source: RegistrationToken,
    /// The handshake's deadline, until the first read is whole.
    deadline: Option<RegistrationToken>,
    facts: Facts,
    /// How many of the first read's replies are still to come: news waits
    /// for none.
    first: usize,
    /// The first read is whole, and that has been acted on.
    following: bool,
    /// What is being read again, so that a change told meanwhile asks for
    /// no second read.
    reading: HashSet<Expect>,
    /// The facts changed since they were last told.
    changed: bool,
    /// The output being chosen: its card's number, its port, and which
    /// choice it is, to be made the default once a sink plays through it.
    choosing: Option<(u32, String, u64)>,
    /// How many choices there have been.
    chosen: u64,
}

impl Client {
    /// A client of the server at `server`, or of the one the environment
    /// names.
    pub(super) fn new(
        server: Option<PathBuf>,
        tell: Box<dyn Fn(News) -> bool + Send>,
        handle: LoopHandle<'static, Client>,
        signal: LoopSignal,
    ) -> Self {
        Self {
            server: server.or_else(socket),
            tell,
            handle,
            signal,
            link: None,
            pause: PAUSE_FIRST,
            told_gone: false,
        }
    }

    /// Tell the shell `news`, and stop if it is no longer there to hear.
    fn tell(&self, news: News) {
        if !(self.tell)(news) {
            self.signal.stop();
        }
    }

    /// Connect to the server, or say why not and try again later.
    pub(super) fn connect(&mut self) {
        if let Err(why) = self.link() {
            self.lost(&why);
        }
    }

    fn link(&mut self) -> Result<(), String> {
        let path = self.server.clone().ok_or_else(|| {
            "no sound server is named, and there is no runtime folder to find one in".to_owned()
        })?;
        let stream =
            UnixStream::connect(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let watched = stream.try_clone().map_err(|error| error.to_string())?;
        let mut wire = Wire::new(stream).map_err(|error| error.to_string())?;
        let cookie = pulseaudio::cookie_path_from_env()
            .and_then(|path| std::fs::read(path).ok())
            .unwrap_or_default();
        let auth = AuthParams {
            version: MAX_VERSION,
            supports_shm: false,
            supports_memfd: false,
            cookie,
        };
        wire.send(&Command::Auth(auth), Expect::Auth)
            .map_err(|error| error.to_string())?;
        let source = self
            .handle
            .insert_source(
                Generic::new(watched, Interest::BOTH, Mode::Edge),
                |_, _, client: &mut Client| {
                    client.ready();
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|error| error.error.to_string())?;
        let deadline = self
            .handle
            .insert_source(Timer::from_duration(ANSWER_WITHIN), |_, (), client| {
                // This timer goes as it fires, so the link no longer holds it.
                if let Some(link) = client.link.as_mut().filter(|link| link.first > 0) {
                    link.deadline = None;
                    client.lost("the server did not answer");
                }
                TimeoutAction::Drop
            })
            .ok();
        tracing::debug!(server = %path.display(), "connecting to the sound server");
        self.link = Some(Link {
            wire,
            source,
            deadline,
            facts: Facts::default(),
            first: usize::MAX,
            following: false,
            reading: HashSet::new(),
            changed: false,
            choosing: None,
            chosen: 0,
        });
        Ok(())
    }

    /// The socket can be read, or written, or both.
    fn ready(&mut self) {
        let Some(link) = &mut self.link else {
            return;
        };
        let result = link.wire.pump().and_then(|incoming| {
            for message in incoming {
                link.receive(message)?;
            }
            link.advance()?;
            link.wire.flush()
        });
        if let Err(error) = result {
            self.lost(&error.to_string());
            return;
        }
        if link.first == 0 {
            if !std::mem::replace(&mut link.following, true) {
                if let Some(deadline) = link.deadline.take() {
                    self.handle.remove(deadline);
                }
                tracing::info!("following the sound server");
                self.pause = PAUSE_FIRST;
                self.told_gone = false;
            }
            if std::mem::take(&mut link.changed) {
                let facts = Box::new(link.facts.clone());
                self.tell(News::Facts(facts));
            }
        }
    }

    /// Do what the shell asked.
    pub(super) fn ask(&mut self, ask: Ask) {
        let Some(link) = self.link.as_mut().filter(|link| link.first == 0) else {
            tracing::debug!("no sound server to ask; {ask:?} is dropped");
            return;
        };
        if let Ask::Choose(plan) = ask {
            let chosen = link.choose(plan);
            match chosen {
                Ok(Some(choice)) => self.give_up_later(choice),
                Ok(None) => {}
                Err(error) => self.lost(&error.to_string()),
            }
            return;
        }
        let command = match ask {
            Ask::Mute {
                target: Target::Sink(name),
                on,
            } => Command::SetSinkMute(SetDeviceMuteParams {
                device_index: None,
                device_name: CString::new(name).ok(),
                mute: on,
            }),
            Ask::Volume {
                target: Target::Sink(name),
                volume,
            } => Command::SetSinkVolume(SetDeviceVolumeParams {
                device_index: None,
                device_name: CString::new(name).ok(),
                volume: channels(&volume),
            }),
            Ask::Choose(_) => return,
        };
        if let Err(error) = link.wire.send(&command, Expect::Done) {
            self.lost(&error.to_string());
        }
    }

    /// Give up on choice `choice` if it is still being made after
    /// [`CHOOSE_WITHIN`].
    fn give_up_later(&mut self, choice: u64) {
        let timer =
            self.handle
                .insert_source(Timer::from_duration(CHOOSE_WITHIN), move |_, (), client| {
                    let given_up = client
                        .link
                        .as_mut()
                        .and_then(|link| link.choosing.take_if(|&mut (.., at)| at == choice));
                    if let Some((card, port, _)) = given_up {
                        let why = format!("card {card} gave no sink that plays through {port}");
                        tracing::warn!("the output was not chosen: {why}");
                        client.tell(News::Failed(why));
                    }
                    TimeoutAction::Drop
                });
        if let Err(error) = timer {
            tracing::debug!("the choice will not be given up on: {error}");
        }
    }

    /// The connection could not be made, or broke, for `why`: say so, the
    /// first time, and try again after a pause.
    fn lost(&mut self, why: &str) {
        if let Some(link) = self.link.take() {
            self.handle.remove(link.source);
            if let Some(deadline) = link.deadline {
                self.handle.remove(deadline);
            }
        }
        if !self.told_gone {
            tracing::info!("no sound server: {why}");
            self.told_gone = true;
            self.tell(News::Gone);
        } else {
            tracing::debug!("still no sound server: {why}");
        }
        // One may start later where it is looked for, but not where nothing
        // is looked for.
        if self.server.is_none() {
            return;
        }
        let pause = self.pause;
        self.pause = (pause * 2).min(PAUSE_MOST);
        let retry = self
            .handle
            .insert_source(Timer::from_duration(pause), |_, (), client| {
                client.connect();
                TimeoutAction::Drop
            });
        if let Err(error) = retry {
            tracing::warn!("the sound server will not be tried again: {error}");
        }
    }
}

impl Link {
    /// Act on `message`.
    fn receive(&mut self, message: Incoming) -> io::Result<()> {
        match message {
            Incoming::Reply(expect, reply) => self.replied(expect, reply)?,
            Incoming::Refused(expect, error) => self.refused(expect, error),
            Incoming::Event(command) => {
                if let Command::SubscribeEvent(event) = *command {
                    self.told(event)?;
                }
            }
        }
        Ok(())
    }

    fn replied(&mut self, expect: Expect, reply: Reply) -> io::Result<()> {
        self.reading.remove(&expect);
        let changed = match reply {
            Reply::Auth(auth) => {
                self.wire.version = auth.version.min(MAX_VERSION);
                let mut props = Props::new();
                props.set(Prop::ApplicationName, c"perspicax-shell");
                props.set(Prop::ApplicationId, c"perspicax-shell");
                self.wire
                    .send(&Command::SetClientName(props), Expect::Named)?;
                let mask =
                    SubscriptionMask::SINK | SubscriptionMask::CARD | SubscriptionMask::SERVER;
                self.wire.send(&Command::Subscribe(mask), Expect::Done)?;
                self.first = 3;
                self.wire.send(&Command::GetServerInfo, Expect::Server)?;
                self.wire.send(&Command::GetSinkInfoList, Expect::Sinks)?;
                self.wire.send(&Command::GetCardInfoList, Expect::Cards)?;
                false
            }
            Reply::Named | Reply::Done => false,
            Reply::Server(server) => {
                self.read_first();
                let default = server.default_sink_name.as_deref().map(text);
                std::mem::replace(&mut self.facts.default_sink, default) != self.facts.default_sink
            }
            Reply::Sinks(sinks) => {
                self.read_first();
                self.facts.sinks = sinks.iter().map(sink).collect();
                true
            }
            Reply::Sink(read) => {
                let read = sink(&read);
                match self
                    .facts
                    .sinks
                    .iter_mut()
                    .find(|had| had.index == read.index)
                {
                    Some(had) => std::mem::replace(had, read) != *had,
                    None => {
                        self.facts.sinks.push(read);
                        true
                    }
                }
            }
            Reply::Cards(cards) => {
                self.read_first();
                self.facts.cards = cards.iter().map(card).collect();
                true
            }
            Reply::Card(read) => {
                let read = card(&read);
                match self
                    .facts
                    .cards
                    .iter_mut()
                    .find(|had| had.index == read.index)
                {
                    Some(had) => std::mem::replace(had, read) != *had,
                    None => {
                        self.facts.cards.push(read);
                        true
                    }
                }
            }
        };
        self.changed |= changed;
        Ok(())
    }

    /// One of the first read's replies came.
    fn read_first(&mut self) {
        self.first = self.first.saturating_sub(1);
    }

    fn refused(&mut self, expect: Expect, error: PulseError) {
        self.reading.remove(&expect);
        match expect {
            // Gone between the change told and the read.
            Expect::Sink(index) if error == PulseError::NoEntity => {
                self.facts.sinks.retain(|sink| sink.index != index);
                self.changed = true;
            }
            Expect::Card(index) if error == PulseError::NoEntity => {
                self.facts.cards.retain(|card| card.index != index);
                self.changed = true;
            }
            _ => tracing::warn!("the sound server refused {expect:?}: {error:?}"),
        }
    }

    /// The server told of a change.
    fn told(&mut self, event: SubscriptionEvent) -> io::Result<()> {
        use SubscriptionEventFacility as Facility;
        let read = match (event.event_facility, event.event_type, event.index) {
            (Facility::Sink, SubscriptionEventType::Removed, Some(index)) => {
                self.facts.sinks.retain(|sink| sink.index != index);
                self.changed = true;
                return Ok(());
            }
            (Facility::Sink, _, Some(index)) => (
                Command::GetSinkInfo(GetSinkInfo {
                    index: Some(index),
                    name: None,
                }),
                Expect::Sink(index),
            ),
            (Facility::Card, SubscriptionEventType::Removed, Some(index)) => {
                self.facts.cards.retain(|card| card.index != index);
                self.changed = true;
                return Ok(());
            }
            (Facility::Card, _, Some(index)) => (
                Command::GetCardInfo(GetCardInfo {
                    index: Some(index),
                    name: None,
                }),
                Expect::Card(index),
            ),
            (Facility::Server, ..) => (Command::GetServerInfo, Expect::Server),
            _ => return Ok(()),
        };
        self.read_again(&read.0, read.1)
    }

    /// Start making the output `plan` names the one sound goes to. The
    /// choice, if it waits on the card.
    fn choose(&mut self, plan: Plan) -> io::Result<Option<u64>> {
        match plan {
            Plan::Sink(name) => {
                self.choosing = None;
                self.wire
                    .send(&Command::SetDefaultSink(cstring(name)), Expect::Done)?;
                Ok(None)
            }
            Plan::Port {
                card,
                card_name,
                profile,
                port,
            } => {
                if let Some(profile) = profile {
                    tracing::info!(card = card_name, profile, "setting a card's profile");
                    let set = SetCardProfileParams {
                        card_index: Some(card),
                        card_name: None,
                        profile_name: cstring(profile),
                    };
                    self.wire
                        .send(&Command::SetCardProfile(set), Expect::Done)?;
                }
                self.chosen += 1;
                self.choosing = Some((card, port, self.chosen));
                self.advance()?;
                Ok(self.choosing.as_ref().map(|&(.., choice)| choice))
            }
        }
    }

    /// Make the output being chosen the default, once a sink of its card
    /// plays through its port.
    fn advance(&mut self) -> io::Result<()> {
        let Some((card, port, _)) = &self.choosing else {
            return Ok(());
        };
        let Some(sink) = self
            .facts
            .sinks
            .iter()
            .find(|sink| sink.card == Some(*card) && sink.ports.contains(port))
        else {
            return Ok(());
        };
        let (name, elsewhere) = (sink.name.clone(), sink.port.as_ref() != Some(port));
        let port = port.clone();
        self.choosing = None;
        if elsewhere {
            let set = SetPortParams {
                index: None,
                name: Some(cstring(name.clone())),
                port_name: cstring(port),
            };
            self.wire.send(&Command::SetSinkPort(set), Expect::Done)?;
        }
        self.wire
            .send(&Command::SetDefaultSink(cstring(name)), Expect::Done)
    }

    /// Read again what `expect` is, unless it is being read already.
    fn read_again(&mut self, command: &Command, expect: Expect) -> io::Result<()> {
        if self.first > 0 || !self.reading.insert(expect) {
            return Ok(());
        }
        self.wire.send(command, expect)
    }
}

/// A sink, as the shell keeps it.
fn sink(info: &SinkInfo) -> Sink {
    let ports: Vec<String> = info.ports.iter().map(|port| text(&port.name)).collect();
    Sink {
        index: info.index,
        name: text(&info.name),
        description: info
            .description
            .as_deref()
            .map_or_else(|| text(&info.name), text),
        volume: info
            .cvolume
            .channels()
            .iter()
            .map(|volume| volume.as_u32())
            .collect(),
        muted: info.muted,
        card: info.card_index,
        port: ports.get(info.active_port).cloned(),
        ports,
    }
}

/// A card, as the shell keeps it.
fn card(info: &CardInfo) -> Card {
    let profiles = info
        .profiles
        .iter()
        .map(|profile| Profile {
            name: text(&profile.name),
            priority: profile.priority,
            available: profile.available != 0,
            sinks: profile.num_sinks,
            sources: profile.num_sources,
        })
        .collect();
    let ports = info
        .ports
        .iter()
        .map(|port| Port {
            name: text(&port.name),
            description: port
                .description
                .as_deref()
                .map_or_else(|| text(&port.name), text),
            kind: kind(port.port_type, &text(&port.name)),
            output: port.dir == PLAYS,
            plugged: match port.available {
                PortAvailable::Yes => Plugged::Yes,
                PortAvailable::No => Plugged::No,
                PortAvailable::Unknown => Plugged::Unknown,
            },
            profiles: port.profiles.iter().map(|profile| text(profile)).collect(),
            monitor: prop(&port.props, c"device.product.name"),
        })
        .collect();
    Card {
        index: info.index,
        name: text(&info.name),
        description: prop(&info.props, c"device.description").unwrap_or_else(|| text(&info.name)),
        bus: prop(&info.props, c"device.bus"),
        ports,
        profiles,
        profile: info.active_profile.as_deref().map(text),
    }
}

/// The direction of a jack that plays sound out. The crate names a port
/// by what it is part of, so a sink's, which sound goes into to be played,
/// is its `Input`: PulseAudio's `PA_DIRECTION_OUTPUT`, 1.
pub(super) const PLAYS: PortDirection = PortDirection::Input;

/// What a jack of type `port_type` and name `name` is for: by its type, or
/// by its name where a server older than protocol 34 gives none.
fn kind(port_type: PortType, name: &str) -> Kind {
    match port_type {
        PortType::Speaker => Kind::Speaker,
        PortType::Headphones => Kind::Headphones,
        PortType::Headset | PortType::Handset | PortType::Handsfree => Kind::Headset,
        PortType::Hdmi | PortType::Tv | PortType::Video => Kind::Display,
        PortType::Unknown => {
            let named = |part: &str| name.contains(part);
            if named("speaker") {
                Kind::Speaker
            } else if named("headphone") {
                Kind::Headphones
            } else if named("headset") {
                Kind::Headset
            } else if named("hdmi") {
                Kind::Display
            } else {
                Kind::Other
            }
        }
        _ => Kind::Other,
    }
}

/// The property `key` of `props`, as text, if it has one that is not blank.
fn prop(props: &Props, key: &std::ffi::CStr) -> Option<String> {
    let value = props.get_bytes(key)?;
    let value = value.strip_suffix(&[0]).unwrap_or(value);
    Some(String::from_utf8_lossy(value).trim().to_owned()).filter(|value| !value.is_empty())
}

/// `written`, as the protocol writes text: up to a NUL inside it.
fn cstring(written: String) -> CString {
    CString::new(written).unwrap_or_else(|error| {
        let at = error.nul_position();
        CString::new(&error.into_vec()[..at]).unwrap_or_default()
    })
}

/// Each channel's volume, as the protocol writes them.
fn channels(volume: &[u32]) -> ChannelVolume {
    let mut channels = ChannelVolume::empty();
    for &channel in volume {
        channels.push(Volume::from_u32_clamped(channel));
    }
    channels
}

/// The server's text, as text.
fn text(written: &std::ffi::CStr) -> String {
    written.to_string_lossy().into_owned()
}

/// The server's socket the environment names, as libpulse finds it:
/// `PULSE_SERVER`, if it is a `unix:` path, or `native` in the runtime
/// folder, `PULSE_RUNTIME_PATH` or `$XDG_RUNTIME_DIR/pulse`.
fn socket() -> Option<PathBuf> {
    socket_from(|name| std::env::var_os(name))
}

fn socket_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(server) = var("PULSE_SERVER") {
        let server = server.to_string_lossy().into_owned();
        return match server.strip_prefix("unix:") {
            Some(path) => Some(PathBuf::from(path)),
            None => {
                tracing::warn!(
                    "PULSE_SERVER names {server}, which is not a path on this machine; \
                     the shell follows only a sound server it can reach by its socket"
                );
                None
            }
        };
    }
    let folder = var("PULSE_RUNTIME_PATH")
        .map(PathBuf::from)
        .or_else(|| var("XDG_RUNTIME_DIR").map(|runtime| Path::new(&runtime).join("pulse")))?;
    Some(folder.join("native"))
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, time::Instant};

    use super::*;
    use crate::audio::{
        fake::{EXTRA1, Fake, HDMI, HDMI_SINK, JACK, SPEAKERS},
        start,
    };

    #[test]
    fn the_socket_is_found_as_libpulse_finds_it() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        assert_eq!(
            socket_from(env(&[("XDG_RUNTIME_DIR", "/run/user/7")])),
            Some(PathBuf::from("/run/user/7/pulse/native"))
        );
        assert_eq!(
            socket_from(env(&[
                ("XDG_RUNTIME_DIR", "/run/user/7"),
                ("PULSE_RUNTIME_PATH", "/tmp/pulse-x")
            ])),
            Some(PathBuf::from("/tmp/pulse-x/native")),
            "the runtime path is pulse's own folder"
        );
        assert_eq!(
            socket_from(env(&[
                ("XDG_RUNTIME_DIR", "/run/user/7"),
                ("PULSE_SERVER", "unix:/tmp/elsewhere")
            ])),
            Some(PathBuf::from("/tmp/elsewhere"))
        );
        assert_eq!(
            socket_from(env(&[
                ("XDG_RUNTIME_DIR", "/run/user/7"),
                ("PULSE_SERVER", "tcp:speakers.local")
            ])),
            None,
            "a server on another machine is not followed"
        );
        assert_eq!(socket_from(env(&[])), None);
    }

    /// News from a client of `fake`, as it comes.
    struct Heard(std::sync::mpsc::Receiver<News>);

    impl Heard {
        fn next(&self) -> News {
            self.0
                .recv_timeout(Duration::from_secs(5))
                .expect("news within five seconds")
        }

        fn facts(&self) -> Facts {
            match self.next() {
                News::Facts(facts) => *facts,
                other => panic!("not the facts: {other:?}"),
            }
        }
    }

    fn follow(fake: &Fake) -> (crate::audio::Running, Heard) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let running = start(Some(fake.path().to_owned()), move |news| {
            sender.send(news).is_ok()
        });
        (running, Heard(receiver))
    }

    #[test]
    fn the_client_subscribes_and_reads_the_server_and_its_sinks() {
        let fake = Fake::start("reads");
        let (_running, heard) = follow(&fake);
        let facts = heard.facts();
        assert_eq!(facts.default_sink.as_deref(), Some(SPEAKERS));
        assert_eq!(facts.sinks.len(), 2);
        assert_eq!(facts.cards.len(), 2);
        assert_eq!(facts.output().expect("the default").level().percent, 24);
        let asked = fake.asked();
        assert!(asked.contains(&"subscribe".to_owned()), "{asked:?}");
    }

    #[test]
    fn a_changed_sink_is_read_again_once_however_often_it_is_told() {
        let fake = Fake::start("again");
        let (_running, heard) = follow(&fake);
        heard.facts();
        fake.change_sink(0, 3, |sink| sink.muted = true);
        let facts = heard.facts();
        assert!(facts.output().expect("the default").muted);
        let reads = fake
            .asked()
            .iter()
            .filter(|asked| *asked == "sink 0")
            .count();
        assert_eq!(reads, 1, "three changes told, one read: {:?}", fake.asked());
    }

    #[test]
    fn a_mute_asked_reaches_the_server_and_comes_back_as_news() {
        let fake = Fake::start("mute");
        let (running, heard) = follow(&fake);
        heard.facts();
        running.ask(Ask::Mute {
            target: Target::Sink("alsa_output.speakers".to_owned()),
            on: true,
        });
        let facts = heard.facts();
        assert!(facts.output().expect("the default").muted);
    }

    #[test]
    fn a_volume_asked_reaches_the_server_each_channel_as_given() {
        let fake = Fake::start("volume");
        let (running, heard) = follow(&fake);
        heard.facts();
        running.ask(Ask::Volume {
            target: Target::Sink("alsa_output.speakers".to_owned()),
            volume: vec![0x8000, 0x4000],
        });
        let facts = heard.facts();
        let output = facts.output().expect("the default");
        assert_eq!(output.volume, [0x8000, 0x4000]);
        assert_eq!(output.level().percent, 50);
    }

    #[test]
    fn a_card_is_read_with_its_jacks_profiles_and_the_monitor_on_one() {
        let fake = Fake::start("cards");
        let (_running, heard) = follow(&fake);
        let facts = heard.facts();
        let hdmi = facts
            .cards
            .iter()
            .find(|card| card.name == HDMI)
            .expect("the HDMI card");
        assert_eq!(hdmi.description, "TU104 HD Audio Controller");
        assert_eq!(hdmi.bus.as_deref(), Some("pci"));
        assert_eq!(hdmi.profile.as_deref(), Some("off"));
        let jack = &hdmi.ports[0];
        assert_eq!(
            (jack.name.as_str(), jack.kind, jack.plugged, jack.output),
            (JACK, Kind::Display, Plugged::Yes, true)
        );
        assert_eq!(jack.monitor.as_deref(), Some("DELL U2720Q"));
        assert_eq!(jack.profiles, [EXTRA1]);
        let built_in = &facts.cards[0];
        let jacks: Vec<(&str, bool)> = built_in
            .ports
            .iter()
            .map(|port| (port.name.as_str(), port.output))
            .collect();
        assert_eq!(
            jacks,
            [
                ("analog-output-speaker", true),
                ("analog-output-headphones", true),
                ("analog-input-internal-mic", false),
            ],
            "a sink's jacks play out, a source's do not"
        );
        let speakers = facts.output().expect("the default");
        assert_eq!(speakers.card, Some(0));
        assert_eq!(speakers.port.as_deref(), Some("analog-output-speaker"));
        assert_eq!(
            facts.output_key().as_deref(),
            Some("alsa_card.pci-0000_00_1f.3/analog-output-speaker")
        );
    }

    #[test]
    fn an_output_on_a_card_whose_profile_is_off_sets_the_profile_then_the_default() {
        let fake = Fake::start("choose-hdmi");
        let (running, heard) = follow(&fake);
        let facts = heard.facts();
        let plan = facts
            .plan(&format!("{HDMI}/{JACK}"))
            .expect("a plan for the HDMI jack");
        running.ask(Ask::Choose(plan));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let facts = heard.facts();
            if facts.default_sink.as_deref() == Some(HDMI_SINK) {
                assert_eq!(
                    facts.output_key().as_deref(),
                    Some(format!("{HDMI}/{JACK}").as_str())
                );
                let hdmi = facts.cards.iter().find(|card| card.name == HDMI);
                assert_eq!(
                    hdmi.and_then(|card| card.profile.as_deref()),
                    Some(EXTRA1),
                    "the card is read again as it changes"
                );
                break;
            }
            assert!(std::time::Instant::now() < deadline, "never the default");
        }
        let asked = fake.asked();
        let at = |line: &str| {
            asked
                .iter()
                .position(|asked| asked == line)
                .unwrap_or_else(|| panic!("{line} not asked: {asked:?}"))
        };
        assert!(at(&format!("profile 1 {EXTRA1}")) < at(&format!("default {HDMI_SINK}")));
        assert!(
            !asked.iter().any(|asked| asked.starts_with("port ")),
            "its sink plays through the jack already"
        );
    }

    #[test]
    fn an_output_on_a_sink_playing_elsewhere_sets_its_port_then_the_default() {
        let fake = Fake::start("choose-port");
        let (running, heard) = follow(&fake);
        let facts = heard.facts();
        let plan = facts
            .plan("alsa_card.pci-0000_00_1f.3/analog-output-headphones")
            .expect("a plan for the headphones");
        running.ask(Ask::Choose(plan));
        let facts = heard.facts();
        assert_eq!(
            facts.output().and_then(|sink| sink.port.as_deref()),
            Some("analog-output-headphones")
        );
        let asked = fake.asked();
        assert!(
            !asked.iter().any(|asked| asked.starts_with("profile ")),
            "the card plays through it already: {asked:?}"
        );
        let port = asked
            .iter()
            .position(|asked| asked == "port analog-output-headphones");
        let default = asked
            .iter()
            .position(|asked| asked == &format!("default {SPEAKERS}"));
        assert!(port.is_some() && port < default, "{asked:?}");
    }

    #[test]
    fn a_choice_the_card_never_answers_is_given_up_on_and_told() {
        let fake = Fake::start("choose-never");
        let (running, heard) = follow(&fake);
        heard.facts();
        running.ask(Ask::Choose(Plan::Port {
            card: 1,
            card_name: HDMI.to_owned(),
            profile: Some("off".to_owned()),
            port: JACK.to_owned(),
        }));
        let failed = loop {
            match heard.next() {
                News::Failed(why) => break why,
                News::Facts(_) => {}
                News::Gone => panic!("the server went"),
            }
        };
        assert!(failed.contains(JACK), "{failed}");
    }

    #[test]
    fn a_lost_server_is_told_and_tried_again_after_a_pause() {
        let fake = Fake::start("lost");
        let (_running, heard) = follow(&fake);
        heard.facts();
        let cut = Instant::now();
        fake.hang_up();
        assert!(matches!(heard.next(), News::Gone));
        heard.facts();
        assert!(
            cut.elapsed() >= PAUSE_FIRST,
            "tried again only after a pause: {:?}",
            cut.elapsed()
        );
        // Reached again, the next loss is told again, after the first pause.
        let cut = Instant::now();
        fake.hang_up();
        assert!(matches!(heard.next(), News::Gone));
        heard.facts();
        assert!(cut.elapsed() < PAUSE_FIRST * 2, "{:?}", cut.elapsed());
    }

    #[test]
    fn no_server_is_told_once_and_tried_again() {
        let path = std::env::temp_dir().join(format!("perspicax-no-sound-{}", std::process::id()));
        let (sender, receiver) = std::sync::mpsc::channel();
        let _running = start(Some(path), move |news| sender.send(news).is_ok());
        let heard = Heard(receiver);
        assert!(matches!(heard.next(), News::Gone));
        assert!(
            heard.0.recv_timeout(PAUSE_FIRST * 4).is_err(),
            "told once, not at each try"
        );
    }

    #[test]
    fn a_message_the_client_does_not_know_is_skipped_not_fatal() {
        let fake = Fake::start("unknown");
        let (_running, heard) = follow(&fake);
        heard.facts();
        fake.send_unknown();
        fake.change_sink(1, 1, |sink| sink.muted = true);
        let facts = heard.facts();
        assert!(facts.sinks.iter().any(|sink| sink.index == 1 && sink.muted));
    }
}
