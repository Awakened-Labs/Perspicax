//! The pointer, owned by a move or a resize until the button comes up.
//!
//! While a grab holds the pointer no client has pointer focus: the window
//! being dragged must not see its own drag as ordinary motion, and nothing
//! under the pointer should light up as it passes. Everything a grab does not
//! care about (scrolling, gestures) is passed through unchanged.

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

/// Moving a window: it follows the pointer from where it was grabbed.
pub(crate) struct MoveGrab {
    start: GrabStartData<Compositor>,
    window: Framed,
    origin: Point<i32, Logical>,
    /// The window is maximized or snapped, and has not been restored yet: it
    /// is, once the pointer has travelled far enough to make the press a
    /// drag. Until then nothing moves, so a click is only a click.
    filled: bool,
}

impl MoveGrab {
    pub(crate) fn new(
        start: GrabStartData<Compositor>,
        window: Framed,
        origin: Point<i32, Logical>,
        filled: bool,
    ) -> Self {
        Self {
            start,
            window,
            origin,
            filled,
        }
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
fn limits(window: &Framed) -> Option<(Size<i32, Logical>, Size<i32, Logical>)> {
    if let Some(toplevel) = window.toplevel() {
        return Some(with_states(toplevel.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            (current.min_size, current.max_size)
        }));
    }
    #[cfg(feature = "xwayland")]
    if let Some(x11) = window.x11_surface() {
        return Some((
            x11.min_size().unwrap_or_default(),
            x11.max_size().unwrap_or_default(),
        ));
    }
    None
}

/// A tab being dragged by its titlebar with the middle button, to join
/// another window's group or leave its own. The window does not move while
/// it is dragged; where it would go is drawn, and it goes there on release.
#[cfg(feature = "seat")]
pub(crate) struct TabDragGrab {
    start: GrabStartData<Compositor>,
    tab: Framed,
}

#[cfg(feature = "seat")]
impl TabDragGrab {
    pub(crate) fn new(start: GrabStartData<Compositor>, tab: Framed) -> Self {
        Self { start, tab }
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
        data.tab_drop = data
            .tab_target(&self.tab, event.location)
            .map(|(_, bar)| bar);
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
            data.tab_drop = None;
            handle.unset_grab(self, data, event.serial, event.time, true);
            data.drop_tab(&self.tab, at);
        }
    }

    pass_through!();

    fn start_data(&self) -> &GrabStartData<Compositor> {
        &self.start
    }

    fn unset(&mut self, data: &mut Compositor) {
        data.tab_drop = None;
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
        let to = (self.origin.to_f64() + (event.location - self.start.location)).to_i32_round();
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
            // Before the grab is unset, which forgets the preview.
            data.finish_snap(&self.window);
            handle.unset_grab(self, data, event.serial, event.time, true);
            // Per output, a window dragged onto another monitor joins the
            // workspace that monitor is showing.
            data.window_moved(&self.window);
            // Once, where it landed, rather than on every motion: occlusion
            // is judged against where a window is, not everywhere it passed.
            data.publish_facts();
        }
    }

    pass_through!();

    fn start_data(&self) -> &GrabStartData<Compositor> {
        &self.start
    }

    fn unset(&mut self, data: &mut Compositor) {
        data.dragging = None;
        if data.snap_preview.take().is_some() {
            data.backend.redraw();
        }
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
            self.configure(false);
            placement(&self.window, |placement| {
                if let Some(resize) = &mut placement.resize {
                    resize.released = true;
                }
            });
        }
    }

    pass_through!();

    fn start_data(&self) -> &GrabStartData<Compositor> {
        &self.start
    }

    fn unset(&mut self, _data: &mut Compositor) {}
}
