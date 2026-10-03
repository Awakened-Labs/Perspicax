//! `perspicax-shell-v1`, the channel from perspicax: the person asked for a
//! menu, the config changed. A changed config is applied in place; the root
//! menu opens where perspicax says the pointer is, or closes if it is open.

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

impl Dispatch<PerspicaxShellV1, ()> for App {
    fn event(
        app: &mut Self,
        channel: &PerspicaxShellV1,
        event: perspicax_shell_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use perspicax_shell_v1::Event;
        match event {
            Event::StartMenu { output } => {
                let output = output.and_then(|output| app.output_name(&output));
                tracing::info!(output, "asked for the start menu");
            }
            Event::RootMenu { output, x, y } => app.root_menu(output, x, y),
            Event::Reconfigure => app.reconfigure(qh),
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
