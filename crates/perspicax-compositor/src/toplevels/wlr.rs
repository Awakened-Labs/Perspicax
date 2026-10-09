//! `wlr-foreign-toplevel-management-unstable-v1`: a taskbar's view of every
//! window, and what it can do to one.
//!
//! What waybar's `wlr/taskbar` speaks. Each window is described as the list
//! describes it, plus its state, its monitor and its parent, and a taskbar
//! may activate, minimize, maximize, fullscreen or close it. Each of those is
//! done by the same code a person's click or key does: activating a window
//! on another workspace shows that workspace, and activating a tab behind
//! another brings it forward in its group's place. Closing asks the client,
//! which may decline.
//!
//! A request is ignored while the session is locked, and from a client the
//! rules no longer admit.

use perspicax_node::SurfaceId;
use perspicax_policy::{Protocol, Zone};
use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
            zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::ClientId,
        },
    },
};

use super::{Known, Snapshot, still_admitted};
use crate::{
    access::Filtered,
    shell::{Fill, surface_of},
    state::Compositor,
};

/// The version this compositor speaks: 3 adds a window's parent.
pub(super) const VERSION: u32 = 3;

/// The handle `client` holds on a window, if any.
pub(super) fn held_by(known: &Known, client: &Client) -> Option<ZwlrForeignToplevelHandleV1> {
    known
        .wlr
        .iter()
        .find(|handle| handle.client().as_ref() == Some(client))
        .cloned()
}

/// Tell one taskbar about one window, all of it. `parent` is the handle the
/// same taskbar holds on the window's parent.
pub(super) fn announce(
    display: &DisplayHandle,
    manager: &ZwlrForeignToplevelManagerV1,
    id: SurfaceId,
    known: &mut Known,
    parent: Option<&ZwlrForeignToplevelHandleV1>,
    outputs: &[Output],
) {
    let Ok(client) = display.get_client(manager.id()) else {
        return;
    };
    let Ok(handle) = client.create_resource::<ZwlrForeignToplevelHandleV1, SurfaceId, Compositor>(
        display,
        manager.version(),
        id,
    ) else {
        return;
    };
    manager.toplevel(&handle);
    let snapshot = &known.snapshot;
    handle.title(snapshot.title.clone());
    handle.app_id(snapshot.app_id.clone());
    for name in &snapshot.outputs {
        enter_or_leave(&handle, &client, outputs, name, true);
    }
    handle.state(states(snapshot));
    if handle.version() >= 3 {
        handle.parent(parent);
    }
    handle.done();
    known.wlr.push(handle);
}

/// Tell every taskbar what changed. `parent_held_by` finds the handle a
/// client holds on the new parent.
pub(super) fn changed(
    known: &Known,
    now: &Snapshot,
    parent_held_by: impl Fn(&Client) -> Option<ZwlrForeignToplevelHandleV1>,
    outputs: &[Output],
) {
    let was = &known.snapshot;
    for handle in &known.wlr {
        let Some(client) = handle.client() else {
            continue;
        };
        if was.title != now.title {
            handle.title(now.title.clone());
        }
        if was.app_id != now.app_id {
            handle.app_id(now.app_id.clone());
        }
        for name in was
            .outputs
            .iter()
            .filter(|name| !now.outputs.contains(name))
        {
            enter_or_leave(handle, &client, outputs, name, false);
        }
        for name in now
            .outputs
            .iter()
            .filter(|name| !was.outputs.contains(name))
        {
            enter_or_leave(handle, &client, outputs, name, true);
        }
        if states(was) != states(now) {
            handle.state(states(now));
        }
        if was.parent != now.parent && handle.version() >= 3 {
            handle.parent(now.parent.and_then(|_| parent_held_by(&client)).as_ref());
        }
        handle.done();
    }
}

/// Tell every taskbar the window has gone.
pub(super) fn closed(known: &Known) {
    for handle in &known.wlr {
        handle.closed();
    }
}

/// Cut off every taskbar whose client the rules no longer admit: each of its
/// windows closed, then the manager finished.
pub(super) fn revoke(state: &mut Compositor) {
    let gate = state.gate.clone();
    let display = state.display.clone();
    let toplevels = &mut state.toplevels;
    let (kept, cut): (Vec<_>, Vec<_>) = toplevels.managers.drain(..).partition(|manager| {
        still_admitted(
            &gate,
            Protocol::ForeignToplevelManagement,
            &display,
            manager,
        )
    });
    toplevels.managers = kept;
    for manager in cut {
        let client = manager.client();
        for known in toplevels.known.values_mut() {
            known.wlr.retain(|handle| {
                if handle.client() == client {
                    handle.closed();
                    false
                } else {
                    true
                }
            });
        }
        manager.finished();
        tracing::info!(
            "foreign-toplevel-management withdrawn from a client [protocols] no longer admits"
        );
    }
}

/// The state array: each state a little-endian `u32`.
fn states(snapshot: &Snapshot) -> Vec<u8> {
    use zwlr_foreign_toplevel_handle_v1::State;
    [
        (snapshot.maximized, State::Maximized),
        (snapshot.minimized, State::Minimized),
        (snapshot.activated, State::Activated),
        (snapshot.fullscreen, State::Fullscreen),
    ]
    .into_iter()
    .filter(|(set, _)| *set)
    .flat_map(|(_, state)| u32::from(state).to_le_bytes())
    .collect()
}

fn enter_or_leave(
    handle: &ZwlrForeignToplevelHandleV1,
    client: &Client,
    outputs: &[Output],
    name: &str,
    enter: bool,
) {
    let Some(output) = outputs.iter().find(|output| output.name() == name) else {
        return;
    };
    for wl_output in output.client_outputs(client) {
        if enter {
            handle.output_enter(&wl_output);
        } else {
            handle.output_leave(&wl_output);
        }
    }
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, Filtered> for Compositor {
    fn bind(
        state: &mut Self,
        display: &DisplayHandle,
        client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _global: &Filtered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        let outputs: Vec<Output> = state.space.outputs().cloned().collect();
        let toplevels = &mut state.toplevels;
        let ids: Vec<SurfaceId> = toplevels.known.keys().copied().collect();
        for id in ids {
            let parent = toplevels.known[&id]
                .snapshot
                .parent
                .and_then(|parent| toplevels.known.get(&parent))
                .and_then(|parent| held_by(parent, client));
            if let Some(known) = toplevels.known.get_mut(&id) {
                announce(display, &manager, id, known, parent.as_ref(), &outputs);
            }
        }
        toplevels.managers.push(manager);
    }

    fn can_view(client: Client, global: &Filtered) -> bool {
        global.admits(&client)
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Stop is the only request.
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            state.toplevels.managers.retain(|held| held != manager);
            manager.finished();
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        manager: &ZwlrForeignToplevelManagerV1,
        _data: &(),
    ) {
        state.toplevels.managers.retain(|held| held != manager);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, SurfaceId> for Compositor {
    fn request(
        state: &mut Self,
        client: &Client,
        _handle: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        id: &SurfaceId,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request;
        if state.lock.is_some()
            || !state
                .gate
                .admits(Protocol::ForeignToplevelManagement, client)
        {
            return;
        }
        let Some(window) = state.any_window(*id) else {
            return;
        };
        let toplevel = window.toplevel().cloned();
        match request {
            Request::Activate { .. } => state.activate_window(&window),
            Request::Close => Self::close(&window),
            Request::SetMinimized => state.minimize(&window),
            Request::UnsetMinimized => {
                state.restore(&window);
                state.publish_facts();
            }
            Request::SetMaximized => match toplevel {
                Some(_) => state.fill(&window, Fill::Maximized, None),
                None if !Self::is_snapped(&window) => state.snap(&window, Zone::Top, None),
                None => {}
            },
            Request::UnsetMaximized => match toplevel {
                Some(_) => state.unfill(&window, Fill::Maximized, None),
                None if Self::is_snapped(&window) => state.unsnap(&window, None),
                None => {}
            },
            Request::SetFullscreen { output } => {
                if toplevel.is_some() {
                    state.fill(&window, Fill::Fullscreen, output.as_ref());
                }
            }
            Request::UnsetFullscreen => {
                if toplevel.is_some() {
                    state.unfill(&window, Fill::Fullscreen, None);
                }
            }
            // Where the taskbar's button is, for a minimize animation there
            // is none of; and Destroy, which `destroyed` handles.
            _ => {}
        }
        state.backend.redraw();
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        handle: &ZwlrForeignToplevelHandleV1,
        id: &SurfaceId,
    ) {
        if let Some(known) = state.toplevels.known.get_mut(id) {
            known.wlr.retain(|held| held != handle);
        }
    }
}

impl Compositor {
    /// Bring a window to the person, as clicking it in a taskbar does: a tab
    /// behind another comes forward in its group's place; anything else is
    /// restored, onto its workspace, raised and given the keyboard.
    pub(crate) fn activate_window(&mut self, window: &crate::framed::Framed) {
        if self.behind_tab(window).is_some() {
            self.activate_tab(window);
            return;
        }
        let (Some(surface), Some(id)) = (surface_of(window), crate::shell::id_of(window)) else {
            return;
        };
        self.restore(window);
        self.space.raise_element(window, false);
        self.focus_surface(surface, id);
        self.publish_facts();
    }
}
