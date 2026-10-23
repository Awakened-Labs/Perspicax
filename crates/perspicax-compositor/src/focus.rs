//! What can hold the keyboard and the pointer.
//!
//! A Wayland surface, or an X11 window. They are different targets, not two
//! spellings of one: giving an X11 window the keyboard means telling the X
//! server too (`SetInputFocus`, or `WM_TAKE_FOCUS` for a client that manages
//! its own focus), and only Smithay's `X11Surface` knows how. With the plain
//! `WlSurface` as the focus type, Xwayland's surface got `wl_keyboard.enter`
//! while the X server's input focus never moved -- so an X11 window looked
//! selected and every key went nowhere. That was found on the first hardware
//! run, typing into an X11 alacritty.
//!
//! Everything here delegates to the target's own implementation; this type
//! only chooses which -- and, for the pointer, what of it a client that has
//! stopped reading is sent now and what it is owed instead (#169, see
//! [`crate::owed`]). Every pointer event a client is sent passes through here,
//! whoever moved the pointer: a hand, an agent, a grab. smithay calls these
//! with the pointer's lock held, so nothing here may ask the pointer anything
//! (its focus, its grab, a hold on it): only the target's client is asked.

use std::borrow::Cow;

use smithay::{
    desktop::PopupKind,
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            PointerTarget, RelativeMotionEvent,
        },
    },
    reexports::wayland_server::{
        Resource as _, backend::ObjectId, protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
};

use crate::{
    owed::{Event, Paid, Plan},
    state::{ClientState, Compositor},
};

/// A keyboard or pointer focus target.
#[derive(Debug, Clone, PartialEq)]
pub enum FocusTarget {
    /// A Wayland client's surface: a toplevel, a popup, a layer or lock
    /// surface, or Xwayland's surface for an X11 window the pointer is over.
    Wayland(WlSurface),
    /// An X11 window, given the keyboard through the X server as well.
    #[cfg(feature = "xwayland")]
    X11(smithay::xwayland::X11Surface),
}

/// Call the same method on whichever target this is.
macro_rules! delegate {
    ($self:ident, $target:ident => $call:expr) => {
        match $self {
            FocusTarget::Wayland($target) => $call,
            #[cfg(feature = "xwayland")]
            FocusTarget::X11($target) => $call,
        }
    };
}

impl From<WlSurface> for FocusTarget {
    fn from(surface: WlSurface) -> Self {
        Self::Wayland(surface)
    }
}

impl From<PopupKind> for FocusTarget {
    fn from(popup: PopupKind) -> Self {
        Self::Wayland(popup.wl_surface().clone())
    }
}

impl FocusTarget {
    /// What to send of a pointer `event`, and what owed to send before it.
    /// Everything as it is to a client this compositor did not accept
    /// itself: Xwayland, which is smithay's, and keeps reading for the X11
    /// applications behind it.
    fn plan(&self, event: Event<'_>) -> Plan {
        self.wl_surface()
            .and_then(|surface| surface.client())
            .and_then(|client| Some(client.get_data::<ClientState>()?.plan(self, event)))
            .unwrap_or_else(Plan::as_is)
    }

    /// Pay what is owed before `event`. Whether `event` itself goes.
    fn pay_before(&self, seat: &Seat<Compositor>, data: &mut Compositor, event: Event<'_>) -> bool {
        let plan = self.plan(event);
        if let Some(paid) = &plan.pay {
            self.deliver(seat, data, paid);
        }
        plan.send
    }

    /// Send what was owed, as it would have been sent: the motion, the
    /// mouse's own, and the frame closing them. Straight to the target,
    /// never back through the pointer.
    pub(crate) fn deliver(&self, seat: &Seat<Compositor>, data: &mut Compositor, paid: &Paid) {
        if let Some(motion) = &paid.motion {
            delegate!(self, target => PointerTarget::motion(target, seat, data, motion));
        }
        if let Some(relative) = &paid.relative {
            delegate!(self, target => PointerTarget::relative_motion(target, seat, data, relative));
        }
        if paid.framed {
            delegate!(self, target => PointerTarget::frame(target, seat, data));
        }
    }
}

impl IsAlive for FocusTarget {
    fn alive(&self) -> bool {
        delegate!(self, target => target.alive())
    }
}

impl WaylandFocus for FocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(surface) => Some(Cow::Borrowed(surface)),
            #[cfg(feature = "xwayland")]
            Self::X11(x11) => x11.wl_surface().map(Cow::Owned),
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        delegate!(self, target => WaylandFocus::same_client_as(target, object_id))
    }
}

impl KeyboardTarget<Compositor> for FocusTarget {
    fn enter(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        delegate!(self, target => KeyboardTarget::enter(target, seat, data, keys, serial));
    }

    fn leave(&self, seat: &Seat<Compositor>, data: &mut Compositor, serial: Serial) {
        delegate!(self, target => KeyboardTarget::leave(target, seat, data, serial));
    }

    fn key(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        key: KeysymHandle<'_>,
        state: smithay::backend::input::KeyState,
        serial: Serial,
        time: u32,
    ) {
        delegate!(self, target => KeyboardTarget::key(target, seat, data, key, state, serial, time));
    }

    fn modifiers(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        delegate!(self, target => KeyboardTarget::modifiers(target, seat, data, modifiers, serial));
    }
}

impl PointerTarget<Compositor> for FocusTarget {
    fn enter(&self, seat: &Seat<Compositor>, data: &mut Compositor, event: &MotionEvent) {
        if self.pay_before(seat, data, Event::Other) {
            delegate!(self, target => PointerTarget::enter(target, seat, data, event));
        }
    }
    fn motion(&self, seat: &Seat<Compositor>, data: &mut Compositor, event: &MotionEvent) {
        if self.pay_before(seat, data, Event::Motion(event)) {
            delegate!(self, target => PointerTarget::motion(target, seat, data, event));
        }
    }
    fn relative_motion(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &RelativeMotionEvent,
    ) {
        if self.pay_before(seat, data, Event::Relative(event)) {
            delegate!(self, target => PointerTarget::relative_motion(target, seat, data, event));
        }
    }
    fn button(&self, seat: &Seat<Compositor>, data: &mut Compositor, event: &ButtonEvent) {
        if self.pay_before(seat, data, Event::Other) {
            delegate!(self, target => PointerTarget::button(target, seat, data, event));
        }
    }
    fn axis(&self, seat: &Seat<Compositor>, data: &mut Compositor, frame: AxisFrame) {
        if self.pay_before(seat, data, Event::Other) {
            delegate!(self, target => PointerTarget::axis(target, seat, data, frame));
        }
    }
    fn frame(&self, seat: &Seat<Compositor>, data: &mut Compositor) {
        if self.pay_before(seat, data, Event::Frame) {
            delegate!(self, target => PointerTarget::frame(target, seat, data));
        }
    }
    fn gesture_swipe_begin(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &GestureSwipeBeginEvent,
    ) {
        if self.pay_before(seat, data, Event::Gesture) {
            delegate!(self, target => PointerTarget::gesture_swipe_begin(target, seat, data, event));
        }
    }
    fn gesture_swipe_update(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &GestureSwipeUpdateEvent,
    ) {
        if self.pay_before(seat, data, Event::Gesture) {
            delegate!(self, target => PointerTarget::gesture_swipe_update(target, seat, data, event));
        }
    }
    fn gesture_swipe_end(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &GestureSwipeEndEvent,
    ) {
        if self.pay_before(seat, data, Event::Gesture) {
            delegate!(self, target => PointerTarget::gesture_swipe_end(target, seat, data, event));
        }
    }
    fn gesture_pinch_begin(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &GesturePinchBeginEvent,
    ) {
        if self.pay_before(seat, data, Event::Gesture) {
            delegate!(self, target => PointerTarget::gesture_pinch_begin(target, seat, data, event));
        }
    }
    fn gesture_pinch_update(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &GesturePinchUpdateEvent,
    ) {
        if self.pay_before(seat, data, Event::Gesture) {
            delegate!(self, target => PointerTarget::gesture_pinch_update(target, seat, data, event));
        }
    }
    fn gesture_pinch_end(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &GesturePinchEndEvent,
    ) {
        if self.pay_before(seat, data, Event::Gesture) {
            delegate!(self, target => PointerTarget::gesture_pinch_end(target, seat, data, event));
        }
    }
    fn gesture_hold_begin(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &GestureHoldBeginEvent,
    ) {
        if self.pay_before(seat, data, Event::Gesture) {
            delegate!(self, target => PointerTarget::gesture_hold_begin(target, seat, data, event));
        }
    }
    fn gesture_hold_end(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        event: &GestureHoldEndEvent,
    ) {
        if self.pay_before(seat, data, Event::Gesture) {
            delegate!(self, target => PointerTarget::gesture_hold_end(target, seat, data, event));
        }
    }
    fn leave(&self, seat: &Seat<Compositor>, data: &mut Compositor, serial: Serial, time: u32) {
        if self.pay_before(seat, data, Event::Leave) {
            delegate!(self, target => PointerTarget::leave(target, seat, data, serial, time));
        }
    }
}
