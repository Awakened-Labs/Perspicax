//! The sound server: what it holds, read and followed on a thread of its
//! own, and what a person asks of it, passed on.
//!
//! The shell speaks the PulseAudio protocol on its native socket, which
//! PipeWire serves through pipewire-pulse as PulseAudio itself does, so one
//! client covers both. The protocol's commands are written and read as the
//! `pulseaudio` crate encodes them; the client around them is the shell's
//! own (see `client`), since that crate's own client reads no server's
//! changes.
//!
//! The thread runs a calloop loop of its own, as the shell's does, woken by
//! the socket and by what the shell asks. What it reads reaches the shell's
//! loop as [`News`]: everything the server holds, once for each wake that
//! changed any of it, so a burst of changes is told once. Dropping the
//! [`Running`] the shell holds ends the thread.

mod client;
#[cfg(test)]
mod fake;
mod levels;
mod wire;

pub(crate) use levels::BANDS;

use std::path::PathBuf;

use smithay_client_toolkit::reexports::calloop::{
    EventLoop,
    channel::{self, Sender},
};

use crate::model::sound::{Facts, Plan};

/// What the thread read, for the shell to show.
#[derive(Debug)]
pub(crate) enum News {
    /// The server, as it is now.
    Facts(Box<Facts>),
    /// There is no server to read: none could be reached, or the one there
    /// was went away. Told once, until one is reached again.
    Gone,
    /// What was asked could not be done, and why.
    Failed(String),
    /// What plays now, as the spectrum's bands, each 0 to 1: silence once
    /// it stops.
    Levels([f32; BANDS]),
}

/// What a person asked of the sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ask {
    /// Mute `target`, or bring its sound back.
    Mute { target: Target, on: bool },
    /// Set `target`'s volume, each channel's, as the server counts it.
    Volume { target: Target, volume: Vec<u32> },
    /// Make an output the one sound goes to, as its plan says.
    Choose(Plan),
    /// Read what plays, for the moving bars, or stop.
    Levels(bool),
}

/// What an [`Ask`] is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// The sink of this name.
    Sink(String),
}

/// The thread, while it runs. Dropping it ends it.
pub(crate) struct Running {
    asks: Sender<Ask>,
}

impl Running {
    /// Pass `ask` to the thread.
    pub(crate) fn ask(&self, ask: Ask) {
        if let Err(error) = self.asks.send(ask) {
            tracing::debug!("the sound's thread has stopped; {:?} is dropped", error.0);
        }
    }
}

/// Follow the sound server at `server`, or the one the environment names,
/// telling `tell` what it holds until `tell` says the shell is gone.
pub(crate) fn start(
    server: Option<PathBuf>,
    tell: impl Fn(News) -> bool + Send + 'static,
) -> Running {
    let (asks, asked) = channel::channel();
    let started = std::thread::Builder::new()
        .name("perspicax-audio".to_owned())
        .spawn(move || {
            let mut event_loop = match EventLoop::<client::Client>::try_new() {
                Ok(event_loop) => event_loop,
                Err(error) => {
                    tracing::warn!("the sound has no loop to run on: {error}");
                    return;
                }
            };
            let signal = event_loop.get_signal();
            let mut client =
                client::Client::new(server, Box::new(tell), event_loop.handle(), signal.clone());
            let inserted = event_loop
                .handle()
                .insert_source(asked, move |event, (), client| match event {
                    channel::Event::Msg(ask) => client.ask(ask),
                    // The shell let go of the sound.
                    channel::Event::Closed => signal.stop(),
                });
            if let Err(error) = inserted {
                tracing::warn!("the sound cannot be asked anything: {error}");
                return;
            }
            client.connect();
            if let Err(error) = event_loop.run(None, &mut client, |_| {}) {
                tracing::warn!("the sound's loop stopped: {error}");
            }
        });
    if let Err(error) = started {
        tracing::warn!("the sound's thread could not start: {error}");
    }
    Running { asks }
}
