//! A sound server for the client's tests: the protocol's side a server
//! speaks, as the `pulseaudio` crate encodes it, over a socket of a test's
//! own, holding two sinks it can be made to change.
//!
//! It answers what the client asks, one connection at a time, and keeps a
//! line for each thing asked, to be counted. A change made from a test is
//! told the way a server tells it, as a subscription's event, as many times
//! as the test says.

use std::{
    ffi::CString,
    io::{BufReader, Write as _},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use pulseaudio::protocol::{
    self, AuthReply, ChannelMap, ChannelVolume, Command, MAX_VERSION, PulseError, SampleSpec,
    ServerInfo, SetClientNameReply, SinkInfo, SubscriptionEvent, SubscriptionEventFacility,
    SubscriptionEventType, Volume,
};

use crate::model::sound::NORM;

/// The server, serving on a thread until it is dropped.
pub(super) struct Fake {
    path: PathBuf,
    shared: Arc<Mutex<Shared>>,
}

struct Shared {
    sinks: Vec<SinkInfo>,
    default: CString,
    /// What was asked, in order: one line each.
    asked: Vec<String>,
    /// The connection being served, to write on.
    writer: Option<UnixStream>,
    version: u16,
}

impl Fake {
    /// A server at a socket named for `name`, holding the speakers, the
    /// default, at 24%, and a Bluetooth headset at 100%.
    pub(super) fn start(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("perspicax-sound-{name}-{}", std::process::id()));
        std::fs::remove_file(&path).ok();
        let listener = UnixListener::bind(&path).expect("a socket for the fake server");
        let shared = Arc::new(Mutex::new(Shared {
            sinks: vec![
                sink(0, "alsa_output.speakers", 0x3d71),
                sink(1, "bluez_output.E8", NORM),
            ],
            default: c"alsa_output.speakers".to_owned(),
            asked: Vec::new(),
            writer: None,
            version: MAX_VERSION,
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
        let version = self.version;
        if let Some(writer) = self.writer.as_mut() {
            let event = SubscriptionEvent {
                event_facility: facility,
                event_type: SubscriptionEventType::Changed,
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
}

/// Serve one client until it goes.
fn serve(shared: &Mutex<Shared>, stream: UnixStream) {
    let mut reader = BufReader::new(stream.try_clone().expect("a second handle"));
    {
        let mut shared = shared.lock().expect("the state");
        shared.writer = Some(stream);
        shared.version = MAX_VERSION;
    }
    loop {
        let version = shared.lock().expect("the state").version;
        let Ok((seq, command)) = protocol::read_command_message(&mut reader, version) else {
            return;
        };
        let mut shared = shared.lock().expect("the state");
        let version = shared.version;
        let Shared {
            sinks,
            default,
            asked,
            writer,
            ..
        } = &mut *shared;
        let Some(writer) = writer.as_mut() else {
            return;
        };
        let (mut changed, mut negotiated) = (None, None);
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
            Command::Subscribe(_) => {
                asked.push("subscribe".to_owned());
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
            Command::SetSinkMute(mute) => {
                asked.push(format!("mute {:?}", mute.device_name));
                match sinks
                    .iter_mut()
                    .find(|sink| Some(&sink.name) == mute.device_name.as_ref())
                {
                    Some(sink) => {
                        sink.muted = mute.mute;
                        changed = Some(sink.index);
                        protocol::write_ack_message(writer, seq)
                    }
                    None => protocol::write_error(writer, seq, &PulseError::NoEntity),
                }
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
        if let Some(index) = changed {
            shared.event(SubscriptionEventFacility::Sink, Some(index));
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
