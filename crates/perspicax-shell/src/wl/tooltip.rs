//! A panel's tooltip: while the pointer rests on a part of a bar that has
//! one, a surface on the `overlay` layer of the bar's monitor, standing just
//! off the bar, saying what the part is (see [`crate::tooltip`] for when).
//! It takes no clicks and never the keyboard, and has no accessibility
//! window: what it says is already the name of the part it is for, in the
//! panel's tree. To an agent it covers the strip it is drawn on. Its
//! namespace is `perspicax-tooltip-<connector>`.
//!
//! Its exclusive zone is -1, so it is placed against the monitor's own edges
//! whatever keeps windows out of them, and stands exactly where it is put.
//! Moving from one part to another on a bar keeps the surface and moves it,
//! its new place and picture in one commit; it is put up again only on
//! another bar, after it has gone.

use perspicax_config::Edge;
use smithay_client_toolkit::{
    reexports::calloop::timer::{TimeoutAction, Timer},
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceConfigure},
    },
};
use wayland_client::{
    QueueHandle,
    protocol::{wl_output, wl_surface},
};

use super::{App, canvas::Canvas, watch};
use crate::{
    layout::{
        Rect,
        tooltip::{TALL, margins},
    },
    paint::{self, Kit},
    tooltip::{DELAY, Effect, Event, State},
};

/// What every tip's namespace starts with, before its monitor's name.
const NAMESPACE: &str = "perspicax-tooltip-";

/// The panels' tooltip: where the pointer is, as far as tips go, and the tip
/// shown, if one is.
pub(super) struct Tips {
    state: State,
    shown: Option<Shown>,
    qh: QueueHandle<App>,
}

/// The tip's surface.
struct Shown {
    /// The connector name of the monitor whose bar it stands off.
    bar: String,
    layer: LayerSurface,
    tip: Tip,
    /// The compositor has sized it, so it may be drawn.
    configured: bool,
}

/// A tip as it is to be shown.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Tip {
    pub(super) words: String,
    /// Where it goes, in its panel's own coordinates.
    pub(super) place: Rect,
    /// The edge its panel is along, and how tall the panel is.
    pub(super) edge: Edge,
    pub(super) height: i32,
    pub(super) scale: u32,
}

impl Tips {
    pub(super) fn new(qh: QueueHandle<App>) -> Self {
        Self {
            state: State::default(),
            shown: None,
            qh,
        }
    }

    /// Pass `event` on, and say what it calls for.
    pub(super) fn send(&mut self, event: Event) -> Vec<Effect> {
        self.state.update(event)
    }

    /// The pointer is off every bar, or tooltips are off: take the tip away.
    /// Any wait still running ends in nothing.
    pub(super) fn forget(&mut self) {
        self.state.update(Event::Off);
        self.shown = None;
    }

    /// The monitor whose bar the tip stands off, if one shows.
    pub(super) fn on(&self) -> Option<&str> {
        self.shown.as_ref().map(|shown| shown.bar.as_str())
    }

    /// Show `tip` off the bar of the monitor named `bar`, which is
    /// `output`: in place of the one shown there, or on a surface of its
    /// own put up for it.
    pub(super) fn show(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        (bar, output): (&str, &wl_output::WlOutput),
        tip: Tip,
    ) {
        if self.shown.as_ref().is_some_and(|shown| shown.bar != bar) {
            self.shown = None;
        }
        match &mut self.shown {
            Some(shown) if shown.tip == tip => {}
            Some(shown) => {
                shown.tip = tip;
                put(&shown.layer, &shown.tip);
                if shown.configured {
                    draw(canvas, kit, shown);
                } else {
                    shown.layer.commit();
                }
            }
            None => {
                let namespace = format!("{NAMESPACE}{bar}");
                let layer = canvas.layer(&self.qh, Layer::Overlay, &namespace, Some(output));
                put(&layer, &tip);
                layer.set_exclusive_zone(-1);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                canvas.take_no_clicks(&layer);
                layer.commit();
                self.shown = Some(Shown {
                    bar: bar.to_owned(),
                    layer,
                    tip,
                    configured: false,
                });
            }
        }
    }

    /// Take the tip away.
    pub(super) fn hide(&mut self) {
        self.shown = None;
    }

    /// The compositor sized `layer`: if it is the tip's, draw it. Whether
    /// it was.
    pub(super) fn configure(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        layer: &LayerSurface,
        configure: &LayerSurfaceConfigure,
    ) -> bool {
        let Some(shown) = self.shown.as_mut().filter(|shown| shown.layer == *layer) else {
            return false;
        };
        let (width, height) = configure.new_size;
        if width != 0 && height != 0 {
            shown.configured = true;
            draw(canvas, kit, shown);
        }
        true
    }

    /// The compositor took `layer` away: if it is the tip's, it is gone.
    pub(super) fn closed(&mut self, layer: &LayerSurface) {
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.layer == *layer)
        {
            self.shown = None;
        }
    }
}

/// Ask for `layer` as big as `tip`, where it goes.
fn put(layer: &LayerSurface, tip: &Tip) {
    let (left, from_edge) = margins(tip.place, tip.height, tip.edge);
    let (anchor, top, bottom) = match tip.edge {
        Edge::Bottom => (Anchor::BOTTOM, 0, from_edge),
        Edge::Top => (Anchor::TOP, from_edge, 0),
    };
    layer.set_anchor(anchor | Anchor::LEFT);
    layer.set_margin(top, 0, bottom, left);
    layer.set_size(tip.place.w.max(1) as u32, TALL as u32);
}

/// Draw `shown`'s tip, as big as it asked to be.
fn draw(canvas: &mut Canvas, kit: &mut Kit, shown: &Shown) {
    let Kit { fonts, palette, .. } = kit;
    let text = fonts.get();
    let tip = &shown.tip;
    let size = (tip.place.w.max(1), TALL);
    canvas.show(
        &shown.layer,
        (size.0 as u32, size.1 as u32),
        tip.scale,
        &[(0, 0, size.0, size.1)],
        |picture| paint::tooltip::paint(&tip.words, size, picture, tip.scale, text, palette),
    );
}

impl App {
    /// The pointer came onto `surface` or moved on it, to `at`: if it is a
    /// panel's, tips follow it.
    pub(super) fn panel_pointed(&mut self, surface: &wl_surface::WlSurface, at: (f64, f64)) {
        let Some((bar, part)) = self.panels.tip_at(surface, at) else {
            return;
        };
        self.tip_event(Event::Covered(self.covered()));
        self.tip_event(Event::Motion { bar, part });
    }

    /// The pointer left `surface`: if it is a panel's, it is off the bar.
    pub(super) fn panel_left(&mut self, surface: &wl_surface::WlSurface) {
        if self.panels.owns(surface) {
            self.tip_event(Event::Leave);
        }
    }

    /// A menu, the mixer or a pie may have opened or closed: tips follow.
    /// Each takes the menus.
    #[cfg(feature = "menus")]
    pub(super) fn tips_covered(&mut self) {
        self.tip_event(Event::Covered(self.covered()));
    }

    /// Whether a menu, the mixer or a pie is open, over which no tip shows.
    fn covered(&self) -> bool {
        #[cfg(feature = "menus")]
        if self.menus.open_menu().is_some() {
            return true;
        }
        #[cfg(feature = "audio")]
        if self.mixers.open_on().is_some() {
            return true;
        }
        #[cfg(feature = "pie")]
        if self.pies.is_open() {
            return true;
        }
        false
    }

    /// Pass `event` to the tips, and start or stop the wait for the pointer
    /// to rest as it calls for; the panels show and hide the tip.
    pub(super) fn tip_event(&mut self, event: Event) {
        let effects = self.panels.tip(&mut self.canvas, &mut self.kit, event);
        for effect in effects {
            match effect {
                Effect::Wait => self.wait_for_rest(),
                Effect::Stop => {
                    if let Some(waiting) = self.tip_waiting.take() {
                        self.handle.remove(waiting);
                    }
                }
                Effect::Show { .. } | Effect::Hide => {}
            }
        }
    }

    /// Wait [`DELAY`] from now, in place of any wait running, then tell the
    /// tips it is over.
    fn wait_for_rest(&mut self) {
        if let Some(waiting) = self.tip_waiting.take() {
            self.handle.remove(waiting);
        }
        let waiting = self
            .handle
            .insert_source(Timer::from_duration(DELAY), |_, (), app| {
                app.tip_waiting = None;
                watch::timed("showing a tooltip", || app.tip_event(Event::Due));
                TimeoutAction::Drop
            });
        match waiting {
            Ok(waiting) => self.tip_waiting = Some(waiting),
            Err(error) => tracing::warn!("no tooltip will show: {error}"),
        }
    }
}
