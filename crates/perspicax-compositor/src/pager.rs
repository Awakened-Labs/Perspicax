//! `ext-workspace-v1`: the workspaces, as a pager is told them.
//!
//! What waybar's `ext/workspaces` speaks. In spanning mode there is one
//! group of workspaces showing on every monitor; per output, a group each.
//! Each workspace is the grid's cell, named by its number as a person counts
//! it, with its column and row, and whether it is the one showing.
//!
//! A pager may switch workspace, and nothing else: workspaces here are a
//! grid the config shapes, so there is none to create, remove or move to
//! another monitor, and the capabilities each one advertises say so.
//! Switching goes through `perspicax_policy::Workspaces::go_to`, as a key
//! binding does, so a pager's click and a person's key cannot disagree.
//!
//! Fed like the taskbar protocols: after every publication of the facts,
//! [`Compositor::sync_workspaces`] compares what the grid looks like with
//! what was last sent and sends the difference. Nothing is sent while the
//! session is locked, and nothing a pager asks for is done.

use perspicax_policy::{Cell, GroupView, Protocol};
use smithay::{
    output::Output,
    reexports::{
        wayland_protocols::ext::workspace::v1::server::{
            ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
            ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
            ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::ClientId,
        },
    },
};

use crate::{
    access::{Filtered, Gate},
    state::Compositor,
};

/// The name a group goes by: its monitor's, or this for the one spanning
/// them all.
const SPANNING: &str = "*";

/// Every pager, and what they were last told.
#[derive(Default)]
pub(crate) struct Pager {
    managers: Vec<Manager>,
    sent: Vec<GroupView<String>>,
    /// Workspaces asked for and not yet committed: the client, the group,
    /// the cell.
    pending: Vec<(ClientId, String, Cell)>,
}

/// One pager's objects.
struct Manager {
    manager: ExtWorkspaceManagerV1,
    groups: Vec<(String, ExtWorkspaceGroupHandleV1)>,
    workspaces: Vec<(String, Cell, ExtWorkspaceHandleV1)>,
}

/// What a workspace handle names.
#[derive(Debug, Clone)]
pub(crate) struct Named {
    group: String,
    cell: Cell,
}

impl Pager {
    /// Advertise the global, filtered by `gate`.
    pub(crate) fn new(display: &DisplayHandle, gate: &Gate) -> Self {
        display.create_global::<Compositor, ExtWorkspaceManagerV1, _>(
            1,
            Filtered {
                gate: gate.clone(),
                protocol: Protocol::Workspace,
            },
        );
        Self::default()
    }
}

fn key(group: &GroupView<String>) -> String {
    group.on.clone().unwrap_or_else(|| SPANNING.to_owned())
}

impl Compositor {
    /// Tell every pager what changed since it was last told.
    pub(crate) fn sync_workspaces(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let now = self.workspace_groups();
        if now == self.pager.sent {
            return;
        }
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        let display = self.display.clone();
        let pager = &mut self.pager;
        for manager in &mut pager.managers {
            send(&display, manager, &pager.sent, &now, &outputs);
        }
        pager.sent = now;
    }

    /// Withdraw the pager from every client the rules no longer admit.
    pub(crate) fn revoke_pager(&mut self) {
        let gate = self.gate.clone();
        let display = self.display.clone();
        self.pager.managers.retain(|manager| {
            let admitted = display
                .get_client(manager.manager.id())
                .is_ok_and(|client| gate.admits(Protocol::Workspace, &client));
            if !admitted {
                for (_, _, workspace) in &manager.workspaces {
                    workspace.removed();
                }
                for (_, group) in &manager.groups {
                    group.removed();
                }
                manager.manager.finished();
                tracing::info!(
                    "ext-workspace withdrawn from a client [protocols] no longer admits"
                );
            }
            admitted
        });
    }

    /// The grid as a pager sees it, over the monitors there are.
    fn workspace_groups(&self) -> Vec<GroupView<String>> {
        let outputs: Vec<String> = self.space.outputs().map(Output::name).collect();
        self.workspaces.groups(&outputs)
    }

    /// A pager committed: switch to whatever it asked for.
    fn commit_workspaces(&mut self, client: &Client) {
        let asked: Vec<(String, Cell)> = self
            .pager
            .pending
            .extract_if(.., |(asker, _, _)| *asker == client.id())
            .map(|(_, group, cell)| (group, cell))
            .collect();
        if self.lock.is_some() || !self.gate.admits(Protocol::Workspace, client) {
            return;
        }
        let mut switched = false;
        for (group, cell) in asked {
            let output = if group == SPANNING {
                self.space.outputs().next().map(Output::name)
            } else {
                Some(group)
            };
            if let Some(output) = output {
                switched |= self.workspaces.go_to(&output, cell).is_some();
            }
        }
        if switched {
            self.workspace_changed();
        }
    }
}

/// Bring one pager from `was` to `now`, then `done`.
fn send(
    display: &DisplayHandle,
    manager: &mut Manager,
    was: &[GroupView<String>],
    now: &[GroupView<String>],
    outputs: &[Output],
) {
    let Ok(client) = display.get_client(manager.manager.id()) else {
        return;
    };
    let wl_outputs = |names: &[String]| -> Vec<_> {
        outputs
            .iter()
            .filter(|output| names.contains(&output.name()))
            .flat_map(|output| output.client_outputs(&client).collect::<Vec<_>>())
            .collect()
    };

    // Gone first: a workspace leaves its group before the group goes.
    manager.workspaces.retain(|(group, cell, workspace)| {
        let kept = now
            .iter()
            .any(|view| key(view) == *group && view.workspaces.iter().any(|ws| ws.cell == *cell));
        if !kept {
            if let Some((_, handle)) = manager.groups.iter().find(|(key, _)| key == group) {
                handle.workspace_leave(workspace);
            }
            workspace.removed();
        }
        kept
    });
    manager.groups.retain(|(group, handle)| {
        let kept = now.iter().any(|view| key(view) == *group);
        if !kept {
            handle.removed();
        }
        kept
    });

    for view in now {
        let group_key = key(view);
        let before = was.iter().find(|old| key(old) == group_key);
        let group = match manager.groups.iter().find(|(key, _)| *key == group_key) {
            Some((_, group)) => {
                let old = before.map(|old| old.outputs.as_slice()).unwrap_or_default();
                let left: Vec<String> = old
                    .iter()
                    .filter(|name| !view.outputs.contains(name))
                    .cloned()
                    .collect();
                for output in wl_outputs(&left) {
                    group.output_leave(&output);
                }
                let entered: Vec<String> = view
                    .outputs
                    .iter()
                    .filter(|name| !old.contains(name))
                    .cloned()
                    .collect();
                for output in wl_outputs(&entered) {
                    group.output_enter(&output);
                }
                group.clone()
            }
            None => {
                let Ok(group) = client
                    .create_resource::<ExtWorkspaceGroupHandleV1, (), Compositor>(
                        display,
                        manager.manager.version(),
                        (),
                    )
                else {
                    continue;
                };
                manager.manager.workspace_group(&group);
                group.capabilities(ext_workspace_group_handle_v1::GroupCapabilities::empty());
                for output in wl_outputs(&view.outputs) {
                    group.output_enter(&output);
                }
                manager.groups.push((group_key.clone(), group.clone()));
                group
            }
        };

        for workspace in &view.workspaces {
            let state = if workspace.active {
                ext_workspace_handle_v1::State::Active
            } else {
                ext_workspace_handle_v1::State::empty()
            };
            let known = manager
                .workspaces
                .iter()
                .find(|(group, cell, _)| *group == group_key && *cell == workspace.cell);
            if let Some((_, _, handle)) = known {
                let was_active = before
                    .and_then(|old| old.workspaces.iter().find(|ws| ws.cell == workspace.cell))
                    .is_some_and(|old| old.active);
                if was_active != workspace.active {
                    handle.state(state);
                }
                continue;
            }
            let Ok(handle) = client.create_resource::<ExtWorkspaceHandleV1, Named, Compositor>(
                display,
                manager.manager.version(),
                Named {
                    group: group_key.clone(),
                    cell: workspace.cell,
                },
            ) else {
                continue;
            };
            manager.manager.workspace(&handle);
            handle.id(format!("perspicax:{group_key}:{}", workspace.cell.0 + 1));
            handle.name((workspace.cell.0 + 1).to_string());
            handle.coordinates(
                workspace
                    .coordinates
                    .iter()
                    .flat_map(|at| at.to_le_bytes())
                    .collect(),
            );
            handle.state(state);
            handle.capabilities(ext_workspace_handle_v1::WorkspaceCapabilities::Activate);
            group.workspace_enter(&handle);
            manager
                .workspaces
                .push((group_key.clone(), workspace.cell, handle));
        }
    }
    manager.manager.done();
}

impl GlobalDispatch<ExtWorkspaceManagerV1, Filtered> for Compositor {
    fn bind(
        state: &mut Self,
        display: &DisplayHandle,
        _client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        _global: &Filtered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let mut manager = Manager {
            manager: data_init.init(resource, ()),
            groups: Vec::new(),
            workspaces: Vec::new(),
        };
        // What was last sent, so a pager that binds while the session is
        // locked learns nothing newer than the others.
        let outputs: Vec<Output> = state.space.outputs().cloned().collect();
        send(display, &mut manager, &[], &state.pager.sent, &outputs);
        state.pager.managers.push(manager);
    }

    fn can_view(client: Client, global: &Filtered) -> bool {
        global.admits(&client)
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        client: &Client,
        manager: &ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Commit => state.commit_workspaces(client),
            ext_workspace_manager_v1::Request::Stop => {
                state.pager.managers.retain(|held| held.manager != *manager);
                manager.finished();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, manager: &ExtWorkspaceManagerV1, _data: &()) {
        state.pager.managers.retain(|held| held.manager != *manager);
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _group: &ExtWorkspaceGroupHandleV1,
        _request: ext_workspace_group_handle_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Creating a workspace is not among the group's capabilities, and
        // destroying the handle is `destroyed`'s.
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        group: &ExtWorkspaceGroupHandleV1,
        _data: &(),
    ) {
        for manager in &mut state.pager.managers {
            manager.groups.retain(|(_, held)| held != group);
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, Named> for Compositor {
    fn request(
        state: &mut Self,
        client: &Client,
        _workspace: &ExtWorkspaceHandleV1,
        request: ext_workspace_handle_v1::Request,
        named: &Named,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Activate is the only capability advertised; the rest are refused
        // by being ignored, as the protocol allows.
        if let ext_workspace_handle_v1::Request::Activate = request {
            state
                .pager
                .pending
                .push((client.id(), named.group.clone(), named.cell));
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        workspace: &ExtWorkspaceHandleV1,
        _data: &Named,
    ) {
        for manager in &mut state.pager.managers {
            manager.workspaces.retain(|(_, _, held)| held != workspace);
        }
    }
}
