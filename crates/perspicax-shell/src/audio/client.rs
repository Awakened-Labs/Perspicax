//! The client: connected, it reads what the server holds and follows it;
//! cut off, it says so and tries again after a pause.
//!
//! Connecting is the protocol's handshake, the version both ends speak and
//! the shell's name, then a subscription to the server's changes, then a
//! read of everything there is to show. Nothing is told until that read is
//! whole. From then on, a change the server tells is read again: the object
//! it names, once however often it is told while that read is out. A change
//! told before the reply to a read was sent before that reply, so the reply
//! already holds it. An object removed is dropped.
//!
//! A server that cannot be reached, or goes away, is told once, and tried
//! again after a pause that doubles each time, from [`PAUSE_FIRST`] to
//! [`PAUSE_MOST`], so a server that is not coming back costs next to
//! nothing; the pause starts afresh once one is reached.

use std::{
    collections::HashSet,
    ffi::CString,
    io,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use pulseaudio::protocol::{
    AuthParams, ChannelVolume, Command, GetSinkInfo, MAX_VERSION, Prop, Props, PulseError,
    SetDeviceMuteParams, SetDeviceVolumeParams, SinkInfo, SubscriptionEvent,
    SubscriptionEventFacility, SubscriptionEventType, SubscriptionMask, Volume,
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
use crate::model::sound::{Facts, Sink};

/// The first pause before trying a server again, and the longest.
pub(super) const PAUSE_FIRST: Duration = Duration::from_millis(500);
pub(super) const PAUSE_MOST: Duration = Duration::from_secs(10);
/// How long a server has to answer the handshake.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);

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
        };
        if let Err(error) = link.wire.send(&command, Expect::Done) {
            self.lost(&error.to_string());
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
                let mask = SubscriptionMask::SINK | SubscriptionMask::SERVER;
                self.wire.send(&Command::Subscribe(mask), Expect::Done)?;
                self.first = 2;
                self.wire.send(&Command::GetServerInfo, Expect::Server)?;
                self.wire.send(&Command::GetSinkInfoList, Expect::Sinks)?;
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
            (Facility::Server, ..) => (Command::GetServerInfo, Expect::Server),
            _ => return Ok(()),
        };
        self.read_again(&read.0, read.1)
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
    Sink {
        index: info.index,
        name: text(&info.name),
        volume: info
            .cvolume
            .channels()
            .iter()
            .map(|volume| volume.as_u32())
            .collect(),
        muted: info.muted,
    }
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
    use crate::audio::{fake::Fake, start};

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
        assert_eq!(facts.default_sink.as_deref(), Some("alsa_output.speakers"));
        assert_eq!(facts.sinks.len(), 2);
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
