//! Resistance, carried out: a window being moved stops at the edge of a
//! screen or a panel until it is pushed far enough past it.
//!
//! Which edges hold, and how, is `perspicax_policy::resist`'s decision, made
//! from where the window's frame is and where the pointer would put it. The
//! frame is the one a person sees: the client's own geometry, without a
//! client-drawn shadow, and the titlebar and border this compositor draws
//! around it. Not [`Framed`]'s bounding box, which also takes in the
//! invisible margin a resize can be started from.

use perspicax_policy::{Rect, outset, resist};
use smithay::utils::{Logical, Point};

use super::rect;
use crate::{framed::Framed, state::Compositor};

impl Compositor {
    /// Where a window being moved goes when the pointer would put it at
    /// `free`: there, or held against an edge it is being pushed past.
    ///
    /// Where the window is now is where the grab last put it. A motion after
    /// anything else moved the window, or the pointer, is a jump rather than a
    /// push, and the grab places it at `free` without asking.
    pub(crate) fn resist(&self, window: &Framed, free: Point<i32, Logical>) -> Point<i32, Logical> {
        let Some(resistance) = self
            .backend
            .resistance()
            .filter(|resistance| resistance.edges > 0 || resistance.seams > 0)
        else {
            return free;
        };
        let Some(extent) = self.extent(window) else {
            return free;
        };
        let insets = self.insets(window);
        let placed = outset(rect(extent), insets);
        // The same frame, following the pointer.
        let following = Rect {
            x: free.x - insets.left,
            y: free.y - insets.top,
            ..placed
        };
        let monitors: Vec<_> = self
            .space
            .outputs()
            .filter_map(|output| {
                let whole = self.space.output_geometry(output)?;
                Some((rect(whole), rect(self.usable_area(output)?)))
            })
            .collect();
        let (x, y) = resist(placed, following, &monitors, resistance);
        (x + insets.left, y + insets.top).into()
    }
}
