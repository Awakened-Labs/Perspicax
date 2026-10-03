//! The shell as a Wayland client: the globals it binds, the event loop, and
//! sctk's handlers, each passing what it was told to the part of the shell
//! it concerns.
//!
//! Thin on purpose. What the shell shows is worked out in `model`, `layout`
//! and `update`, and painted in `paint`; this module only finds out what
//! there is to show it on, hands over the pixels, and passes on what the
//! person did.

#[cfg(any(feature = "wallpaper", feature = "menus"))]
mod canvas;
mod channel;
#[cfg(feature = "wallpaper")]
mod desktop;
#[cfg(feature = "menus")]
mod menu;
#[cfg(feature = "menus")]
mod seat;

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

/// Run the shell on `connection` until the compositor goes away.
pub(crate) fn run(
    connection: Connection,
    shell: Shell,
    config: Option<PathBuf>,
) -> Result<(), Error> {
    let (globals, queue) =
        registry_queue_init::<App>(&connection).map_err(|error| wayland(&error))?;
    let qh = queue.handle();
    let mut event_loop = EventLoop::<'static, App>::try_new().map_err(|error| wayland(&error))?;
    WaylandSource::new(connection.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|error| wayland(&error.error))?;
    let channel = channel::bind(&globals, &qh);
    #[cfg(feature = "menus")]
    let actions = {
        // What an assistive technology asks of a menu arrives on AccessKit's
        // thread, and is handled here, in the loop, like anything else.
        let (sender, receiver) = calloop::channel::channel();
        event_loop
            .handle()
            .insert_source(receiver, |event, (), app: &mut App| {
                if let calloop::channel::Event::Msg(event) = event {
                    app.menu_event(event);
                }
            })
            .map_err(|error| wayland(&error.error))?;
        sender
    };
    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        #[cfg(any(feature = "wallpaper", feature = "menus"))]
        canvas: canvas::Canvas::bind(&globals, &qh)?,
        #[cfg(feature = "wallpaper")]
        desktops: desktop::Desktops::new(&shell, config.as_deref()),
        #[cfg(feature = "menus")]
        seat: seat::Seat::new(&globals, &qh),
        #[cfg(feature = "menus")]
        menus: menu::Menus::new(&shell, config.as_deref(), actions, channel.is_some()),
        #[cfg(feature = "menus")]
        reaping: false,
        channel,
        config,
        handle: event_loop.handle(),
        qh,
    };
    #[cfg(not(any(feature = "wallpaper", feature = "menus")))]
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
    #[cfg(any(feature = "wallpaper", feature = "menus"))]
    canvas: canvas::Canvas,
    #[cfg(feature = "wallpaper")]
    desktops: desktop::Desktops,
    #[cfg(feature = "menus")]
    seat: seat::Seat,
    #[cfg(feature = "menus")]
    menus: menu::Menus,
    /// Whether a timer is collecting the programs the menus started.
    #[cfg(feature = "menus")]
    reaping: bool,
    /// The config file, read again when perspicax says it changed.
    config: Option<PathBuf>,
    #[cfg_attr(
        not(feature = "menus"),
        expect(dead_code, reason = "the menus' timers and keyboard")
    )]
    handle: LoopHandle<'static, App>,
    #[cfg_attr(
        not(feature = "menus"),
        expect(dead_code, reason = "the menus' surface, made from the loop")
    )]
    qh: QueueHandle<App>,
}

impl App {
    /// A monitor's connector name, `DP-1`, as the compositor gave it.
    fn output_name(&self, output: &wl_output::WlOutput) -> Option<String> {
        self.outputs.info(output).and_then(|info| info.name)
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
        #[cfg(feature = "wallpaper")]
        self.desktops.reconfigure(
            &mut self.canvas,
            qh,
            &shell,
            self.config.as_deref(),
            &self.outputs,
        );
        #[cfg(feature = "menus")]
        self.menus.reconfigure(&shell, self.config.as_deref());
        #[cfg(not(feature = "wallpaper"))]
        let _ = (qh, shell);
    }
}

#[cfg(feature = "menus")]
impl App {
    /// Pass `event` to the menus, and carry out what it calls for.
    fn menu_event(&mut self, event: crate::update::Event) {
        use crate::update::Effect;
        let qh = self.qh.clone();
        let effects = self.menus.send(&mut self.canvas, &qh, &self.outputs, event);
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
                Effect::Redraw => {}
            }
        }
    }

    /// The menus' area on `output`: the whole monitor, in its logical
    /// pixels.
    fn area(&self, output: &wl_output::WlOutput) -> crate::layout::Rect {
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
        self.desktops.add(&self.canvas, qh, output, &info);
        #[cfg(not(feature = "wallpaper"))]
        let _ = (qh, output);
    }

    fn update_output(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        #[cfg(feature = "wallpaper")]
        if let Some(info) = self.outputs.info(&output) {
            self.desktops
                .rescale(&mut self.canvas, &output, info.scale_factor);
        }
        #[cfg(not(feature = "wallpaper"))]
        let _ = output;
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        #[cfg(feature = "wallpaper")]
        self.desktops.remove(&output);
        #[cfg(not(feature = "wallpaper"))]
        let _ = output;
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    #[cfg(feature = "menus")]
    registry_handlers![OutputState, smithay_client_toolkit::seat::SeatState];
    #[cfg(not(feature = "menus"))]
    registry_handlers![OutputState];
}

delegate_output!(App);
delegate_registry!(App);
