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
//! That is also what lets [`HostView`](wm_index::HostView)'s three read methods
//! be synchronous. They answer from a published snapshot rather than by asking
//! a compositor a question and waiting for its event loop to get round to it.
//! Acting has to cross in the other direction and will be a message, not a
//! lock.
//!
//! # A snapshot is consistent, and it is not current
//!
//! A reader holds a set of facts that were true when the compositor published
//! them. That is a feature: judging half a tree against one arrangement of
//! windows and the other half against the next would produce a verdict that was
//! never true at any instant. Currency is what the generation counter and the
//! damage bookkeeping are for -- being *behind* is a state the index can
//! detect and refuse on, whereas being *inconsistent* is not.

use std::sync::{Arc, PoisonError, RwLock};

use smithay::{
    desktop::Window,
    reexports::wayland_server::Resource as _,
    utils::IsAlive,
    wayland::compositor::{RectangleKind, SurfaceAttributes, with_states},
};
use wm_index::{HostFacts, SurfaceFacts};
use wm_node::{Origin, Rect, SurfaceId, Vec2};

use crate::state::{ClientState, Compositor};

/// A handle to whatever the compositor last published.
///
/// Cloning it is cheap and shares one snapshot; every clone sees each
/// publication. Created by the caller and handed to
/// [`run`](crate::run), so that whoever wants to read the facts does not have
/// to be the thread running the compositor.
#[derive(Debug, Clone, Default)]
pub struct Facts(Arc<RwLock<HostFacts>>);

impl Facts {
    /// A handle to no facts at all, which is what a compositor that has not
    /// started yet honestly knows.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replace the snapshot.
    fn publish(&self, facts: HostFacts) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = facts;
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
        self.generation += 1;
        let generation = self.generation;

        // `Space::elements()` iterates back to front, which is the order
        // `HostFacts::bottom_to_top` wants. The two agree by construction
        // rather than through a conversion somebody has to keep right.
        let surfaces: Vec<SurfaceFacts> = self
            .space
            .elements()
            .filter_map(|window| self.facts_for(window))
            .collect();

        self.facts
            .publish(HostFacts::bottom_to_top(surfaces, generation));
    }

    /// One window's facts, or `None` if it has no id yet -- which means it has
    /// not been mapped through `new_toplevel` and is not ours to describe.
    fn facts_for(&self, window: &Window) -> Option<SurfaceFacts> {
        let id = *window.user_data().get::<SurfaceId>()?;
        let toplevel = window.toplevel()?;
        let surface = toplevel.wl_surface();

        let geometry = self.space.element_geometry(window)?;
        let bbox = self.space.element_bbox(window)?;

        let (opaque, mapped) = with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            let current = attributes.current();
            (
                current.opaque_region.as_ref().map(regions),
                current.buffer.is_some() || !geometry.size.is_empty(),
            )
        });

        Some(SurfaceFacts {
            id,
            // Alive and carrying something to look at. A toplevel that has been
            // created but never committed a buffer is a window in name only,
            // and reporting it as mapped would let a node be judged visible on
            // a surface with nothing on it.
            mapped: window.alive() && mapped && !geometry.size.is_empty(),
            geometry: to_rect(geometry),
            // Zero until it is measured against a real toolkit. Guessing the
            // decoration margin here would be the single easiest way to build a
            // system that clicks confidently beside the button.
            node_space_offset: Vec2::ZERO,
            // The buffer starts at the bounding box, which under client-side
            // decoration is outside the window geometry by the shadow margin.
            buffer_origin: Vec2::new(f64::from(bbox.loc.x), f64::from(bbox.loc.y)),
            opaque,
            origin: self.origin_of(window),
            damage_generation: 0,
        })
    }

    /// Who owns the client that drew this window.
    ///
    /// Read from the per-client state recorded when the connection was
    /// accepted, not asked for again here: the credentials are a fact about a
    /// socket at the moment it was accepted, and re-deriving them later from a
    /// pid that may have been recycled would be strictly worse information
    /// wearing a fresher timestamp.
    fn origin_of(&self, window: &Window) -> Origin {
        let Some(toplevel) = window.toplevel() else {
            return Origin::Unattributed;
        };
        self.display
            .get_client(toplevel.wl_surface().id())
            .ok()
            .and_then(|client| client.get_data::<ClientState>().map(ClientState::origin))
            .unwrap_or(Origin::Unattributed)
    }
}

/// The `Add` rectangles of a region, in surface-local coordinates.
///
/// `Subtract` rectangles are dropped, and the direction of that error is the
/// reason it is acceptable: ignoring a subtraction can only make this compositor
/// believe a surface is opaque over more of itself than it really is, which
/// refuses more nodes rather than fewer. The opposite rounding would report a
/// covered node as visible.
fn regions(region: &smithay::wayland::compositor::RegionAttributes) -> Vec<Rect> {
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

/// Smithay's integer logical rectangle as the schema's floating-point one.
fn to_rect(rect: smithay::utils::Rectangle<i32, smithay::utils::Logical>) -> Rect {
    Rect::new(
        f64::from(rect.loc.x),
        f64::from(rect.loc.y),
        f64::from(rect.loc.x + rect.size.w),
        f64::from(rect.loc.y + rect.size.h),
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
}
