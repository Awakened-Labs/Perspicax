//! Publishing what the compositor knows, so another thread can read it.
//!
//! Everything Wayland lives on one thread. Smithay's state, `wl_surface`
//! handles and the calloop loop are not `Send`, and making them so would mean
//! locking the compositor from outside, which is how a compositor acquires
//! latency it can never explain. The accessibility ingest, meanwhile, is tokio
//! and D-Bus and belongs on a thread of its own.
//!
//! So the boundary is one-directional and made of data. The compositor rebuilds
//! a whole [`HostFacts`] whenever anything changes and publishes it behind a
//! lock; readers take a copy and answer from that. Nothing crosses the boundary
//! but plain values -- no surface handle, no Smithay type, nothing that has to
//! be dropped on the right thread.
//!
//! That is also what lets [`HostView`](perspicax_index::HostView)'s three read
//! methods be synchronous. They answer from a published snapshot rather than by
//! asking a compositor a question and waiting for its event loop to get round
//! to it. Acting has to cross in the other direction and will be a message, not
//! a lock.
//!
//! # A snapshot is consistent, and it is not current
//!
//! A reader holds a set of facts that were true when the compositor published
//! them. That is a feature: judging half a tree against one arrangement of
//! windows and the other half against the next would produce a verdict that was
//! never true at any instant. Currency is what the generation counter and the
//! damage bookkeeping are for -- being *behind* is a state the index can detect
//! and refuse on, whereas being *inconsistent* is not.
//!
//! Behind is not good enough for the moment an act lands. An agent's click is
//! judged against the snapshot it read, and carried out on this thread later,
//! by when a menu may have opened over what it aims at. So a click or a scroll
//! is judged again here, against facts built in the turn that sends it
//! (`Compositor::host_facts`), and refused if what it would land on can no
//! longer be seen (#158).

use std::sync::{Arc, Mutex, PoisonError, RwLock, mpsc};

use perspicax_index::{HostFacts, SurfaceFacts, SurfaceKind};
use perspicax_node::{Origin, Rect, SurfaceId, Vec2};
use smithay::{
    desktop::PopupKind,
    reexports::wayland_server::{Resource as _, protocol::wl_surface::WlSurface},
    utils::{IsAlive, Logical, Point},
    wayland::{
        compositor::{RectangleKind, SurfaceAttributes, with_states},
        shell::xdg::XdgToplevelSurfaceData,
    },
};

use crate::{
    framed::Framed,
    state::{ClientState, Compositor},
};

/// A handle to whatever the compositor last published.
///
/// Cloning it is cheap and shares one snapshot; every clone sees each
/// publication. Created by the caller and handed to
/// [`run`](crate::run), so that whoever wants to read the facts does not have
/// to be the thread running the compositor.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    host: Arc<RwLock<HostFacts>>,
    session: Arc<Mutex<Session>>,
}

/// Where this session's displays are, and how it looks: what a program has
/// to be told to reach this compositor, and to look like the rest of the
/// desktop, when it was not started by it.
///
/// Programs this compositor starts are told in their environment. Programs
/// D-Bus starts on request -- a keyring's unlock prompt, a notification
/// daemon, a portal -- are given the bus's environment instead, and that is
/// the reader these facts are published for (issue #27).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFacts {
    /// The Wayland socket's name, as `WAYLAND_DISPLAY` says it, once the
    /// socket is listening.
    pub wayland_display: Option<String>,
    /// Xwayland's display number, once it is up: what `DISPLAY` must say
    /// for an X11 program to reach this compositor.
    pub x11_display: Option<u32>,
    /// What applications are told about the theme: dark or light, the
    /// accent, the contrast. The settings portal serves it.
    pub appearance: perspicax_policy::Appearance,
}

/// The session's facts and everyone watching them, behind one lock, so that a
/// watcher is told every change after the value it started from and none
/// twice.
#[derive(Debug, Default)]
struct Session {
    now: SessionFacts,
    watchers: Vec<mpsc::Sender<SessionFacts>>,
}

impl Facts {
    /// A handle to no facts at all, which is what a compositor that has not
    /// started yet honestly knows.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A handle that already holds a snapshot.
    ///
    /// Not a way to write to a live one -- [`publish`](Self::publish) is
    /// private and stays that way, because "whatever the compositor last
    /// published" is a claim that a second writer would quietly make false.
    /// This makes a *different* handle, over a screen the caller described, so
    /// that everything downstream of `Facts` can be exercised against a
    /// deliberate arrangement of windows rather than only against one a real
    /// compositor happened to produce.
    #[must_use]
    pub fn of(facts: HostFacts) -> Self {
        Self {
            host: Arc::new(RwLock::new(facts)),
            session: Arc::default(),
        }
    }

    /// A copy of the latest snapshot.
    ///
    /// A clone rather than a guard, deliberately: a caller holding a read guard
    /// while it judges a few thousand nodes would block the compositor's next
    /// publication for the duration, which is a stall in the one thread that
    /// must not have any.
    ///
    /// A poisoned lock is recovered rather than propagated. Poisoning means a
    /// thread panicked while publishing; the data behind it is a snapshot that
    /// was consistent when written, and refusing to read it would turn one
    /// thread's panic into a silent, permanent blindness in another.
    #[must_use]
    pub fn read(&self) -> HostFacts {
        self.host
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replace the snapshot.
    fn publish(&self, facts: HostFacts) {
        *self.host.write().unwrap_or_else(PoisonError::into_inner) = facts;
    }

    /// Xwayland's display number, if this compositor started one and it is
    /// ready. `None` otherwise, including in a build without the `xwayland`
    /// feature.
    #[must_use]
    pub fn x11_display(&self) -> Option<u32> {
        self.session().x11_display
    }

    /// Where this session's displays are, as they stand.
    #[must_use]
    pub fn session(&self) -> SessionFacts {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .now
            .clone()
    }

    /// Every change to [`session`](Self::session) from now on, beginning
    /// with the facts as they stand.
    ///
    /// A channel rather than a callback, because the compositor's thread is
    /// the one that publishes, and it must never wait on whoever is watching:
    /// a slow watcher only lets changes queue, and one that has gone is
    /// forgotten at the next change.
    #[must_use]
    pub fn watch_session(&self) -> mpsc::Receiver<SessionFacts> {
        let (tell, watch) = mpsc::channel();
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        // Under the lock, so no change can fall between the facts as they
        // stand and the first one this watcher is told.
        let _ = tell.send(session.now.clone());
        session.watchers.push(tell);
        watch
    }

    /// Change the session's facts, and tell every watcher if that changed
    /// anything.
    pub(crate) fn publish_session(&self, change: impl FnOnce(&mut SessionFacts)) {
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        let Session { now, watchers } = &mut *session;
        let before = now.clone();
        change(now);
        if *now != before {
            watchers.retain(|watcher| watcher.send(now.clone()).is_ok());
        }
    }

    #[cfg(feature = "xwayland")]
    pub(crate) fn publish_x11_display(&self, display: Option<u32>) {
        self.publish_session(|session| session.x11_display = display);
    }
}

/// A menu where smithay draws it: which popup, the window or panel it hangs
/// from, and where it is.
#[derive(Debug, Clone)]
pub(crate) struct PlacedPopup {
    pub(crate) popup: PopupKind,
    pub(crate) id: SurfaceId,
    pub(crate) from: SurfaceId,
    /// Where its surface-local `(0, 0)` is, in global space.
    pub(crate) buffer_origin: Vec2,
    /// Its window geometry, in its surface's own coordinates.
    pub(crate) geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
}

impl PlacedPopup {
    /// Its window geometry, in global space: what the index measures its
    /// nodes from, and what a rect an agent aims at on it is relative to.
    pub(crate) fn window(&self) -> Rect {
        crate::damage::to_rect(self.geometry) + self.buffer_origin
    }
}

impl Compositor {
    /// Rebuild the snapshot from the space and publish it.
    ///
    /// Rebuilt whole rather than patched. The z-order, geometry and origins of
    /// a handful of windows cost nothing to walk, and an incremental update is
    /// how a cache comes to disagree with the thing it caches -- which here
    /// would mean an occlusion verdict computed against a window that had
    /// already moved.
    pub(crate) fn publish_facts(&mut self) {
        // Every change a window's fullscreen state can come from ends here,
        // so this is where it is settled whether one covers the panels: the
        // window in use, through any menu of its own that has the keyboard.
        let in_use = self.window_in_use();
        self.restack(in_use);
        self.generation += 1;
        self.facts.publish(self.host_facts(in_use));
        self.announce();
    }

    /// The facts as they stand, built whole and not published: what
    /// [`Compositor::publish_facts`] publishes, and what an agent's click or
    /// scroll is judged against again in the turn that sends it (#158).
    ///
    /// Built from what is drawn now. A few changes on screen publish nothing
    /// of their own -- an X11 window's configure, a tab dragged over
    /// another, the stacking that keyboard focus settles -- so the snapshot
    /// last published is not always this. It carries the generation last
    /// published: nothing that tells snapshots apart by it ever reads one
    /// that was not published.
    ///
    /// `in_use` is the window in use, [`Compositor::window_in_use`], which
    /// reads the keyboard: asked by the caller, outside smithay's keyboard
    /// callbacks, so that this never reads it. This takes each layer map's
    /// lock and `with_states` in turn, so it is never called under either.
    pub(crate) fn host_facts(&self, in_use: Option<SurfaceId>) -> HostFacts {
        // `as_drawn` iterates back to front, as the windows are drawn: the
        // order `HostFacts::bottom_to_top` wants, with a window kept on top
        // in front of the others whichever was raised last. The facts, the
        // screen and the pointer all read it there, and agree by
        // construction rather than through a conversion somebody has to keep
        // right.
        //
        // Parked windows first, as unmapped: below everything, and judged
        // `Unmapped` (minimized) or `OtherWorkspace` rather than forgotten,
        // so an agent asking about a node in one is told the window is hidden
        // or where it is -- which it can do something about -- rather than
        // that no such surface exists.
        //
        // Layer surfaces around them, where the person sees them: background
        // and bottom under every window, top and overlay over. A panel on
        // `top` covering the foot of a maximized window is an occlusion the
        // index has to know about. A fullscreen window in use goes over the
        // panels and under `overlay` (`crate::shell::covers_panels`). And
        // while the session is locked, the lock surfaces go over everything,
        // so every node beneath is honestly judged covered.
        let below = self.layers_in(&crate::layers::BELOW);
        let top = self.layers_in(&crate::layers::TOP);
        let overlay = self.layers_in(&crate::layers::OVERLAY);
        let (raised, windows): (Vec<&crate::framed::Framed>, Vec<_>) =
            crate::shell::as_drawn(&self.space)
                .partition(|window| crate::shell::covers_panels(window));
        // How see-through each window on screen is drawn, for an agent
        // puzzled by a picture. Never judged by: see `drawn_opacity`.
        //
        // Each window's menus directly above it, as smithay draws them and
        // finds the pointer on them (issue #112): its popups before its
        // client's own surfaces. A window not described -- one that has drawn
        // nothing yet -- still has its menus described, for smithay draws
        // them all the same, over whatever is under them.
        let window = |window: &crate::framed::Framed| {
            let facts = self.facts_for(window).map(|facts| {
                let mut facts = self.grouped(window, facts);
                // Here, for a Wayland window and an X11 one alike, and as
                // `restack` worked it out: kept on top by the person, or
                // with the window it hangs from.
                facts.on_top = crate::shell::is_kept_on_top(window);
                if facts.mapped {
                    facts.drawn_opacity = Some(self.opacity_of(window, in_use))
                        .filter(|&percent| percent < perspicax_policy::OPAQUE);
                }
                facts
            });
            let popups = self.window_popups(window);
            facts.into_iter().chain(popups).collect::<Vec<_>>()
        };
        let layer = |(layer, placed): &(smithay::desktop::LayerSurface, _)| {
            let Some(id) = layer.user_data().get::<SurfaceId>().copied() else {
                return Vec::new();
            };
            let mut facts = self.plain_facts(id, layer.wl_surface(), *placed);
            facts.kind = SurfaceKind::Layer {
                layer: crate::layers::level(layer.layer()),
                namespace: layer.namespace().to_owned(),
            };
            // A panel has no window geometry: its menus hang from its corner.
            let popups = self
                .placed_popups(layer.wl_surface(), id, facts.buffer_origin)
                .into_iter()
                .map(|placed| self.popup_facts(&placed));
            core::iter::once(facts).chain(popups).collect()
        };
        let covers = self
            .lock
            .iter()
            .flat_map(|locked| &locked.surfaces)
            .filter_map(|cover| {
                let area = self.space.output_geometry(&cover.output)?;
                let facts = self.plain_facts(cover.id, cover.surface.wl_surface(), area);
                // The lock client may paint only its ring, and declare
                // nothing opaque. Beneath it, on whatever monitor shows it,
                // the compositor paints the lock's own colour over the whole
                // output, and stands behind that as covering what is under
                // it (`crate::backdrop`).
                let whole = facts.geometry;
                Some(facts.lock_cover().framed([whole]))
            });
        let surfaces: Vec<SurfaceFacts> = below
            .iter()
            .flat_map(layer)
            .chain(self.parked.iter().chain(windows).flat_map(window))
            .chain(top.iter().flat_map(layer))
            .chain(raised.into_iter().flat_map(window))
            .chain(overlay.iter().flat_map(layer))
            .chain(covers)
            .collect();

        HostFacts::bottom_to_top(surfaces, self.generation)
            .with_consent(self.grants.consent().clone())
            .with_outputs(self.output_rects().into_iter().map(|area| {
                let (x, y) = (f64::from(area.x), f64::from(area.y));
                Rect::new(x, y, x + f64::from(area.w), y + f64::from(area.h))
            }))
    }

    /// Tell the clients that watch other programs' windows what changed.
    /// After every publication, because a publication follows every change
    /// that could matter to them.
    fn announce(&mut self) {
        self.sync_toplevels();
        self.sync_workspaces();
        self.sync_heads();
        self.shells.windows_changed();
    }

    /// What a window's facts say about where it belongs, beyond where it
    /// is: its app id, its tab group and its workspace.
    fn grouped(&self, window: &Framed, mut facts: SurfaceFacts) -> SurfaceFacts {
        facts.app_id = crate::toplevels::app_id(window);
        facts.tabs = self
            .tabs
            .tabs(facts.id)
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        facts.workspace = match self.workspaces.home(facts.id) {
            Some(perspicax_policy::Home::On(cell)) => Some(cell.0 + 1),
            _ => None,
        };
        facts
    }

    /// One window's facts, or `None` if it has no id yet -- which means it has
    /// not been mapped through `new_toplevel` and is not ours to describe.
    fn facts_for(&self, window: &Framed) -> Option<SurfaceFacts> {
        let id = *window.user_data().get::<SurfaceId>()?;
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            return self.x11_facts(id, window, x11);
        }
        let toplevel = window.toplevel()?;
        let surface = toplevel.wl_surface();

        // Where we put it, which is a fact this compositor owns outright. A
        // parked window is described where it will come back to.
        let parked = self.parked.contains(window);
        let location = match self.space.element_location(window) {
            Some(location) => location,
            None => crate::shell::placement(window, |placement| placement.parked)?,
        };

        // Where the window is inside its surface, and how big: the geometry
        // its client declared, clipped to what it draws, or the full bounds
        // of what it draws if it declared none, as xdg-shell says
        // (`crate::geometry`). Measured here rather than taken from
        // `Window::geometry()`, which smithay derives from buffer sizes only
        // a backend that keeps buffers records -- the seat, and headless
        // built with `capture` -- so in a plain headless build it is always
        // `0x0`, and every node would judge `Unmapped`. Measured the same
        // way on every backend, the facts a seat publishes are the ones CI
        // tested; measured as smithay measures, they are where the seat
        // draws. Under client-side decoration it is the visible frame,
        // without the shadow, which is what window space is measured from;
        // what the window covers is everything it draws, the shadow too
        // (`drawn`, below). Which origin each toolkit's own coordinates are
        // measured from is a
        // different question, and the index answers it from the toolkit's
        // window node (`Index::window_origin`).
        //
        // A window that has never drawn and declared nothing has none, and
        // is not described: it is on nobody's screen. Until issue #44, one
        // that drew without declaring was left out too, and so covered
        // nothing, so a node under it was judged visible.
        let geometry = crate::geometry::window_geometry(surface)?;
        let hidden = if parked {
            self.hidden_why(window)
        } else {
            (None, None)
        };
        // In floating point, as everything a client's numbers are added to
        // here is: each is any `i32` it likes (#154).
        let placed =
            crate::damage::to_rect(smithay::utils::Rectangle::new(location, geometry.size));
        // Surface-local (0,0) sits at the geometry's own offset *back* from
        // where we placed that geometry -- under CSD that is the shadow
        // margin, and it is where opaque regions are measured from.
        let buffer_origin = back(location, geometry.loc);

        Some(SurfaceFacts {
            id,
            // Alive, and carrying something to look at. A toplevel that has been
            // created but never presented a buffer is a window in name only,
            // and reporting it as mapped would let a node be judged visible on
            // a surface with nothing on it.
            mapped: window.alive() && self.has_presented(id) && !parked,
            mapped_at: None,
            geometry: placed,
            buffer_origin,
            // The shadow too, and every subsurface, however small a window
            // its client said it was.
            drawn: drawn(surface, buffer_origin, placed),
            opaque: opaque_of(surface),
            origin: self.origin_of(window),
            title: title(surface),
            focused_at: self.focused_at(id),
            damage_generation: self.damage_generation(id),
            damage: self.damage_history(id),
            off_workspace: hidden.0,
            behind_tab: hidden.1,
            // Rolled up, its frame is the bar alone, and covers only that.
            shaded: crate::shell::is_shaded(window),
            // Set by `host_facts`, which every window passes through.
            on_top: false,
            // A parked window draws nothing, frame included.
            frame: if parked {
                Vec::new()
            } else {
                self.frame_facts(
                    window,
                    smithay::utils::Rectangle::new(location, geometry.size),
                )
            },
            // Filled by `publish_facts`, which asks once which window is in
            // use.
            drawn_opacity: None,
            // Filled by `grouped`, for X11 windows too.
            app_id: None,
            tabs: Vec::new(),
            workspace: None,
            kind: SurfaceKind::Window,
        })
    }

    /// A window's menus, bottom to top, as the facts describe them.
    fn window_popups(&self, window: &Framed) -> Vec<SurfaceFacts> {
        self.window_placed(window)
            .into_iter()
            .map(|placed| self.popup_facts(&placed))
            .collect()
    }

    /// A window's menus, bottom to top, measured from where its surface's
    /// own `(0, 0)` is: where it was placed, back by its geometry's offset,
    /// as `facts_for` places it. A parked window is not in the space, and
    /// smithay draws none of its menus: it has none here. Nor has an X11
    /// window: its menus place themselves, and are described where X put
    /// them, as popups of their own (`x11_facts`).
    fn window_placed(&self, window: &Framed) -> Vec<PlacedPopup> {
        let (Some(toplevel), Some(id), Some(location)) = (
            window.toplevel(),
            crate::shell::id_of(window),
            self.space.element_location(window),
        ) else {
            return Vec::new();
        };
        let surface = toplevel.wl_surface();
        // A window that has declared and drawn nothing has its menus hung
        // from its corner, as `popup::hung` hangs them.
        let offset =
            crate::geometry::window_geometry(surface).map_or_else(Default::default, |g| g.loc);
        self.placed_popups(surface, id, back(location, offset))
    }

    /// The menu with this id, where smithay draws it now: hung from a window
    /// in the space or a layer surface on an output. `None` for one that is
    /// gone, dismissed, or hangs from a window that is not on screen, none
    /// of which smithay draws.
    pub(crate) fn popup_placed(&self, id: SurfaceId) -> Option<PlacedPopup> {
        let windows = self
            .space
            .elements()
            .flat_map(|window| self.window_placed(window));
        let layers = self
            .layers_in(
                &[
                    &crate::layers::BELOW[..],
                    &crate::layers::TOP,
                    &crate::layers::OVERLAY,
                ]
                .concat(),
            )
            .into_iter()
            .flat_map(|(layer, placed)| {
                let Some(from) = layer.user_data().get::<SurfaceId>().copied() else {
                    return Vec::new();
                };
                let origin = Vec2::new(f64::from(placed.loc.x), f64::from(placed.loc.y));
                self.placed_popups(layer.wl_surface(), from, origin)
            });
        windows.chain(layers).find(|placed| placed.id == id)
    }

    /// The menus hanging from `root`, the surface of the window or panel
    /// `from`, whose surface-local `(0, 0)` is at `buffer_origin`: bottom to
    /// top, a submenu above its menu, each placed where smithay draws it
    /// (`popup::hung`). A menu that has drawn nothing yet, and declared
    /// nothing, is not one.
    fn placed_popups(
        &self,
        root: &WlSurface,
        from: SurfaceId,
        buffer_origin: Vec2,
    ) -> Vec<PlacedPopup> {
        let mut popups: Vec<PlacedPopup> = smithay::desktop::PopupManager::popups_for_surface(root)
            .filter_map(|(popup, _)| {
                let surface = popup.wl_surface();
                Some(PlacedPopup {
                    id: self.popup_id(surface)?,
                    from,
                    buffer_origin: buffer_origin + crate::popup::hung(&popup)?.at,
                    geometry: crate::geometry::window_geometry(surface)?,
                    popup,
                })
            })
            .collect();
        // Smithay lists them front to back.
        popups.reverse();
        popups
    }

    /// A menu as the facts describe it: measured as a window is, from what
    /// it draws, where it is drawn.
    fn popup_facts(&self, placed: &PlacedPopup) -> SurfaceFacts {
        let surface = placed.popup.wl_surface();
        let id = placed.id;
        let window = placed.window();
        let mut facts = SurfaceFacts::new(id, window)
            .with_buffer_origin(placed.buffer_origin)
            .drawing_over(drawn(surface, placed.buffer_origin, window))
            .declaring_opaque(opaque_of(surface))
            .owned_by(self.origin_of_surface(surface));
        facts.mapped = surface.is_alive() && crate::geometry::shown(surface).is_some();
        facts.mapped_at = self.popups_shown.get(&id).copied().filter(|_| facts.mapped);
        facts.damage_generation = self.damage_generation(id);
        facts.damage = self.damage_history(id);
        facts.kind = SurfaceKind::Popup {
            from: Some(placed.from),
        };
        facts
    }

    /// The facts for a surface that is not a window -- a layer surface, a lock
    /// surface -- placed by the compositor at `placed`. No declared window
    /// geometry and no shadow margin: the rectangle is the surface.
    fn plain_facts(
        &self,
        id: SurfaceId,
        surface: &WlSurface,
        placed: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) -> SurfaceFacts {
        let geometry = crate::damage::to_rect(placed);
        let buffer_origin = Vec2::new(f64::from(placed.loc.x), f64::from(placed.loc.y));
        SurfaceFacts {
            id,
            mapped: surface.is_alive() && self.has_presented(id) && !placed.size.is_empty(),
            mapped_at: None,
            geometry,
            buffer_origin,
            // As measured, not as placed: a layer's subsurfaces are not in
            // the size a plain headless build gives it, and an X11 window's
            // new buffer can outgrow the size it was just configured to.
            drawn: drawn(surface, buffer_origin, geometry),
            opaque: opaque_of(surface),
            origin: self.origin_of_surface(surface),
            // A layer's namespace is not a title: it goes in `kind`, set by
            // the caller, which the join reads in a title's place.
            title: None,
            focused_at: self.focused_at(id),
            damage_generation: self.damage_generation(id),
            damage: self.damage_history(id),
            off_workspace: None,
            behind_tab: None,
            shaded: false,
            on_top: false,
            frame: Vec::new(),
            // Layers and lock covers are drawn as their clients drew them.
            drawn_opacity: None,
            app_id: None,
            tabs: Vec::new(),
            workspace: None,
            kind: SurfaceKind::Window,
        }
    }

    /// An X11 window's facts. No xdg geometry and no shadow margin: the X
    /// window's own rectangle is the window. A window Xwayland has not yet
    /// given a surface is described, unmapped, so its nodes are refused
    /// rather than judged against nothing.
    ///
    /// One that places itself -- a menu, a tooltip -- is a popup, hung from
    /// the window that opened it if it names one (issue #159). It stays where
    /// X stacks it, which is where it is drawn: over every window when it
    /// maps. No toolkit measured draws a shadow inside such a window, so its
    /// rectangle is its window geometry too.
    #[cfg(feature = "xwayland")]
    fn x11_facts(
        &self,
        id: SurfaceId,
        window: &Framed,
        x11: &smithay::xwayland::X11Surface,
    ) -> Option<SurfaceFacts> {
        let location = match self.space.element_location(window) {
            Some(location) => location,
            None => crate::shell::placement(window, |placement| placement.parked)?,
        };
        let size = x11.geometry().size;
        let placed = smithay::utils::Rectangle::new(location, size);
        let mut facts = match x11.wl_surface() {
            Some(surface) => self.plain_facts(id, &surface, placed),
            None => SurfaceFacts::new(id, crate::damage::to_rect(placed)).unmapped(),
        };
        if self.parked.contains(window) {
            facts.mapped = false;
            (facts.off_workspace, facts.behind_tab) = self.hidden_why(window);
        } else {
            facts.frame = self.frame_facts(window, placed);
        }
        facts.shaded = crate::shell::is_shaded(window);
        facts.origin = Self::x11_origin(window);
        facts.title = Some(x11.title()).filter(|title| !title.is_empty());
        if crate::shell::places_itself(window) {
            facts.kind = SurfaceKind::Popup {
                from: self.x11_from(x11),
            };
            facts.mapped_at = Self::override_mapped_at(window).filter(|_| facts.mapped);
        }
        Some(facts)
    }

    /// Who owns the client that drew this window.
    ///
    /// Read from the per-client state recorded when the connection was
    /// accepted, not asked for again here: the credentials are a fact about a
    /// socket at the moment it was accepted, and re-deriving them later from a
    /// pid that may have been recycled would be strictly worse information
    /// wearing a fresher timestamp.
    fn origin_of(&self, window: &Framed) -> Origin {
        let Some(toplevel) = window.toplevel() else {
            return Origin::Unattributed;
        };
        self.origin_of_surface(toplevel.wl_surface())
    }

    /// Who owns the client that drew this window, X11's or not: what its
    /// facts say.
    pub(crate) fn window_origin(&self, window: &Framed) -> Origin {
        #[cfg(feature = "xwayland")]
        if window.x11_surface().is_some() {
            return Self::x11_origin(window);
        }
        self.origin_of(window)
    }

    /// Who owns the client that drew this surface.
    fn origin_of_surface(&self, surface: &WlSurface) -> Origin {
        self.display
            .get_client(surface.id())
            .ok()
            .and_then(|client| client.get_data::<ClientState>().map(ClientState::origin))
            .unwrap_or(Origin::Unattributed)
    }
}

/// The title the client set on this toplevel.
///
/// A string the application chooses for itself and may change to anything,
/// which is why the join treats it as something that separates candidates
/// rather than as something that admits them.
pub(crate) fn title(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Option<String> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| {
                data.lock()
                    .ok()
                    .and_then(|attributes| attributes.title.clone())
            })
    })
}

/// Everything the tree under `surface` draws, in global space, its
/// surface-local `(0, 0)` being at `buffer_origin`: what it covers. Where it
/// was `placed` while it has drawn nothing to measure.
fn drawn(surface: &WlSurface, buffer_origin: Vec2, placed: Rect) -> Rect {
    crate::geometry::drawn(surface).map_or(placed, |bounds| {
        crate::damage::to_rect(bounds) + buffer_origin
    })
}

/// Where the tree under `surface` says it is opaque, in its surface's own
/// coordinates: what the surface itself declared, and what each of its
/// subsurfaces did, placed where it is. Empty for a tree that declared
/// nothing, which covers all the same: an opaque region is proof only of
/// where a surface is opaque, never of where it is not.
fn opaque_of(surface: &WlSurface) -> Vec<Rect> {
    let mut opaque = with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .opaque_region
            .as_ref()
            .map(regions)
            .unwrap_or_default()
    });
    opaque.extend(crate::geometry::declared_opaque(surface));
    opaque
}

/// The `Add` rectangles of a region, in surface-local coordinates, measured
/// in floating point: a client may add one whose far edge is past `i32`.
///
/// `Subtract` rectangles are dropped, and the direction of that error is the
/// reason it is acceptable: ignoring a subtraction can only make this compositor
/// believe a surface is opaque over more of itself than it really is, which
/// makes a refusal under it look proven where it rests on policy. No region,
/// whatever it says, makes a node visible.
pub(crate) fn regions(region: &smithay::wayland::compositor::RegionAttributes) -> Vec<Rect> {
    region
        .rects
        .iter()
        .filter(|(kind, _)| matches!(kind, RectangleKind::Add))
        .map(|(_, rect)| crate::damage::to_rect(*rect))
        .collect()
}

/// Where a surface's corner is, `at` less the `offset` it sits back from
/// there, in floating point.
fn back(at: Point<i32, Logical>, offset: Point<i32, Logical>) -> Vec2 {
    Vec2::new(
        f64::from(at.x) - f64::from(offset.x),
        f64::from(at.y) - f64::from(offset.y),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handle is the boundary, so what it guarantees is worth pinning: a
    /// clone sees what the original published, because the two halves of this
    /// design live on different threads and a copy that quietly stopped
    /// updating would look exactly like a desktop where nothing ever moves.
    #[test]
    fn every_clone_of_a_handle_sees_the_latest_publication() {
        let publisher = Facts::new();
        let reader = publisher.clone();
        assert!(reader.read().surfaces().is_empty());
        assert_eq!(reader.read().generation(), 0);

        publisher.publish(HostFacts::bottom_to_top(
            [SurfaceFacts::new(
                SurfaceId(1),
                Rect::new(0.0, 0.0, 10.0, 10.0),
            )],
            7,
        ));

        assert_eq!(reader.read().generation(), 7);
        assert_eq!(reader.read().surfaces().len(), 1);
    }

    /// A watcher of the session hears where things stood when it began, then
    /// each change once, and nothing for a publication that changed nothing:
    /// it is what tells the session bus, and a repeated or a missed
    /// `WAYLAND_DISPLAY` would be a program D-Bus starts finding no display.
    #[test]
    fn a_session_watcher_hears_the_start_then_each_change_once() {
        let facts = Facts::new();
        facts.publish_session(|session| session.wayland_display = Some("wayland-1".into()));
        let watch = facts.watch_session();

        facts.publish_session(|session| session.wayland_display = Some("wayland-1".into()));
        facts.publish_session(|session| session.x11_display = Some(2));

        let heard: Vec<_> = watch.try_iter().collect();
        assert_eq!(
            heard,
            [
                SessionFacts {
                    wayland_display: Some("wayland-1".into()),
                    ..SessionFacts::default()
                },
                SessionFacts {
                    wayland_display: Some("wayland-1".into()),
                    x11_display: Some(2),
                    ..SessionFacts::default()
                },
            ]
        );
        assert_eq!(facts.x11_display(), Some(2));
    }

    /// A region's rectangle is any four `i32`s a client likes, so its far
    /// edges can be past what an `i32` holds: they are measured where they
    /// are, not wrapped round to the other side, nor a panic (#154).
    #[test]
    fn regions_reaching_past_i32_are_measured_in_floating_point() {
        let far = smithay::utils::Rectangle::new((i32::MAX, i32::MAX).into(), (1, 1).into());
        let region = smithay::wayland::compositor::RegionAttributes {
            rects: vec![(RectangleKind::Add, far)],
        };
        let edge = f64::from(i32::MAX);

        assert_eq!(
            regions(&region),
            [Rect::new(edge, edge, edge + 1.0, edge + 1.0)]
        );
    }

    /// A watcher that has gone is forgotten rather than told forever.
    #[test]
    fn a_watcher_that_has_gone_is_forgotten() {
        let facts = Facts::new();
        drop(facts.watch_session());
        let kept = facts.watch_session();

        facts.publish_session(|session| session.x11_display = Some(0));

        assert_eq!(facts.session.lock().unwrap().watchers.len(), 1);
        assert_eq!(kept.try_iter().count(), 2);
    }
}
