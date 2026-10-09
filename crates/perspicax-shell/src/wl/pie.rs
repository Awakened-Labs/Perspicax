//! The pie's surface: while a pie is open, one surface on the `overlay`
//! layer of the monitor it opened on, over everything, as the menus' is.
//!
//! It covers the whole monitor, panels included, and takes clicks
//! everywhere on it: a pie is pointed at by direction, out to the screen's
//! edge, so there is no "off the pie" to click. It is clear but for the
//! pie's square, and says it is opaque nowhere, since what is under a pie
//! shows through it. It takes the keyboard while it is up, so opening a
//! pie closes the menus and opening a menu closes the pie, each losing the
//! keyboard to the other. Its namespace is `perspicax-pie-<connector>`.
//!
//! A pie is built from the config each time it opens: what it lists is
//! never older than the installed applications and the folder of icons.
//!
//! It is drawn again on every move of the pointer, as its icons grow, but
//! no faster than the monitor shows it: a picture waits for the compositor
//! to say it showed the one before, and only the last of what changed
//! meanwhile is drawn.

use std::path::{Path, PathBuf};

use perspicax_config::{Shell, pie::Pie};
use smithay_client_toolkit::{
    output::OutputState,
    shell::{
        WaylandSurface,
        wlr_layer::{LayerSurface, LayerSurfaceConfigure},
    },
};
use wayland_client::{QueueHandle, protocol::wl_surface};

use super::{
    App,
    canvas::{Canvas, Frames, whole},
    installed::Installed,
};
use crate::{
    model::{
        fs::Disk,
        image,
        pie::{Folder, Slot, Sources, seen},
    },
    paint::{self, Kit},
    pie::{Effect, Event, State},
};

/// The pies, and what they are built from.
pub(super) struct Pies {
    state: State,
    /// The surface, while a pie is open.
    shown: Option<Shown>,
    /// What the config says of the pies, if it has any.
    pie: Option<Pie>,
    /// The pie's own folder of icons, read when a pie opens.
    folder: Option<PathBuf>,
    config: Option<PathBuf>,
    home: Option<PathBuf>,
    /// What the wheel turned short of a notch.
    wheel: f64,
}

/// The pie's surface.
struct Shown {
    /// The monitor's connector name.
    name: String,
    layer: LayerSurface,
    scale: u32,
    /// In the surface's own units, once the compositor has said.
    size: Option<(u32, u32)>,
    frames: Frames,
    /// Whether a picture is shown that the compositor has not yet said it
    /// put on the monitor.
    waiting: bool,
    /// Whether what is shown is older than what the pie is.
    stale: bool,
}

impl Pies {
    pub(super) fn new(shell: &Shell, config: Option<&Path>) -> Self {
        let mut pies = Self {
            state: State::default(),
            shown: None,
            pie: None,
            folder: None,
            config: config.map(Path::to_owned),
            home: std::env::var_os("HOME").map(PathBuf::from),
            wheel: 0.0,
        };
        pies.reconfigure(shell, config);
        pies
    }

    /// Take up a changed config: from the next pie that opens.
    pub(super) fn reconfigure(&mut self, shell: &Shell, config: Option<&Path>) {
        self.config = config.map(Path::to_owned);
        self.pie = shell.pie.clone();
        self.folder = self
            .pie
            .as_ref()
            .and_then(|pie| pie.icons.as_deref())
            .map(|written| image::locate(written, config, self.home.as_deref()));
    }

    /// Whether `surface` is the pie's.
    pub(super) fn owns(&self, surface: &wl_surface::WlSurface) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|shown| shown.layer.wl_surface() == surface)
    }

    /// How big the pies are across, and the slots of the one named `name`,
    /// if the config has it, with the running `windows`, each `(serial,
    /// app-id, title)`.
    pub(super) fn built(
        &self,
        name: &str,
        installed: &mut Installed,
        windows: &[(u64, String, String)],
    ) -> Option<(u32, Vec<Slot>)> {
        let pie = self.pie.as_ref()?;
        let items = pie.menus.get(name)?;
        let running = seen(
            windows
                .iter()
                .map(|(serial, app_id, title)| (*serial, app_id.as_str(), title.as_str())),
            &pie.aliases,
            &pie.ignore,
        );
        installed.refresh();
        let folder = self
            .folder
            .as_deref()
            .map(|dir| Folder::read(&Disk, dir))
            .unwrap_or_default();
        let sources = Sources {
            fs: &Disk,
            apps: installed.apps(),
            folder: &folder,
            home: self.home.as_deref(),
            config: self.config.as_deref(),
            locale: &installed.places().locale,
            running: &running,
        };
        Some((pie.size, sources.pie(items)))
    }

    /// The wheel turned `notches`, up negative, in fractions of one: the
    /// whole notches it comes to with what was left over before.
    pub(super) fn turned(&mut self, notches: f64) -> i32 {
        self.wheel += notches;
        let whole = self.wheel.trunc();
        self.wheel -= whole;
        whole as i32
    }

    /// Handle `event`, and show what it changed. What is left to do is
    /// returned.
    pub(super) fn send(
        &mut self,
        canvas: &mut Canvas,
        qh: &QueueHandle<App>,
        outputs: &OutputState,
        kit: &mut Kit,
        event: Event,
    ) -> Vec<Effect> {
        if matches!(event, Event::Asked { .. }) {
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
    /// monitor the pie opened on, or draw it again.
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
                    "a pie was opened on a monitor that is gone"
                );
                return;
            };
            let namespace = format!("perspicax-pie-{}", view.output);
            self.shown = Some(Shown {
                name: view.output.to_owned(),
                layer: canvas.overlay(qh, &namespace, &output),
                scale: whole(scale),
                size: None,
                frames: Frames::default(),
                waiting: false,
                stale: false,
            });
        }
        self.draw(canvas, kit, qh);
    }

    /// Draw the pie on the surface, once the compositor has given it a
    /// size, and not before it has shown the last picture.
    fn draw(&mut self, canvas: &mut Canvas, kit: &mut Kit, qh: &QueueHandle<App>) {
        let (Some(shown), Some(view)) = (self.shown.as_mut(), self.state.view()) else {
            return;
        };
        let Some(size) = shown.size else {
            return;
        };
        if shown.waiting {
            shown.stale = true;
            return;
        }
        let square = view.ring.square();
        let Kit {
            fonts,
            images,
            palette,
        } = kit;
        let text = fonts.get();
        let scale = shown.scale;
        let started = std::time::Instant::now();
        // Asked with the picture, so the compositor says when it shows it.
        let surface = shown.layer.wl_surface();
        surface.frame(qh, surface.clone());
        let shown_now = canvas.show_part(
            &shown.layer,
            &mut shown.frames,
            size,
            scale,
            (square.x, square.y, square.w, square.h),
            |picture| paint::pie::paint(&view, picture, scale, text, images, palette),
        );
        tracing::debug!(took = ?started.elapsed(), "the pie was drawn");
        shown.waiting = shown_now;
        shown.stale = !shown_now;
    }

    /// The compositor showed the last picture on `surface`: draw what
    /// changed since, if anything did.
    pub(super) fn frame(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        qh: &QueueHandle<App>,
        surface: &wl_surface::WlSurface,
    ) {
        let Some(shown) = self
            .shown
            .as_mut()
            .filter(|shown| shown.layer.wl_surface() == surface)
        else {
            return;
        };
        shown.waiting = false;
        if shown.stale {
            self.draw(canvas, kit, qh);
        }
    }

    /// The compositor sized the surface: draw the pie on it.
    pub(super) fn configure(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        qh: &QueueHandle<App>,
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
        self.draw(canvas, kit, qh);
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

    /// Whether the keyboard came to, or left, the pie's surface.
    pub(super) fn keyboard(&self, surface: &wl_surface::WlSurface) -> bool {
        self.owns(surface)
    }
}
