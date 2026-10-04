//! `ext-workspace-v1`: the workspaces, as the compositor tells a pager
//! them, and switching to one a press asks for.
//!
//! What waybar's `ext/workspaces` speaks. perspicax offers it to any
//! program unless `[protocols]` says otherwise; without it the panels page
//! no workspaces, and are otherwise as they were.

use wayland_client::{Connection, Dispatch, QueueHandle, event_created_child, globals::GlobalList};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};

use super::App;
use crate::model::{
    Button,
    pager::{Grouped, Told},
};

/// Bind the pager's protocol, if the compositor offers it to us.
pub(super) fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<ExtWorkspaceManagerV1> {
    let bound = globals.bind(qh, 1..=1, ()).ok();
    if bound.is_none() {
        tracing::info!(
            "the compositor offers no ext_workspace_manager_v1; the pager shows nothing"
        );
    }
    bound
}

impl App {
    /// A button went down on the cell of workspace `serial`.
    pub(super) fn press_workspace(&mut self, serial: u64, button: Button) {
        let (Some(workspace), Some(pager)) = (
            self.panels.workspaces.pressed(serial, button),
            &self.panels.pager,
        ) else {
            return;
        };
        workspace.activate();
        pager.commit();
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
                app.panels.workspaces.add_group(workspace_group);
            }
            Event::Workspace { workspace } => app.panels.workspaces.add_workspace(workspace),
            Event::Done => app.panels_changed(),
            Event::Finished => {
                tracing::info!("the compositor stopped telling the pager of workspaces");
                app.panels.pager = None;
                app.panels.workspaces.clear();
                app.panels_changed();
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
        app.panels.workspaces.group(group, told);
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
        app.panels.workspaces.workspace(workspace, told);
        if removed {
            workspace.destroy();
        }
    }
}
