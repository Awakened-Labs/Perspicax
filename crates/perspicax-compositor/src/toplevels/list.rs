//! `ext-foreign-toplevel-list-v1`: every window's title and app id, and
//! nothing a client can do about them.
//!
//! Smithay has an implementation of its own. This one exists because a
//! reload can take the protocol away from a client that holds it, and
//! Smithay keeps its lists to itself, so it can neither stop telling such a
//! client about new windows nor tell it that it has been cut off.

use perspicax_node::SurfaceId;
use perspicax_policy::Protocol;
use smithay::reexports::{
    wayland_protocols::ext::foreign_toplevel_list::v1::server::{
        ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
        ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
    },
    wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::ClientId,
    },
};

use super::{Known, Snapshot, still_admitted};
use crate::{access::Filtered, state::Compositor};

/// Tell one list about one window, all of it.
pub(super) fn announce(
    display: &DisplayHandle,
    instance: &ExtForeignToplevelListV1,
    id: SurfaceId,
    known: &mut Known,
) {
    let Ok(client) = display.get_client(instance.id()) else {
        return;
    };
    let Ok(handle) = client.create_resource::<ExtForeignToplevelHandleV1, SurfaceId, Compositor>(
        display,
        instance.version(),
        id,
    ) else {
        return;
    };
    instance.toplevel(&handle);
    handle.identifier(known.identifier.clone());
    handle.title(known.snapshot.title.clone());
    handle.app_id(known.snapshot.app_id.clone());
    handle.done();
    known.lists.push(handle);
}

/// Tell every list what changed. Only the title and app id are this
/// protocol's to tell.
pub(super) fn changed(known: &Known, now: &Snapshot) {
    let title = known.snapshot.title != now.title;
    let app_id = known.snapshot.app_id != now.app_id;
    if !title && !app_id {
        return;
    }
    for handle in &known.lists {
        if title {
            handle.title(now.title.clone());
        }
        if app_id {
            handle.app_id(now.app_id.clone());
        }
        handle.done();
    }
}

/// Tell every list the window has gone.
pub(super) fn closed(known: &Known) {
    for handle in &known.lists {
        handle.closed();
    }
}

/// Cut off every list whose client the rules no longer admit: each of its
/// windows closed, then the list finished.
pub(super) fn revoke(state: &mut Compositor) {
    let gate = state.gate.clone();
    let display = state.display.clone();
    let toplevels = &mut state.toplevels;
    let (kept, cut): (Vec<_>, Vec<_>) = toplevels
        .lists
        .drain(..)
        .partition(|list| still_admitted(&gate, Protocol::ForeignToplevelList, &display, list));
    toplevels.lists = kept;
    for list in cut {
        let client = list.client();
        for known in toplevels.known.values_mut() {
            known.lists.retain(|handle| {
                if handle.client() == client {
                    handle.closed();
                    false
                } else {
                    true
                }
            });
        }
        list.finished();
        tracing::info!(
            "foreign-toplevel-list withdrawn from a client [protocols] no longer admits"
        );
    }
}

impl GlobalDispatch<ExtForeignToplevelListV1, Filtered> for Compositor {
    fn bind(
        state: &mut Self,
        display: &DisplayHandle,
        _client: &Client,
        resource: New<ExtForeignToplevelListV1>,
        _global: &Filtered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let instance = data_init.init(resource, ());
        let toplevels = &mut state.toplevels;
        for (id, known) in &mut toplevels.known {
            announce(display, &instance, *id, known);
        }
        toplevels.lists.push(instance);
    }

    fn can_view(client: Client, global: &Filtered) -> bool {
        global.admits(&client)
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        instance: &ExtForeignToplevelListV1,
        request: ext_foreign_toplevel_list_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Destroy is the other request, and `destroyed` does its work.
        if let ext_foreign_toplevel_list_v1::Request::Stop = request {
            state.toplevels.lists.retain(|list| list != instance);
            instance.finished();
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        instance: &ExtForeignToplevelListV1,
        _data: &(),
    ) {
        state.toplevels.lists.retain(|list| list != instance);
    }
}

impl Dispatch<ExtForeignToplevelHandleV1, SurfaceId> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _handle: &ExtForeignToplevelHandleV1,
        _request: ext_foreign_toplevel_handle_v1::Request,
        _id: &SurfaceId,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Destroy is the only request, and `destroyed` does its work.
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        handle: &ExtForeignToplevelHandleV1,
        id: &SurfaceId,
    ) {
        if let Some(known) = state.toplevels.known.get_mut(id) {
            known.lists.retain(|held| held != handle);
        }
    }
}
