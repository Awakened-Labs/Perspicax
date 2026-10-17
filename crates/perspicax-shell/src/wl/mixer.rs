//! The mixer's surface: while it is open, one surface on the `overlay`
//! layer of the monitor it opened on, as the menus' is, clear but for the
//! mixer's box, which is opaque. It takes clicks everywhere but on the
//! shell's own panel, which keeps its own: a press off the box closes the
//! mixer, and a press on the panel closes it too, or, on the mixer's own
//! item, only that. It takes the keyboard while it is up, so opening it
//! closes the menus and a pie, and opening either closes it. Its namespace
//! is `perspicax-mixer-<connector>`.
//!
//! What it shows is the sound as the panel's mixer item last heard it,
//! sent again each time that changes, and what is done in it is asked of
//! the sound server as the item's own presses are.
//!
//! While it is open it meters each output it lists, and the input whose
//! details are shown, and nothing once it closes. Its meters move as the
//! peaks come, drawn again whole, but no sooner than the compositor shows
//! the last, and only once one has moved as far as can be seen: a quiet
//! mixer is drawn no more than a still one.

use std::collections::HashMap;

use accesskit::{Action, ActionData, ActionHandler, ActionRequest};
use perspicax_config::Edge;
use smithay_client_toolkit::{
    output::OutputState,
    reexports::calloop::channel::Sender,
    seat::pointer::{PointerEvent, PointerEventKind},
    shell::{
        WaylandSurface,
        wlr_layer::{LayerSurface, LayerSurfaceConfigure},
    },
};
use wayland_client::{QueueHandle, protocol::wl_surface};

use super::{
    App, Asked,
    canvas::{Area, Canvas, whole},
    watch,
};
use crate::{
    a11y::{self, adapter::Served},
    layout::mixer::{Anchor, Listed, Master, Mixed, Owner, Program},
    mixer::{Act, Effect, Event, Key, State},
    model::{
        kept::Kept,
        sound::{Choice, Facts, Side, loudness},
    },
    paint::{self, Kit},
};

/// The mixer, and its surface while it is open.
pub(super) struct Mixers {
    state: State,
    shown: Option<Shown>,
    /// Where the panel is on the monitor the mixer opens on, which takes its
    /// own clicks.
    cut: Option<Area>,
    /// What the wheel turned short of a notch.
    wheel: f64,
    /// Where an assistive technology's requests are sent, to reach the loop.
    actions: Sender<Asked>,
    /// The peak of each source metered, by name; how far along each meter
    /// was last drawn, in fiftieths; whether the compositor has yet to show
    /// the last drawn, and whether a meter moved since.
    peaks: HashMap<String, f32>,
    drawn: Vec<(String, u8)>,
    waiting: bool,
    stale: bool,
}

/// The mixer's surface.
struct Shown {
    /// The monitor's connector name.
    name: String,
    namespace: String,
    layer: LayerSurface,
    a11y: Served,
    scale: u32,
    /// In the surface's own units, once the compositor has said.
    size: Option<(u32, u32)>,
}

impl Mixers {
    pub(super) fn new(actions: Sender<Asked>) -> Self {
        Self {
            state: State::default(),
            shown: None,
            cut: None,
            wheel: 0.0,
            actions,
            peaks: HashMap::new(),
            drawn: Vec::new(),
            waiting: false,
            stale: false,
        }
    }

    /// The meters' newest `peaks`: drawn, once the compositor has shown the
    /// last, if one moved as far as can be seen.
    fn peaks(&mut self, canvas: &mut Canvas, kit: &mut Kit, peaks: Vec<(String, f32)>) {
        self.peaks.extend(peaks);
        if reaches(&self.peaks) == self.drawn {
            return;
        }
        if self.waiting {
            self.stale = true;
            return;
        }
        self.draw(canvas, kit, true);
    }

    /// The compositor showed what was drawn on `surface`: draw the meters
    /// again if they moved meanwhile.
    pub(super) fn frame(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        surface: &wl_surface::WlSurface,
    ) {
        if !self.owns(surface) {
            return;
        }
        self.waiting = false;
        if std::mem::take(&mut self.stale) {
            self.draw(canvas, kit, true);
        }
    }

    /// The monitor the mixer is open on, if it is.
    pub(super) fn open_on(&self) -> Option<&str> {
        self.state.open_on()
    }

    /// Whether `surface` is the mixer's.
    pub(super) fn owns(&self, surface: &wl_surface::WlSurface) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|shown| shown.layer.wl_surface() == surface)
    }

    /// The wheel turned `notches`, up negative, in fractions of one: the
    /// whole notches it comes to with what was left over before.
    fn turned(&mut self, notches: f64) -> i32 {
        self.wheel += notches;
        let whole = self.wheel.trunc();
        self.wheel -= whole;
        whole as i32
    }

    /// Handle `event`, and show what it changed. What is left to do is
    /// returned.
    fn send(
        &mut self,
        canvas: &mut Canvas,
        qh: &QueueHandle<App>,
        outputs: &OutputState,
        kit: &mut Kit,
        event: Event,
    ) -> Vec<Effect> {
        if matches!(event, Event::Open { .. }) {
            self.wheel = 0.0;
        }
        let effects = self.state.update(event);
        if effects.contains(&Effect::Redraw) {
            self.sync(canvas, qh, outputs, kit);
        }
        effects
            .into_iter()
            .filter(|effect| *effect != Effect::Redraw)
            .collect()
    }

    /// Make the surface match what is open: take it away, put it up on the
    /// monitor the mixer opened on, or draw it again.
    fn sync(
        &mut self,
        canvas: &mut Canvas,
        qh: &QueueHandle<App>,
        outputs: &OutputState,
        kit: &mut Kit,
    ) {
        let Some(view) = self.state.view() else {
            self.shown = None;
            self.peaks.clear();
            self.drawn.clear();
            (self.waiting, self.stale) = (false, false);
            return;
        };
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.name != view.output)
        {
            self.shown = None;
        }
        if self.shown.is_none() {
            let found = outputs.outputs().find_map(|output| {
                let info = outputs.info(&output)?;
                (info.name.as_deref() == Some(view.output)).then_some((output, info.scale_factor))
            });
            let Some((output, scale)) = found else {
                tracing::warn!(
                    output = view.output,
                    "the mixer was opened on a monitor that is gone"
                );
                return;
            };
            let namespace = format!("perspicax-mixer-{}", view.output);
            let a11y = Served::acting(
                a11y::mixer(&namespace, None, None),
                Forward(self.actions.clone()),
            );
            self.shown = Some(Shown {
                name: view.output.to_owned(),
                layer: canvas.overlay(qh, &namespace, &output),
                namespace,
                a11y,
                scale: whole(scale),
                size: None,
            });
        }
        self.draw(canvas, kit, false);
    }

    /// Draw the mixer on the surface, once the compositor has given it a
    /// size; `paced`, asking the compositor to say when it shows it.
    fn draw(&mut self, canvas: &mut Canvas, kit: &mut Kit, paced: bool) {
        let (Some(shown), Some(view)) = (self.shown.as_mut(), self.state.view()) else {
            return;
        };
        let Some(size) = shown.size else {
            return;
        };
        let Kit {
            fonts,
            palette,
            images,
        } = kit;
        let text = fonts.get();
        let scale = shown.scale;
        let started = std::time::Instant::now();
        canvas.take_clicks(&shown.layer, size, self.cut);
        let rect = view.placed.rect;
        let peaks = &self.peaks;
        if paced {
            canvas.pace(&shown.layer);
            self.waiting = true;
        }
        canvas.show(
            &shown.layer,
            size,
            scale,
            &[(rect.x, rect.y, rect.w, rect.h)],
            |picture| paint::mixer::paint(&view, picture, (scale, palette), text, images, peaks),
        );
        self.drawn = reaches(peaks);
        shown
            .a11y
            .show(a11y::mixer(&shown.namespace, Some(size), Some(&view)));
        watch::slow("drawing the mixer", started.elapsed());
    }

    /// The compositor sized the surface: draw the mixer on it.
    pub(super) fn configure(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        layer: &LayerSurface,
        configure: &LayerSurfaceConfigure,
    ) {
        let Some(shown) = self.shown.as_mut().filter(|shown| shown.layer == *layer) else {
            return;
        };
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        shown.size = Some((width, height));
        self.draw(canvas, kit, false);
    }

    /// The compositor took the surface away: the monitor went.
    pub(super) fn closed(&mut self, layer: &LayerSurface) {
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.layer == *layer)
        {
            self.shown = None;
            self.state.update(Event::KeyboardLost);
        }
    }

    /// The keyboard came to `surface`, or left it: whether it is the
    /// mixer's.
    pub(super) fn keyboard(&mut self, surface: &wl_surface::WlSurface, entered: bool) -> bool {
        let Some(shown) = self
            .shown
            .as_mut()
            .filter(|shown| shown.layer.wl_surface() == surface)
        else {
            return false;
        };
        shown.a11y.focused(entered);
        true
    }
}

/// How far along each meter `peaks` reach, in fiftieths: as far as can be
/// seen to move.
fn reaches(peaks: &HashMap<String, f32>) -> Vec<(String, u8)> {
    let mut reaches: Vec<(String, u8)> = peaks
        .iter()
        .map(|(source, &peak)| (source.clone(), (loudness(peak) * 50.0).round() as u8))
        .collect();
    reaches.sort();
    reaches
}

/// `choice` on `side` as the mixer lists it, `chosen` or not, named and
/// hidden as `kept` says, live and metered as `facts` say.
fn listed(side: Side, choice: Choice, chosen: bool, (kept, facts): (&Kept, &Facts)) -> Listed {
    let card = choice.card.map(|key| Owner {
        hidden: kept.hidden.contains(&key),
        name: choice.device.clone(),
        key,
    });
    let live = match side {
        Side::Output => facts.live_sink(&choice.key).map(|sink| sink.index),
        Side::Input => None,
    };
    let meter = facts.meter(side, &choice.key);
    Listed {
        name: kept.name(&choice.key).map(str::to_owned),
        hidden: kept.hidden.contains(&choice.key),
        key: choice.key,
        device: choice.device,
        jack: choice.jack,
        short: choice.short,
        kind: choice.kind,
        connected: choice.connected,
        chosen,
        card,
        details: choice.details,
        live,
        meter,
    }
}

/// An assistive technology's requests of the mixer, sent on to the shell's
/// loop.
struct Forward(Sender<Asked>);

impl ActionHandler for Forward {
    fn do_action(&mut self, request: ActionRequest) {
        let Some(control) = a11y::control_of(request.target_node) else {
            return;
        };
        let act = match (request.action, request.data) {
            (Action::Click, _) => Act::Click,
            (Action::Increment, _) => Act::Increment,
            (Action::Decrement, _) => Act::Decrement,
            (Action::Focus, _) => Act::Focus,
            (Action::SetValue, Some(ActionData::NumericValue(value))) if value.is_finite() => {
                Act::Set(value.round().clamp(0.0, f64::from(u32::MAX)) as u32)
            }
            (Action::SetValue, Some(ActionData::Value(name))) => Act::Name(name.into()),
            _ => return,
        };
        if self.0.send(Asked::Mixer(Event::Act(control, act))).is_err() {
            tracing::debug!("the shell is stopping; the request is dropped");
        }
    }
}

impl App {
    /// Open the mixer from the mixer item on the panel of the monitor
    /// named `name`.
    pub(super) fn open_mixer(&mut self, name: &str) {
        let Some(output) = self.output_named(name) else {
            return;
        };
        let monitor = self.monitor(&output);
        let Some(button) = self.panels.mixer_at(name, monitor) else {
            return;
        };
        let area = self.area(&output);
        let edge = self.panels.edge().unwrap_or(Edge::Bottom);
        // The programs playing are shown with their applications' icons.
        self.installed.refresh();
        self.mixers.cut = self
            .panels
            .strip(name, monitor)
            .map(|strip| (strip.x, strip.y, strip.w, strip.h));
        let mixed = self.mixed();
        self.mixer_event(Event::Open {
            output: name.to_owned(),
            anchor: Anchor { button, edge, area },
            mixed,
        });
    }

    /// Close the mixer. Whether it was open.
    pub(super) fn close_mixer(&mut self) -> bool {
        let open = self.mixers.open_on().is_some();
        if open {
            self.mixer_event(Event::Close);
        }
        open
    }

    /// Pass `event` to the mixer, and carry out what it calls for.
    pub(super) fn mixer_event(&mut self, event: Event) {
        let qh = self.qh.clone();
        let effects = self
            .mixers
            .send(&mut self.canvas, &qh, &self.outputs, &mut self.kit, event);
        let open_on = self.mixers.open_on().map(str::to_owned);
        if self.panels.mixer_open(open_on) {
            self.panels_changed();
        }
        self.follow_meters();
        for effect in effects {
            match effect {
                Effect::Mute(whose) => self.audio.mute(whose),
                Effect::Volume(whose, change) => self.audio.change(change, whose),
                Effect::Move(stream, sink) => self.audio.move_stream(stream, sink),
                Effect::Test(sink) => self.audio.test(sink),
                Effect::Hide(key, hidden) => {
                    if self.audio.keep(|kept| kept.hide(&key, hidden)) {
                        self.heard();
                    }
                }
                Effect::Rename(key, name) => {
                    if self.audio.keep(|kept| kept.rename(&key, &name)) {
                        self.heard();
                    }
                }
                Effect::Choose(side, key) => self.audio.choose(side, &key),
                Effect::Redraw => {}
            }
        }
    }

    /// What the mixer shows of the sound now.
    pub(super) fn mixed(&self) -> Mixed {
        let kept = self.audio.kept();
        let facts = self.audio.facts();
        // Each side's default, named as the panel names it, and what the
        // side can be moved to.
        let side = |side| {
            let Some(facts) = facts else {
                return (None, Vec::new());
            };
            let chosen = facts.chosen(side);
            let choices = facts.choices(side);
            let master = match side {
                Side::Output => facts.output().map(|sink| (&sink.description, sink.level())),
                Side::Input => facts
                    .input()
                    .map(|source| (&source.description, source.level())),
            }
            .map(|(description, level)| Master {
                name: choices
                    .iter()
                    .find(|choice| Some(&choice.key) == chosen.as_ref())
                    .map_or_else(|| description.clone(), |choice| kept.called(choice)),
                level,
            });
            let listed = choices
                .into_iter()
                .map(|choice| {
                    let chosen = Some(&choice.key) == chosen.as_ref();
                    listed(side, choice, chosen, (kept, facts))
                })
                .collect();
            (master, listed)
        };
        let (output, outputs) = side(Side::Output);
        let (input, inputs) = side(Side::Input);
        let programs = facts.map_or_else(Vec::new, |facts| {
            facts
                .apps()
                .map(|stream| Program {
                    stream: stream.index,
                    name: stream.name().to_owned(),
                    icon: stream.icon_from(self.installed.apps()),
                    level: stream.level(),
                    sink: stream.sink,
                })
                .collect()
        });
        let sinks = facts.map_or_else(Vec::new, |facts| {
            facts
                .sink_choices()
                .into_iter()
                .map(|(sink, choice)| (sink, listed(Side::Output, choice, false, (kept, facts))))
                .collect()
        });
        let mixer = self.audio.mixer();
        Mixed {
            output,
            input,
            outputs,
            inputs,
            programs,
            sinks,
            step: mixer.step,
            limit: mixer.limit,
        }
    }

    /// The pointer, a button or the wheel, on the mixer's surface.
    pub(super) fn mixer_pointer(&mut self, event: &PointerEvent) {
        let at = event.position;
        let mixer = match event.kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => Event::Motion(at),
            PointerEventKind::Press { button, .. } => Event::Press(at, super::seat::button(button)),
            PointerEventKind::Release { button, .. } => {
                Event::Release(at, super::seat::button(button))
            }
            PointerEventKind::Axis { vertical, .. } => {
                match self.mixers.turned(super::seat::notches(&vertical)) {
                    0 => return,
                    whole => Event::Wheel(whole),
                }
            }
            PointerEventKind::Leave { .. } => return,
        };
        self.mixer_event(mixer);
    }

    /// A key on the mixer.
    pub(super) fn mixer_key(&mut self, key: Key) {
        self.mixer_event(Event::Key(key));
    }

    /// The sound changed: show it in the mixer, if it is open.
    pub(super) fn mixer_heard(&mut self) {
        if self.mixers.open_on().is_some() {
            let mixed = self.mixed();
            self.mixer_event(Event::Heard(mixed));
        }
    }

    /// How loud the sources metered were last: shown on the mixer's
    /// meters, if it is open.
    pub(super) fn mixer_peaks(&mut self, peaks: Vec<(String, f32)>) {
        if self.mixers.open_on().is_some() {
            self.mixers.peaks(&mut self.canvas, &mut self.kit, peaks);
        }
    }

    /// Meter what the mixer shows, while it is open: each output it lists,
    /// and the input whose details are shown; and nothing once it closes.
    fn follow_meters(&mut self) {
        let sources = self
            .mixers
            .state
            .view()
            .map(|view| view.metered())
            .unwrap_or_default();
        self.mixers
            .peaks
            .retain(|source, _| sources.contains(source));
        self.audio.meter(sources);
    }
}
