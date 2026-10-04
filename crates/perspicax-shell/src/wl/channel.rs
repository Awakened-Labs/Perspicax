//! `perspicax-shell-v1`, the channel from perspicax: the person asked for a
//! menu, the config changed. A changed config is applied in place, and a
//! protocol its rules give back is taken back; the root
//! menu opens where perspicax says the pointer is, and the start menu from
//! the start button of the monitor the pointer is on, or each closes if it
//! is the one open.

use perspicax_protocols::shell::v1::client::perspicax_shell_v1::{self, PerspicaxShellV1};
use wayland_client::{Connection, Dispatch, QueueHandle, globals::GlobalList, protocol::wl_output};

use super::App;

/// Bind the channel, if this compositor offers it to us. One that does not
/// is not perspicax, or is a perspicax whose `[protocols] shell` rule leaves
/// this program out; the shell still draws, and its menus open only by
/// pointer.
pub(super) fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<PerspicaxShellV1> {
    let bound = globals.bind::<PerspicaxShellV1, _, _>(qh, 1..=1, ()).ok();
    if bound.is_none() {
        tracing::info!("the compositor offers no perspicax_shell_v1; no menu will open on a key");
    }
    bound
}

impl App {
    /// perspicax asked for the root menu at `x`, `y` on `output`: the
    /// monitor the pointer is on, or `None` for one this client has not
    /// bound yet, when the first monitor stands in.
    fn root_menu(&mut self, output: Option<wl_output::WlOutput>, x: i32, y: i32) {
        #[cfg(feature = "menus")]
        {
            let Some(output) = output.or_else(|| self.outputs.outputs().next()) else {
                return;
            };
            let Some(name) = self.output_name(&output) else {
                return;
            };
            let area = self.area(&output);
            self.menu_event(crate::update::Event::RootMenu {
                output: name,
                area,
                at: (x, y),
            });
        }
        #[cfg(not(feature = "menus"))]
        tracing::info!(
            output = output.and_then(|output| self.output_name(&output)),
            x,
            y,
            "asked for the root menu, and this shell was built without menus"
        );
    }
}

impl App {
    /// perspicax asked for the start menu on `output`: the monitor the
    /// pointer is on, or `None` for one this client has not bound yet, when
    /// the first monitor stands in.
    fn start_menu_on(&mut self, output: Option<wl_output::WlOutput>) {
        #[cfg(feature = "menus")]
        if let Some(output) = output.or_else(|| self.outputs.outputs().next()) {
            self.start_menu(&output);
        }
        #[cfg(not(feature = "menus"))]
        tracing::info!(
            output = output.and_then(|output| self.output_name(&output)),
            "asked for the start menu, and this shell was built without menus"
        );
    }
}

impl Dispatch<PerspicaxShellV1, ()> for App {
    fn event(
        app: &mut Self,
        channel: &PerspicaxShellV1,
        event: perspicax_shell_v1::Event,
        _: &(),
        connection: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use perspicax_shell_v1::Event;
        match event {
            Event::StartMenu { output } => app.start_menu_on(output),
            Event::RootMenu { output, x, y } => app.root_menu(output, x, y),
            Event::Reconfigure => {
                app.reconfigure(qh);
                #[cfg(feature = "panel")]
                app.take_back(connection);
                #[cfg(not(feature = "panel"))]
                let _ = connection;
            }
            Event::Finished => {
                tracing::info!("perspicax withdrew the shell channel");
                channel.destroy();
                app.channel = None;
                #[cfg(feature = "menus")]
                app.menus.set_log_out(false);
            }
            _ => {}
        }
    }
}
