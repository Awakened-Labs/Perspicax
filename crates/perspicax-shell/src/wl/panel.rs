//! The panels: one surface on the `top` layer of each monitor the config
//! names, along its bottom or its top, with an exclusive zone as tall as it
//! is, so that windows are kept out of the strip it takes. A fullscreen
//! window in use still covers it, as perspicax arranges. The surface is the
//! whole strip even when the bar drawn on it is narrower: the rest is clear
//! and takes no clicks, so a click there reaches the desktop under it. To an
//! agent it still covers the desktop there: a surface covers everything it
//! draws, clear or not.
//! Anchored to a corner instead, the surface would have smithay keep
//! windows out of a column down the monitor's side, not the strip.
//!
//! Its namespace is `perspicax-panel-<connector>`, and its accessibility
//! window carries the same name, which is how perspicax joins the two.
//!
//! A changed config redraws the panels where they are, moved to another
//! edge or made another height in place, so an agent holding a panel's id
//! still holds it; only the monitors that no longer have one lose theirs.
//! Which monitor is the first is worked out again whenever one comes or
//! goes.
//!
//! Every panel lists its windows and pages its workspaces from what the
//! compositor tells the shell over the taskbar's and the pager's protocols
//! (see `taskbar` and `pager`), and shows the status icons the tray reads
//! off the session bus (see `tray`), and the keyboard layout in use from
//! what perspicax tells the shell channel (see `channel`). News of any draws
//! the panels again once the loop has nothing else to do, so a batch of it
//! is drawn once.
//!
//! A panel showing the mixer's moving bars keeps its last picture, and
//! moves the bars by drawing them alone over it, once each time the monitor
//! shows the last: the compositor takes in their slot and nothing else, and
//! is asked to say when it has shown it. With nothing playing, the bars come
//! to rest and nothing more is drawn.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use perspicax_config::{Edge, Item, Panel, Shell};
use perspicax_protocols::shell::v1::client::perspicax_shell_v1;
use smithay_client_toolkit::{
    output::OutputState,
    reexports::calloop::channel::Sender,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceConfigure},
    },
};
use wayland_client::{
    Proxy as _, QueueHandle,
    protocol::{wl_output, wl_surface},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
};

use super::{
    App, Asked,
    canvas::{Canvas, whole},
    pager::Model,
    watch,
};
#[cfg(feature = "menus")]
use crate::layout::Rect;
use crate::{
    a11y::{self, adapter::Served},
    layout::panel::{Cell, Holding, Part, Placed, Task, chosen, lay_out},
    model::{
        Button,
        apps::{self, App as Application},
        clock::Clock,
        image,
        layouts::Layouts,
        tasks::{Tasks, Window},
    },
    paint::{self, Kit},
};

/// What every panel's namespace starts with, before its monitor's name.
const NAMESPACE: &str = "perspicax-panel-";

/// Every monitor's panel, and what they show.
pub(super) struct Panels {
    /// `None` is no panel, and then no surface either.
    panel: Option<Panel>,
    clock: Clock,
    /// The time, as the clock shows it now.
    time: String,
    each: Vec<Bar>,
    /// The monitor whose start menu is open, for its button to show it.
    open_on: Option<String>,
    /// The config file, beside which a relative path to the start button's
    /// icon is.
    config: Option<PathBuf>,
    /// The start button's icon, as the icon theme looks for it; `None` for
    /// Perspicax's mark.
    start: Option<String>,
    /// The start button's icon was not found or could not be read, and the
    /// log has said so since the config last changed.
    told: bool,
    /// The windows, as the compositor tells a taskbar them, and what it
    /// tells them with: `None` if it does not, or stopped.
    pub(super) tasks: Tasks<ZwlrForeignToplevelHandleV1, wl_output::WlOutput>,
    pub(super) taskbar: Option<ZwlrForeignToplevelManagerV1>,
    /// Each window's icon, by the app id it gives, as found in the
    /// applications' read of number `icons_from`.
    icons: HashMap<String, Option<String>>,
    icons_from: u64,
    /// The status icons, by key, in the order they came.
    #[cfg(feature = "tray")]
    tray: Vec<(u64, crate::model::tray::Item)>,
    /// The keyboard's layouts, as perspicax lists them.
    pub(super) layouts: Layouts,
    /// The mixer, as `[shell.mixer]` writes it.
    #[cfg(feature = "audio")]
    mixer: perspicax_config::mixer::Mixer,
    /// How loud the default output is, and what it is, or `None` with no
    /// sound server to ask.
    #[cfg(feature = "audio")]
    sound: Option<crate::model::sound::Level>,
    #[cfg(feature = "audio")]
    playing: Option<crate::layout::panel::Playing>,
    /// How loud the default input is, or `None` with none.
    #[cfg(feature = "audio")]
    mic: Option<crate::model::sound::Level>,
    /// The monitor the mixer is open on, for its item there to show it.
    #[cfg(feature = "audio")]
    mixer_open_on: Option<String>,
    /// The mixer's moving bars, and when the monitor last showed them, by
    /// the compositor's clock, in milliseconds.
    #[cfg(feature = "audio")]
    bars: crate::model::levels::Bars,
    #[cfg(feature = "audio")]
    shown_at: Option<u32>,
    /// The panels are to be drawn again once the loop is idle.
    stale: bool,
    /// Where an assistive technology's press of something on a panel is
    /// sent.
    actions: Sender<Asked>,
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
    placed: Placed,
    /// Its last picture, while it shows the mixer's moving bars.
    #[cfg(feature = "audio")]
    kept: Option<Kept>,
}

/// A panel's last picture, kept to draw the moving bars over.
#[cfg(feature = "audio")]
struct Kept {
    picture: tiny_skia::Pixmap,
    copies: super::canvas::Copies,
    /// How many whole pictures have been drawn.
    generation: u64,
    /// The bars' slot, where they were last placed.
    place: crate::layout::Rect,
    /// The compositor has yet to show the bars last drawn.
    waiting: bool,
    /// The bars moved while the compositor had yet to.
    stale: bool,
}

impl Panels {
    pub(super) fn new(
        shell: &Shell,
        config: Option<&Path>,
        actions: Sender<Asked>,
        taskbar: Option<ZwlrForeignToplevelManagerV1>,
    ) -> Self {
        let clock = Clock::new(shell.panel.as_ref().map_or("%H:%M", |panel| &panel.clock));
        Self {
            panel: shell.panel.clone(),
            clock,
            time: String::new(),
            each: Vec::new(),
            open_on: None,
            config: config.map(Path::to_owned),
            start: start_icon(shell, config),
            told: false,
            tasks: Tasks::default(),
            taskbar,
            icons: HashMap::new(),
            icons_from: 0,
            #[cfg(feature = "tray")]
            tray: Vec::new(),
            layouts: Layouts::default(),
            #[cfg(feature = "audio")]
            mixer: shell.mixer.clone(),
            #[cfg(feature = "audio")]
            sound: None,
            #[cfg(feature = "audio")]
            playing: None,
            #[cfg(feature = "audio")]
            mic: None,
            #[cfg(feature = "audio")]
            mixer_open_on: None,
            #[cfg(feature = "audio")]
            bars: crate::model::levels::Bars::default(),
            #[cfg(feature = "audio")]
            shown_at: None,
            stale: false,
            actions,
        }
    }

    /// Show the panel `shell` asks for now: on the monitors it names, along
    /// its edge, as tall as it says, holding what it lists, its start button
    /// wearing the icon it names. An icon file that could not be read is
    /// looked for again, since a save may follow mending it.
    pub(super) fn reconfigure(
        &mut self,
        canvas: &mut Canvas,
        qh: &QueueHandle<App>,
        shell: &Shell,
        outputs: &OutputState,
        kit: &mut Kit,
        workspaces: &Model,
    ) {
        let start = start_icon(shell, self.config.as_deref());
        let retried = start
            .as_deref()
            .is_some_and(|icon| kit.images.forget_failed(icon));
        let restarted = retried || start != self.start;
        if restarted {
            self.start = start;
            self.told = false;
        }
        // Another mixer is drawn in place, as another icon is.
        #[cfg(feature = "audio")]
        let restarted = {
            let remixed = shell.mixer != self.mixer;
            if remixed {
                self.mixer = shell.mixer.clone();
            }
            restarted || remixed
        };
        if shell.panel == self.panel {
            if restarted {
                self.redraw(canvas, kit, workspaces);
            }
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
            self.redraw(canvas, kit, workspaces);
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
                let layer = canvas.layer(qh, Layer::Top, &namespace, Some(&output));
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
                    placed: Placed::default(),
                    #[cfg(feature = "audio")]
                    kept: None,
                });
            }
        }
    }

    /// A panel's tree on the bus, what can be pressed on it pressable on the
    /// panel of the monitor named `name`.
    fn served(&self, namespace: &str, name: &str) -> Served {
        Served::acting(
            a11y::panel(namespace, None, &Placed::default(), &self.time, false, None),
            Press {
                output: name.to_owned(),
                actions: self.actions.clone(),
            },
        )
    }

    /// Draw a monitor's panel again if its scale changed.
    pub(super) fn rescale(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        workspaces: &Model,
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
            self.draw(canvas, kit, workspaces, at);
        }
    }

    /// A monitor was unplugged: no window is on it now.
    pub(super) fn gone(&mut self, output: &wl_output::WlOutput) {
        self.tasks.gone(output);
    }

    pub(super) fn closed(&mut self, layer: &LayerSurface) {
        self.each.retain(|bar| bar.layer != *layer);
    }

    /// The compositor sized a panel: draw it.
    pub(super) fn configure(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        workspaces: &Model,
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
        self.draw(canvas, kit, workspaces, at);
    }

    /// Read the clock, and draw the panels again if what it shows changed.
    /// How long until it next might.
    pub(super) fn tick(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        workspaces: &Model,
    ) -> Duration {
        let now = chrono::Local::now();
        let time = self.clock.show(&now);
        if time != self.time {
            self.time = time;
            self.redraw(canvas, kit, workspaces);
        }
        self.clock.until_next(&now)
    }

    /// Show the start button of the monitor `on` as open, and every other as
    /// shut.
    #[cfg(feature = "menus")]
    pub(super) fn set_open(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        workspaces: &Model,
        on: Option<&str>,
    ) {
        if self.open_on.as_deref() == on {
            return;
        }
        let was = std::mem::replace(&mut self.open_on, on.map(str::to_owned));
        for at in 0..self.each.len() {
            let name = Some(self.each[at].name.as_str());
            if name == was.as_deref() || name == on {
                self.draw(canvas, kit, workspaces, at);
            }
        }
    }

    /// Mark the panels to be drawn again, and say whether they were not
    /// marked already.
    pub(super) fn mark_stale(&mut self) -> bool {
        !std::mem::replace(&mut self.stale, true)
    }

    /// Draw every panel again, if they were marked to be.
    pub(super) fn draw_if_stale(&mut self, canvas: &mut Canvas, kit: &mut Kit, workspaces: &Model) {
        if std::mem::take(&mut self.stale) {
            self.redraw(canvas, kit, workspaces);
        }
    }

    /// Whether a taskbar lists a window whose application's icon has not
    /// been looked for yet.
    pub(super) fn icon_unknown(&self) -> bool {
        let taskbar = self
            .panel
            .as_ref()
            .is_some_and(|panel| panel.items.contains(&Item::Taskbar));
        let icons = &self.icons;
        taskbar
            && self
                .tasks
                .windows()
                .any(|window| !icons.contains_key(&window.app_id))
    }

    /// Look for the icon of each window's application in `apps`, the
    /// applications' read of number `read`.
    pub(super) fn find_icons(&mut self, read: u64, apps: &[Application]) {
        if read != self.icons_from {
            self.icons.clear();
            self.icons_from = read;
        }
        for window in self.tasks.windows() {
            if !self.icons.contains_key(&window.app_id) {
                let icon = apps::of_window(apps, &window.app_id)
                    .and_then(|app| app.icon.clone())
                    // Many an application names its icon as it names its
                    // windows, entry or none.
                    .or_else(|| Some(window.app_id.clone()).filter(|id| !id.is_empty()));
                self.icons.insert(window.app_id.clone(), icon);
            }
        }
    }

    /// The monitor of the panel that is `surface`, and what on it is at
    /// `point`, if `surface` is a panel's.
    pub(super) fn at(
        &self,
        surface: &wl_surface::WlSurface,
        point: (f64, f64),
    ) -> Option<(&str, Option<Part>)> {
        let bar = self
            .each
            .iter()
            .find(|bar| bar.layer.wl_surface() == surface)?;
        Some((&bar.name, bar.placed.at(point)))
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

    /// Show the mixer `level`, the output `playing` and the input's `mic`,
    /// or `None` for none. Whether that changed what it shows.
    #[cfg(feature = "audio")]
    pub(super) fn hear(
        &mut self,
        level: Option<crate::model::sound::Level>,
        playing: Option<crate::layout::panel::Playing>,
        mic: Option<crate::model::sound::Level>,
    ) -> bool {
        let level_changed = std::mem::replace(&mut self.sound, level) != self.sound;
        let playing_changed = std::mem::replace(&mut self.playing, playing) != self.playing;
        let mic_changed = std::mem::replace(&mut self.mic, mic) != self.mic;
        level_changed || playing_changed || mic_changed
    }

    /// Show the mixer open on the monitor named `on`, or on none. Whether
    /// that changed what the panels show.
    #[cfg(feature = "audio")]
    pub(super) fn mixer_open(&mut self, on: Option<String>) -> bool {
        std::mem::replace(&mut self.mixer_open_on, on) != self.mixer_open_on
    }

    /// Where on `monitor` the mixer item of the panel on the monitor named
    /// `name` is, if it has one.
    #[cfg(feature = "audio")]
    pub(super) fn mixer_at(&self, name: &str, monitor: Rect) -> Option<Rect> {
        let strip = self.strip(name, monitor)?;
        let bar = self.each.iter().find(|bar| bar.name == name)?;
        let at = bar.placed.item(Item::Mixer)?;
        Some(Rect::new(strip.x + at.x, strip.y + at.y, at.w, at.h))
    }

    /// The status icon of `key`.
    #[cfg(feature = "tray")]
    pub(super) fn status(&self, key: u64) -> Option<&crate::model::tray::Item> {
        self.tray
            .iter()
            .find(|(shown, _)| *shown == key)
            .map(|(_, item)| item)
    }

    /// Show the status icon of `key` as `item`, where it was, or after the
    /// rest if it is new.
    #[cfg(feature = "tray")]
    pub(super) fn show_status(&mut self, key: u64, item: crate::model::tray::Item) {
        match self.tray.iter_mut().find(|(shown, _)| *shown == key) {
            Some((_, shown)) => *shown = item,
            None => self.tray.push((key, item)),
        }
    }

    /// Take the status icon of `key` away. Whether there was one.
    #[cfg(feature = "tray")]
    pub(super) fn forget_status(&mut self, key: u64) -> bool {
        let before = self.tray.len();
        self.tray.retain(|(shown, _)| *shown != key);
        self.tray.len() != before
    }

    /// Take every status icon away. Whether there were any.
    #[cfg(feature = "tray")]
    pub(super) fn clear_tray(&mut self) -> bool {
        !std::mem::take(&mut self.tray).is_empty()
    }

    /// Where on `monitor` the status icon of `key` is, on the panel of the
    /// monitor named `name`.
    #[cfg(feature = "tray")]
    pub(super) fn status_at(&self, name: &str, key: u64, monitor: Rect) -> Option<Rect> {
        let strip = self.strip(name, monitor)?;
        let bar = self.each.iter().find(|bar| bar.name == name)?;
        let at = bar.placed.tray_icon(key)?;
        Some(Rect::new(strip.x + at.x, strip.y + at.y, at.w, at.h))
    }

    /// Where on `monitor` the task of window `serial` is, on the panel of
    /// the monitor named `name`.
    #[cfg(feature = "menus")]
    pub(super) fn task_at(&self, name: &str, serial: u64, monitor: Rect) -> Option<Rect> {
        let strip = self.strip(name, monitor)?;
        let bar = self.each.iter().find(|bar| bar.name == name)?;
        let at = bar.placed.task(serial)?;
        Some(Rect::new(strip.x + at.x, strip.y + at.y, at.w, at.h))
    }

    /// Where on `monitor` the start button of the panel on the monitor named
    /// `name` is, if it has one.
    #[cfg(feature = "menus")]
    pub(super) fn start_button(&self, name: &str, monitor: Rect) -> Option<Rect> {
        let strip = self.strip(name, monitor)?;
        let bar = self.each.iter().find(|bar| bar.name == name)?;
        let at = bar.placed.item(Item::Start)?;
        Some(Rect::new(strip.x + at.x, strip.y + at.y, at.w, at.h))
    }

    /// Draw every panel.
    fn redraw(&mut self, canvas: &mut Canvas, kit: &mut Kit, workspaces: &Model) {
        (0..self.each.len()).for_each(|at| self.draw(canvas, kit, workspaces, at));
    }

    /// Draw panel `at`, once the compositor has given it a size.
    fn draw(&mut self, canvas: &mut Canvas, kit: &mut Kit, workspaces: &Model, at: usize) {
        let Some(panel) = &self.panel else {
            return;
        };
        let bar = &mut self.each[at];
        let Some(size) = bar.size else {
            return;
        };
        let (width, height) = (size.0 as i32, size.1 as i32);
        let tasks = self
            .tasks
            .listed(&bar.output, panel.taskbar)
            .map(|(serial, window)| Task {
                serial,
                title: title(window),
                icon: self.icons.get(&window.app_id).cloned().flatten(),
                active: window.active,
                minimized: window.minimized,
            })
            .collect();
        let cells = workspaces
            .on(&bar.output)
            .into_iter()
            .map(|(workspace, (column, row))| Cell {
                serial: workspace.serial,
                name: workspace.name.clone(),
                column,
                row,
                active: workspace.active,
            })
            .collect();
        #[cfg(feature = "tray")]
        let tray = self
            .tray
            .iter()
            .filter(|(_, item)| item.shown())
            .map(|(key, item)| crate::layout::panel::TrayIcon {
                key: *key,
                title: item.title.clone(),
                menu: item.menu,
            })
            .collect();
        let layout = self.layouts.shown();
        let holding = Holding {
            time: &self.time,
            layout: layout.map(|layout| layout.short.as_str()),
            tasks,
            task_titles: panel.task_titles,
            cells,
            #[cfg(feature = "tray")]
            tray,
            #[cfg(feature = "audio")]
            mixer: crate::layout::panel::Mixing {
                pieces: &self.mixer.panel,
                sound: crate::layout::panel::Sounding {
                    level: self.sound,
                    step: self.mixer.step,
                    limit: self.mixer.limit,
                    output: self.playing.clone(),
                    mic: self.mic,
                    open: self.mixer_open_on.as_deref() == Some(bar.name.as_str()),
                },
            },
        };
        let Kit {
            fonts,
            images,
            palette,
        } = kit;
        let text = fonts.get();
        bar.placed = lay_out(
            &panel.items,
            holding,
            (width, height),
            (panel.width, panel.align),
            &mut *text,
        );
        if let (Some(icon), false) = (self.start.as_deref(), self.told)
            && let Some(button) = bar.placed.item(Item::Start)
            && images
                .get(icon, paint::panel::fitted(button.h) as u32, bar.scale)
                .is_none()
        {
            tracing::warn!(
                "the start button's icon {icon} cannot be found or read; showing Perspicax's mark"
            );
            self.told = true;
        }
        let open = self.open_on.as_deref() == Some(bar.name.as_str());
        let shown = paint::panel::Shown {
            size: (width, height),
            edge: panel.edge,
            placed: &bar.placed,
            time: &self.time,
            layout: layout.map(|layout| layout.short.as_str()),
            open,
            start: self.start.as_deref(),
            #[cfg(feature = "tray")]
            tray: &self.tray,
            #[cfg(feature = "audio")]
            bars: &self.bars,
        };
        let drawn = bar.placed.bar;
        let area = (drawn.x, drawn.y, drawn.w, drawn.h);
        canvas.take_clicks_in(&bar.layer, area);
        let started = Instant::now();
        // The bar wholly opaque, which lets the compositor skip what is under
        // it, and the rest of the strip clear. Clear is not uncovered: to an
        // agent the whole strip covers what is under it.
        #[cfg(feature = "audio")]
        let levels = bar
            .placed
            .mixer
            .iter()
            .find(|(piece, _)| *piece == perspicax_config::mixer::Piece::Levels)
            .map(|&(_, place)| place);
        #[cfg(feature = "audio")]
        match levels {
            Some(place) => {
                let scale = bar.scale;
                keep(
                    canvas,
                    (&bar.layer, &mut bar.kept),
                    (size, scale),
                    area,
                    place,
                    |picture| {
                        paint::panel::paint(&shown, picture, scale, text, images, palette);
                    },
                );
            }
            None => {
                bar.kept = None;
                canvas.show(&bar.layer, size, bar.scale, &[area], |picture| {
                    paint::panel::paint(&shown, picture, bar.scale, text, images, palette);
                });
            }
        }
        #[cfg(not(feature = "audio"))]
        canvas.show(&bar.layer, size, bar.scale, &[area], |picture| {
            paint::panel::paint(&shown, picture, bar.scale, text, images, palette);
        });
        tracing::debug!(output = bar.name, took = ?started.elapsed(), "a panel was drawn");
        bar.a11y.show(a11y::panel(
            &bar.namespace,
            Some(size),
            &bar.placed,
            &self.time,
            open,
            layout,
        ));
        watch::slow("drawing a panel", started.elapsed());
    }
}

/// Draw a panel's whole picture, `size` logical pixels at `scale`, with
/// `paint`, into the picture `kept` for its moving bars, at `place`, and show
/// it on `layer`, opaque in `area`.
#[cfg(feature = "audio")]
fn keep(
    canvas: &mut Canvas,
    (layer, kept): (&LayerSurface, &mut Option<Kept>),
    (size, scale): ((u32, u32), u32),
    area: super::canvas::Area,
    place: crate::layout::Rect,
    paint: impl FnOnce(&mut tiny_skia::PixmapMut<'_>),
) {
    let (wide, high) = (size.0 * scale, size.1 * scale);
    let fits = |kept: &Kept| (kept.picture.width(), kept.picture.height()) == (wide, high);
    if !kept.as_ref().is_some_and(fits) {
        *kept = tiny_skia::Pixmap::new(wide, high).map(|picture| Kept {
            picture,
            copies: super::canvas::Copies::default(),
            generation: 0,
            place,
            waiting: false,
            stale: false,
        });
    }
    let Some(kept) = kept else {
        return;
    };
    kept.picture.fill(tiny_skia::Color::TRANSPARENT);
    paint(&mut kept.picture.as_mut());
    kept.generation += 1;
    kept.place = place;
    // A whole picture asks for no frame, so the bars wait on none.
    kept.waiting = false;
    canvas.show_kept(
        layer,
        &mut kept.copies,
        &kept.picture,
        scale,
        (&[area], None),
        kept.generation,
    );
}

#[cfg(feature = "audio")]
impl Panels {
    /// The spectrum heard now: move the bars to it, on every panel that
    /// shows them.
    pub(super) fn hear_levels(
        &mut self,
        canvas: &mut Canvas,
        kit: &Kit,
        bands: [f32; crate::audio::BANDS],
    ) {
        self.bars.hear(bands);
        for at in 0..self.each.len() {
            self.move_bars(canvas, kit, at);
        }
    }

    /// The compositor showed what was drawn on `surface` at `time`, by its
    /// clock: let the bars fall for the time since they were last shown,
    /// and draw them again if they are still moving.
    pub(super) fn frame(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        surface: &wl_surface::WlSurface,
        time: u32,
    ) {
        let Some(at) = self
            .each
            .iter()
            .position(|bar| bar.layer.wl_surface() == surface)
        else {
            return;
        };
        let Some(kept) = &mut self.each[at].kept else {
            return;
        };
        kept.waiting = false;
        let since = self
            .shown_at
            .map_or(0, |shown| time.wrapping_sub(shown).min(100));
        self.shown_at = Some(time);
        let moving = self.bars.step(Duration::from_millis(u64::from(since)));
        let stale = self.each[at]
            .kept
            .as_mut()
            .is_some_and(|kept| std::mem::take(&mut kept.stale));
        if moving || stale {
            self.move_bars(canvas, kit, at);
        } else {
            // At rest: the next to be heard starts the clock afresh.
            self.shown_at = None;
        }
    }

    /// Draw panel `at`'s bars alone, as they stand, over its kept picture,
    /// unless the compositor has yet to show the last.
    fn move_bars(&mut self, canvas: &mut Canvas, kit: &Kit, at: usize) {
        let bar = &mut self.each[at];
        let Some(kept) = &mut bar.kept else {
            return;
        };
        if kept.waiting {
            kept.stale = true;
            return;
        }
        let colours = paint::panel::Colours::of(&kit.palette);
        let place = kept.place;
        paint::panel::levels(
            &mut kept.picture.as_mut(),
            place,
            &self.bars,
            bar.scale,
            &colours,
        );
        let within = crate::layout::panel::bars_in(place);
        let changed = (within.x, within.y, within.w, within.h);
        kept.waiting = canvas.show_kept(
            &bar.layer,
            &mut kept.copies,
            &kept.picture,
            bar.scale,
            (&[], Some(changed)),
            kept.generation,
        );
    }
}

/// The start button's icon `shell` names, as the icon theme looks for it.
fn start_icon(shell: &Shell, config: Option<&Path>) -> Option<String> {
    let written = shell.start_menu.as_ref()?.icon.as_deref()?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Some(image::start_icon(written, config, home.as_deref()))
}

/// What a window's task is called: its title, or with none, its app id.
pub(super) fn title<O>(window: &Window<O>) -> String {
    [&window.title, &window.app_id]
        .into_iter()
        .find(|name| !name.is_empty())
        .map_or_else(|| "Window".to_owned(), String::clone)
}

/// Put `layer` along the panel's edge, as tall as it and as wide as the
/// monitor whatever the width of its bar, keeping windows out.
fn place(layer: &LayerSurface, panel: &Panel) {
    let edge = match panel.edge {
        Edge::Top => Anchor::TOP,
        Edge::Bottom => Anchor::BOTTOM,
    };
    layer.set_anchor(edge | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, panel.height);
    layer.set_exclusive_zone(panel.height as i32);
}

impl App {
    /// The panels' news changed what they show: draw them again once the
    /// loop is idle, with the icon of any application new to them.
    pub(super) fn panels_changed(&mut self) {
        if !self.panels.mark_stale() {
            return;
        }
        self.handle.insert_idle(|app| {
            watch::timed("showing the panels' news", || {
                if app.panels.icon_unknown() {
                    let read = app.installed.refresh();
                    app.panels.find_icons(read, app.installed.apps());
                }
                app.panels
                    .draw_if_stale(&mut app.canvas, &mut app.kit, &app.workspaces.model);
            });
        });
    }

    /// A button went down on the panel of the monitor named `name`, on
    /// `part` of it, or on none.
    pub(super) fn panel_pressed(&mut self, name: &str, part: Option<Part>, button: Button) {
        // Any press on a panel closes the mixer, as one off it does.
        #[cfg(feature = "audio")]
        let mixer_was_open = self.close_mixer();
        #[cfg(feature = "menus")]
        {
            if (part, button) == (Some(Part::Start), Button::Left) {
                if let Some(output) = self.output_named(name) {
                    self.start_menu(&output);
                }
                return;
            }
            use crate::update::Which;

            let was_open = self.menus.open_menu();
            // Anything else on a panel closes the menus, as a press anywhere
            // off them does; a press on the tray icon whose menu is open, or
            // a right-click on the task whose menu is open, does only that.
            self.menu_event(crate::update::Event::PanelPress);
            match (part, was_open) {
                #[cfg(feature = "tray")]
                (Some(Part::Tray(key)), Some(Which::Tray(open))) if key == open => return,
                (Some(Part::Task(serial)), Some(Which::Task(open)))
                    if serial == open && button == Button::Right =>
                {
                    return;
                }
                _ => {}
            }
        }
        #[cfg(not(feature = "menus"))]
        let _ = name;
        match part {
            Some(Part::Task(serial)) => self.press_task(name, serial, button),
            Some(Part::Workspace(serial)) => self.press_workspace(serial, button),
            #[cfg(feature = "tray")]
            Some(Part::Tray(key)) => self.press_status(name, key, button),
            #[cfg(feature = "audio")]
            Some(Part::Mixer(piece)) => self.press_mixer(name, piece, button, mixer_was_open),
            Some(Part::Layout) if button == Button::Left => self.next_layout(),
            Some(Part::Start | Part::Layout) | None => {}
        }
    }

    /// Ask perspicax for the keyboard's next layout. What it switched to
    /// comes back on the channel, and is drawn then.
    fn next_layout(&self) {
        let (Some(channel), Some(next)) = (&self.channel, self.panels.layouts.next()) else {
            return;
        };
        if channel.version() >= perspicax_shell_v1::REQ_SET_LAYOUT_SINCE {
            channel.set_layout(next);
        }
    }
}

/// An assistive technology's click on a panel: on its start button, a task
/// or a workspace, sent on to the shell's loop as a left click there; or a
/// step or a setting of the mixer's volume.
struct Press {
    output: String,
    actions: Sender<Asked>,
}

impl accesskit::ActionHandler for Press {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        use accesskit::Action;

        let Some(part) = a11y::part_of(request.target_node) else {
            return;
        };
        let asked = match request.action {
            Action::Click => Asked::Panel {
                output: self.output.clone(),
                part,
            },
            #[cfg(feature = "audio")]
            Action::Increment | Action::Decrement | Action::SetValue => {
                match volume_asked(part, request.action, request.data) {
                    Some(change) => Asked::Volume(change),
                    None => return,
                }
            }
            _ => return,
        };
        if self.actions.send(asked).is_err() {
            tracing::debug!("the shell is stopping; the press is dropped");
        }
    }
}

/// The change to the volume `action` asks of `part` of a panel, if it is
/// one: a step up or down, or a setting, in percent.
#[cfg(feature = "audio")]
fn volume_asked(
    part: Part,
    action: accesskit::Action,
    data: Option<accesskit::ActionData>,
) -> Option<crate::model::sound::Change> {
    use accesskit::{Action, ActionData};
    use perspicax_config::mixer::Piece;

    use crate::model::sound::Change;

    if part != Part::Mixer(Piece::Volume) {
        return None;
    }
    match (action, data) {
        (Action::Increment, _) => Some(Change::By(1)),
        (Action::Decrement, _) => Some(Change::By(-1)),
        (Action::SetValue, Some(ActionData::NumericValue(value))) if value.is_finite() => Some(
            Change::To(value.round().clamp(0.0, f64::from(u32::MAX)) as u32),
        ),
        _ => None,
    }
}

#[cfg(all(test, feature = "audio"))]
mod tests {
    use accesskit::{Action, ActionData};
    use perspicax_config::mixer::Piece;

    use super::*;
    use crate::model::sound::Change;

    #[test]
    fn the_volume_sliders_steps_and_setting_are_changes_to_the_volume() {
        let volume = Part::Mixer(Piece::Volume);
        assert_eq!(
            volume_asked(volume, Action::Increment, None),
            Some(Change::By(1))
        );
        assert_eq!(
            volume_asked(volume, Action::Decrement, None),
            Some(Change::By(-1))
        );
        assert_eq!(
            volume_asked(
                volume,
                Action::SetValue,
                Some(ActionData::NumericValue(41.6))
            ),
            Some(Change::To(42))
        );
        assert_eq!(
            volume_asked(
                volume,
                Action::SetValue,
                Some(ActionData::NumericValue(-3.0))
            ),
            Some(Change::To(0)),
            "below silence is silence"
        );
        assert_eq!(
            volume_asked(
                volume,
                Action::SetValue,
                Some(ActionData::NumericValue(f64::NAN))
            ),
            None
        );
        assert_eq!(
            volume_asked(
                volume,
                Action::SetValue,
                Some(ActionData::Value("loud".into()))
            ),
            None
        );
        assert_eq!(
            volume_asked(Part::Mixer(Piece::Mute), Action::Increment, None),
            None,
            "only the volume steps"
        );
        assert_eq!(volume_asked(Part::Start, Action::Increment, None), None);
    }
}
