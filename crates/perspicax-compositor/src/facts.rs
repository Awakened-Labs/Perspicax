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

use std::sync::{Arc, Mutex, PoisonError, RwLock, mpsc};

use perspicax_index::{HostFacts, SurfaceFacts, SurfaceKind};
use perspicax_node::{Origin, Rect, SurfaceId, Vec2};
use smithay::{
    reexports::wayland_server::{Resource as _, protocol::wl_surface::WlSurface},
    utils::IsAlive,
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
        // so this is where it is settled whether one covers the panels.
        self.stack_fullscreen(self.focused_surface());
        self.generation += 1;
        let generation = self.generation;

        // `Space::elements()` iterates back to front, which is the order
        // `HostFacts::bottom_to_top` wants. The two agree by construction
        // rather than through a conversion somebody has to keep right.
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
        let (raised, windows): (Vec<&crate::framed::Framed>, Vec<_>) = self
            .space
            .elements()
            .partition(|window| crate::shell::covers_panels(window));
        // How see-through each window on screen is drawn, for an agent
        // puzzled by a picture. Never judged by: see `drawn_opacity`.
        let in_use = self.window_in_use();
        let window = |window: &crate::framed::Framed| {
            let mut facts = self.grouped(window, self.facts_for(window)?);
            if facts.mapped {
                facts.drawn_opacity = Some(self.opacity_of(window, in_use))
                    .filter(|&percent| percent < perspicax_policy::OPAQUE);
            }
            Some(facts)
        };
        let layer = |(layer, placed): &(smithay::desktop::LayerSurface, _)| {
            let id = *layer.user_data().get::<SurfaceId>()?;
            let mut facts = self.plain_facts(id, layer.wl_surface(), *placed);
            facts.kind = SurfaceKind::Layer {
                layer: crate::layers::level(layer.layer()),
                namespace: layer.namespace().to_owned(),
            };
            Some(facts)
        };
        let covers = self
            .lock
            .iter()
            .flat_map(|locked| &locked.surfaces)
            .filter_map(|cover| {
                let area = self.space.output_geometry(&cover.output)?;
                Some(
                    self.plain_facts(cover.id, cover.surface.wl_surface(), area)
                        .lock_cover(),
                )
            });
        let surfaces: Vec<SurfaceFacts> = below
            .iter()
            .filter_map(layer)
            .chain(self.parked.iter().chain(windows).filter_map(window))
            .chain(top.iter().filter_map(layer))
            .chain(raised.into_iter().filter_map(window))
            .chain(overlay.iter().filter_map(layer))
            .chain(covers)
            .collect();

        self.facts.publish(
            HostFacts::bottom_to_top(surfaces, generation)
                .with_consent(self.consent.clone())
                .with_outputs(self.output_rects().into_iter().map(|area| {
                    Rect::new(
                        f64::from(area.x),
                        f64::from(area.y),
                        f64::from(area.x + area.w),
                        f64::from(area.y + area.h),
                    )
                })),
        );
        self.announce();
    }

    /// Tell the clients that watch other programs' windows what changed.
    /// After every publication, because a publication follows every change
    /// that could matter to them.
    fn announce(&mut self) {
        self.sync_toplevels();
        self.sync_workspaces();
        self.sync_heads();
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
        // without the shadow, which is what window space is measured from.
        // Which origin each toolkit's own coordinates are measured from is a
        // different question, and the index answers it from the toolkit's
        // window node (`Index::window_origin`).
        //
        // A window that has never drawn and declared nothing has none, and
        // is not described: it is on nobody's screen. Until issue #44, one
        // that drew without declaring was left out too, and so covered
        // nothing, so a node under it was judged visible.
        let geometry = crate::geometry::window_geometry(surface)?;

        // What the client declared opaque, on its own surface. That says
        // nothing about its subsurfaces, which are drawn over it, so where
        // they are opaque is added; one that declared nothing covers all
        // of itself. A surface that declared nothing proves nothing either,
        // so with no region on the root the whole window covers.
        let opaque = with_states(surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .opaque_region
                .as_ref()
                .map(regions)
        })
        .map(|mut opaque| {
            opaque.extend(crate::geometry::covers(surface));
            opaque
        });
        let hidden = if parked {
            self.hidden_why(window)
        } else {
            (None, None)
        };

        Some(SurfaceFacts {
            id,
            // Alive, and carrying something to look at. A toplevel that has been
            // created but never presented a buffer is a window in name only,
            // and reporting it as mapped would let a node be judged visible on
            // a surface with nothing on it.
            mapped: window.alive() && self.has_presented(id) && !parked,
            geometry: Rect::new(
                f64::from(location.x),
                f64::from(location.y),
                f64::from(location.x + geometry.size.w),
                f64::from(location.y + geometry.size.h),
            ),
            // Surface-local (0,0) sits at the geometry's own offset *back*
            // from where we placed that geometry -- under CSD that is the
            // shadow margin, and it is where opaque regions are measured
            // from.
            buffer_origin: Vec2::new(
                f64::from(location.x - geometry.loc.x),
                f64::from(location.y - geometry.loc.y),
            ),
            opaque,
            origin: self.origin_of(window),
            title: title(surface),
            focused_at: self.focused_at(id),
            damage_generation: self.damage_generation(id),
            damage: self.damage_history(id),
            off_workspace: hidden.0,
            behind_tab: hidden.1,
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

    /// The facts for a surface that is not a window -- a layer surface, a lock
    /// surface -- placed by the compositor at `placed`. No declared window
    /// geometry and no shadow margin: the rectangle is the surface.
    fn plain_facts(
        &self,
        id: SurfaceId,
        surface: &WlSurface,
        placed: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) -> SurfaceFacts {
        let opaque = with_states(surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .opaque_region
                .as_ref()
                .map(regions)
        });
        SurfaceFacts {
            id,
            mapped: surface.is_alive() && self.has_presented(id) && !placed.size.is_empty(),
            geometry: Rect::new(
                f64::from(placed.loc.x),
                f64::from(placed.loc.y),
                f64::from(placed.loc.x + placed.size.w),
                f64::from(placed.loc.y + placed.size.h),
            ),
            buffer_origin: Vec2::new(f64::from(placed.loc.x), f64::from(placed.loc.y)),
            opaque,
            origin: self.origin_of_surface(surface),
            // A layer's namespace is not a title: it goes in `kind`, set by
            // the caller, which the join reads in a title's place.
            title: None,
            focused_at: self.focused_at(id),
            damage_generation: self.damage_generation(id),
            damage: self.damage_history(id),
            off_workspace: None,
            behind_tab: None,
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
            None => SurfaceFacts::new(
                id,
                Rect::new(
                    f64::from(location.x),
                    f64::from(location.y),
                    f64::from(location.x + size.w),
                    f64::from(location.y + size.h),
                ),
            )
            .unmapped(),
        };
        if self.parked.contains(window) {
            facts.mapped = false;
            (facts.off_workspace, facts.behind_tab) = self.hidden_why(window);
        } else {
            facts.frame = self.frame_facts(window, placed);
        }
        facts.origin = Self::x11_origin(window);
        facts.title = Some(x11.title()).filter(|title| !title.is_empty());
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

/// The `Add` rectangles of a region, in surface-local coordinates.
///
/// `Subtract` rectangles are dropped, and the direction of that error is the
/// reason it is acceptable: ignoring a subtraction can only make this compositor
/// believe a surface is opaque over more of itself than it really is, which
/// refuses more nodes rather than fewer. The opposite rounding would report a
/// covered node as visible.
pub(crate) fn regions(region: &smithay::wayland::compositor::RegionAttributes) -> Vec<Rect> {
    region
        .rects
        .iter()
        .filter(|(kind, _)| matches!(kind, RectangleKind::Add))
        .map(|(_, rect)| {
            Rect::new(
                f64::from(rect.loc.x),
                f64::from(rect.loc.y),
                f64::from(rect.loc.x + rect.size.w),
                f64::from(rect.loc.y + rect.size.h),
            )
        })
        .collect()
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
