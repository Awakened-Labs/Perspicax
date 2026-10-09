//! `perspicax-shell-v1`, the channel from perspicax: the person asked for a
//! menu, the config changed, the keyboard's layouts. A changed config is
//! applied in place, and a protocol its rules give back is taken back; the
//! root menu opens where perspicax says the pointer is, and the start menu
//! from the start button of the monitor the pointer is on, or each closes if
//! it is the one open. The layouts, from version 2, are what the panel's
//! indicator shows. A pie, from version 3, opens by name where perspicax
//! says the pointer is.

use perspicax_protocols::shell::v1::client::perspicax_shell_v1::{self, PerspicaxShellV1};
use wayland_client::{Connection, Dispatch, QueueHandle, globals::GlobalList, protocol::wl_output};

use super::App;

/// Bind the channel, if this compositor offers it to us. One that does not
/// is not perspicax, or is a perspicax whose `[protocols] shell` rule leaves
/// this program out; the shell still draws, and its menus open only by
/// pointer.
pub(super) fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<PerspicaxShellV1> {
    let bound = globals.bind::<PerspicaxShellV1, _, _>(qh, 1..=3, ()).ok();
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
    /// perspicax asked for the pie `name` at `x`, `y` on `output`: the
    /// monitor the pointer is on, or `None` for one this client has not
    /// bound yet.
    fn pie_menu(&mut self, output: Option<wl_output::WlOutput>, x: i32, y: i32, name: &str) {
        #[cfg(feature = "pie")]
        {
            let Some(output) = output.or_else(|| self.outputs.outputs().next()) else {
                return;
            };
            let Some(monitor) = self.output_name(&output) else {
                return;
            };
            // The windows as they are now, before the pie takes the
            // keyboard and none of them has it.
            let windows: Vec<_> = self
                .panels
                .tasks
                .all()
                .map(|(serial, window)| (serial, window.app_id.clone(), window.title.clone()))
                .collect();
            let active = self
                .panels
                .tasks
                .all()
                .find(|(_, window)| window.active)
                .map(|(serial, _)| serial);
            let Some((size, slots)) = self.pies.built(name, &mut self.installed, &windows) else {
                tracing::warn!(name, "asked for a pie the config does not have");
                return;
            };
            let area = self.area(&output);
            self.pie_event(crate::pie::Event::Asked {
                name: name.to_owned(),
                output: monitor,
                area,
                size,
                at: (x, y),
                slots,
                active,
            });
        }
        #[cfg(not(feature = "pie"))]
        tracing::info!(
            output = output.and_then(|output| self.output_name(&output)),
            x,
            y,
            name,
            "asked for a pie, and this shell was built without pies"
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
            Event::PieMenu { output, x, y, name } => app.pie_menu(output, x, y, &name),
            Event::Reconfigure => {
                app.reconfigure(qh);
                #[cfg(any(feature = "panel", feature = "wallpaper"))]
                app.take_back(connection);
                #[cfg(not(any(feature = "panel", feature = "wallpaper")))]
                let _ = connection;
            }
            Event::Finished => {
                tracing::info!("perspicax withdrew the shell channel");
                channel.destroy();
                app.channel = None;
                #[cfg(feature = "menus")]
                app.menus.set_log_out(false);
            }
            #[cfg(feature = "panel")]
            Event::Layout { name, short, .. } => app.panels.layouts.layout(name, short),
            #[cfg(feature = "panel")]
            Event::LayoutsDone => {
                app.panels.layouts.done();
                app.panels_changed();
            }
            #[cfg(feature = "panel")]
            Event::ActiveLayout { index } => {
                app.panels.layouts.activate(index);
                app.panels_changed();
            }
            _ => {}
        }
    }
}
