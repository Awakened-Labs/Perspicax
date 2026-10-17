//! The sound's glue: its thread, running while a panel holds the mixer;
//! what it reads, shown on the panels; and a press or a turn of the wheel
//! on one of the mixer's pieces, asked of the server.
//!
//! The wheel moves the default output's volume a step a notch, from where
//! it was last asked to be until the server says it is there, so that a
//! quick turn of several notches goes as far as it was turned rather than
//! one step from where the server last was. A server that never says so,
//! because something else set the volume meanwhile, is believed again
//! after [`ASKED_STANDS`].

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use perspicax_config::{
    Item, Shell,
    mixer::{Mixer, Piece},
};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use wayland_client::protocol::wl_surface;

use super::App;
use crate::{
    audio::{Ask, News, Running, Target},
    layout::panel::{Part, Playing},
    model::{
        Button,
        sound::{self, Facts},
    },
};

/// The longest a volume asked for stands for the server's, waiting for the
/// server to say it took.
const ASKED_STANDS: Duration = Duration::from_secs(1);

/// A change to the volume, from an assistive technology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// This many steps, up positive.
    By(i32),
    /// To this, in percent.
    To(u32),
}

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
    /// The mixer, as `[shell.mixer]` writes it.
    mixer: Mixer,
    /// What the wheel turned short of a notch.
    wheel: f64,
    /// The volume last asked of a sink, by its name, and when: what the
    /// next step is from, until the server says the sink is there.
    asked: Option<(String, u32, Instant)>,
}

impl Audio {
    pub(super) fn new(server: Option<PathBuf>, news: Sender<(u64, News)>) -> Self {
        Self {
            server,
            news,
            running: None,
            started: 0,
            facts: None,
            mixer: Mixer::default(),
            wheel: 0.0,
            asked: None,
        }
    }

    /// Run the thread if `shell`'s panel holds the mixer, and stop it if
    /// not. Whether it stopped.
    fn follow(&mut self, shell: &Shell) -> bool {
        self.mixer = shell.mixer.clone();
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

    /// What the server holds now, or that there is no server. A volume
    /// asked for that the server now says stands for it no longer.
    fn hear(&mut self, news: News) {
        self.facts = match news {
            News::Facts(facts) => Some(facts),
            News::Gone => None,
            News::Failed(why) => {
                tracing::warn!("the sound server did not do what was asked: {why}");
                return;
            }
        };
        let facts = self.facts.as_deref();
        self.asked.take_if(|(sink, asked, _)| {
            facts.is_none_or(|facts| {
                facts
                    .sinks
                    .iter()
                    .any(|had| had.name == *sink && had.level().percent == *asked)
            })
        });
    }

    /// The wheel turned `notches`, down positive, in fractions of one: the
    /// whole notches it comes to with what was left over before.
    fn turned(&mut self, notches: f64) -> i32 {
        self.wheel += notches;
        let whole = self.wheel.trunc();
        self.wheel -= whole;
        whole as i32
    }

    /// Set the default output's volume to what `change` makes of it.
    fn change(&mut self, change: Change) {
        let Some(output) = self.facts.as_ref().and_then(|facts| facts.output()) else {
            return;
        };
        let now = match &self.asked {
            Some((sink, asked, at)) if *sink == output.name && at.elapsed() < ASKED_STANDS => {
                *asked
            }
            _ => output.level().percent,
        };
        let percent = match change {
            Change::By(steps) => sound::stepped(now, steps, self.mixer.step, self.mixer.limit),
            Change::To(percent) => percent,
        };
        if percent == now {
            return;
        }
        let (name, volume) = (output.name.clone(), sound::scaled(&output.volume, percent));
        self.asked = Some((name.clone(), percent, Instant::now()));
        self.ask(Ask::Volume {
            target: Target::Sink(name),
            volume,
        });
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
        self.audio.hear(news);
        self.heard();
    }

    /// Show the panels what was last heard of the sound.
    fn heard(&mut self) {
        let facts = self.audio.facts.as_deref();
        let level = facts
            .and_then(|facts| facts.output())
            .map(|output| output.level());
        let playing = facts.and_then(|facts| {
            let key = facts.output_key()?;
            let output = facts
                .outputs()
                .into_iter()
                .find(|output| output.key == key)?;
            Some(Playing {
                kind: output.kind,
                name: output.short,
                long: output.long,
            })
        });
        if self.panels.hear(level, playing) {
            self.panels_changed();
        }
    }

    /// `button` went down on the mixer's `piece`.
    pub(super) fn press_mixer(&mut self, piece: Piece, button: Button) {
        let Some(output) = self.audio.facts.as_ref().and_then(|facts| facts.output()) else {
            return;
        };
        let ask = match (piece, button) {
            (Piece::Mute, Button::Left) | (_, Button::Middle) => Ask::Mute {
                target: Target::Sink(output.name.clone()),
                on: !output.muted,
            },
            (Piece::Output, Button::Left) => {
                let Some(facts) = self.audio.facts.as_deref() else {
                    return;
                };
                let outputs = facts.outputs();
                let now = facts.output_key();
                let Some(plan) = sound::next_output(&outputs, now.as_deref())
                    .and_then(|next| facts.plan(&next.key))
                else {
                    return;
                };
                Ask::Choose(plan)
            }
            _ => return,
        };
        self.audio.ask(ask);
    }

    /// The wheel turned `notches`, down positive, at `at` on `surface`: on
    /// the mixer, it moves the default output's volume, up for up.
    pub(super) fn panel_turned(
        &mut self,
        surface: &wl_surface::WlSurface,
        at: (f64, f64),
        notches: f64,
    ) {
        let Some((_, Some(Part::Mixer(_)))) = self.panels.at(surface, at) else {
            return;
        };
        match self.audio.turned(notches) {
            0 => {}
            down => self.change_volume(Change::By(-down)),
        }
    }

    /// Change the default output's volume.
    pub(super) fn change_volume(&mut self, change: Change) {
        self.audio.change(change);
    }
}

#[cfg(test)]
mod tests {
    use smithay_client_toolkit::reexports::calloop::channel;

    use super::*;
    use crate::model::sound::{NORM, Sink};

    /// The sound with a default output at `percent`, its thread not
    /// running.
    fn at(percent: u32) -> Audio {
        let (news, _) = channel::channel();
        let mut audio = Audio::new(None, news);
        audio.hear(News::Facts(Box::new(facts(percent))));
        audio
    }

    fn facts(percent: u32) -> Facts {
        Facts {
            default_sink: Some("speakers".to_owned()),
            sinks: vec![Sink {
                index: 0,
                name: "speakers".to_owned(),
                description: "Speakers".to_owned(),
                volume: sound::scaled(&[NORM, NORM], percent),
                muted: false,
                card: None,
                ports: Vec::new(),
                port: None,
            }],
            cards: Vec::new(),
        }
    }

    fn asked(audio: &Audio) -> Option<u32> {
        audio.asked.as_ref().map(|&(_, percent, _)| percent)
    }

    #[test]
    fn two_notches_before_the_server_answers_go_two_steps() {
        let mut audio = at(24);
        audio.change(Change::By(1));
        assert_eq!(asked(&audio), Some(25));
        audio.change(Change::By(1));
        assert_eq!(asked(&audio), Some(30), "from what was asked, not from 24");
        audio.hear(News::Facts(Box::new(facts(25))));
        assert_eq!(asked(&audio), Some(30), "the first answer is not the last");
        audio.hear(News::Facts(Box::new(facts(30))));
        assert_eq!(asked(&audio), None, "the server says it now");
    }

    #[test]
    fn an_asked_volume_gives_way_to_the_server_after_a_while_or_with_it_gone() {
        let mut audio = at(24);
        audio.change(Change::By(1));
        // Set elsewhere meanwhile: the server never says 25.
        audio.hear(News::Facts(Box::new(facts(60))));
        if let Some((_, _, when)) = &mut audio.asked {
            *when = Instant::now() - ASKED_STANDS;
        }
        audio.change(Change::By(1));
        assert_eq!(
            asked(&audio),
            Some(65),
            "from the server's, once it stood long enough"
        );
        audio.hear(News::Gone);
        assert_eq!(asked(&audio), None);
    }

    #[test]
    fn a_setting_or_a_step_that_changes_nothing_asks_nothing() {
        let mut audio = at(100);
        audio.change(Change::By(1));
        assert_eq!(asked(&audio), None, "at the limit");
        audio.change(Change::To(100));
        assert_eq!(asked(&audio), None);
        audio.change(Change::To(40));
        assert_eq!(asked(&audio), Some(40));
    }
}
