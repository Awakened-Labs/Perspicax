//! The panels: one surface on the `top` layer of each monitor the config
//! names, along its bottom or its top, with an exclusive zone as tall as it
//! is, so that windows are kept out of the strip it takes. A fullscreen
//! window in use still covers it, as perspicax arranges. The surface is the
//! whole strip even when the bar drawn on it is narrower: the rest is clear
//! and takes no clicks, so a click there reaches the desktop under it.
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

use std::{
    collections::HashMap,
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
}

impl Panels {
    pub(super) fn new(
        shell: &Shell,
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
            tasks: Tasks::default(),
            taskbar,
            icons: HashMap::new(),
            icons_from: 0,
            #[cfg(feature = "tray")]
            tray: Vec::new(),
            layouts: Layouts::default(),
            stale: false,
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
        kit: &mut Kit,
        workspaces: &Model,
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
                    placed: Placed::default(),
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
        let open = self.open_on.as_deref() == Some(bar.name.as_str());
        let shown = paint::panel::Shown {
            size: (width, height),
            edge: panel.edge,
            placed: &bar.placed,
            time: &self.time,
            layout: layout.map(|layout| layout.short.as_str()),
            open,
            #[cfg(feature = "tray")]
            tray: &self.tray,
        };
        let drawn = bar.placed.bar;
        let area = (drawn.x, drawn.y, drawn.w, drawn.h);
        canvas.take_clicks_in(&bar.layer, area);
        let started = Instant::now();
        // The bar wholly opaque, which lets the compositor skip what is under
        // it, and the rest of the strip clear.
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
    }
}

/// What a window's task is called: its title, or with none, its app id.
fn title<O>(window: &Window<O>) -> String {
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
            if app.panels.icon_unknown() {
                let read = app.installed.refresh();
                app.panels.find_icons(read, app.installed.apps());
            }
            app.panels
                .draw_if_stale(&mut app.canvas, &mut app.kit, &app.workspaces.model);
        });
    }

    /// A button went down on the panel of the monitor named `name`, on
    /// `part` of it, or on none.
    pub(super) fn panel_pressed(&mut self, name: &str, part: Option<Part>, button: Button) {
        #[cfg(feature = "menus")]
        {
            if (part, button) == (Some(Part::Start), Button::Left) {
                if let Some(output) = self.output_named(name) {
                    self.start_menu(&output);
                }
                return;
            }
            #[cfg(feature = "tray")]
            let was_open = self.menus.open_menu();
            // Anything else on a panel closes the menus, as a press anywhere
            // off them does; a press on the tray icon whose menu is open
            // does only that.
            self.menu_event(crate::update::Event::PanelPress);
            #[cfg(feature = "tray")]
            if let Some(Part::Tray(key)) = part
                && was_open == Some(crate::update::Which::Tray(key))
            {
                return;
            }
        }
        #[cfg(not(feature = "menus"))]
        let _ = name;
        match part {
            Some(Part::Task(serial)) => self.press_task(serial, button),
            Some(Part::Workspace(serial)) => self.press_workspace(serial, button),
            #[cfg(feature = "tray")]
            Some(Part::Tray(key)) => self.press_status(name, key, button),
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
/// or a workspace, sent on to the shell's loop as a left click there.
struct Press {
    output: String,
    actions: Sender<Asked>,
}

impl accesskit::ActionHandler for Press {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        if request.action != accesskit::Action::Click {
            return;
        }
        let Some(part) = a11y::part_of(request.target_node) else {
            return;
        };
        let asked = Asked::Panel {
            output: self.output.clone(),
            part,
        };
        if self.actions.send(asked).is_err() {
            tracing::debug!("the shell is stopping; the press is dropped");
        }
    }
}
