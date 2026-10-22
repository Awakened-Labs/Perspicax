//! `wlr-foreign-toplevel-management`: the windows, as the compositor tells
//! a taskbar them, and what a press on a task asks of its window.
//!
//! What waybar's `wlr/taskbar` speaks. perspicax offers it to any program
//! unless `[protocols]` says otherwise; without it the panels list no
//! windows, and are otherwise as they were. Bringing a window forward,
//! putting it away and closing it are the compositor's to do, as a person's
//! click or key would.
//!
//! A right-click on a task opens its window's menu beside it, built from
//! what perspicax tells its own shell of the window, if it does. What is
//! chosen there is asked of the window by this protocol where it can be:
//! minimize, restore, maximize and close. Making it sticky, keeping it on
//! top, rolling it up and sending it to a workspace are asked of perspicax,
//! over its channel from version 5.

#[cfg(feature = "menus")]
use perspicax_config::Edge;
use wayland_client::{Connection, Dispatch, QueueHandle, event_created_child, globals::GlobalList};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::App;
#[cfg(feature = "menus")]
use super::panel::title;
use crate::model::{
    Button,
    tasks::{Ask, Told},
};
#[cfg(feature = "menus")]
use crate::{
    model::window_menu::{self, Verb},
    update::{Event as MenuEvent, Which},
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
    /// A button went down on the task of window `serial`, on the panel of
    /// the monitor named `name`.
    pub(super) fn press_task(&mut self, name: &str, serial: u64, button: Button) {
        let Some((handle, ask)) = self.panels.tasks.pressed(serial, button) else {
            return;
        };
        match ask {
            Ask::Activate => self.bring_forward(handle),
            Ask::Minimize => handle.set_minimized(),
            Ask::Close => handle.close(),
            #[cfg(feature = "menus")]
            Ask::Menu => self.open_task_menu(name, serial),
        }
        #[cfg(not(feature = "menus"))]
        let _ = name;
    }

    /// Bring the window of `handle` forward: show it, minimized or not, and
    /// give it the keyboard.
    fn bring_forward(&self, handle: &ZwlrForeignToplevelHandleV1) {
        match self.seat.state.seats().next() {
            Some(seat) => handle.activate(&seat),
            None => tracing::debug!("no seat to bring a window forward with"),
        }
    }
}

impl App {
    /// Bring window `serial` forward, as a left click on its task does: a
    /// pie's choice.
    #[cfg(feature = "pie")]
    pub(super) fn activate_window(&self, serial: u64) {
        match self.panels.tasks.handle(serial) {
            Some(handle) => self.bring_forward(handle),
            None => tracing::debug!(serial, "a window the pie chose has gone"),
        }
    }
}

#[cfg(feature = "menus")]
impl App {
    /// Open the menu of window `serial` beside its task, on the panel of
    /// the monitor named `name`. It offers the workspaces of the group the
    /// window is on, or with none, of the group on its monitor.
    fn open_task_menu(&mut self, name: &str, serial: u64) {
        let Some(output) = self.output_named(name) else {
            return;
        };
        let Some(button) = self.panels.task_at(name, serial, self.monitor(&output)) else {
            return;
        };
        let Some((window, placing)) = self.panels.tasks.window(serial) else {
            return;
        };
        let pager = &self.workspaces.model;
        let group = match placing.and_then(|placing| placing.workspace) {
            Some(workspace) => pager.beside(workspace),
            None => pager.on(window.outputs.first().unwrap_or(&output)),
        };
        let workspaces: Vec<(u64, &str)> = group
            .iter()
            .map(|(workspace, _)| (workspace.serial, workspace.name.as_str()))
            .collect();
        let menu = window_menu::build(window, placing, &workspaces);
        let called = title(window);
        let area = self.area(&output);
        let edge = self.panels.edge().unwrap_or(Edge::Bottom);
        self.menu_event(MenuEvent::TaskMenu {
            serial,
            name: called,
            menu,
            output: name.to_owned(),
            area,
            button,
            edge,
        });
    }

    /// Ask `verb` of window `serial`, chosen in its menu.
    pub(super) fn act_on_window(&self, serial: u64, verb: Verb) {
        use perspicax_protocols::shell::v1::client::perspicax_shell_v1 as shell;
        use wayland_client::Proxy;

        let Some(handle) = self.panels.tasks.handle(serial) else {
            tracing::debug!(serial, "a window chosen for in its menu has gone");
            return;
        };
        // perspicax's channel, if it is bound at a version that has `since`.
        let channel = |since| {
            self.channel
                .as_ref()
                .filter(|channel| channel.version() >= since)
        };
        match verb {
            Verb::Minimize => handle.set_minimized(),
            Verb::Restore => self.bring_forward(handle),
            Verb::Maximize(true) => handle.set_maximized(),
            Verb::Maximize(false) => handle.unset_maximized(),
            Verb::Close => handle.close(),
            Verb::Sticky(sticky) => {
                if let Some(channel) = channel(shell::REQ_SET_STICKY_SINCE) {
                    channel.set_sticky(handle, sticky.into());
                }
            }
            Verb::OnTop(on_top) => {
                if let Some(channel) = channel(shell::REQ_SET_ON_TOP_SINCE) {
                    channel.set_on_top(handle, on_top.into());
                }
            }
            Verb::Shade(shaded) => {
                if let Some(channel) = channel(shell::REQ_SET_SHADED_SINCE) {
                    channel.set_shaded(handle, shaded.into());
                }
            }
            Verb::SendTo(workspace) => {
                if let (Some(channel), Some(workspace)) = (
                    channel(shell::REQ_SEND_TO_WORKSPACE_SINCE),
                    self.workspaces.model.handle_of(workspace),
                ) {
                    channel.send_to_workspace(handle, workspace);
                }
            }
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
                // A window's menu goes with the windows.
                #[cfg(feature = "menus")]
                if let Some(Which::Task(serial)) = app.menus.open_menu() {
                    app.menu_event(MenuEvent::TaskClosed(serial));
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
                    maximized: states.contains(&State::Maximized),
                }
            }
            Event::Parent { parent } => Told::Parent(parent.is_some()),
            Event::Done => Told::Done,
            Event::Closed => Told::Closed,
            _ => return,
        };
        let closed = matches!(told, Told::Closed);
        // Its menu goes with it.
        #[cfg(feature = "menus")]
        let gone = closed.then(|| app.panels.tasks.serial_of(handle)).flatten();
        if app.panels.tasks.tell(handle, told) {
            app.panels_changed();
        }
        if closed {
            handle.destroy();
        }
        #[cfg(feature = "menus")]
        if let Some(serial) = gone {
            app.menu_event(MenuEvent::TaskClosed(serial));
        }
    }
}
