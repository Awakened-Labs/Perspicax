//! The sound's glue: its thread, running while a panel holds the mixer;
//! what it reads, shown on the panels; and a press or a turn of the wheel
//! on one of the mixer's pieces, asked of the server.
//!
//! The wheel moves the default output's volume a step a notch, or a
//! program's in the mixer, from where it was last asked to be until the
//! server says it is there, so that a
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
        sound::{self, Change, Facts, Level, Side, Whose},
    },
};

/// The longest a volume asked for stands for the server's, waiting for the
/// server to say it took.
const ASKED_STANDS: Duration = Duration::from_secs(1);

/// The sound, as the loop knows it.
pub(super) struct Audio {
    /// The server's socket: `None` for the one the environment names.
    server: Option<PathBuf>,
    /// Where its news reaches the loop, marked with which start of it the
    /// news is from.
    news: Sender<(u64, News)>,
    /// Its thread, while a panel holds the mixer.
    running: Option<Running>,
    /// A panel shows the moving bars, so the thread reads what plays.
    levels: bool,
    /// How many times it has started: news from an earlier start is from a
    /// thread since stopped.
    started: u64,
    /// What the server holds, as last told: `None` with no server.
    facts: Option<Box<Facts>>,
    /// The mixer, as `[shell.mixer]` writes it.
    mixer: Mixer,
    /// What the wheel turned short of a notch.
    wheel: f64,
    /// The volume last asked of a device or a program, and when: what the
    /// next step is from, until the server says it is there.
    asked: Option<(Target, u32, Instant)>,
}

impl Audio {
    pub(super) fn new(server: Option<PathBuf>, news: Sender<(u64, News)>) -> Self {
        Self {
            server,
            news,
            running: None,
            levels: false,
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
        let levels = wanted && self.mixer.panel.contains(&Piece::Levels);
        if let Some(running) = self.running.as_ref().filter(|_| levels != self.levels) {
            running.ask(Ask::Levels(levels));
        }
        self.levels = levels;
        match (wanted, &self.running) {
            (true, None) => {
                self.started += 1;
                let (news, started) = (self.news.clone(), self.started);
                let running = crate::audio::start(self.server.clone(), move |told| {
                    news.send((started, told)).is_ok()
                });
                running.ask(Ask::Levels(levels));
                self.running = Some(running);
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

    /// Mute `whose`, or bring its sound back.
    pub(super) fn mute(&self, whose: Whose) {
        if let Some((target, _, level)) = self.facts().and_then(|facts| target(facts, whose)) {
            self.ask(Ask::Mute {
                target,
                on: !level.muted,
            });
        }
    }

    /// Move the program playing `stream` to the sink numbered `sink`.
    pub(super) fn move_stream(&self, stream: u32, sink: u32) {
        self.ask(Ask::Move { stream, sink });
    }

    /// Move `side` to the device keyed `key`, as its plan says.
    pub(super) fn choose(&self, side: Side, key: &str) {
        if let Some(plan) = self.facts().and_then(|facts| facts.plan(side, key)) {
            self.ask(Ask::Choose(plan));
        }
    }

    /// What the server holds, as last told: `None` with no server.
    pub(super) fn facts(&self) -> Option<&Facts> {
        self.facts.as_deref()
    }

    /// The mixer, as `[shell.mixer]` writes it.
    pub(super) fn mixer(&self) -> &Mixer {
        &self.mixer
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
            // Not facts: the bars move with them, on the panels.
            News::Levels(_) => return,
        };
        let facts = self.facts.as_deref();
        self.asked.take_if(|(target, asked, _)| {
            facts.is_none_or(|facts| {
                loudness(facts, target).is_none_or(|level| level.percent == *asked)
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

    /// Set the volume of `whose` to what `change` makes of it.
    pub(super) fn change(&mut self, change: Change, whose: Whose) {
        let Some((target, volume, level)) =
            self.facts.as_deref().and_then(|facts| target(facts, whose))
        else {
            return;
        };
        let now = match &self.asked {
            Some((of, asked, at)) if *of == target && at.elapsed() < ASKED_STANDS => *asked,
            _ => level.percent,
        };
        let percent = match change {
            Change::By(steps) => sound::stepped(now, steps, self.mixer.step, self.mixer.limit),
            Change::To(percent) => percent,
        };
        if percent == now {
            return;
        }
        let volume = sound::scaled(volume, percent);
        self.asked = Some((target.clone(), percent, Instant::now()));
        self.ask(Ask::Volume { target, volume });
    }
}

/// What `whose` is among `facts`, to ask of: what to ask, each channel's
/// volume, and how loud it is. The shell's own streams are no one's.
fn target(facts: &Facts, whose: Whose) -> Option<(Target, &[u32], Level)> {
    match whose {
        Whose::Side(Side::Output) => facts.output().map(|sink| {
            let target = Target::Sink(sink.name.clone());
            (target, sink.volume.as_slice(), sink.level())
        }),
        Whose::Side(Side::Input) => facts.input().map(|source| {
            let target = Target::Source(source.name.clone());
            (target, source.volume.as_slice(), source.level())
        }),
        Whose::App(index) => facts
            .apps()
            .find(|stream| stream.index == index)
            .map(|stream| {
                let target = Target::Stream(index);
                (target, stream.volume.as_slice(), stream.level())
            }),
    }
}

/// How loud `target` is among `facts`, if it is there.
fn loudness(facts: &Facts, target: &Target) -> Option<Level> {
    match target {
        Target::Sink(name) => facts
            .sinks
            .iter()
            .find(|sink| sink.name == *name)
            .map(|sink| sink.level()),
        Target::Source(name) => facts
            .sources
            .iter()
            .find(|source| source.name == *name)
            .map(|source| source.level()),
        Target::Stream(index) => facts
            .streams
            .iter()
            .find(|stream| stream.index == *index)
            .map(|stream| stream.level()),
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
        if let News::Levels(bands) = news {
            self.panels.hear_levels(&mut self.canvas, &self.kit, bands);
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
                long: output.long(),
                name: output.short,
            })
        });
        let mic = facts
            .and_then(|facts| facts.input())
            .map(|input| input.level());
        if self.panels.hear(level, playing, mic) {
            self.panels_changed();
        }
        self.mixer_heard();
    }

    /// `button` went down on the mixer's `piece`, on the panel of the
    /// monitor named `name`; `was_open` says whether the mixer was open,
    /// which the press closed.
    pub(super) fn press_mixer(&mut self, name: &str, piece: Piece, button: Button, was_open: bool) {
        let opens = matches!(
            (piece, button),
            (Piece::Open | Piece::Volume | Piece::Levels, Button::Left) | (_, Button::Right)
        );
        if opens {
            // A press on what opened it closes it, as a menu's button does.
            if !was_open {
                self.open_mixer(name);
            }
            return;
        }
        match (piece, button) {
            (Piece::Mic, Button::Left | Button::Middle) => {
                self.audio.mute(Whose::Side(Side::Input));
            }
            (Piece::Mute, Button::Left) | (_, Button::Middle) => {
                self.audio.mute(Whose::Side(Side::Output));
            }
            (Piece::Output, Button::Left) => {
                let Some(facts) = self.audio.facts() else {
                    return;
                };
                let outputs = facts.outputs();
                let now = facts.output_key();
                if let Some(next) = sound::next_choice(&outputs, now.as_deref()) {
                    self.audio.choose(Side::Output, &next.key);
                }
            }
            _ => {}
        }
    }

    /// The wheel turned `notches`, down positive, at `at` on `surface`: on
    /// the mixer, it moves the default output's volume, up for up, or on its
    /// microphone the default input's.
    pub(super) fn panel_turned(
        &mut self,
        surface: &wl_surface::WlSurface,
        at: (f64, f64),
        notches: f64,
    ) {
        let Some((_, Some(Part::Mixer(piece)))) = self.panels.at(surface, at) else {
            return;
        };
        let side = if piece == Piece::Mic {
            Side::Input
        } else {
            Side::Output
        };
        match self.audio.turned(notches) {
            0 => {}
            down => self.audio.change(Change::By(-down), Whose::Side(side)),
        }
    }

    /// Change the default output's volume.
    pub(super) fn change_volume(&mut self, change: Change) {
        self.audio.change(change, Whose::Side(Side::Output));
    }
}

#[cfg(test)]
mod tests {
    use smithay_client_toolkit::reexports::calloop::channel;

    use super::*;
    use crate::model::sound::{NORM, Sink, Stream};

    const OUT: Whose = Whose::Side(Side::Output);

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
                running: false,
                monitor: None,
            }],
            cards: Vec::new(),
            default_source: None,
            sources: Vec::new(),
            ..Facts::default()
        }
    }

    fn asked(audio: &Audio) -> Option<u32> {
        audio.asked.as_ref().map(|&(_, percent, _)| percent)
    }

    #[test]
    fn two_notches_before_the_server_answers_go_two_steps() {
        let mut audio = at(24);
        audio.change(Change::By(1), OUT);
        assert_eq!(asked(&audio), Some(25));
        audio.change(Change::By(1), OUT);
        assert_eq!(asked(&audio), Some(30), "from what was asked, not from 24");
        audio.hear(News::Facts(Box::new(facts(25))));
        assert_eq!(asked(&audio), Some(30), "the first answer is not the last");
        audio.hear(News::Facts(Box::new(facts(30))));
        assert_eq!(asked(&audio), None, "the server says it now");
    }

    #[test]
    fn an_asked_volume_gives_way_to_the_server_after_a_while_or_with_it_gone() {
        let mut audio = at(24);
        audio.change(Change::By(1), OUT);
        // Set elsewhere meanwhile: the server never says 25.
        audio.hear(News::Facts(Box::new(facts(60))));
        if let Some((_, _, when)) = &mut audio.asked {
            *when = Instant::now() - ASKED_STANDS;
        }
        audio.change(Change::By(1), OUT);
        assert_eq!(
            asked(&audio),
            Some(65),
            "from the server's, once it stood long enough"
        );
        audio.hear(News::Gone);
        assert_eq!(asked(&audio), None);
    }

    #[test]
    fn a_programs_volume_and_mute_are_asked_of_its_stream_but_never_the_shells_own() {
        let stream = |index, client| Stream {
            index,
            client: Some(client),
            sink: 0,
            application: None,
            binary: None,
            media: "AudioStream".to_owned(),
            icon: None,
            id: None,
            volume: sound::scaled(&[NORM, NORM], 30),
            muted: false,
        };
        let (news, _) = channel::channel();
        let mut audio = Audio::new(None, news);
        audio.hear(News::Facts(Box::new(Facts {
            streams: vec![stream(40, 12), stream(41, 7)],
            client: Some(7),
            ..facts(80)
        })));
        audio.change(Change::By(1), Whose::App(40));
        assert_eq!(
            audio
                .asked
                .as_ref()
                .map(|(target, percent, _)| (target, *percent)),
            Some((&Target::Stream(40), 35))
        );
        audio.asked = None;
        audio.change(Change::By(1), Whose::App(41));
        assert_eq!(asked(&audio), None, "the shell's own");
        let facts = audio.facts().expect("heard");
        assert_eq!(
            target(facts, Whose::App(40)).map(|(target, _, level)| (target, level.muted)),
            Some((Target::Stream(40), false))
        );
        assert_eq!(
            loudness(facts, &Target::Stream(40)).map(|level| level.percent),
            Some(30)
        );
        assert_eq!(loudness(facts, &Target::Stream(9)), None);
    }

    #[test]
    fn a_setting_or_a_step_that_changes_nothing_asks_nothing() {
        let mut audio = at(100);
        audio.change(Change::By(1), OUT);
        assert_eq!(asked(&audio), None, "at the limit");
        audio.change(Change::To(100), OUT);
        assert_eq!(asked(&audio), None);
        audio.change(Change::To(40), OUT);
        assert_eq!(asked(&audio), Some(40));
    }
}
