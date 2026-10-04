//! The panels: one surface on the `top` layer of each monitor the config
//! names, along its bottom or its top, with an exclusive zone as tall as it
//! is, so that windows are kept out of the strip it takes. A fullscreen
//! window in use still covers it, as perspicax arranges.
//!
//! Its namespace is `perspicax-panel-<connector>`, and its accessibility
//! window carries the same name, which is how perspicax joins the two.
//!
//! A changed config redraws the panels where they are, moved to another
//! edge or made another height in place, so an agent holding a panel's id
//! still holds it; only the monitors that no longer have one lose theirs.
//! Which monitor is the first is worked out again whenever one comes or
//! goes.

use std::time::{Duration, Instant};

use perspicax_config::{Edge, Item, Panel, Shell};
#[cfg(feature = "menus")]
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use smithay_client_toolkit::{
    output::OutputState,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceConfigure},
    },
};
#[cfg(feature = "menus")]
use wayland_client::protocol::wl_surface;
use wayland_client::{QueueHandle, protocol::wl_output};

use super::{
    App,
    canvas::{Canvas, whole},
};
use crate::{
    a11y::{self, adapter::Served},
    layout::{
        Rect,
        panel::{chosen, lay_out},
    },
    model::clock::Clock,
    paint::{self, text::Fonts},
};

/// What every panel's namespace starts with, before its monitor's name.
const NAMESPACE: &str = "perspicax-panel-";

/// Every monitor's panel, and the clock they show.
pub(super) struct Panels {
    /// `None` is no panel, and then no surface either.
    panel: Option<Panel>,
    clock: Clock,
    /// The time, as the clock shows it now.
    time: String,
    each: Vec<Bar>,
    /// The monitor whose start menu is open, for its button to show it.
    open_on: Option<String>,
    /// Where an assistive technology's press of a start button is sent.
    #[cfg(feature = "menus")]
    actions: Sender<super::Asked>,
}

/// One monitor's panel.
struct Bar {
    output: wl_output::WlOutput,
    /// The monitor's connector name.
    name: String,
    namespace: String,
    layer: LayerSurface,
    a11y: Served,
    scale: u32,
    /// In the surface's own units, once the compositor has said.
    size: Option<(u32, u32)>,
    /// What it holds, where it was last drawn.
    placed: Vec<(Item, Rect)>,
}

impl Panels {
    pub(super) fn new(
        shell: &Shell,
        #[cfg(feature = "menus")] actions: Sender<super::Asked>,
    ) -> Self {
        let clock = Clock::new(shell.panel.as_ref().map_or("%H:%M", |panel| &panel.clock));
        Self {
            panel: shell.panel.clone(),
            clock,
            time: String::new(),
            each: Vec::new(),
            open_on: None,
            #[cfg(feature = "menus")]
            actions,
        }
    }

    /// Show the panel `shell` asks for now: on the monitors it names, along
    /// its edge, as tall as it says, holding what it lists.
    pub(super) fn reconfigure(
        &mut self,
        canvas: &mut Canvas,
        qh: &QueueHandle<App>,
        shell: &Shell,
        outputs: &OutputState,
        fonts: &mut Fonts,
    ) {
        if shell.panel == self.panel {
            return;
        }
        let was = std::mem::replace(&mut self.panel, shell.panel.clone());
        let mut moved = false;
        if let Some(panel) = &self.panel {
            self.clock = Clock::new(&panel.clock);
            self.time = self.clock.show(&chrono::Local::now());
            // Another edge or height: the compositor sizes each panel again,
            // and it is drawn when it has.
            moved = was.is_some_and(|was| (was.edge, was.height) != (panel.edge, panel.height));
            if moved {
                for bar in &self.each {
                    place(&bar.layer, panel);
                    bar.layer.commit();
                }
            }
        }
        self.sync(canvas, qh, outputs, None);
        if !moved {
            (0..self.each.len()).for_each(|at| self.draw(canvas, fonts, at));
        }
    }

    /// Put a panel on each monitor that should have one and has none, and
    /// take it from each that has one and should not. `gone` is a monitor
    /// unplugged, still listed while the compositor's word of it is handled.
    pub(super) fn sync(
        &mut self,
        canvas: &Canvas,
        qh: &QueueHandle<App>,
        outputs: &OutputState,
        gone: Option<&wl_output::WlOutput>,
    ) {
        let Some(panel) = &self.panel else {
            self.each.clear();
            return;
        };
        let monitors: Vec<_> = outputs
            .outputs()
            .filter(|output| Some(output) != gone)
            .filter_map(|output| {
                let info = outputs.info(&output)?;
                let name = info.name.clone()?;
                let at = info.logical_position.unwrap_or((i32::MAX, i32::MAX));
                Some((output, name, at, info.scale_factor))
            })
            .collect();
        let named: Vec<(&str, (i32, i32))> = monitors
            .iter()
            .map(|(_, name, at, _)| (name.as_str(), *at))
            .collect();
        let wanted: Vec<String> = chosen(&panel.outputs, &named)
            .into_iter()
            .map(str::to_owned)
            .collect();
        self.each.retain(|bar| wanted.contains(&bar.name));
        for (output, name, _, scale) in monitors {
            if wanted.contains(&name) && !self.each.iter().any(|bar| bar.name == name) {
                let namespace = format!("{NAMESPACE}{name}");
                let layer = canvas.layer(qh, Layer::Top, &namespace, &output);
                place(&layer, panel);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                layer.commit();
                self.each.push(Bar {
                    output,
                    a11y: self.served(&namespace, &name),
                    name,
                    namespace,
                    layer,
                    scale: whole(scale),
                    size: None,
                    placed: Vec::new(),
                });
            }
        }
    }

    /// A panel's tree on the bus, its start button pressable for the start
    /// menu of the monitor named `name`.
    #[cfg(feature = "menus")]
    fn served(&self, namespace: &str, name: &str) -> Served {
        Served::acting(
            a11y::panel(namespace, None, &[], &self.time, false),
            Press {
                output: name.to_owned(),
                actions: self.actions.clone(),
            },
        )
    }

    /// A panel's tree on the bus, with no menu to open from it.
    #[cfg(not(feature = "menus"))]
    fn served(&self, namespace: &str, _: &str) -> Served {
        Served::new(a11y::panel(namespace, None, &[], &self.time, false))
    }

    /// Draw a monitor's panel again if its scale changed.
    pub(super) fn rescale(
        &mut self,
        canvas: &mut Canvas,
        fonts: &mut Fonts,
        output: &wl_output::WlOutput,
        scale: i32,
    ) {
        let scale = whole(scale);
        if let Some(at) = self
            .each
            .iter()
            .position(|bar| bar.output == *output && bar.scale != scale)
        {
            self.each[at].scale = scale;
            self.draw(canvas, fonts, at);
        }
    }

    pub(super) fn closed(&mut self, layer: &LayerSurface) {
        self.each.retain(|bar| bar.layer != *layer);
    }

    /// The compositor sized a panel: draw it.
    pub(super) fn configure(
        &mut self,
        canvas: &mut Canvas,
        fonts: &mut Fonts,
        layer: &LayerSurface,
        configure: &LayerSurfaceConfigure,
    ) {
        let Some(at) = self.each.iter().position(|bar| bar.layer == *layer) else {
            return;
        };
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        self.each[at].size = Some((width, height));
        self.draw(canvas, fonts, at);
    }

    /// Read the clock, and draw the panels again if what it shows changed.
    /// How long until it next might.
    pub(super) fn tick(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) -> Duration {
        let now = chrono::Local::now();
        let time = self.clock.show(&now);
        if time != self.time {
            self.time = time;
            (0..self.each.len()).for_each(|at| self.draw(canvas, fonts, at));
        }
        self.clock.until_next(&now)
    }

    /// Show the start button of the monitor `on` as open, and every other as
    /// shut.
    #[cfg(feature = "menus")]
    pub(super) fn set_open(&mut self, canvas: &mut Canvas, fonts: &mut Fonts, on: Option<&str>) {
        if self.open_on.as_deref() == on {
            return;
        }
        let was = std::mem::replace(&mut self.open_on, on.map(str::to_owned));
        for at in 0..self.each.len() {
            let name = Some(self.each[at].name.as_str());
            if name == was.as_deref() || name == on {
                self.draw(canvas, fonts, at);
            }
        }
    }

    /// The monitor of the panel that is `surface`, and what on it is at
    /// `point`, if `surface` is a panel's.
    #[cfg(feature = "menus")]
    pub(super) fn at(
        &self,
        surface: &wl_surface::WlSurface,
        point: (f64, f64),
    ) -> Option<(&str, Option<Item>)> {
        let bar = self
            .each
            .iter()
            .find(|bar| bar.layer.wl_surface() == surface)?;
        Some((
            &bar.name,
            crate::layout::panel::item_at(&bar.placed, point).map(|(item, _)| item),
        ))
    }

    /// The edge the panels are along, if there are panels.
    #[cfg(feature = "menus")]
    pub(super) fn edge(&self) -> Option<Edge> {
        self.panel.as_ref().map(|panel| panel.edge)
    }

    /// The strip of `monitor` the panel on the monitor named `name` takes,
    /// if it has one.
    #[cfg(feature = "menus")]
    pub(super) fn strip(&self, name: &str, monitor: Rect) -> Option<Rect> {
        let panel = self.panel.as_ref()?;
        self.each
            .iter()
            .any(|bar| bar.name == name)
            .then(|| crate::layout::panel::strip(panel.edge, panel.height as i32, monitor))
    }

    /// Where on `monitor` the start button of the panel on the monitor named
    /// `name` is, if it has one.
    #[cfg(feature = "menus")]
    pub(super) fn start_button(&self, name: &str, monitor: Rect) -> Option<Rect> {
        let strip = self.strip(name, monitor)?;
        let bar = self.each.iter().find(|bar| bar.name == name)?;
        bar.placed
            .iter()
            .find(|(item, _)| *item == Item::Start)
            .map(|&(_, at)| Rect::new(strip.x + at.x, strip.y + at.y, at.w, at.h))
    }

    /// Draw panel `at`, once the compositor has given it a size.
    fn draw(&mut self, canvas: &mut Canvas, fonts: &mut Fonts, at: usize) {
        let Some(panel) = &self.panel else {
            return;
        };
        let bar = &mut self.each[at];
        let Some(size) = bar.size else {
            return;
        };
        let (width, height) = (size.0 as i32, size.1 as i32);
        let text = fonts.get();
        bar.placed = lay_out(&panel.items, &self.time, (width, height), &mut *text);
        let open = self.open_on.as_deref() == Some(bar.name.as_str());
        let shown = paint::panel::Shown {
            size: (width, height),
            edge: panel.edge,
            placed: &bar.placed,
            time: &self.time,
            open,
        };
        let started = Instant::now();
        // Wholly opaque, which lets the compositor skip what is under it.
        canvas.show(
            &bar.layer,
            size,
            bar.scale,
            &[(0, 0, width, height)],
            |picture| {
                paint::panel::paint(&shown, picture, bar.scale, text);
            },
        );
        tracing::debug!(output = bar.name, took = ?started.elapsed(), "a panel was drawn");
        bar.a11y.show(a11y::panel(
            &bar.namespace,
            Some(size),
            &bar.placed,
            &self.time,
            open,
        ));
    }
}

/// Put `layer` along the panel's edge, as tall as it, keeping windows out.
fn place(layer: &LayerSurface, panel: &Panel) {
    let edge = match panel.edge {
        Edge::Top => Anchor::TOP,
        Edge::Bottom => Anchor::BOTTOM,
    };
    layer.set_anchor(edge | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, panel.height);
    layer.set_exclusive_zone(panel.height as i32);
}

/// An assistive technology's press of a panel's start button, sent on to
/// the shell's loop.
#[cfg(feature = "menus")]
struct Press {
    output: String,
    actions: Sender<super::Asked>,
}

#[cfg(feature = "menus")]
impl accesskit::ActionHandler for Press {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        if request.action != accesskit::Action::Click || request.target_node != a11y::START {
            return;
        }
        if self
            .actions
            .send(super::Asked::StartMenu(self.output.clone()))
            .is_err()
        {
            tracing::debug!("the shell is stopping; the press is dropped");
        }
    }
}
