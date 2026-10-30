//! What is in front of what: a fullscreen window and the panels, and a
//! window the person keeps on top of the others.
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
//! The rule is applied in one place, [`Compositor::restack`], which runs
//! whenever the keyboard moves and before every publication of the facts:
//! that is after every change a window's fullscreen state can come from, so
//! no path that sets or clears it can leave a window stranded over the
//! panels. Smithay orders windows and layers by z-index, so raising a window
//! is giving it one above the `top` layer's. The facts and hit-testing stack
//! it there too, by asking [`covers_panels`], and the screen and pictures are
//! both drawn from [`stack`], the one account of what is in front of what on
//! a monitor.
//!
//! A window the person keeps on top (#64) is in front of every other window,
//! whichever has the keyboard, and under the panels; the fullscreen window
//! in use still covers it. Of two kept on top, the one raised last is in
//! front. Keeping a window on top changes how the stack is drawn, not how it
//! is ordered. Smithay's stack stays the order the windows were raised in,
//! which cycling focus walks and the keyboard goes back to, as it did before
//! any was kept on top, and a window raised under one kept on top is raised
//! behind it. A z-index of its own would have done the drawing, but smithay
//! sorts its whole stack by z-index at every raise, and the order the
//! windows were raised in would have been lost with it.
//!
//! A dialog or a menu stands with the window it hangs from. One of a window
//! kept on top is kept on top with it: worked out afresh each time from the
//! window it hangs from, never copied, so it is let go with its window. An
//! X11 menu, which is a window of its own here, is raised over the panels
//! with the fullscreen window it hangs from, as an xdg menu, drawn with its
//! window, always was; it was drawn under it, out of sight.
//!
//! So whatever asks what is in front reads the stack through [`as_drawn`]:
//! the screen and pictures, through [`stack`]; the pointer, through
//! [`window_under`]; and the facts an agent is judged by. None of them can
//! disagree with another. Smithay's own readers of its stack,
//! `Space::element_under` and `Space::render_elements_for_output`, are
//! refused by `clippy.toml`, so no new one reads the raise order as if it
//! were what is drawn.

use std::cell::Cell;

use smithay::{
    desktop::{
        Space,
        space::{RenderZindex, SpaceElement as _},
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point},
};

use perspicax_node::SurfaceId;

use super::{id_of, is_on_top, placement, wants_attention};
use crate::{framed::Framed, state::Compositor};

/// A window's z-index among the windows: smithay's default.
const AMONG_WINDOWS: u8 = RenderZindex::Shell as u8;
/// Over the `top` layer, where the panels are, and under `overlay`.
const OVER_PANELS: u8 = RenderZindex::Top as u8 + 1;

/// How many windows up [`Compositor::keeps_on_top`] looks: a dialog of a
/// dialog of a window is three. Smithay refuses an xdg window that would be
/// its own parent's parent, but nothing stops X11 windows naming each other
/// in `WM_TRANSIENT_FOR`, and the walk would go round them for ever.
const DEEPEST: usize = 8;

/// Whether a window has been raised over the panels: the fullscreen window
/// the person is using.
pub(crate) fn covers_panels(window: &Framed) -> bool {
    window.z_index() > RenderZindex::Top as u8
}

/// Whether a window is drawn in front of the others, as [`Compositor::restack`]
/// last worked it out. Kept apart from the person's own flag, in a cell of
/// its own, because it is read while windows are drawn and hit, where
/// nothing but the space is to hand.
struct Kept(Cell<bool>);

/// Mark `window` kept on top or not, saying whether that changed.
fn mark_kept(window: &Framed, kept: bool) -> bool {
    window
        .user_data()
        .insert_if_missing(|| Kept(Cell::new(false)));
    window
        .user_data()
        .get::<Kept>()
        .is_some_and(|flag| flag.0.replace(kept) != kept)
}

/// Whether `window` is kept in front of the other windows. A window never
/// worked out yet, just opened, is not.
pub(crate) fn is_kept_on_top(window: &Framed) -> bool {
    window
        .user_data()
        .get::<Kept>()
        .is_some_and(|flag| flag.0.get())
}

/// The windows in `space` back to front, as they are drawn: the order they
/// were raised in, with those kept on top in front of the rest, and the
/// fullscreen window in use in front of them all. Three passes over the
/// space rather than a sorted copy, since the pointer asks at every motion.
pub(crate) fn as_drawn(space: &Space<Framed>) -> impl DoubleEndedIterator<Item = &Framed> {
    let among = |kept: bool| {
        space
            .elements()
            .filter(move |window| !covers_panels(window) && is_kept_on_top(window) == kept)
    };
    among(false)
        .chain(among(true))
        .chain(space.elements().filter(|window| covers_panels(window)))
}

/// The window drawn frontmost at `at` whose input region takes it, and
/// where its surface's origin is: what smithay's `Space::element_under`
/// answers, asked of the windows as they are drawn ([`as_drawn`]) rather
/// than in the order they were raised.
pub(crate) fn window_under(
    space: &Space<Framed>,
    at: Point<f64, Logical>,
) -> Option<(&Framed, Point<i32, Logical>)> {
    as_drawn(space).rev().find_map(|window| {
        if !space.element_bbox(window)?.to_f64().contains(at) {
            return None;
        }
        let origin = space.element_location(window)? - window.geometry().loc;
        window
            .is_in_input_region(&(at - origin.to_f64()))
            .then_some((window, origin))
    })
}

#[cfg(any(feature = "seat", feature = "capture"))]
pub(crate) use drawn::{Stack, stack};

/// What is drawn, by a seat on its monitors and by a picture.
#[cfg(any(feature = "seat", feature = "capture"))]
mod drawn {
    use smithay::{
        desktop::{LayerSurface, Space, layer_map_for_output},
        output::Output,
        utils::{Logical, Point},
        wayland::shell::wlr_layer::Layer,
    };

    use super::{as_drawn, covers_panels};
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
        /// The other windows, those kept on top in front.
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
        for window in as_drawn(space).rev() {
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
pub(crate) fn is_fullscreen(window: &Framed) -> bool {
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
    /// The window `window` is a dialog or a menu of, mapped or parked: its
    /// xdg parent, or the window its X11 `WM_TRANSIENT_FOR` names, through
    /// any menus between.
    fn hangs_from(&self, window: &Framed) -> Option<Framed> {
        if let Some(toplevel) = window.toplevel() {
            return self.window_for(&toplevel.parent()?);
        }
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            let id = self.x11_from(x11)?;
            return self.window_for_id(id).or_else(|| self.parked_with(id));
        }
        None
    }

    /// Whether `window` is kept in front of the other windows: by the
    /// person, or with the window it hangs from, and so on up.
    fn keeps_on_top(&self, window: &Framed) -> bool {
        std::iter::successors(Some(window.clone()), |window| self.hangs_from(window))
            .take(DEEPEST)
            .any(|window| super::is_on_top(&window))
    }

    /// Work out which windows are kept on top, raise `focused`, the window
    /// in use -- the one with the keyboard, or whose own menu has it -- over
    /// the panels if it is fullscreen, with its X11 menus, and put every
    /// other window back among the windows.
    ///
    /// Told which window is in use rather than asking: while the seat is
    /// reporting a change of focus it holds the keyboard, and asking it then
    /// would wait on itself.
    pub(crate) fn restack(&mut self, focused: Option<SurfaceId>) {
        // Parked windows too: a workspace sliding in draws its own.
        let mut redraw = false;
        for window in self.space.elements().chain(&self.parked) {
            redraw |= mark_kept(window, self.keeps_on_top(window));
        }
        let raised = focused
            .and_then(|id| self.window_for_id(id))
            .filter(is_fullscreen)
            .as_ref()
            .and_then(id_of);
        // What moved: whether a window did, and the menus of the window in
        // use that did, back to front.
        let (mut moved, mut menus) = (false, Vec::new());
        for window in self.space.elements() {
            let menu = super::places_itself(window);
            let stands_with = if menu {
                self.hangs_from(window).as_ref().and_then(id_of)
            } else {
                id_of(window)
            };
            let over = raised.is_some() && stands_with == raised;
            let z = if over { OVER_PANELS } else { AMONG_WINDOWS };
            if window.z_index() == z {
                continue;
            }
            window.override_z_index(z);
            if !menu {
                moved = true;
            } else if focused.is_some() && stands_with == focused {
                menus.push(window.clone());
            }
        }
        // Smithay sorts its stack by z-index only when a window is raised.
        // Raising the focused window both sorts a newly raised one over the
        // rest and, when a fullscreen window was lowered because the person
        // moved on, keeps the window they moved to above it. Its menus go
        // over it again; a menu of the window they left stays under the
        // window they moved to, with its own.
        if moved && let Some(window) = focused.and_then(|id| self.window_for_id(id)) {
            self.space.raise_element(&window, false);
        }
        for menu in &menus {
            self.space.raise_element(menu, false);
        }
        if moved || !menus.is_empty() || redraw {
            self.backend.redraw();
        }
    }

    /// Raise `window` over the others, and its X11 menus over it: they are
    /// windows here, as an xdg menu is not, and would be left behind.
    ///
    /// An X11 window is raised in X's own stacking too, as one activated is,
    /// so a client asking what is on top is told the same. Not while it has a
    /// menu open, which X would then have under it, and which is what it is
    /// the client's business to have on top: a menu goes in no list of the
    /// windows a window manager stacks.
    pub(crate) fn raise_with_menus(&mut self, window: &Framed) {
        let menus: Vec<Framed> = self
            .space
            .elements()
            .filter(|other| {
                super::places_itself(other) && self.hangs_from(other).as_ref() == Some(window)
            })
            .cloned()
            .collect();
        self.space.raise_element(window, false);
        for menu in &menus {
            self.space.raise_element(menu, false);
        }
        #[cfg(feature = "xwayland")]
        if menus.is_empty()
            && let Some(x11) = window.x11_surface().cloned()
        {
            self.raise_x11(window, &x11);
        }
    }

    /// Put `window` just beneath the window in use, or beneath the one the
    /// keys would go back to, if they are with no window: where a window
    /// the person did not ask for opens (#259), in sight, and behind what
    /// they are typing into. Every window over that one stays over it.
    pub(crate) fn put_beneath(&mut self, window: &Framed) {
        let anchor = self
            .window_in_use()
            .and_then(|id| self.window_for_id(id))
            .or_else(|| self.window_for_keys())
            .filter(|anchor| anchor != window);
        let Some(anchor) = anchor else {
            return;
        };
        let over: Vec<Framed> = self
            .space
            .elements()
            .skip_while(|other| **other != anchor)
            .filter(|other| *other != window)
            .cloned()
            .collect();
        for other in &over {
            self.space.raise_element(other, false);
        }
    }

    /// Move `window` to `at`, as a window is placed while it opens, fitted
    /// to its frame or filling its monitor. One that wants attention keeps
    /// its place in the stack, beneath the window in use (#259): smithay's
    /// map brings it to the top, and what was over it goes back over it.
    /// Any other comes to the top, as it always has.
    pub(crate) fn move_window(&mut self, window: &Framed, at: Point<i32, Logical>) {
        let over: Vec<Framed> = if wants_attention(window) {
            self.space
                .elements()
                .skip_while(|other| *other != window)
                .skip(1)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        self.space.map_element(window.clone(), at, false);
        for other in &over {
            self.space.raise_element(other, false);
        }
    }

    /// Keep `window` in front of the other windows, or let it go back among
    /// them, if it is not so already: the person's, by a binding or the
    /// window's own titlebar button. Raised either way, as a click on it
    /// would raise it, so a window kept on top is in front of the others
    /// kept on top, and one let go is in front of the windows it goes back
    /// among rather than lost behind them.
    pub(crate) fn set_on_top(&mut self, window: &Framed, on_top: bool) {
        if is_on_top(window) == on_top {
            return;
        }
        placement(window, |placement| placement.on_top = on_top);
        tracing::debug!(id = ?id_of(window), on_top, "kept on top, or let go");
        self.space.raise_element(window, false);
        self.backend.redraw();
        // Published first, for that works out which windows are kept on
        // top, and the pointer then finds what is now in front of it.
        self.publish_facts();
        self.repoint();
    }

    /// Keep `window` in front of the other windows, or let it go back among
    /// them if it is kept there.
    pub(crate) fn toggle_on_top(&mut self, window: &Framed) {
        self.set_on_top(window, !is_on_top(window));
    }
}
