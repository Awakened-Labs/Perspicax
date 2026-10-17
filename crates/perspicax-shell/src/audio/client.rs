//! The client: connected, it reads what the server holds and follows it;
//! cut off, it says so and tries again after a pause.
//!
//! Connecting is the protocol's handshake, the version both ends speak and
//! the shell's name, then a subscription to the server's changes, then a
//! read of everything there is to show. Nothing is told until that read is
//! whole. pipewire-pulse answers a new client's first reads before it has
//! found what it holds, with no sinks and no cards, then tells each as new;
//! so the first facts told may be empty, and the rest follow at once.
//!
//! A change the server tells is read again, the first read whole or not:
//! the object it names, once however often it is told while that read is
//! out. The server answers in turn, so a read again comes after the first
//! read of its kind, and is the newer. An object removed is dropped.
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
//!
//! What programs play, their streams, is read and followed as the devices
//! are, the shell's own among them: the shell leaves those out by the
//! number the server gives its client.
//!
//! While the moving bars are wanted, and something plays through the
//! default output, its monitor is recorded, one channel of floats, and its
//! spectrum told (see `levels`). Recording is what a VU meter does to a
//! monitor; the stream is marked not to keep the output from suspending,
//! and it is let go of as soon as nothing plays, or another output becomes
//! the default, so a quiet desktop records nothing.

use std::{
    collections::HashSet,
    ffi::CString,
    io,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use pulseaudio::protocol::{
    AuthParams, CardInfo, ChannelMap, ChannelVolume, Command, GetCardInfo, GetSinkInfo,
    GetSourceInfo, MAX_VERSION, MoveStreamParams, Prop, Props, PulseError, RecordStreamParams,
    SampleFormat, SampleSpec, SetCardProfileParams, SetDeviceMuteParams, SetDeviceVolumeParams,
    SetPortParams, SetStreamMuteParams, SetStreamVolumeParams, SinkInfo, SinkInputInfo, SinkState,
    SourceInfo, SubscriptionEvent, SubscriptionEventFacility, SubscriptionEventType,
    SubscriptionMask, Volume,
    port_info::{PortAvailable, PortDirection, PortType},
    stream::{BufferAttr, StreamFlags},
};
use smithay_client_toolkit::reexports::calloop::{
    Interest, LoopHandle, LoopSignal, Mode, PostAction, RegistrationToken,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};

use super::{
    Ask, BANDS, News, Target,
    levels::{HOP, RATE, Spectrum},
    wire::{Expect, Incoming, Reply, Wire},
};
use crate::model::sound::{
    Card, Facts, Kind, Plan, Plugged, Port, Profile, Side, Sink, Source, Stream,
};

/// The first pause before trying a server again, and the longest.
pub(super) const PAUSE_FIRST: Duration = Duration::from_millis(500);
pub(super) const PAUSE_MOST: Duration = Duration::from_secs(10);
/// How long a server has to answer the handshake.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);
/// How long a card has to give the sink an output chosen plays through.
const CHOOSE_WITHIN: Duration = Duration::from_secs(3);
/// What the first read reads: each kind's list, and the server's defaults.
const LISTS: [Expect; 5] = [
    Expect::Server,
    Expect::Sinks,
    Expect::Sources,
    Expect::Cards,
    Expect::Streams,
];

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
    /// The moving bars are wanted.
    levels: bool,
}

/// A connection, and what it has read.
struct Link {
    wire: Wire,
    /// The socket's source in the loop.
    source: RegistrationToken,
    /// The handshake's deadline, until the first read is whole.
    deadline: Option<RegistrationToken>,
    facts: Facts,
    /// What of the first read is still to come, each kind's list, or the
    /// handshake before them: news waits for none of it.
    first: HashSet<Expect>,
    /// The first read is whole, and that has been acted on.
    following: bool,
    /// What is being read again, so that a change told meanwhile asks for
    /// no second read.
    reading: HashSet<Expect>,
    /// The facts changed since they were last told.
    changed: bool,
    /// The device being chosen: its side, its card's number, its port, and
    /// which choice it is, to be made the default once a sink or a source
    /// plays or records through it.
    choosing: Option<(Side, u32, String, u64)>,
    /// How many choices there have been.
    chosen: u64,
    /// The moving bars are wanted.
    levels: bool,
    /// The record stream they are read from, while there is one.
    recording: Option<Recording>,
    /// How many record streams have been asked for.
    recordings: u64,
    spectrum: Spectrum,
    /// The bands to tell, newest, since they were last told.
    heard: Option<[f32; BANDS]>,
}

/// A record stream of the default output's monitor.
struct Recording {
    /// Which it is, to match the server's reply to.
    id: u64,
    /// The source it records, by name.
    source: String,
    /// Its channel, once the server has made it.
    channel: Option<u32>,
    /// What came of a sample short of whole.
    leftover: Vec<u8>,
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
            levels: false,
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
                if let Some(link) = client.link.as_mut().filter(|link| !link.first.is_empty()) {
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
            first: HashSet::from([Expect::Auth]),
            following: false,
            reading: HashSet::new(),
            changed: false,
            choosing: None,
            chosen: 0,
            levels: self.levels,
            recording: None,
            recordings: 0,
            spectrum: Spectrum::new(),
            heard: None,
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
            link.follow_levels()?;
            link.wire.flush()
        });
        if let Err(error) = result {
            self.lost(&error.to_string());
            return;
        }
        if !link.first.is_empty() {
            return;
        }
        let met = !std::mem::replace(&mut link.following, true);
        let deadline = met.then(|| link.deadline.take()).flatten();
        let facts = std::mem::take(&mut link.changed).then(|| Box::new(link.facts.clone()));
        let heard = link.heard.take();
        if met {
            if let Some(deadline) = deadline {
                self.handle.remove(deadline);
            }
            tracing::info!("following the sound server");
            self.pause = PAUSE_FIRST;
            self.told_gone = false;
        }
        if let Some(facts) = facts {
            self.tell(News::Facts(facts));
        }
        if let Some(bands) = heard {
            self.tell(News::Levels(bands));
        }
    }

    /// Do what the shell asked.
    pub(super) fn ask(&mut self, ask: Ask) {
        if let Ask::Levels(on) = ask {
            self.levels = on;
            if let Some(link) = &mut self.link {
                link.levels = on;
                let followed = link.follow_levels().and_then(|()| link.wire.flush());
                if let Err(error) = followed {
                    self.lost(&error.to_string());
                    return;
                }
                let heard = link.heard.take();
                if let Some(bands) = heard {
                    self.tell(News::Levels(bands));
                }
            }
            return;
        }
        let Some(link) = self.link.as_mut().filter(|link| link.first.is_empty()) else {
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
        let mute = |name: String, on| SetDeviceMuteParams {
            device_index: None,
            device_name: CString::new(name).ok(),
            mute: on,
        };
        let set = |name: String, volume: &[u32]| SetDeviceVolumeParams {
            device_index: None,
            device_name: CString::new(name).ok(),
            volume: channels(volume),
        };
        let command = match ask {
            Ask::Mute { target, on } => match target {
                Target::Sink(name) => Command::SetSinkMute(mute(name, on)),
                Target::Source(name) => Command::SetSourceMute(mute(name, on)),
                Target::Stream(index) => {
                    Command::SetSinkInputMute(SetStreamMuteParams { index, mute: on })
                }
            },
            Ask::Volume { target, volume } => match target {
                Target::Sink(name) => Command::SetSinkVolume(set(name, &volume)),
                Target::Source(name) => Command::SetSourceVolume(set(name, &volume)),
                Target::Stream(index) => Command::SetSinkInputVolume(SetStreamVolumeParams {
                    index,
                    volume: channels(&volume),
                }),
            },
            Ask::Move { stream, sink } => Command::MoveSinkInput(MoveStreamParams {
                index: Some(stream),
                device_index: Some(sink),
                device_name: None,
            }),
            Ask::Choose(_) | Ask::Levels(_) => return,
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
                    if let Some((_, card, port, _)) = given_up {
                        let why = format!("card {card} gave nothing that goes through {port}");
                        tracing::warn!("the device was not chosen: {why}");
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
            Incoming::Event(command) => match *command {
                Command::SubscribeEvent(event) => self.told(event)?,
                Command::RecordStreamKilled(channel) => {
                    self.recording
                        .take_if(|recording| recording.channel == Some(channel));
                }
                _ => {}
            },
            Incoming::Data(channel, data) => self.recorded(channel, &data),
        }
        Ok(())
    }

    /// Record the default output's monitor if the bars are wanted and
    /// something plays through it, and stop recording if not, or if another
    /// output is the default now.
    fn follow_levels(&mut self) -> io::Result<()> {
        let wanted = self
            .facts
            .output()
            .filter(|sink| self.levels && self.first.is_empty() && sink.running)
            .and_then(|sink| sink.monitor.clone());
        if self.recording.as_ref().map(|recording| &recording.source) == wanted.as_ref() {
            return Ok(());
        }
        if let Some(recording) = self.recording.take() {
            if let Some(channel) = recording.channel {
                self.wire
                    .send(&Command::DeleteRecordStream(channel), Expect::Done)?;
            }
            self.heard = Some([0.0; BANDS]);
        }
        let Some(source) = wanted else {
            return Ok(());
        };
        self.recordings += 1;
        let mut props = Props::new();
        props.set(Prop::MediaName, c"Sound levels");
        let record = RecordStreamParams {
            sample_spec: SampleSpec {
                format: SampleFormat::Float32Le,
                channels: 1,
                sample_rate: RATE,
            },
            channel_map: ChannelMap::mono(),
            source_name: Some(cstring(source.clone())),
            buffer_attr: BufferAttr {
                max_length: u32::MAX,
                target_length: u32::MAX,
                pre_buffering: u32::MAX,
                minimum_request_length: u32::MAX,
                fragment_size: (HOP * 4) as u32,
            },
            flags: StreamFlags {
                adjust_latency: true,
                no_inhibit_auto_suspend: true,
                no_move: true,
                ..StreamFlags::default()
            },
            props,
            ..RecordStreamParams::default()
        };
        self.wire.send(
            &Command::CreateRecordStream(record),
            Expect::Recording(self.recordings),
        )?;
        self.recording = Some(Recording {
            id: self.recordings,
            source,
            channel: None,
            leftover: Vec::new(),
        });
        Ok(())
    }

    /// What the stream of `channel` recorded.
    fn recorded(&mut self, channel: u32, data: &[u8]) {
        let Some(recording) = self
            .recording
            .as_mut()
            .filter(|recording| recording.channel == Some(channel))
        else {
            return;
        };
        recording.leftover.extend_from_slice(data);
        let whole = recording.leftover.len() / 4 * 4;
        let samples: Vec<f32> = recording.leftover[..whole]
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        recording.leftover.drain(..whole);
        if let Some(bands) = self.spectrum.feed(&samples) {
            self.heard = Some(bands);
        }
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
                let mask = SubscriptionMask::SINK
                    | SubscriptionMask::SOURCE
                    | SubscriptionMask::CARD
                    | SubscriptionMask::SERVER
                    | SubscriptionMask::SINK_INPUT;
                self.wire.send(&Command::Subscribe(mask), Expect::Done)?;
                self.first = HashSet::from(LISTS);
                self.wire.send(&Command::GetServerInfo, Expect::Server)?;
                self.wire.send(&Command::GetSinkInfoList, Expect::Sinks)?;
                self.wire
                    .send(&Command::GetSourceInfoList, Expect::Sources)?;
                self.wire.send(&Command::GetCardInfoList, Expect::Cards)?;
                self.wire
                    .send(&Command::GetSinkInputInfoList, Expect::Streams)?;
                false
            }
            Reply::Named(client) => {
                self.facts.client = Some(client);
                true
            }
            Reply::Done => false,
            Reply::Recording(made) => {
                let Expect::Recording(id) = expect else {
                    return Ok(());
                };
                match &mut self.recording {
                    Some(recording) if recording.id == id => {
                        recording.channel = Some(made.channel);
                    }
                    // Let go of before the server made it.
                    _ => self
                        .wire
                        .send(&Command::DeleteRecordStream(made.channel), Expect::Done)?,
                }
                false
            }
            Reply::Server(server) => {
                self.read_first(Expect::Server);
                let sink = server.default_sink_name.as_deref().map(text);
                let source = server.default_source_name.as_deref().map(text);
                let sink = std::mem::replace(&mut self.facts.default_sink, sink)
                    != self.facts.default_sink;
                let source = std::mem::replace(&mut self.facts.default_source, source)
                    != self.facts.default_source;
                sink || source
            }
            Reply::Sources(sources) => {
                self.read_first(Expect::Sources);
                self.facts.sources = sources.iter().map(source).collect();
                true
            }
            Reply::Source(read) => {
                let read = source(&read);
                match self
                    .facts
                    .sources
                    .iter_mut()
                    .find(|had| had.index == read.index)
                {
                    Some(had) => std::mem::replace(had, read) != *had,
                    None => {
                        self.facts.sources.push(read);
                        true
                    }
                }
            }
            Reply::Sinks(sinks) => {
                self.read_first(Expect::Sinks);
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
                self.read_first(Expect::Cards);
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
            Reply::Streams(streams) => {
                self.read_first(Expect::Streams);
                self.facts.streams = streams.iter().map(stream).collect();
                true
            }
            Reply::Stream(read) => {
                let read = stream(&read);
                match self
                    .facts
                    .streams
                    .iter_mut()
                    .find(|had| had.index == read.index)
                {
                    Some(had) => std::mem::replace(had, read) != *had,
                    None => {
                        self.facts.streams.push(read);
                        true
                    }
                }
            }
        };
        self.changed |= changed;
        Ok(())
    }

    /// The first read's reply of `expect`, one of the [`LISTS`], came.
    fn read_first(&mut self, expect: Expect) {
        self.first.remove(&expect);
    }

    fn refused(&mut self, expect: Expect, error: PulseError) {
        self.reading.remove(&expect);
        // A list refused is read as empty: the rest is told all the same.
        if LISTS.contains(&expect) {
            self.read_first(expect);
        }
        match expect {
            // Gone between the change told and the read.
            Expect::Sink(index) if error == PulseError::NoEntity => {
                self.facts.sinks.retain(|sink| sink.index != index);
                self.changed = true;
            }
            Expect::Source(index) if error == PulseError::NoEntity => {
                self.facts.sources.retain(|source| source.index != index);
                self.changed = true;
            }
            Expect::Card(index) if error == PulseError::NoEntity => {
                self.facts.cards.retain(|card| card.index != index);
                self.changed = true;
            }
            Expect::Stream(index) if error == PulseError::NoEntity => {
                self.facts.streams.retain(|stream| stream.index != index);
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
            (Facility::Source, SubscriptionEventType::Removed, Some(index)) => {
                self.facts.sources.retain(|source| source.index != index);
                self.changed = true;
                return Ok(());
            }
            (Facility::Source, _, Some(index)) => (
                Command::GetSourceInfo(GetSourceInfo {
                    index: Some(index),
                    name: None,
                }),
                Expect::Source(index),
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
            (Facility::SinkInput, SubscriptionEventType::Removed, Some(index)) => {
                self.facts.streams.retain(|stream| stream.index != index);
                self.changed = true;
                return Ok(());
            }
            (Facility::SinkInput, _, Some(index)) => {
                (Command::GetSinkInputInfo(index), Expect::Stream(index))
            }
            (Facility::Server, ..) => (Command::GetServerInfo, Expect::Server),
            _ => return Ok(()),
        };
        self.read_again(&read.0, read.1)
    }

    /// Start making the device `plan` names the default on its side. The
    /// choice, if it waits on the card.
    fn choose(&mut self, plan: Plan) -> io::Result<Option<u64>> {
        match plan {
            Plan::Device(side, name) => {
                self.choosing = None;
                self.wire.send(&default(side, name), Expect::Done)?;
                Ok(None)
            }
            Plan::Port {
                side,
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
                self.choosing = Some((side, card, port, self.chosen));
                self.advance()?;
                Ok(self.choosing.as_ref().map(|&(.., choice)| choice))
            }
        }
    }

    /// Make the device being chosen the default, once a sink or a source
    /// of its card plays or records through its port.
    fn advance(&mut self) -> io::Result<()> {
        let Some((side, card, port, _)) = &self.choosing else {
            return Ok(());
        };
        let (side, at) = (*side, Some(*card));
        let found = match side {
            Side::Output => self
                .facts
                .sinks
                .iter()
                .find(|sink| sink.card == at && sink.ports.contains(port))
                .map(|sink| (sink.name.clone(), sink.port.as_ref() != Some(port))),
            Side::Input => self
                .facts
                .sources
                .iter()
                .find(|source| source.card == at && source.ports.contains(port))
                .map(|source| (source.name.clone(), source.port.as_ref() != Some(port))),
        };
        let Some((name, elsewhere)) = found else {
            return Ok(());
        };
        let port = port.clone();
        self.choosing = None;
        if elsewhere {
            let set = SetPortParams {
                index: None,
                name: Some(cstring(name.clone())),
                port_name: cstring(port),
            };
            let command = match side {
                Side::Output => Command::SetSinkPort(set),
                Side::Input => Command::SetSourcePort(set),
            };
            self.wire.send(&command, Expect::Done)?;
        }
        self.wire.send(&default(side, name), Expect::Done)
    }

    /// Read again what `expect` is, unless it is being read again already.
    fn read_again(&mut self, command: &Command, expect: Expect) -> io::Result<()> {
        if !self.reading.insert(expect) {
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
        running: info.state == SinkState::Running,
        monitor: info.monitor_source_name.as_deref().map(text),
        card: info.card_index,
        port: ports.get(info.active_port).cloned(),
        ports,
    }
}

/// The command that makes `name` the default on `side`.
fn default(side: Side, name: String) -> Command {
    match side {
        Side::Output => Command::SetDefaultSink(cstring(name)),
        Side::Input => Command::SetDefaultSource(cstring(name)),
    }
}

/// A source, as the shell keeps it.
fn source(info: &SourceInfo) -> Source {
    let ports: Vec<String> = info.ports.iter().map(|port| text(&port.name)).collect();
    Source {
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
        monitor: info.monitor_of_sink_index.is_some(),
    }
}

/// A program's stream, as the shell keeps it.
fn stream(info: &SinkInputInfo) -> Stream {
    Stream {
        index: info.index,
        client: info.client_index,
        sink: info.sink_index,
        application: prop(&info.props, c"application.name"),
        binary: prop(&info.props, c"application.process.binary"),
        media: text(&info.name),
        icon: prop(&info.props, c"application.icon_name"),
        id: prop(&info.props, c"application.id"),
        volume: info
            .cvolume
            .channels()
            .iter()
            .map(|volume| volume.as_u32())
            .collect(),
        muted: info.muted,
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
        details: details(&info.props),
        ports,
        profiles,
        profile: info.active_profile.as_deref().map(text),
    }
}

/// What tells a card apart from another of its make, of what its `props`
/// say: where it is attached, its chip, its ALSA card, its Bluetooth
/// address.
fn details(props: &Props) -> Vec<(&'static str, String)> {
    let alsa = match (prop(props, c"alsa.card"), prop(props, c"alsa.card_name")) {
        (Some(card), Some(name)) => Some(format!("{card} \u{b7} {name}")),
        (card, name) => card.or(name),
    };
    [
        ("Attached at", prop(props, c"device.bus_path")),
        ("Chip", prop(props, c"alsa.mixer_name")),
        ("ALSA card", alsa),
        ("Bluetooth", prop(props, c"api.bluez5.address")),
    ]
    .into_iter()
    .filter_map(|(label, value)| Some((label, value?)))
    .collect()
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
        PortType::Mic => Kind::Mic,
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
            } else if named("mic") {
                Kind::Mic
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
        fake::{BUILT_IN, EXTRA1, Fake, HDMI, HDMI_SINK, HEADSET_MIC, JACK, MIC, SPEAKERS},
        levels::RATE,
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

    /// The facts once `ready` says they are as awaited.
    fn until_heard(heard: &Heard, ready: impl Fn(&Facts) -> bool) -> Facts {
        loop {
            let facts = heard.facts();
            if ready(&facts) {
                return facts;
            }
        }
    }

    #[test]
    fn a_programs_stream_is_read_and_followed_and_the_shells_number_told() {
        let fake = Fake::start("streams");
        fake.plays(40, 12, Some("Firefox"), Some("firefox"));
        let (_running, heard) = follow(&fake);
        let facts = heard.facts();
        assert_eq!(facts.client, Some(7), "the shell's own");
        let firefox = &facts.streams[0];
        assert_eq!(
            (firefox.index, firefox.client, firefox.sink, firefox.name()),
            (40, Some(12), 0, "Firefox")
        );
        assert_eq!(firefox.binary.as_deref(), Some("firefox"));
        assert_eq!(firefox.level().percent, 100);
        fake.plays(41, 13, None, Some("mpv"));
        let facts = until_heard(&heard, |facts| facts.streams.len() == 2);
        assert_eq!(facts.streams[1].name(), "mpv", "a new one read");
        fake.stops(40);
        let facts = until_heard(&heard, |facts| facts.streams.len() == 1);
        assert_eq!(facts.streams[0].index, 41, "an ended one dropped");
    }

    #[test]
    fn nothing_is_told_until_every_list_is_read() {
        let fake = Fake::start("held");
        fake.hold_streams();
        fake.plays(40, 12, Some("Firefox"), None);
        let (_running, heard) = follow(&fake);
        until_asked(&fake, "streams");
        assert!(
            heard.0.recv_timeout(Duration::from_millis(200)).is_err(),
            "nothing told while the streams are still to come"
        );
        fake.answer_streams();
        assert_eq!(heard.facts().streams.len(), 1, "then everything at once");
    }

    #[test]
    fn a_change_told_after_its_kinds_first_read_is_read_while_another_kind_is_still_out() {
        let fake = Fake::start("between");
        fake.hold_streams();
        let (_running, heard) = follow(&fake);
        until_asked(&fake, "streams");
        // The sinks were read whole before this, and the streams are not
        // yet: pipewire-pulse tells each sink it finds just so.
        fake.change_sink(1, 1, |sink| sink.muted = true);
        until_asked(&fake, "sink 1");
        fake.answer_streams();
        let muted = |facts: &Facts| facts.sinks.iter().any(|sink| sink.index == 1 && sink.muted);
        until_heard(&heard, muted);
    }

    #[test]
    fn a_streams_mute_volume_and_move_reach_the_server_for_it_alone() {
        let fake = Fake::start("stream-asks");
        fake.plays(40, 12, Some("Firefox"), None);
        fake.plays(41, 13, Some("mpv"), None);
        let (running, heard) = follow(&fake);
        heard.facts();
        running.ask(Ask::Mute {
            target: Target::Stream(40),
            on: true,
        });
        running.ask(Ask::Volume {
            target: Target::Stream(40),
            volume: vec![0x8000, 0x8000],
        });
        running.ask(Ask::Move {
            stream: 40,
            sink: 1,
        });
        let facts = until_heard(&heard, |facts| facts.streams[0].sink == 1);
        let (firefox, mpv) = (&facts.streams[0], &facts.streams[1]);
        assert_eq!(
            (firefox.muted, firefox.level().percent),
            (true, 50),
            "muted and set"
        );
        assert_eq!((mpv.sink, mpv.muted, mpv.level().percent), (0, false, 100));
        let asked = fake.asked();
        for line in ["mute stream 40", "volume stream 40", "move 40 to 1"] {
            assert!(asked.iter().any(|asked| asked == line), "{line}: {asked:?}");
        }
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
        assert_eq!(
            hdmi.details,
            [
                ("Attached at", "pci-0000:01:00.1".to_owned()),
                ("Chip", "Nvidia GPU 99 HDMI/DP".to_owned()),
                ("ALSA card", "1 \u{b7} HDA NVidia".to_owned()),
            ],
            "what tells it apart from another of its make"
        );
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
                (HEADSET_MIC, false),
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
            .plan(Side::Output, &format!("{HDMI}/{JACK}"))
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
            .plan(
                Side::Output,
                "alsa_card.pci-0000_00_1f.3/analog-output-headphones",
            )
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
            side: Side::Output,
            card: 1,
            card_name: HDMI.to_owned(),
            profile: Some("off".to_owned()),
            port: JACK.to_owned(),
        }));
        let failed = loop {
            match heard.next() {
                News::Failed(why) => break why,
                News::Facts(_) | News::Levels(_) => {}
                News::Gone => panic!("the server went"),
            }
        };
        assert!(failed.contains(JACK), "{failed}");
    }

    /// A loud 1 kHz sine, `count` samples of it.
    fn sine(count: usize) -> Vec<f32> {
        (0..count)
            .map(|at| {
                let phase = 2.0 * std::f32::consts::PI * 1000.0 * at as f32 / RATE as f32;
                0.5 * phase.sin()
            })
            .collect()
    }

    /// The next bands heard, past any facts.
    fn levels(heard: &Heard) -> [f32; BANDS] {
        loop {
            match heard.next() {
                News::Levels(bands) => return bands,
                News::Facts(_) => {}
                other => panic!("not the bands: {other:?}"),
            }
        }
    }

    /// Wait until `fake` is asked `line`.
    fn until_asked(fake: &Fake, line: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !fake.asked().iter().any(|asked| asked == line) {
            assert!(
                std::time::Instant::now() < deadline,
                "never asked {line}: {:?}",
                fake.asked()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_default_outputs_monitor_is_recorded_only_while_wanted_and_playing() {
        let fake = Fake::start("levels");
        let (running, heard) = follow(&fake);
        heard.facts();
        running.ask(Ask::Levels(true));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fake.recording(), None, "nothing plays");
        fake.play(0, true);
        until_asked(&fake, &format!("record {SPEAKERS}.monitor"));
        let channel = fake.recording().expect("a channel");
        fake.send_samples(channel, &sine(1024));
        let bands = levels(&heard);
        assert!(bands.iter().any(|&band| band > 0.8), "{bands:?}");
        // It stops: let go of, and silence told, for the bars to fall.
        fake.play(0, false);
        until_asked(&fake, &format!("unrecord {channel}"));
        assert_eq!(levels(&heard), [0.0; BANDS]);
    }

    #[test]
    fn the_bars_unwanted_record_nothing_and_what_another_stream_sends_is_not_heard() {
        let fake = Fake::start("unwanted");
        let (running, heard) = follow(&fake);
        heard.facts();
        fake.play(0, true);
        heard.facts();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fake.recording(), None, "the bars not wanted");
        running.ask(Ask::Levels(true));
        until_asked(&fake, &format!("record {SPEAKERS}.monitor"));
        let channel = fake.recording().expect("a channel");
        fake.send_samples(channel + 1, &sine(2048));
        fake.send_samples(channel, &[0.0; 1024]);
        assert_eq!(
            levels(&heard),
            [0.0; BANDS],
            "the other channel's sine unheard"
        );
        running.ask(Ask::Levels(false));
        until_asked(&fake, &format!("unrecord {channel}"));
    }

    #[test]
    fn the_default_input_is_read_and_muted_and_set() {
        let fake = Fake::start("mic");
        let (running, heard) = follow(&fake);
        let facts = heard.facts();
        let mic = facts.input().expect("the default input");
        assert_eq!((mic.name.as_str(), mic.level().percent), (MIC, 50));
        running.ask(Ask::Mute {
            target: Target::Source(MIC.to_owned()),
            on: true,
        });
        assert!(heard.facts().input().expect("the input").muted);
        running.ask(Ask::Volume {
            target: Target::Source(MIC.to_owned()),
            volume: vec![0x4000],
        });
        assert_eq!(
            heard.facts().input().expect("the input").level().percent,
            25
        );
        assert!(
            !fake
                .asked()
                .iter()
                .any(|asked| asked.starts_with("mute Some(\"alsa_output"))
        );
    }

    #[test]
    fn an_input_of_no_card_is_made_the_default_source() {
        let fake = Fake::start("choose-input");
        let (running, heard) = follow(&fake);
        heard.facts();
        running.ask(Ask::Choose(Plan::Device(Side::Input, MIC.to_owned())));
        until_asked(&fake, &format!("default source {MIC}"));
        assert!(
            !fake
                .asked()
                .iter()
                .any(|asked| asked.starts_with("default alsa")),
            "the output left alone"
        );
    }

    #[test]
    fn a_cards_input_jack_is_set_on_its_source_then_made_the_default() {
        let fake = Fake::start("choose-headset-mic");
        let (running, heard) = follow(&fake);
        let facts = heard.facts();
        let key = format!("{BUILT_IN}/{HEADSET_MIC}");
        let plan = facts.plan(Side::Input, &key).expect("a plan for the jack");
        running.ask(Ask::Choose(plan));
        until_asked(&fake, &format!("default source {MIC}"));
        let asked = fake.asked();
        let at = |line: &str| asked.iter().position(|asked| asked == line);
        let port = at(&format!("source port {HEADSET_MIC}")).expect("the port set");
        assert!(port < at(&format!("default source {MIC}")).expect("the default"));
        assert!(
            !asked.iter().any(|asked| asked.starts_with("profile")),
            "the card's profile has the jack"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while heard.facts().chosen(Side::Input).as_deref() != Some(key.as_str()) {
            assert!(std::time::Instant::now() < deadline, "never read again");
        }
    }

    #[test]
    fn a_list_the_server_refuses_is_read_as_empty_and_the_rest_told() {
        let fake = Fake::start("refused");
        fake.refuse_sources();
        let (_running, heard) = follow(&fake);
        let facts = heard.facts();
        assert!(facts.sources.is_empty());
        assert_eq!(facts.sinks.len(), 2);
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
