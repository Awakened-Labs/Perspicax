//! `wlr-foreign-toplevel-management`: the windows, as the compositor tells
//! a taskbar them, and what a press on a task asks of its window.
//!
//! What waybar's `wlr/taskbar` speaks. perspicax offers it to any program
//! unless `[protocols]` says otherwise; without it the panels list no
//! windows, and are otherwise as they were. Bringing a window forward,
//! putting it away and closing it are the compositor's to do, as a person's
//! click or key would.

use wayland_client::{Connection, Dispatch, QueueHandle, event_created_child, globals::GlobalList};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::App;
use crate::model::{
    Button,
    tasks::{Ask, Told},
};

/// Bind the taskbar's protocol, if the compositor offers it to us.
pub(super) fn bind(
    globals: &GlobalList,
    qh: &QueueHandle<App>,
) -> Option<ZwlrForeignToplevelManagerV1> {
    let bound = globals.bind(qh, 1..=3, ()).ok();
    if bound.is_none() {
        tracing::info!(
            "the compositor offers no zwlr_foreign_toplevel_manager_v1; the taskbar lists no windows"
        );
    }
    bound
}

impl App {
    /// A button went down on the task of window `serial`.
    pub(super) fn press_task(&mut self, serial: u64, button: Button) {
        let Some((handle, ask)) = self.panels.tasks.pressed(serial, button) else {
            return;
        };
        match ask {
            Ask::Activate => match self.seat.state.seats().next() {
                Some(seat) => handle.activate(&seat),
                None => tracing::debug!("no seat to bring a window forward with"),
            },
            Ask::Minimize => handle.set_minimized(),
            Ask::Close => handle.close(),
        }
    }
}

impl App {
    /// Bring window `serial` forward, as a left click on its task does: a
    /// pie's choice.
    #[cfg(feature = "pie")]
    pub(super) fn activate_window(&self, serial: u64) {
        let Some(handle) = self.panels.tasks.handle(serial) else {
            tracing::debug!(serial, "a window the pie chose has gone");
            return;
        };
        match self.seat.state.seats().next() {
            Some(seat) => handle.activate(&seat),
            None => tracing::debug!("no seat to bring a window forward with"),
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                app.panels.tasks.add(toplevel);
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                tracing::info!("the compositor stopped telling the taskbar of windows");
                app.panels.taskbar = None;
                if app.panels.tasks.clear() {
                    app.panels_changed();
                }
            }
            _ => {}
        }
    }

    event_created_child!(App, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for App {
    fn event(
        app: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::{Event, State};
        let told = match event {
            Event::Title { title } => Told::Title(title),
            Event::AppId { app_id } => Told::AppId(app_id),
            Event::OutputEnter { output } => Told::OutputEnter(output),
            Event::OutputLeave { output } => Told::OutputLeave(output),
            Event::State { state } => {
                // Each state a little-endian `u32`.
                let states: Vec<State> = state
                    .chunks_exact(4)
                    .filter_map(|bytes| {
                        let value = u32::from_le_bytes(bytes.try_into().ok()?);
                        State::try_from(value).ok()
                    })
                    .collect();
                Told::State {
                    active: states.contains(&State::Activated),
                    minimized: states.contains(&State::Minimized),
                }
            }
            Event::Parent { parent } => Told::Parent(parent.is_some()),
            Event::Done => Told::Done,
            Event::Closed => Told::Closed,
            _ => return,
        };
        let closed = matches!(told, Told::Closed);
        if app.panels.tasks.tell(handle, told) {
            app.panels_changed();
        }
        if closed {
            handle.destroy();
        }
    }
}
