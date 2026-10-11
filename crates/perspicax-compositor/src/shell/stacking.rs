//! A fullscreen window and the panels: which is on top.
//!
//! Layer-shell stacks a panel on the `top` layer over every window, and a
//! fullscreen video under a taskbar is the result. Plasma and Windows answer
//! it the same way, and so does this: the fullscreen window the person is
//! using covers the panels, and one they have left goes back under them, so
//! the panel shows again while they work in another window or open the
//! start menu. A menu of the window's own, a video's right-click menu, does
//! not leave it: its keys are the window's, and the window stays over the
//! panels. `overlay` surfaces stay over everything, a fullscreen window
//! included: a launcher or a menu opened over a video is still seen.
//!
//! The rule is applied in one place, [`Compositor::stack_fullscreen`], which
//! runs whenever the keyboard moves and before every publication of the
//! facts: that is after every change a window's fullscreen state can come
//! from, so no path that sets or clears it can leave a window stranded over
//! the panels. Smithay orders windows and layers by z-index, so raising a
//! window is giving it one above the `top` layer's. The facts and
//! hit-testing stack it there too, by asking [`covers_panels`], and the
//! screen and pictures are both drawn from [`stack`], the one account of what
//! is in front of what on a monitor.

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

#[cfg(any(feature = "seat", feature = "capture"))]
pub(crate) use drawn::{Stack, stack};

/// What is drawn, by a seat on its monitors and by a picture.
#[cfg(any(feature = "seat", feature = "capture"))]
mod drawn {
    use smithay::{
        desktop::{LayerSurface, Space, layer_map_for_output, space::SpaceElement as _},
        output::Output,
        utils::{Logical, Point},
        wayland::shell::wlr_layer::Layer,
    };

    use super::covers_panels;
    use crate::framed::Framed;

    /// What a monitor shows, front to back: each group in front of the next,
    /// and within one, each piece in front of the next. A piece is where its
    /// surface's origin is drawn, in the monitor's own logical coordinates.
    pub(crate) struct Stack {
        /// `overlay` surfaces: a menu, a launcher, over everything.
        pub(crate) overlay: Vec<(LayerSurface, Point<i32, Logical>)>,
        /// The fullscreen window in use, over the panels.
        pub(crate) raised: Vec<(Framed, Point<i32, Logical>)>,
        /// `top` surfaces: the panels.
        pub(crate) top: Vec<(LayerSurface, Point<i32, Logical>)>,
        pub(crate) windows: Vec<(Framed, Point<i32, Logical>)>,
        /// `bottom` and `background` surfaces: the wallpaper.
        pub(crate) lower: Vec<(LayerSurface, Point<i32, Logical>)>,
    }

    /// How `space` stacks on `output`, or `None` for an output it does not
    /// show.
    ///
    /// Each layer surface is where its layer map placed it. Smithay's own
    /// `Space::render_elements_for_output` draws every layer surface at the
    /// output's corner whatever its anchor, which drew a panel along the
    /// bottom at the top of the screen (found on hardware): the screen is
    /// drawn from this instead, as pictures are.
    pub(crate) fn stack(space: &Space<Framed>, output: &Output) -> Option<Stack> {
        let area = space.output_geometry(output)?;
        let mut stack = Stack {
            overlay: Vec::new(),
            raised: Vec::new(),
            top: Vec::new(),
            windows: Vec::new(),
            lower: Vec::new(),
        };
        let layers = layer_map_for_output(output);
        for layer in layers.layers().rev() {
            let Some(placed) = layers.layer_geometry(layer) else {
                continue;
            };
            let piece = (layer.clone(), placed.loc);
            match layer.layer() {
                Layer::Overlay => stack.overlay.push(piece),
                Layer::Top => stack.top.push(piece),
                Layer::Background | Layer::Bottom => stack.lower.push(piece),
            }
        }
        drop(layers);
        for window in space.elements().rev() {
            let (Some(bbox), Some(location)) =
                (space.element_bbox(window), space.element_location(window))
            else {
                continue;
            };
            if !bbox.overlaps(area) {
                continue;
            }
            let piece = (window.clone(), location - window.geometry().loc - area.loc);
            if covers_panels(window) {
                stack.raised.push(piece);
            } else {
                stack.windows.push(piece);
            }
        }
        Some(stack)
    }
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
    /// Raise `focused`, the window in use -- the one with the keyboard, or
    /// whose own menu has it -- over the panels if it is fullscreen, and put
    /// every other window back among the windows.
    ///
    /// Told which window is in use rather than asking: while the seat is
    /// reporting a change of focus it holds the keyboard, and asking it then
    /// would wait on itself.
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
