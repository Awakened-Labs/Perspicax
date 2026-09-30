//! The pointer, owned by a move or a resize until the button comes up.
//!
//! While a grab holds the pointer no client has pointer focus: the window
//! being dragged must not see its own drag as ordinary motion, and nothing
//! under the pointer should light up as it passes. Everything a grab does not
//! care about (scrolling, gestures) is passed through unchanged.

use perspicax_policy::{Edges, Rect, resize};
use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};

use super::{Resize, placement};
use crate::state::Compositor;

/// The half of `PointerGrab` neither grab changes: scrolling, frames and
/// gestures go wherever they would have gone.
macro_rules! pass_through {
    () => {
        fn relative_motion(
            &mut self,
            data: &mut Compositor,
            handle: &mut PointerInnerHandle<'_, Compositor>,
            focus: Option<(WlSurface, Point<f64, Logical>)>,
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
    window: Window,
    origin: Point<i32, Logical>,
}

impl MoveGrab {
    pub(crate) fn new(
        start: GrabStartData<Compositor>,
        window: Window,
        origin: Point<i32, Logical>,
    ) -> Self {
        Self {
            start,
            window,
            origin,
        }
    }
}

/// Resizing a window: every motion asks the client for a new size, and the
/// client's commits are anchored by [`Compositor::settle_resize`].
pub(crate) struct ResizeGrab {
    start: GrabStartData<Compositor>,
    window: Window,
    edges: Edges,
    from: Rect,
    last: (i32, i32),
}

impl ResizeGrab {
    pub(crate) fn new(
        start: GrabStartData<Compositor>,
        window: Window,
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
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let to = self.origin.to_f64() + (event.location - self.start.location);
        data.space
            .map_element(self.window.clone(), to.to_i32_round(), false);
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
            handle.unset_grab(self, data, event.serial, event.time, true);
            // Once, where it landed, rather than on every motion: occlusion
            // is judged against where a window is, not everywhere it passed.
            data.publish_facts();
        }
    }

    pass_through!();

    fn start_data(&self) -> &GrabStartData<Compositor> {
        &self.start
    }

    fn unset(&mut self, _data: &mut Compositor) {}
}

impl PointerGrab<Compositor> for ResizeGrab {
    fn motion(
        &mut self,
        data: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let Some(toplevel) = self.window.toplevel() else {
            return;
        };
        let (min, max) = with_states(toplevel.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            (current.min_size, current.max_size)
        });
        self.last = resize(
            self.from,
            self.edges,
            whole(event.location - self.start.location),
            (min.w, min.h),
            (max.w, max.h),
        );
        self.configure(true);
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
