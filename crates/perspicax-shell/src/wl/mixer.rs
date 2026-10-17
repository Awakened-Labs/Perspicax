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
    layout::mixer::{Anchor, Listed, Master, Mixed},
    mixer::{Act, Effect, Event, Key, State},
    model::sound::Side,
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
        self.draw(canvas, kit);
    }

    /// Draw the mixer on the surface, once the compositor has given it a
    /// size.
    fn draw(&mut self, canvas: &mut Canvas, kit: &mut Kit) {
        let (Some(shown), Some(view)) = (self.shown.as_mut(), self.state.view()) else {
            return;
        };
        let Some(size) = shown.size else {
            return;
        };
        let Kit { fonts, palette, .. } = kit;
        let text = fonts.get();
        let scale = shown.scale;
        let started = std::time::Instant::now();
        canvas.take_clicks(&shown.layer, size, self.cut);
        let rect = view.placed.rect;
        canvas.show(
            &shown.layer,
            size,
            scale,
            &[(rect.x, rect.y, rect.w, rect.h)],
            |picture| paint::mixer::paint(&view, picture, scale, text, palette),
        );
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
        self.draw(canvas, kit);
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
        for effect in effects {
            match effect {
                Effect::Mute(side) => self.mute_default(side),
                Effect::Volume(side, change) => self.audio.change(change, side),
                Effect::Choose(side, key) => self.audio.choose(side, &key),
                Effect::Redraw => {}
            }
        }
    }

    /// What the mixer shows of the sound now.
    pub(super) fn mixed(&self) -> Mixed {
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
                    .map_or_else(|| description.clone(), |choice| choice.short.clone()),
                level,
            });
            let listed = choices
                .into_iter()
                .map(|choice| Listed {
                    chosen: Some(&choice.key) == chosen.as_ref(),
                    key: choice.key,
                    device: choice.device,
                    jack: choice.jack,
                    kind: choice.kind,
                    connected: choice.connected,
                })
                .collect();
            (master, listed)
        };
        let (output, outputs) = side(Side::Output);
        let (input, inputs) = side(Side::Input);
        let mixer = self.audio.mixer();
        Mixed {
            output,
            input,
            outputs,
            inputs,
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
}
