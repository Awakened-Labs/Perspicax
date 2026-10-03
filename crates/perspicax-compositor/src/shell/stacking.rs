//! A fullscreen window and the panels: which is on top.
//!
//! Layer-shell stacks a panel on the `top` layer over every window, and a
//! fullscreen video under a taskbar is the result. Plasma and Windows answer
//! it the same way, and so does this: the fullscreen window the person is
//! using covers the panels, and one they have left goes back under them, so
//! the panel shows again while they work in another window or open a menu.
//! `overlay` surfaces stay over everything, a fullscreen window included:
//! a launcher or a menu opened over a video is still seen.
//!
//! The rule is applied in one place, [`Compositor::stack_fullscreen`], which
//! runs whenever the keyboard moves and before every publication of the
//! facts: that is after every change a window's fullscreen state can come
//! from, so no path that sets or clears it can leave a window stranded over
//! the panels. Smithay orders windows and layers by z-index, so raising a
//! window is giving it one above the `top` layer's. The facts, hit-testing
//! and pictures stack it there too, by asking [`covers_panels`].

use smithay::{
    desktop::space::{RenderZindex, SpaceElement as _},
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
};

use perspicax_node::SurfaceId;

use super::id_of;
use crate::{framed::Framed, state::Compositor};

/// A window's z-index among the windows: smithay's default.
const AMONG_WINDOWS: u8 = RenderZindex::Shell as u8;
/// Over the `top` layer, where the panels are, and under `overlay`.
const OVER_PANELS: u8 = RenderZindex::Top as u8 + 1;

/// Whether a window has been raised over the panels: the fullscreen window
/// the person is using.
pub(crate) fn covers_panels(window: &Framed) -> bool {
    window.z_index() > RenderZindex::Top as u8
}

/// Whether a window is fullscreen: asked to be, for a Wayland window, or
/// said to be, for an X11 one.
fn is_fullscreen(window: &Framed) -> bool {
    if let Some(toplevel) = window.toplevel() {
        return toplevel.with_pending_state(|pending| {
            pending.states.contains(xdg_toplevel::State::Fullscreen)
        });
    }
    #[cfg(feature = "xwayland")]
    if let Some(x11) = window.x11_surface() {
        return x11.is_fullscreen();
    }
    false
}

impl Compositor {
    /// Raise `focused`, the window with the keyboard, over the panels if it
    /// is fullscreen, and put every other window back among the windows.
    ///
    /// Told which window has the keyboard rather than asking: while the seat
    /// is reporting a change of focus it holds the keyboard, and asking it
    /// then would wait on itself.
    pub(crate) fn stack_fullscreen(&mut self, focused: Option<SurfaceId>) {
        let mut changed = false;
        for window in self.space.elements() {
            let over = focused.is_some() && id_of(window) == focused && is_fullscreen(window);
            let z = if over { OVER_PANELS } else { AMONG_WINDOWS };
            if window.z_index() != z {
                window.override_z_index(z);
                changed = true;
            }
        }
        if !changed {
            return;
        }
        // Smithay sorts its stack by z-index only when a window is raised.
        // Raising the focused window both sorts a newly raised one over the
        // rest and, when a fullscreen window was lowered because the person
        // moved on, keeps the window they moved to above it.
        if let Some(window) = focused.and_then(|id| self.window_for_id(id)) {
            self.space.raise_element(&window, false);
        }
        self.backend.redraw();
    }
}
