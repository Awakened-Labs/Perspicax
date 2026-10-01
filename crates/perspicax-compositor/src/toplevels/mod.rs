//! The windows, as a taskbar is told them.
//!
//! Two protocols describe every window to clients that did not draw it:
//! `ext-foreign-toplevel-list-v1`, which lists them (`list`), and
//! `wlr-foreign-toplevel-management-unstable-v1`, which also lets a taskbar
//! activate, minimize and close them (`wlr`). Both are fed from one place.
//! [`Compositor::sync_toplevels`] runs whenever the facts are published,
//! which is after every change that could matter. It takes a snapshot of
//! every window and tells each protocol what differs from the last one it
//! sent. A diff against what was sent, rather than an event at each place a
//! window changes, is what keeps a taskbar from missing the one path nobody
//! remembered to hook.
//!
//! While the session is locked, nothing is sent: a taskbar learns what
//! changed behind the lock once it is lifted, in one batch.

mod list;
mod wlr;

use std::collections::BTreeMap;

use perspicax_node::SurfaceId;
use perspicax_policy::Protocol;
use smithay::{
    reexports::{
        wayland_protocols::{
            ext::foreign_toplevel_list::v1::server::{
                ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
                ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
            },
            xdg::shell::server::xdg_toplevel,
        },
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
            zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        },
        wayland_server::{DisplayHandle, Resource},
    },
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};

use crate::{
    access::{Filtered, Gate},
    framed::Framed,
    shell::id_of,
    state::Compositor,
};

/// One window, as far as a taskbar can tell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Snapshot {
    pub(crate) title: String,
    pub(crate) app_id: String,
    /// The monitors it is on, by name: the one, in practice.
    pub(crate) outputs: Vec<String>,
    pub(crate) maximized: bool,
    pub(crate) minimized: bool,
    pub(crate) activated: bool,
    pub(crate) fullscreen: bool,
    /// The window it is a dialog of.
    pub(crate) parent: Option<SurfaceId>,
}

/// A window, and every handle a client holds on it.
struct Known {
    /// What was last sent.
    snapshot: Snapshot,
    /// The list protocol's id for it: unique for as long as this compositor
    /// runs, and never reused.
    identifier: String,
    lists: Vec<ExtForeignToplevelHandleV1>,
    wlr: Vec<ZwlrForeignToplevelHandleV1>,
}

/// Every window a taskbar has been told about, and every taskbar.
pub(crate) struct Toplevels {
    /// Ordered by id, which is the order they opened in.
    known: BTreeMap<SurfaceId, Known>,
    lists: Vec<ExtForeignToplevelListV1>,
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    /// Different every run, so an identifier from one compositor is never
    /// mistaken for a window of the next.
    run: u64,
}

impl Toplevels {
    /// Advertise the globals, each filtered by `gate`.
    pub(crate) fn new(display: &DisplayHandle, gate: &Gate) -> Self {
        display.create_global::<Compositor, ExtForeignToplevelListV1, _>(
            1,
            Filtered {
                gate: gate.clone(),
                protocol: Protocol::ForeignToplevelList,
            },
        );
        display.create_global::<Compositor, ZwlrForeignToplevelManagerV1, _>(
            wlr::VERSION,
            Filtered {
                gate: gate.clone(),
                protocol: Protocol::ForeignToplevelManagement,
            },
        );
        let run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_nanos() & u128::from(u64::MAX)).unwrap_or(0)
            });
        Self {
            known: BTreeMap::new(),
            lists: Vec::new(),
            managers: Vec::new(),
            run,
        }
    }

    fn identifier(&self, id: SurfaceId) -> String {
        format!("{:016x}{:016x}", self.run, id.0)
    }
}

impl Compositor {
    /// Tell every taskbar what changed since it was last told.
    pub(crate) fn sync_toplevels(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let now = self.snapshots();
        let outputs: Vec<smithay::output::Output> = self.space.outputs().cloned().collect();
        let toplevels = &mut self.toplevels;

        let gone: Vec<SurfaceId> = toplevels
            .known
            .keys()
            .filter(|id| !now.iter().any(|(open, _)| open == *id))
            .copied()
            .collect();
        for id in gone {
            if let Some(known) = toplevels.known.remove(&id) {
                list::closed(&known);
                wlr::closed(&known);
            }
        }

        for (id, snapshot) in now {
            let parent = snapshot
                .parent
                .and_then(|parent| toplevels.known.get(&parent));
            if let Some(known) = toplevels.known.get(&id) {
                if known.snapshot != snapshot {
                    list::changed(known, &snapshot);
                    wlr::changed(
                        known,
                        &snapshot,
                        |client| parent.and_then(|parent| wlr::held_by(parent, client)),
                        &outputs,
                    );
                    if let Some(known) = toplevels.known.get_mut(&id) {
                        known.snapshot = snapshot;
                    }
                }
                continue;
            }
            let mut known = Known {
                snapshot,
                identifier: toplevels.identifier(id),
                lists: Vec::new(),
                wlr: Vec::new(),
            };
            for instance in &toplevels.lists {
                list::announce(&self.display, instance, id, &mut known);
            }
            for manager in &toplevels.managers {
                let parent = parent.and_then(|parent| {
                    wlr::held_by(parent, &self.display.get_client(manager.id()).ok()?)
                });
                wlr::announce(
                    &self.display,
                    manager,
                    id,
                    &mut known,
                    parent.as_ref(),
                    &outputs,
                );
            }
            toplevels.known.insert(id, known);
        }
    }

    /// Withdraw what the current rules no longer allow from the clients
    /// that hold it.
    pub(crate) fn revoke_toplevels(&mut self, protocol: Protocol) {
        match protocol {
            Protocol::ForeignToplevelList => list::revoke(self),
            Protocol::ForeignToplevelManagement => wlr::revoke(self),
            _ => {}
        }
    }

    /// Every window a taskbar shows, in the order they opened. Menus and
    /// tooltips are not windows to a taskbar.
    fn snapshots(&self) -> Vec<(SurfaceId, Snapshot)> {
        let focused = self.focused_surface();
        let mut snapshots: Vec<_> = self
            .space
            .elements()
            .chain(&self.parked)
            .filter_map(|window| {
                let id = id_of(window)?;
                let mut snapshot = self.snapshot(window)?;
                snapshot.activated = focused == Some(id);
                Some((id, snapshot))
            })
            .collect();
        snapshots.sort_by_key(|(id, _)| *id);
        snapshots
    }

    fn snapshot(&self, window: &Framed) -> Option<Snapshot> {
        let outputs = self.monitor_of(window).into_iter().collect();
        let minimized = Self::is_minimized(window);
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            if x11.is_override_redirect() {
                return None;
            }
            return Some(Snapshot {
                title: x11.title(),
                app_id: x11.class(),
                outputs,
                maximized: x11.is_maximized() || Self::is_snapped(window),
                minimized,
                activated: false,
                fullscreen: x11.is_fullscreen(),
                parent: None,
            });
        }
        let toplevel = window.toplevel()?;
        let (maximized, fullscreen) = toplevel.with_pending_state(|pending| {
            (
                pending.states.contains(xdg_toplevel::State::Maximized),
                pending.states.contains(xdg_toplevel::State::Fullscreen),
            )
        });
        let (title, app_id) = with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .map(|attributes| {
                    (
                        attributes.title.clone().unwrap_or_default(),
                        attributes.app_id.clone().unwrap_or_default(),
                    )
                })
                .unwrap_or_default()
        });
        let parent = toplevel
            .parent()
            .and_then(|parent| self.window_for(&parent))
            .and_then(|parent| id_of(&parent));
        Some(Snapshot {
            title,
            app_id,
            outputs,
            maximized,
            minimized,
            activated: false,
            fullscreen,
            parent,
        })
    }
}

/// Whether `resource` belongs to a client the rules still admit to
/// `protocol`. A client that has gone counts as not admitted.
fn still_admitted(
    gate: &Gate,
    protocol: Protocol,
    display: &DisplayHandle,
    resource: &impl Resource,
) -> bool {
    display
        .get_client(resource.id())
        .is_ok_and(|client| gate.admits(protocol, &client))
}
