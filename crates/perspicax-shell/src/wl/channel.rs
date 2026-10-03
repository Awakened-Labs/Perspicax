//! `perspicax-shell-v1`, the channel from perspicax: the person asked for a
//! menu, the config changed. A changed config is applied in place; of the
//! menus the shell only says what it was told, until they are drawn.

use perspicax_protocols::shell::v1::client::perspicax_shell_v1::{self, PerspicaxShellV1};
use wayland_client::{Connection, Dispatch, QueueHandle, globals::GlobalList};

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
            Event::RootMenu { output, x, y } => {
                let output = output.and_then(|output| app.output_name(&output));
                tracing::info!(output, x, y, "asked for the root menu");
            }
            Event::Reconfigure => app.reconfigure(qh),
            Event::Finished => {
                tracing::info!("perspicax withdrew the shell channel");
                channel.destroy();
                app.channel = None;
            }
            _ => {}
        }
    }
}
