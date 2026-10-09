//! The shell as a Wayland client: the globals it binds, the event loop, and
//! sctk's handlers, each passing what it was told to the part of the shell
//! it concerns.
//!
//! Thin on purpose. What the shell shows is worked out in `model`, `layout`
//! and `update`, and painted in `paint`; this module only finds out what
//! there is to show it on, hands over the pixels, and passes on what the
//! person did.

#[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
mod canvas;
mod channel;
#[cfg(feature = "wallpaper")]
mod desktop;
#[cfg(any(feature = "menus", feature = "panel"))]
mod installed;
#[cfg(feature = "menus")]
mod menu;
#[cfg(any(feature = "panel", feature = "wallpaper"))]
mod pager;
#[cfg(feature = "panel")]
mod panel;
#[cfg(feature = "pie")]
mod pie;
#[cfg(any(feature = "menus", feature = "panel"))]
mod seat;
#[cfg(feature = "panel")]
mod taskbar;
#[cfg(feature = "tray")]
mod tray;

use std::path::PathBuf;

use perspicax_config::Shell;
use smithay_client_toolkit::{
    delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{self, EventLoop, LoopHandle},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
};
use wayland_client::{
    Connection, QueueHandle, backend::WaylandError, globals::registry_queue_init,
    protocol::wl_output,
};

use crate::Error;

/// Run the shell on `connection` until the compositor goes away, with its
/// tray on the bus at address `bus`, or the session's.
pub(crate) fn run(
    connection: Connection,
    shell: Shell,
    config: Option<PathBuf>,
    bus: Option<String>,
) -> Result<(), Error> {
    let (globals, queue) =
        registry_queue_init::<App>(&connection).map_err(|error| wayland(&error))?;
    let qh = queue.handle();
    let mut event_loop = EventLoop::<'static, App>::try_new().map_err(|error| wayland(&error))?;
    WaylandSource::new(connection.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|error| wayland(&error.error))?;
    let channel = channel::bind(&globals, &qh);
    #[cfg(any(feature = "menus", feature = "panel"))]
    let actions = {
        // What an assistive technology asks of a menu or a panel arrives on
        // AccessKit's thread, and is handled here, in the loop, like
        // anything else.
        let (sender, receiver) = calloop::channel::channel();
        event_loop
            .handle()
            .insert_source(receiver, |event, (), app: &mut App| {
                if let calloop::channel::Event::Msg(asked) = event {
                    app.asked(asked);
                }
            })
            .map_err(|error| wayland(&error.error))?;
        sender
    };
    #[cfg(feature = "tray")]
    let news = {
        // What the tray reads arrives on its thread, and is shown here.
        let (sender, receiver) = calloop::channel::channel();
        event_loop
            .handle()
            .insert_source(receiver, |event, (), app: &mut App| {
                if let calloop::channel::Event::Msg((started, news)) = event {
                    app.tray_news(started, news);
                }
            })
            .map_err(|error| wayland(&error.error))?;
        sender
    };
    #[cfg(not(feature = "tray"))]
    let _ = bus;
    #[cfg(any(feature = "menus", feature = "panel"))]
    let installed = installed::Installed::new(crate::model::apps::Places::from_env());
    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        #[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
        canvas: canvas::Canvas::bind(&globals, &qh)?,
        #[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
        kit: crate::paint::Kit {
            #[cfg(any(feature = "menus", feature = "panel"))]
            fonts: crate::paint::text::Fonts::find(shell.font.clone()),
            #[cfg(any(feature = "menus", feature = "panel"))]
            images: images(shell.icon_theme.as_deref(), installed.places()),
            #[cfg(any(feature = "menus", feature = "panel"))]
            palette: shell.palette,
        },
        #[cfg(all(feature = "wallpaper", not(feature = "icons")))]
        desktops: desktop::Desktops::new(&shell, config.as_deref()),
        #[cfg(feature = "icons")]
        desktops: desktop::Desktops::new(&shell, config.as_deref(), actions.clone()),
        #[cfg(feature = "icons")]
        watching: false,
        #[cfg(any(feature = "panel", feature = "wallpaper"))]
        workspaces: pager::Workspaces::bind(&globals, &qh),
        #[cfg(feature = "panel")]
        panels: panel::Panels::new(&shell, actions.clone(), taskbar::bind(&globals, &qh)),
        #[cfg(feature = "panel")]
        ticking: None,
        #[cfg(feature = "tray")]
        tray: tray::Tray::new(bus, news),
        #[cfg(any(feature = "menus", feature = "panel"))]
        seat: seat::Seat::new(&globals, &qh),
        #[cfg(feature = "menus")]
        menus: menu::Menus::new(
            &shell,
            config.as_deref(),
            installed.places(),
            actions,
            channel.is_some(),
        ),
        #[cfg(feature = "pie")]
        pies: pie::Pies::new(&shell, config.as_deref()),
        #[cfg(any(feature = "menus", feature = "panel"))]
        installed,
        #[cfg(feature = "menus")]
        reaping: false,
        channel,
        config,
        handle: event_loop.handle(),
        qh,
    };
    #[cfg(feature = "panel")]
    app.keep_time();
    #[cfg(feature = "icons")]
    app.watch_folder();
    #[cfg(feature = "tray")]
    app.follow_tray(&shell);
    #[cfg(not(any(feature = "wallpaper", feature = "menus", feature = "panel")))]
    let _ = shell;
    loop {
        if let Err(error) = event_loop.dispatch(None, &mut app) {
            return ended(&connection, &error);
        }
    }
}

/// The loop stopped. The compositor going away is how a session ends, so a
/// connection that broke is no error; one the compositor closed over a
/// request it refused is, and so is anything else.
fn ended(connection: &Connection, error: &calloop::Error) -> Result<(), Error> {
    match connection.flush() {
        Err(WaylandError::Protocol(refused)) => Err(Error::Protocol(refused)),
        Err(WaylandError::Io(gone)) => {
            tracing::info!("the compositor went away ({gone}); stopping");
            Ok(())
        }
        Ok(()) => Err(wayland(error)),
    }
}

fn wayland(error: &impl std::fmt::Display) -> Error {
    Error::Wayland(error.to_string())
}

/// Everything the shell holds while it runs.
pub(crate) struct App {
    registry: RegistryState,
    outputs: OutputState,
    /// The channel to perspicax, if this compositor offers it to us.
    channel: Option<perspicax_protocols::shell::v1::client::perspicax_shell_v1::PerspicaxShellV1>,
    #[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
    canvas: canvas::Canvas,
    /// The fonts and icons the menus, the panels and the desktop folder's
    /// icons are drawn with.
    #[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
    kit: crate::paint::Kit,
    /// The installed applications, which the menus list and among which
    /// the taskbar finds each window's icon.
    #[cfg(any(feature = "menus", feature = "panel"))]
    installed: installed::Installed,
    /// The workspaces, which the pagers show and the wallpapers follow.
    #[cfg(any(feature = "panel", feature = "wallpaper"))]
    workspaces: pager::Workspaces,
    #[cfg(feature = "wallpaper")]
    desktops: desktop::Desktops,
    /// Whether a timer is looking at the desktop folder for changes.
    #[cfg(feature = "icons")]
    watching: bool,
    #[cfg(feature = "panel")]
    panels: panel::Panels,
    /// The timer that moves the clock on.
    #[cfg(feature = "panel")]
    ticking: Option<calloop::RegistrationToken>,
    /// Other programs' status icons, from the session bus.
    #[cfg(feature = "tray")]
    tray: tray::Tray,
    #[cfg(any(feature = "menus", feature = "panel"))]
    seat: seat::Seat,
    #[cfg(feature = "menus")]
    menus: menu::Menus,
    #[cfg(feature = "pie")]
    pies: pie::Pies,
    /// Whether a timer is collecting the programs the menus started.
    #[cfg(feature = "menus")]
    reaping: bool,
    /// The config file, read again when perspicax says it changed.
    config: Option<PathBuf>,
    #[cfg_attr(
        not(any(feature = "menus", feature = "panel")),
        expect(dead_code, reason = "the menus' and the clock's timers")
    )]
    handle: LoopHandle<'static, App>,
    #[cfg_attr(
        not(any(feature = "menus", feature = "panel", feature = "wallpaper")),
        expect(
            dead_code,
            reason = "the menus' surface, and the panels' and wallpapers' protocols, from the loop"
        )
    )]
    qh: QueueHandle<App>,
}

/// What an assistive technology asked of the shell, passed from AccessKit's
/// thread to the loop.
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) enum Asked {
    /// Something of the open menus: to choose an item, or move to one.
    #[cfg(feature = "menus")]
    Menu(crate::update::Event),
    /// To press `part` of the panel of the monitor of connector name
    /// `output`, as a left click would.
    #[cfg(feature = "panel")]
    Panel {
        output: String,
        part: crate::layout::panel::Part,
    },
    /// To open the desktop folder's icon that is this node of its tree.
    #[cfg(feature = "icons")]
    OpenIcon(accesskit::NodeId),
    /// To select it.
    #[cfg(feature = "icons")]
    SelectIcon(accesskit::NodeId),
}

/// The icon theme named `theme`, looked for in the data folders of
/// `places` and the home folder.
#[cfg(any(feature = "menus", feature = "panel"))]
fn images(theme: Option<&str>, places: &crate::model::apps::Places) -> crate::paint::icons::Images {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    crate::paint::icons::Images::new(theme, &places.data, home.as_deref())
}

impl App {
    /// A monitor's connector name, `DP-1`, as the compositor gave it.
    fn output_name(&self, output: &wl_output::WlOutput) -> Option<String> {
        self.outputs.info(output).and_then(|info| info.name)
    }

    /// The monitor of connector name `name`.
    #[cfg(all(feature = "menus", feature = "panel"))]
    fn output_named(&self, name: &str) -> Option<wl_output::WlOutput> {
        self.outputs
            .outputs()
            .find(|output| self.output_name(output).as_deref() == Some(name))
    }

    /// The config file changed: read it again, and show what it says now on
    /// the surfaces already there. A file that can no longer be used is
    /// reported, and the desktop stays as it is: a typo mid-session must not
    /// take the wallpaper away.
    fn reconfigure(&mut self, qh: &QueueHandle<Self>) {
        let shell = match crate::read(self.config.as_deref()) {
            Ok(shell) => shell,
            Err(error) => {
                tracing::warn!(
                    "the config changed and cannot be used: {error}; keeping what is shown"
                );
                return;
            }
        };
        tracing::info!("the config changed; applying it");
        #[cfg(any(feature = "menus", feature = "panel"))]
        let theme_changed = shell.icon_theme.as_deref() != self.kit.images.theme();
        #[cfg(any(feature = "menus", feature = "panel"))]
        if theme_changed {
            self.kit.images = images(shell.icon_theme.as_deref(), self.installed.places());
        }
        // What everything is drawn in. The menus are drawn afresh each time
        // one opens; the panels and the desktop's icons are drawn again below.
        #[cfg(any(feature = "menus", feature = "panel"))]
        let looks_changed = {
            let palette_changed = shell.palette != self.kit.palette;
            self.kit.palette = shell.palette;
            let font_changed = self.kit.fonts.set_font(&shell.font);
            palette_changed || font_changed
        };
        #[cfg(all(feature = "menus", not(any(feature = "panel", feature = "icons"))))]
        let _ = looks_changed;
        #[cfg(feature = "wallpaper")]
        {
            self.desktops.reconfigure(
                &mut self.canvas,
                &mut self.kit,
                qh,
                &shell,
                self.config.as_deref(),
                &self.outputs,
            );
            // Desktops put back with the wallpaper turned on again.
            self.follow_workspaces();
        }
        // The desktop folder's icons, from the new icon theme or in the new
        // colours.
        #[cfg(feature = "icons")]
        if theme_changed || looks_changed {
            self.desktops.draw_icons(&mut self.canvas, &mut self.kit);
        }
        #[cfg(feature = "panel")]
        {
            self.panels.reconfigure(
                &mut self.canvas,
                qh,
                &shell,
                &self.outputs,
                &mut self.kit,
                &self.workspaces.model,
            );
            self.keep_time();
            // The taskbar's icons, from the new icon theme, or everything in
            // the new colours. The menus are drawn afresh each time one opens.
            if theme_changed || looks_changed {
                self.panels_changed();
            }
        }
        #[cfg(feature = "tray")]
        self.follow_tray(&shell);
        #[cfg(feature = "menus")]
        self.menus.reconfigure(&shell, self.config.as_deref());
        #[cfg(feature = "pie")]
        self.pies.reconfigure(&shell, self.config.as_deref());
        self.monitors_changed();
        #[cfg(feature = "icons")]
        self.watch_folder();
        #[cfg(not(any(feature = "wallpaper", feature = "panel")))]
        let _ = (qh, shell);
    }

    /// The monitors, or the panels on them, changed: tell the menus where
    /// the panels are now, to keep off them, and put the desktop folder's
    /// icons on the first monitor, clear of its panel.
    fn monitors_changed(&mut self) {
        #[cfg(all(feature = "menus", feature = "panel"))]
        {
            let reserved = self
                .outputs
                .outputs()
                .filter_map(|output| {
                    let name = self.output_name(&output)?;
                    let strip = self.panels.strip(&name, self.monitor(&output))?;
                    Some((name, strip))
                })
                .collect();
            self.menus.reserve(reserved);
        }
        #[cfg(feature = "icons")]
        {
            let monitors: Vec<desktop::Monitor> = self
                .outputs
                .outputs()
                .filter_map(|output| {
                    let info = self.outputs.info(&output)?;
                    let name = info.name?;
                    let at = info.logical_position.unwrap_or((i32::MAX, i32::MAX));
                    #[cfg(feature = "panel")]
                    let strip = self.panels.strip(&name, self.monitor(&output));
                    #[cfg(not(feature = "panel"))]
                    let strip = None;
                    Some((name, at, strip))
                })
                .collect();
            self.desktops
                .arrange(&mut self.canvas, &mut self.kit, &monitors);
        }
    }
}

#[cfg(feature = "panel")]
impl App {
    /// Read the clock now, and again whenever what it shows next changes.
    fn keep_time(&mut self) {
        use calloop::timer::{TimeoutAction, Timer};

        if let Some(ticking) = self.ticking.take() {
            self.handle.remove(ticking);
        }
        let next = self
            .panels
            .tick(&mut self.canvas, &mut self.kit, &self.workspaces.model);
        let ticking = self
            .handle
            .insert_source(Timer::from_duration(next), |_, (), app| {
                TimeoutAction::ToDuration(app.panels.tick(
                    &mut app.canvas,
                    &mut app.kit,
                    &app.workspaces.model,
                ))
            });
        match ticking {
            Ok(ticking) => self.ticking = Some(ticking),
            Err(error) => tracing::warn!("the clock will stand still: {error}"),
        }
    }
}

#[cfg(any(feature = "menus", feature = "panel"))]
impl App {
    /// Do what an assistive technology asked.
    fn asked(&mut self, asked: Asked) {
        match asked {
            #[cfg(feature = "menus")]
            Asked::Menu(event) => self.menu_event(event),
            #[cfg(feature = "panel")]
            Asked::Panel { output, part } => {
                self.panel_pressed(&output, Some(part), crate::model::Button::Left);
            }
            #[cfg(feature = "icons")]
            Asked::OpenIcon(node) => {
                if let Some(run) = self.desktops.open(node) {
                    self.start(&run);
                }
            }
            #[cfg(feature = "icons")]
            Asked::SelectIcon(node) => {
                self.desktops.select(&mut self.canvas, &mut self.kit, node);
            }
        }
    }
}

#[cfg(feature = "menus")]
impl App {
    /// Open the start menu on `output`, or close it, from its start button:
    /// the one on that monitor's panel, or where one would be on a monitor
    /// with none.
    fn start_menu(&mut self, output: &wl_output::WlOutput) {
        use perspicax_config::Edge;

        use crate::layout::{Rect, usable};

        let Some(name) = self.output_name(output) else {
            return;
        };
        let monitor = self.monitor(output);
        let area = usable(monitor, self.menus.reserved_on(&name));
        #[cfg(feature = "panel")]
        let (button, edge) = (self.panels.start_button(&name, monitor), self.panels.edge());
        #[cfg(not(feature = "panel"))]
        let (button, edge) = (None, None);
        let edge = edge.unwrap_or(Edge::Bottom);
        let button = button.unwrap_or(match edge {
            Edge::Bottom => Rect::new(area.x, area.bottom(), 0, 0),
            Edge::Top => Rect::new(area.x, area.y, 0, 0),
        });
        self.menu_event(crate::update::Event::StartMenu {
            output: name,
            area,
            button,
            edge,
        });
    }

    /// Pass `event` to the menus, and carry out what it calls for.
    fn menu_event(&mut self, event: crate::update::Event) {
        use crate::update::Effect;
        let qh = self.qh.clone();
        let effects = self.menus.send(
            &mut self.canvas,
            &qh,
            &self.outputs,
            &mut self.kit,
            &mut self.installed,
            event,
        );
        #[cfg(feature = "panel")]
        self.panels.set_open(
            &mut self.canvas,
            &mut self.kit,
            &self.workspaces.model,
            self.menus.start_open_on(),
        );
        #[cfg(feature = "tray")]
        self.menu_opened(self.menus.open_menu());
        for effect in effects {
            match effect {
                Effect::Run(run) => self.start(&run),
                Effect::LogOut => match &self.channel {
                    Some(channel) => {
                        tracing::info!("logging out");
                        channel.exit_session();
                    }
                    None => tracing::warn!("Log Out was chosen with no compositor to ask"),
                },
                #[cfg(feature = "tray")]
                Effect::Tell { key, id } => self.tell_status(key, id),
                Effect::Redraw => {}
            }
        }
    }

    /// Pass `event` to the pie, and carry out what it calls for.
    #[cfg(feature = "pie")]
    fn pie_event(&mut self, event: crate::pie::Event) {
        let qh = self.qh.clone();
        let effects = self
            .pies
            .send(&mut self.canvas, &qh, &self.outputs, &mut self.kit, event);
        for effect in effects {
            match effect {
                crate::pie::Effect::Run(run) => self.start(&run),
                crate::pie::Effect::Redraw => {}
            }
        }
    }

    /// The menus' area on `output`: the monitor, less the shell's panel on
    /// it.
    fn area(&self, output: &wl_output::WlOutput) -> crate::layout::Rect {
        let monitor = self.monitor(output);
        let strip = self
            .output_name(output)
            .and_then(|name| self.menus.reserved_on(&name));
        crate::layout::usable(monitor, strip)
    }

    /// The whole of `output`, in its logical pixels.
    fn monitor(&self, output: &wl_output::WlOutput) -> crate::layout::Rect {
        let info = self.outputs.info(output);
        let (width, height) = info
            .as_ref()
            .and_then(|info| info.logical_size)
            .or_else(|| {
                let info = info.as_ref()?;
                let mode = info.modes.iter().find(|mode| mode.current)?;
                let scale = info.scale_factor.max(1);
                Some((mode.dimensions.0 / scale, mode.dimensions.1 / scale))
            })
            .unwrap_or((0, 0));
        crate::layout::Rect::new(0, 0, width, height)
    }

    /// Start a program a menu named, and collect it when it ends.
    fn start(&mut self, run: &crate::model::apps::Run) {
        use std::time::Duration;

        use calloop::timer::{TimeoutAction, Timer};

        if let Err(error) = self.menus.launcher.run(run) {
            tracing::warn!("{} could not be started: {error}", run.argv.join(" "));
            return;
        }
        if self.reaping {
            return;
        }
        const EVERY: Duration = Duration::from_secs(1);
        let reaping = self
            .handle
            .insert_source(Timer::from_duration(EVERY), |_, (), app| {
                if app.menus.launcher.reap() {
                    TimeoutAction::ToDuration(EVERY)
                } else {
                    app.reaping = false;
                    TimeoutAction::Drop
                }
            });
        match reaping {
            Ok(_) => self.reaping = true,
            Err(error) => tracing::warn!("ended programs will not be collected: {error}"),
        }
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(info) = self.outputs.info(&output) else {
            return;
        };
        tracing::debug!(output = info.name.as_deref(), "a monitor");
        #[cfg(feature = "wallpaper")]
        {
            self.desktops.add(&self.canvas, qh, output, &info);
            // The compositor may have told which workspace it shows before
            // it told of the monitor.
            self.follow_workspaces();
        }
        #[cfg(feature = "panel")]
        self.panels.sync(&self.canvas, qh, &self.outputs, None);
        self.monitors_changed();
        #[cfg(not(feature = "wallpaper"))]
        let _ = (qh, output);
    }

    fn update_output(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        #[cfg(feature = "wallpaper")]
        if let Some(info) = self.outputs.info(&output) {
            self.desktops
                .rescale(&mut self.canvas, &mut self.kit, &output, info.scale_factor);
        }
        #[cfg(feature = "panel")]
        {
            if let Some(info) = self.outputs.info(&output) {
                self.panels.rescale(
                    &mut self.canvas,
                    &mut self.kit,
                    &self.workspaces.model,
                    &output,
                    info.scale_factor,
                );
            }
            // A monitor moved may be the first now, or no longer.
            self.panels.sync(&self.canvas, qh, &self.outputs, None);
        }
        self.monitors_changed();
        #[cfg(not(any(feature = "wallpaper", feature = "panel")))]
        let _ = output;
        #[cfg(not(feature = "panel"))]
        let _ = qh;
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        #[cfg(feature = "wallpaper")]
        self.desktops.remove(&output);
        #[cfg(any(feature = "panel", feature = "wallpaper"))]
        self.workspaces.model.gone(&output);
        #[cfg(feature = "panel")]
        {
            self.panels
                .sync(&self.canvas, qh, &self.outputs, Some(&output));
            self.panels.gone(&output);
            self.panels_changed();
        }
        self.monitors_changed();
        #[cfg(not(any(feature = "wallpaper", feature = "panel")))]
        let _ = output;
        #[cfg(not(feature = "panel"))]
        let _ = qh;
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    #[cfg(any(feature = "menus", feature = "panel"))]
    registry_handlers![OutputState, smithay_client_toolkit::seat::SeatState];
    #[cfg(not(any(feature = "menus", feature = "panel")))]
    registry_handlers![OutputState];
}

delegate_output!(App);
delegate_registry!(App);
