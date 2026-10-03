//! The shell as a Wayland client: the globals it binds, the event loop, and
//! sctk's handlers, each passing what it was told to the part of the shell
//! it concerns.
//!
//! Thin on purpose. What the shell shows is worked out in `model` and
//! painted in `paint`; this module only finds out what there is to show it
//! on, and hands over the pixels.

mod channel;
#[cfg(feature = "wallpaper")]
mod desktop;

use std::path::PathBuf;

use perspicax_config::Shell;
use smithay_client_toolkit::{
    delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{self, EventLoop},
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
    let mut event_loop = EventLoop::<App>::try_new().map_err(|error| wayland(&error))?;
    WaylandSource::new(connection.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|error| wayland(&error.error))?;
    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        channel: channel::bind(&globals, &qh),
        #[cfg(feature = "wallpaper")]
        desktops: desktop::Desktops::new(&globals, &qh, &shell, config.as_deref())?,
        config,
    };
    #[cfg(not(feature = "wallpaper"))]
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
    #[cfg(feature = "wallpaper")]
    desktops: desktop::Desktops,
    /// The config file, read again when perspicax says it changed.
    config: Option<PathBuf>,
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
        self.desktops
            .reconfigure(qh, &shell, self.config.as_deref(), &self.outputs);
        #[cfg(not(feature = "wallpaper"))]
        let _ = (qh, shell);
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
        self.desktops.add(qh, output, &info);
        #[cfg(not(feature = "wallpaper"))]
        let _ = qh;
    }

    fn update_output(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        #[cfg(feature = "wallpaper")]
        if let Some(info) = self.outputs.info(&output) {
            self.desktops.rescale(&output, info.scale_factor);
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
    registry_handlers![OutputState];
}

delegate_output!(App);
delegate_registry!(App);
