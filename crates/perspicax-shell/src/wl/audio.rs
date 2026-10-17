//! The sound's glue: its thread, running while a panel holds the mixer;
//! what it reads, shown on the panels; and a press on one of the mixer's
//! pieces, asked of the server.

use std::path::PathBuf;

use perspicax_config::{Item, Shell, mixer::Piece};
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::App;
use crate::{
    audio::{Ask, News, Running, Target},
    model::{Button, sound::Facts},
};

/// The sound, as the loop knows it.
pub(super) struct Audio {
    /// The server's socket: `None` for the one the environment names.
    server: Option<PathBuf>,
    /// Where its news reaches the loop, marked with which start of it the
    /// news is from.
    news: Sender<(u64, News)>,
    /// Its thread, while a panel holds the mixer.
    running: Option<Running>,
    /// How many times it has started: news from an earlier start is from a
    /// thread since stopped.
    started: u64,
    /// What the server holds, as last told: `None` with no server.
    facts: Option<Box<Facts>>,
}

impl Audio {
    pub(super) fn new(server: Option<PathBuf>, news: Sender<(u64, News)>) -> Self {
        Self {
            server,
            news,
            running: None,
            started: 0,
            facts: None,
        }
    }

    /// Run the thread if `shell`'s panel holds the mixer, and stop it if
    /// not. Whether it stopped.
    fn follow(&mut self, shell: &Shell) -> bool {
        let wanted = shell
            .panel
            .as_ref()
            .is_some_and(|panel| panel.items.contains(&Item::Mixer));
        match (wanted, &self.running) {
            (true, None) => {
                self.started += 1;
                let (news, started) = (self.news.clone(), self.started);
                self.running = Some(crate::audio::start(self.server.clone(), move |told| {
                    news.send((started, told)).is_ok()
                }));
                false
            }
            (false, Some(_)) => {
                self.running = None;
                self.facts = None;
                true
            }
            _ => false,
        }
    }

    fn ask(&self, ask: Ask) {
        if let Some(running) = &self.running {
            running.ask(ask);
        }
    }
}

impl App {
    /// Run the sound's thread if the panel `shell` describes holds the
    /// mixer, and stop it if not.
    pub(super) fn follow_audio(&mut self, shell: &Shell) {
        if self.audio.follow(shell) {
            self.heard();
        }
    }

    /// News from start `started` of the sound's thread.
    pub(super) fn audio_news(&mut self, started: u64, news: News) {
        if started != self.audio.started {
            return;
        }
        self.audio.facts = match news {
            News::Facts(facts) => Some(facts),
            News::Gone => None,
        };
        self.heard();
    }

    /// Show the panels what was last heard of the sound.
    fn heard(&mut self) {
        let level = self
            .audio
            .facts
            .as_ref()
            .and_then(|facts| facts.output())
            .map(|output| output.level());
        if self.panels.hear(level) {
            self.panels_changed();
        }
    }

    /// `button` went down on the mixer's `piece`.
    pub(super) fn press_mixer(&mut self, piece: Piece, button: Button) {
        let Some(output) = self.audio.facts.as_ref().and_then(|facts| facts.output()) else {
            return;
        };
        let ask = match (piece, button) {
            (Piece::Mute, Button::Left) => Ask::Mute {
                target: Target::Sink(output.name.clone()),
                on: !output.muted,
            },
            _ => return,
        };
        self.audio.ask(ask);
    }
}
