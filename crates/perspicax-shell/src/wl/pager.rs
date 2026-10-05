//! `ext-workspace-v1`: the workspaces, as the compositor tells a pager
//! them; switching to one a press asks for, and following the one each
//! monitor shows, for its wallpaper.
//!
//! What waybar's `ext/workspaces` speaks. perspicax offers it to any
//! program unless `[protocols]` says otherwise; without it the panels page
//! no workspaces, and every monitor shows the shell's own wallpaper, and
//! otherwise all is as it was.

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    globals::GlobalList,
    protocol::{wl_output, wl_registry},
};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
#[cfg(feature = "panel")]
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;

use super::App;
#[cfg(feature = "panel")]
use crate::model::Button;
use crate::model::pager::{Grouped, Pager, Told};

/// The workspaces, as the compositor tells them.
pub(super) type Model = Pager<ExtWorkspaceGroupHandleV1, ExtWorkspaceHandleV1, wl_output::WlOutput>;

/// The workspaces, and what the compositor tells them with: `None` if it
/// does not, or stopped.
pub(super) struct Workspaces {
    pub(super) model: Model,
    pub(super) manager: Option<ExtWorkspaceManagerV1>,
}

impl Workspaces {
    /// Bind the pager's protocol, if the compositor offers it to us.
    pub(super) fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Self {
        let manager = globals.bind(qh, 1..=1, ()).ok();
        if manager.is_none() {
            tracing::info!(
                "the compositor offers no ext_workspace_manager_v1; the pager shows nothing, \
                 and every monitor the shell's own wallpaper"
            );
        }
        Self {
            model: Pager::default(),
            manager,
        }
    }
}

impl App {
    /// The compositor told of a batch of changes to the workspaces: show
    /// them, on the pagers and in the wallpapers.
    fn workspaces_changed(&mut self) {
        #[cfg(feature = "panel")]
        self.panels_changed();
        #[cfg(feature = "wallpaper")]
        self.follow_workspaces();
    }

    /// Paint each monitor's desktop with the wallpaper of the workspace it
    /// shows.
    #[cfg(feature = "wallpaper")]
    pub(super) fn follow_workspaces(&mut self) {
        let model = &self.workspaces.model;
        self.desktops
            .follow(&mut self.canvas, &mut self.kit, |output| {
                model.showing(output)?.number()
            });
    }

    /// A button went down on the cell of workspace `serial`.
    #[cfg(feature = "panel")]
    pub(super) fn press_workspace(&mut self, serial: u64, button: Button) {
        let (Some(workspace), Some(manager)) = (
            self.workspaces.model.pressed(serial, button),
            &self.workspaces.manager,
        ) else {
            return;
        };
        workspace.activate();
        manager.commit();
    }

    /// Take back the taskbar's or the pager's protocol, if one was taken away
    /// and the rules now give it back. The compositor filters each listing of
    /// its globals by the rules in force when it is asked for, so a listing
    /// asked for now offers exactly what may be bound now, and binding from
    /// it never binds what is still refused.
    pub(super) fn take_back(&mut self, connection: &Connection) {
        #[cfg(feature = "panel")]
        let taskbar_gone = self.panels.taskbar.is_none();
        #[cfg(not(feature = "panel"))]
        let taskbar_gone = false;
        if taskbar_gone || self.workspaces.manager.is_none() {
            connection.display().get_registry(&self.qh, Relisted);
        }
    }
}

/// A listing of the compositor's globals, asked for afresh.
pub(super) struct Relisted;

impl Dispatch<wl_registry::WlRegistry, Relisted> for App {
    fn event(
        app: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &Relisted,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        #[cfg(feature = "panel")]
        if interface == ZwlrForeignToplevelManagerV1::interface().name
            && app.panels.taskbar.is_none()
        {
            tracing::info!("the taskbar's protocol is offered again; taking it back");
            app.panels.taskbar = Some(registry.bind(name, version.min(3), qh, ()));
            return;
        }
        if interface == ExtWorkspaceManagerV1::interface().name && app.workspaces.manager.is_none()
        {
            tracing::info!("the pager's protocol is offered again; taking it back");
            app.workspaces.manager = Some(registry.bind(name, version.min(1), qh, ()));
        }
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_workspace_manager_v1::Event;
        match event {
            Event::WorkspaceGroup { workspace_group } => {
                app.workspaces.model.add_group(workspace_group);
            }
            Event::Workspace { workspace } => app.workspaces.model.add_workspace(workspace),
            Event::Done => app.workspaces_changed(),
            Event::Finished => {
                tracing::info!("the compositor stopped telling the pager of workspaces");
                app.workspaces.manager = None;
                app.workspaces.model.clear();
                app.workspaces_changed();
            }
            _ => {}
        }
    }

    event_created_child!(App, ExtWorkspaceManagerV1, [
        ext_workspace_manager_v1::EVT_WORKSPACE_GROUP_OPCODE => (ExtWorkspaceGroupHandleV1, ()),
        ext_workspace_manager_v1::EVT_WORKSPACE_OPCODE => (ExtWorkspaceHandleV1, ())
    ]);
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for App {
    fn event(
        app: &mut Self,
        group: &ExtWorkspaceGroupHandleV1,
        event: ext_workspace_group_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_workspace_group_handle_v1::Event;
        let told = match event {
            Event::OutputEnter { output } => Grouped::OutputEnter(output),
            Event::OutputLeave { output } => Grouped::OutputLeave(output),
            Event::WorkspaceEnter { workspace } => Grouped::WorkspaceEnter(workspace),
            Event::WorkspaceLeave { workspace } => Grouped::WorkspaceLeave(workspace),
            Event::Removed => Grouped::Removed,
            _ => return,
        };
        let removed = matches!(told, Grouped::Removed);
        app.workspaces.model.group(group, told);
        if removed {
            group.destroy();
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for App {
    fn event(
        app: &mut Self,
        workspace: &ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_workspace_handle_v1::{Event, State};
        let told = match event {
            Event::Name { name } => Told::Name(name),
            Event::Coordinates { coordinates } => Told::Coordinates(
                // Each coordinate a little-endian `u32`.
                coordinates
                    .chunks_exact(4)
                    .filter_map(|bytes| Some(u32::from_le_bytes(bytes.try_into().ok()?)))
                    .collect(),
            ),
            Event::State { state } => Told::Active(
                state
                    .into_result()
                    .is_ok_and(|state| state.contains(State::Active)),
            ),
            Event::Removed => Told::Removed,
            _ => return,
        };
        let removed = matches!(told, Told::Removed);
        app.workspaces.model.workspace(workspace, told);
        if removed {
            workspace.destroy();
        }
    }
}
