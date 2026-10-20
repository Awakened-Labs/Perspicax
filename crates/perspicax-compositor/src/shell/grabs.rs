//! The pointer, owned by a move or a resize until the button comes up.
//!
//! While a grab holds the pointer no client has pointer focus: the window
//! being dragged must not see its own drag as ordinary motion, and nothing
//! under the pointer should light up as it passes. Everything a grab does not
//! care about (scrolling, gestures) is passed through unchanged.
//!
//! A grab is finished in `unset`, not on the button coming up, because
//! whatever ends a grab goes through `unset`: the release, another grab
//! taking the pointer, or the session locking (`Compositor::let_go`). There,
//! a moved window's new place is told to the workspaces and the facts, and a
//! resized one is told its resize is over. A tab dropped nowhere goes back
//! to where it was picked up.
//!
//! A window can go while a grab still holds it: closed, or unmapped by its
//! client. The grab then goes on to the release with nothing in it: the
//! window is not moved, not told, and not put back.

use perspicax_policy::{Edges, Rect, dragged, resize};
use smithay::{
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};

use super::{Resize, placement};
use crate::{framed::Framed, state::Compositor};

/// The half of `PointerGrab` neither grab changes: scrolling, frames and
/// gestures go wherever they would have gone.
macro_rules! pass_through {
    () => {
        fn relative_motion(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            focus: Option<(crate::focus::FocusTarget, Point<f64, Logical>)>,
            event: &RelativeMotionEvent,
        ) {
            handle.relative_motion(data, focus, event);
        }
        fn axis(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            details: AxisFrame,
        ) {
            handle.axis(data, details);
        }
        fn frame(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
        ) {
            handle.frame(data);
        }
        fn gesture_swipe_begin(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            event: &GestureSwipeBeginEvent,
        ) {
            handle.gesture_swipe_begin(data, event);
        }
        fn gesture_swipe_update(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            event: &GestureSwipeUpdateEvent,
        ) {
            handle.gesture_swipe_update(data, event);
        }
        fn gesture_swipe_end(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            event: &GestureSwipeEndEvent,
        ) {
            handle.gesture_swipe_end(data, event);
        }
        fn gesture_pinch_begin(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            event: &GesturePinchBeginEvent,
        ) {
            handle.gesture_pinch_begin(data, event);
        }
        fn gesture_pinch_update(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            event: &GesturePinchUpdateEvent,
        ) {
            handle.gesture_pinch_update(data, event);
        }
        fn gesture_pinch_end(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            event: &GesturePinchEndEvent,
        ) {
            handle.gesture_pinch_end(data, event);
        }
        fn gesture_hold_begin(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            event: &GestureHoldBeginEvent,
        ) {
            handle.gesture_hold_begin(data, event);
        }
        fn gesture_hold_end(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            event: &GestureHoldEndEvent,
        ) {
            handle.gesture_hold_end(data, event);
        }
    };
}

/// The window in the person's hand, and what the hand is doing with it, from
/// the grab's start until it is unset. Either way it goes where the person
/// goes across a switch (see `Compositor::show_what_belongs`), but only a
/// window being moved is carried across an edge of the desk: resizing up to
/// one is not a push to the next workspace.
pub(crate) enum Hand {
    Moving(Framed),
    Resizing(Framed),
}

impl Hand {
    pub(crate) fn window(&self) -> &Framed {
        match self {
            Self::Moving(window) | Self::Resizing(window) => window,
        }
    }
}

/// Moving a window: it follows the pointer from where it was grabbed, and
/// the edges of screens and panels hold it a while (see `shell::resist`).
pub(crate) struct MoveGrab {
    start: GrabStartData<Compositor>,
    window: Framed,
    origin: Point<i32, Logical>,
    /// The window is maximized or snapped, and has not been restored yet: it
    /// is, once the pointer has travelled far enough to make the press a
    /// drag. Until then nothing moves, so a click is only a click.
    filled: bool,
    /// Where this grab last put the window. Anywhere else, and something
    /// else moved it.
    last: Point<i32, Logical>,
    /// [`Compositor::desk_jumps`] as this grab last saw it.
    jumps: u64,
}

impl MoveGrab {
    pub(crate) fn new(
        start: GrabStartData<Compositor>,
        window: Framed,
        origin: Point<i32, Logical>,
        filled: bool,
        jumps: u64,
    ) -> Self {
        Self {
            start,
            window,
            origin,
            filled,
            last: origin,
            jumps,
        }
    }

    /// Whether the window is still on screen to be moved. One minimized, put
    /// behind its group or closed mid-drag has left the person's hand, and
    /// the drag goes on to the release with nothing in it: the grab never
    /// puts back what something else took away.
    fn holds(&self, data: &Compositor) -> bool {
        data.space.element_location(&self.window).is_some()
    }
}

/// Resizing a window: every motion asks the client for a new size, and the
/// client's commits are anchored by [`Compositor::settle_resize`].
pub(crate) struct ResizeGrab {
    start: GrabStartData<Compositor>,
    window: Framed,
    edges: Edges,
    from: Rect,
    last: (i32, i32),
}

impl ResizeGrab {
    pub(crate) fn new(
        start: GrabStartData<Compositor>,
        window: Framed,
        edges: Edges,
        from: Rect,
    ) -> Self {
        placement(&window, |placement| {
            placement.resize = Some(Resize {
                start: from,
                edges,
                released: false,
            });
        });
        Self {
            start,
            window,
            edges,
            from,
            last: (from.w, from.h),
        }
    }

    /// Whether the window is still on screen to be resized. One minimized or
    /// put behind its group mid-resize has left the person's hand, as one
    /// being moved does: it is asked for no new size, and so is never mapped
    /// again by its commits (see [`Compositor::settle_resize`]).
    fn holds(&self, data: &Compositor) -> bool {
        data.space.element_location(&self.window).is_some()
    }

    /// Ask the client for a size, marked as mid-resize or not.
    fn configure(&self, resizing: bool) {
        let Some(toplevel) = self.window.toplevel() else {
            return;
        };
        toplevel.with_pending_state(|pending| {
            if resizing {
                pending.states.set(xdg_toplevel::State::Resizing);
            } else {
                pending.states.unset(xdg_toplevel::State::Resizing);
            }
            pending.size = Some(self.last.into());
        });
        toplevel.send_pending_configure();
    }
}

/// The smallest and largest size a window declared, `0` meaning no limit:
/// from its xdg cached state, or an X client's size hints.
pub(super) fn limits(window: &Framed) -> Option<(Size<i32, Logical>, Size<i32, Logical>)> {
    if let Some(toplevel) = window.toplevel() {
        return Some(with_states(toplevel.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            (current.min_size, current.max_size)
        }));
    }
    #[cfg(feature = "xwayland")]
    if let Some(x11) = window.x11_surface() {
        // Smithay builds a `Size` of an X client's hints as they are, which
        // a debug build asserts against for a negative side (#106): read
        // them first, and take a limit with one as no limit.
        let hints = x11.size_hints();
        let stated = |limit: Option<(i32, i32)>| limit.is_some_and(|(w, h)| w >= 0 && h >= 0);
        let min = stated(hints.and_then(|hints| hints.min_size)).then(|| x11.min_size());
        let max = stated(hints.and_then(|hints| hints.max_size)).then(|| x11.max_size());
        return Some((
            min.flatten().unwrap_or_default(),
            max.flatten().unwrap_or_default(),
        ));
    }
    None
}

/// A tab being dragged by its titlebar with the middle button, to join
/// another window's group or leave its own. Once the press has become a drag,
/// the tab comes to the front of its group and follows the pointer, and the
/// window it would join is outlined.
#[cfg(feature = "seat")]
pub(crate) struct TabDragGrab {
    start: GrabStartData<Compositor>,
    tab: Framed,
    /// Where the tab was when the drag began, and so where its group stays.
    /// `None` until the press has travelled far enough to be a drag: a
    /// middle click on a titlebar does nothing.
    home: Option<Point<i32, Logical>>,
}

#[cfg(feature = "seat")]
impl TabDragGrab {
    pub(crate) fn new(start: GrabStartData<Compositor>, tab: Framed) -> Self {
        Self {
            start,
            tab,
            home: None,
        }
    }

    /// Whether the tab is still one of the person's: on screen, or parked.
    /// One that went mid-drag is never mapped again by it, nor dropped.
    fn holds(&self, data: &Compositor) -> bool {
        data.has_window(&self.tab)
    }
}

#[cfg(feature = "seat")]
impl PointerGrab<Compositor> for TabDragGrab {
    fn motion(
        &mut self,
        data: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        _focus: Option<(crate::focus::FocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        if !self.holds(data) {
            if data.tab_drop.take().is_some() {
                data.backend.redraw();
            }
            return;
        }
        let (from, now) = (self.start.location, event.location);
        let home = match self.home {
            Some(home) => home,
            None if dragged((from.x, from.y), (now.x, now.y)) => {
                // A tab behind its group comes forward to be dragged.
                data.activate_tab(&self.tab);
                let Some(home) = data.space.element_location(&self.tab) else {
                    return;
                };
                self.home = Some(home);
                home
            }
            None => return,
        };
        let to = (home.to_f64() + (now - from)).to_i32_round();
        data.space.map_element(self.tab.clone(), to, false);
        data.space.raise_element(&self.tab, false);
        #[cfg(feature = "xwayland")]
        if let Some(x11) = self.tab.x11_surface() {
            let _ = x11.configure(smithay::utils::Rectangle::new(to, x11.geometry().size));
        }
        data.tab_drop = data.tab_target(&self.tab, now).map(|(_, outline)| outline);
        data.backend.redraw();
    }

    fn button(
        &mut self,
        data: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            let at = handle.current_location();
            // Taken before the grab is unset, which puts back a tab that
            // was never dropped.
            let home = self.home.take();
            handle.unset_grab(self, data, event.serial, event.time, true);
            if let Some(home) = home
                && self.holds(data)
            {
                data.drop_tab(&self.tab, at, home);
            }
        }
    }

    pass_through!();

    fn start_data(&self) -> &GrabStartData<Compositor> {
        &self.start
    }

    /// Ended with the tab still in hand, before its button came up: it is
    /// dropped nowhere, and goes back to where it was picked up. Not if
    /// something took it off screen meanwhile.
    fn unset(&mut self, data: &mut Compositor) {
        data.tab_drop = None;
        let Some(home) = self.home.take() else {
            return;
        };
        if data.space.element_location(&self.tab).is_none() {
            return;
        }
        data.space.map_element(self.tab.clone(), home, false);
        #[cfg(feature = "xwayland")]
        if let Some(x11) = self.tab.x11_surface() {
            let _ = x11.configure(smithay::utils::Rectangle::new(home, x11.geometry().size));
        }
        data.backend.redraw();
    }
}

/// Round a pointer delta to whole logical pixels.
fn whole(delta: Point<f64, Logical>) -> (i32, i32) {
    let delta = delta.to_i32_round();
    (delta.x, delta.y)
}

impl PointerGrab<Compositor> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        _focus: Option<(crate::focus::FocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        if !self.holds(data) {
            if data.snap_preview.take().is_some() {
                data.backend.redraw();
            }
            return;
        }
        if self.filled {
            let (from, now) = (self.start.location, event.location);
            if !dragged((from.x, from.y), (now.x, now.y)) {
                return;
            }
            self.filled = false;
            data.release_fill(&self.window, from);
            if let Some(origin) = data.space.element_location(&self.window) {
                self.origin = origin;
            }
        }
        let free = (self.origin.to_f64() + (event.location - self.start.location)).to_i32_round();
        // An edge holds the window against a push, not a jump: the pointer
        // carried round the desk by a flip, or the window put somewhere new
        // by anything but this grab, as the restore just above does.
        let jumped = self.jumps != data.desk_jumps
            || data.space.element_location(&self.window) != Some(self.last);
        self.jumps = data.desk_jumps;
        let to = if jumped {
            free
        } else {
            data.resist(&self.window, free)
        };
        self.last = to;
        data.space.map_element(self.window.clone(), to, false);
        // An X client keeps its own idea of where it is, and places its
        // menus from it, so it is told.
        #[cfg(feature = "xwayland")]
        if let Some(x11) = self.window.x11_surface() {
            let _ = x11.configure(smithay::utils::Rectangle::new(to, x11.geometry().size));
        }
        let snapping = data.backend.snapping();
        data.track_snap(event.location, snapping);
        data.backend.redraw();
    }

    fn button(
        &mut self,
        data: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            // Snapped only on the button coming up, and before the grab is
            // unset, which forgets the preview.
            if self.holds(data) {
                data.finish_snap(&self.window);
            }
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    pass_through!();

    fn start_data(&self) -> &GrabStartData<Compositor> {
        &self.start
    }

    fn unset(&mut self, data: &mut Compositor) {
        data.hand = None;
        if data.snap_preview.take().is_some() {
            data.backend.redraw();
        }
        if !self.holds(data) {
            return;
        }
        // Per output, a window dragged onto another monitor joins the
        // workspace that monitor is showing.
        data.window_moved(&self.window);
        // Once, where it landed, rather than on every motion: occlusion is
        // judged against where a window is, not everywhere it passed.
        data.publish_facts();
    }
}

impl PointerGrab<Compositor> for ResizeGrab {
    fn motion(
        &mut self,
        data: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        _focus: Option<(crate::focus::FocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        if !self.holds(data) {
            return;
        }
        let Some((min, max)) = limits(&self.window) else {
            return;
        };
        self.last = resize(
            self.from,
            self.edges,
            whole(event.location - self.start.location),
            (min.w, min.h),
            (max.w, max.h),
        );
        self.configure(true);
        // X11 has no configure-and-wait: the window is simply given its new
        // rectangle, anchored now rather than on a later commit.
        #[cfg(feature = "xwayland")]
        if let Some(x11) = self.window.x11_surface() {
            let at = perspicax_policy::anchor(self.from, self.edges, self.last);
            let _ = x11.configure(smithay::utils::Rectangle::new(at.into(), self.last.into()));
            data.space.map_element(self.window.clone(), at, false);
            data.backend.redraw();
        }
    }

    fn button(
        &mut self,
        data: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    pass_through!();

    fn start_data(&self) -> &GrabStartData<Compositor> {
        &self.start
    }

    /// The resize is over: the client is told, and its next commit is the
    /// last one anchored to the edge it was resized from.
    ///
    /// A window parked mid-resize is told too, or its client would go on
    /// resizing it. One that went is told nothing more, since to a client
    /// that unmapped its window a configure is the answer it waits for to map
    /// it again.
    fn unset(&mut self, data: &mut Compositor) {
        data.hand = None;
        if !data.has_window(&self.window) {
            return;
        }
        self.configure(false);
        placement(&self.window, |placement| {
            if let Some(resize) = &mut placement.resize {
                resize.released = true;
            }
        });
    }
}
