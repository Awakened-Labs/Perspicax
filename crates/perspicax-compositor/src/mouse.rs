//! The person's mouse, as both backends see it: what the pointer is over,
//! and what a `[mouse]` binding makes of a press or a notch of the wheel
//! there.
//!
//! A seat comes here from libinput, through `backend::seat::input`, which
//! keeps for itself what only a seat does with a press: the frame's own
//! buttons and edges, dragging tabs and windows, and flipping workspaces
//! with the wheel. Headless comes here from a test, through
//! [`crate::Command::Click`] -- or a [`crate::Command::Press`] held down
//! until its [`crate::Command::Release`] -- and [`crate::Command::Scroll`],
//! which stand in for a person's hand as [`crate::Command::Perform`] stands
//! in for their keys.
//!
//! Both move the pointer the same way, through [`Compositor::travel`]: kept
//! on the outputs, given to what is under it, and the device's own motion
//! passed on to the client there, as a game's mouselook reads it. And both
//! put each motion and press to the focus policy the same way
//! ([`Compositor::follow_pointer`], [`Compositor::follow_press`]): a seat's
//! is its person's `[focus]`, and headless has none until a test puts one
//! in force ([`crate::Command::FocusPolicy`]).
//!
//! An agent never comes here. Its clicks and scrolls go straight to the
//! pointer (`crate::act`), so no binding can be set off by one. An agent
//! that could flip a person's workspaces, or start a program, by clicking an
//! empty spot of the desktop would be reaching past the window it was
//! given.

use std::time::Instant;

use perspicax_policy::{Button, Context, Focus, Gesture, Mods, Part, Wheel};
use smithay::{
    backend::input::{Axis, AxisSource, ButtonState},
    desktop::WindowSurfaceType,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, MotionEvent, PointerHandle,
        RelativeMotionEvent,
    },
    output::Output,
    reexports::wayland_server::{Resource as _, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
    wayland::seat::WaylandFocus,
};

use crate::{framed::Framed, hold::Hold, layers, shell::id_of, state::Compositor};

/// What the pointer is over.
pub(crate) struct Hit {
    /// Set when it is a window this compositor manages, which is all the
    /// focus policy and the bindings decide about. An X11 window that places
    /// itself ([`crate::shell::places_itself`]), an application's menu, is
    /// drawn, hit and given the pointer like any, but comes with its surface
    /// and no window, as a panel's menu does.
    pub(crate) window: Option<Framed>,
    /// Set when it is a layer surface that may take the keyboard on a click.
    pub(crate) takes_focus: bool,
    /// The surface there (a subsurface, a popup, the thing itself) and its
    /// origin in global space -- the pair `PointerHandle::motion` wants.
    /// `None` on a window's frame, which no client drew.
    pub(crate) surface: Option<WlSurface>,
    pub(crate) origin: Point<f64, Logical>,
    /// Set when it is the frame this compositor drew around a window.
    pub(crate) frame: Option<Part>,
}

/// The topmost thing at `at`, in the order the person sees them: while
/// locked, only the lock surface; otherwise the top and overlay layers, then
/// the windows, then the bottom and background layers.
pub(crate) fn under(state: &Compositor, at: Point<f64, Logical>) -> Option<Hit> {
    if state.lock.is_some() {
        let (surface, origin) = state.lock_surface_at(at)?;
        return Some(Hit {
            window: None,
            takes_focus: true,
            surface: Some(surface),
            origin,
            frame: None,
        });
    }
    let layer = |layers: &[_]| {
        state
            .layer_surface_under(layers, at)
            .map(|(layer, surface, origin)| Hit {
                window: None,
                takes_focus: layer.can_receive_keyboard_focus(),
                surface: Some(surface),
                origin,
                frame: None,
            })
    };
    // `raised`: only a window over the panels, the fullscreen one in use.
    let window = |raised: bool| {
        let (window, location) = crate::shell::window_under(&state.space, at)?;
        if raised && !crate::shell::covers_panels(window) {
            return None;
        }
        // One that places itself is its client's alone (#181).
        let managed = (!crate::shell::places_itself(window)).then(|| window.clone());
        // The client first, so a popup hanging over the titlebar gets its
        // clicks; then the frame around it. Rolled up to its titlebar, the
        // frame alone: nothing of the client is drawn to be pressed on.
        if !crate::shell::is_shaded(window)
            && let Some((surface, offset)) =
                window.surface_under(at - location.to_f64(), WindowSurfaceType::ALL)
        {
            return Some(Hit {
                window: managed,
                takes_focus: false,
                surface: Some(surface),
                origin: (location + offset).to_f64(),
                frame: None,
            });
        }
        Some(Hit {
            window: managed,
            takes_focus: false,
            surface: None,
            origin: location.to_f64(),
            frame: Some(state.frame_part(window, at)?),
        })
    };
    layer(&layers::OVERLAY)
        .or_else(|| window(true))
        .or_else(|| layer(&layers::TOP))
        .or_else(|| window(false))
        .or_else(|| layer(&layers::BELOW))
}

/// The focus policy, unless the session is locked: then the lock surface
/// holds the keyboard and no pointing may move it. Headless has none until a
/// test puts one in force.
pub(crate) fn policy(state: &Compositor) -> Option<Focus> {
    if state.lock.is_some() {
        return None;
    }
    state.backend.focus()
}

/// One axis of a scroll, as a device reported it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Scrolled {
    /// The wheel's 120ths of a notch, where the device counts them.
    pub(crate) v120: Option<f64>,
    /// How far, in pixels: all a touchpad says.
    pub(crate) pixels: f64,
    /// A finger lifted off a touchpad: the scroll is over.
    pub(crate) lifted: bool,
}

impl Compositor {
    /// What the pointer at `at` is over, as far as a press or a scroll goes:
    /// the surface of the window holding it while one does ([`crate::hold`]),
    /// whatever is drawn over it; otherwise what is drawn there.
    #[cfg_attr(
        not(feature = "seat"),
        expect(dead_code, reason = "the seat's presses and scrolls")
    )]
    pub(crate) fn pointed(&self, at: Point<f64, Logical>) -> Option<Hit> {
        self.hold()
            .map(|hold| hold.hit())
            .or_else(|| under(self, at))
    }

    /// Whether the empty desktop is at `at`: no window, and no panel or menu
    /// over the wallpaper. A widget on the bottom layer is part of the
    /// desktop, as the wallpaper is.
    pub(crate) fn over_desktop(&self, at: Point<f64, Logical>) -> bool {
        self.layer_surface_under(&layers::ABOVE, at).is_none()
            && crate::shell::window_under(&self.space, at).is_none()
    }

    /// Where `hit` is, as far as a binding cares. A panel or a menu is in no
    /// context of its own, so only `anywhere` reaches it.
    fn context(&self, hit: Option<&Hit>, at: Point<f64, Logical>) -> Context {
        match hit {
            Some(Hit {
                window: Some(_),
                frame: Some(Part::Title | Part::Tab(_)),
                ..
            }) => Context::Titlebar,
            Some(Hit {
                window: Some(_), ..
            }) => Context::Window,
            _ if self.over_desktop(at) => Context::Desktop,
            _ => Context::Anywhere,
        }
    }

    /// Whether a binding may be looked for now. Not at the lock screen, where
    /// a binding could be a way past it. And not while a grab holds the
    /// pointer -- a move, a resize, a button already down in a client -- for
    /// whatever holds it has the press spoken for.
    fn listening(&self) -> bool {
        self.lock.is_none() && !self.pointer.as_ref().is_some_and(PointerHandle::is_grabbed)
    }

    /// A button went down at `at`, over `hit`, with `mods` held. Whether a
    /// binding took it, in which case it has been carried out, and neither
    /// this press nor its release is any client's.
    pub(crate) fn bound_press(
        &mut self,
        hit: Option<&Hit>,
        at: Point<f64, Logical>,
        code: u32,
        mods: Mods,
        time: u32,
    ) -> bool {
        if !self.listening() {
            return false;
        }
        let button = Button::from_code(code);
        let within = self.backend.double_click_ms();
        let gesture = self.clicks.press(button, (time, (at.x, at.y)), within);
        let context = self.context(hit, at);
        let Some(action) = self.mouse.resolve(context, mods, gesture).cloned() else {
            return false;
        };
        self.clicks.take(button);
        self.perform_on(&action, self.monitor_at(at));
        true
    }

    /// A button came up. Whether its press was a binding's, which makes the
    /// release the binding's too.
    pub(crate) fn bound_release(&mut self, code: u32) -> bool {
        self.clicks.release(Button::from_code(code))
    }

    /// The wheel turned at `at`, over `hit`, with `mods` held: `axes`
    /// horizontal, then vertical. Whether a binding took the scroll, in
    /// which case no client hears of it.
    ///
    /// A binding counts whole notches, carried over from one event to the
    /// next, so a high-resolution wheel or a touchpad steps once per notch
    /// as a plain wheel does. The scroll is the binding's from its first
    /// event, before a whole notch has gathered: a client scrolling a little
    /// on the way to a binding would be scrolling for nothing.
    pub(crate) fn bound_scroll(
        &mut self,
        hit: Option<&Hit>,
        at: Point<f64, Logical>,
        mods: Mods,
        axes: [Scrolled; 2],
    ) -> bool {
        if !self.listening() {
            return false;
        }
        let context = self.context(hit, at);
        let mut taken = false;
        let mut actions = Vec::new();
        for (axis, (scrolled, vertical)) in axes.into_iter().zip([false, true]).enumerate() {
            if scrolled.lifted {
                self.wheel[axis].reset();
            }
            let Some(turn) = Wheel::of(vertical, scrolled.v120.unwrap_or(scrolled.pixels)) else {
                continue;
            };
            let gesture = Gesture::Wheel(turn);
            let Some(action) = self.mouse.resolve(context, mods, gesture).cloned() else {
                continue;
            };
            taken = true;
            let notches = self.wheel[axis].feed(scrolled.v120, scrolled.pixels);
            // Notches the other way are what an earlier scroll left over.
            if Wheel::of(vertical, f64::from(notches)) == Some(turn) {
                actions.extend(std::iter::repeat_n(action, notches.unsigned_abs() as usize));
            }
        }
        if actions.is_empty() {
            return taken;
        }
        // Over a window, a binding acts on that window, as it would had the
        // hand clicked it first: a scroll alone never moves the focus, but a
        // scroll that does something to a window does.
        self.follow_press(hit);
        let monitor = self.monitor_at(at);
        for action in &actions {
            self.perform_on(action, monitor.clone());
        }
        true
    }

    /// The pointer moved, and is now over `hit`: the focus policy says where
    /// the keyboard goes, and it goes there. Asked after every motion, on
    /// either backend, and before the hold is settled, so the window in use
    /// is the one the policy chose.
    ///
    /// Not while a window's menu holds the pointer and keyboard. The menu
    /// holds the pointer for its own application, and smithay tells any
    /// other window under the hand nothing, so the hand is not pointing at
    /// that window. Asked, the policy would hand it keys the menu's grab
    /// ignores, and under autoraise raise it over the menu (#185). A click
    /// outside the menu is what closes it.
    pub(crate) fn follow_pointer(&mut self, hit: Option<&Hit>) {
        if self.held_menu_root().is_some() {
            return;
        }
        // The focus policy is about windows. Over a panel, or an X11 menu,
        // it has nothing to say: passing it the panel as "no window" would
        // make strict focus drop the keyboard every time the pointer crossed
        // the taskbar, and the menu's window every time it opened one.
        let over_other = hit.is_some_and(|hit| hit.window.is_none());
        let over = hit.and_then(|hit| hit.window.as_ref()).and_then(id_of);
        if let Some(focus) = policy(self)
            && !over_other
        {
            let decision = focus.pointer_over(over, self.focused_surface());
            self.apply_focus(decision);
        }
    }

    /// A button went down over `hit` while a window's menu holds the pointer
    /// and keyboard. Unless it is over the menu's own application, the menu
    /// closes, as smithay's grab closes it for such a press, only first
    /// (#185). The keyboard goes back to the window the menu was opened
    /// from, and the pointer to what is under it. Then the focus policy, a
    /// `[mouse]` binding and a window's frame have the press as though no
    /// menu had been open. Otherwise no binding is looked for while the
    /// menu holds the pointer, and a press on the menu's own window's
    /// titlebar starts a move whose grab ends the menu's, leaving the menu
    /// drawn as the window moves. Called first, by the seat and the
    /// stand-in alike.
    pub(crate) fn press_past_menu(&mut self, hit: Option<&Hit>) {
        let Some(root) = self.held_menu_root() else {
            return;
        };
        let own = hit
            .and_then(|hit| hit.surface.as_ref())
            .is_some_and(|surface| surface.id().same_client_as(&root.id()));
        if !own {
            self.dismiss_menu();
        }
    }

    /// A button went down over `hit`: a panel or launcher there that takes
    /// the keyboard on a click gets it, without anything being raised, for
    /// layers stack by layer, not by click. So a launcher shown without the
    /// keys, nobody having asked for it, has them once it is clicked (#267).
    /// Before the focus policy, which decides about windows alone; and only
    /// for a press, not for a scroll that sets off a binding.
    pub(crate) fn focus_pressed_layer(&mut self, hit: Option<&Hit>) {
        if let Some(surface) = hit
            .filter(|hit| hit.window.is_none() && hit.takes_focus)
            .and_then(|hit| hit.surface.clone())
        {
            self.focus_plain(surface);
        }
    }

    /// A button went down over `hit`: the focus policy focuses and raises
    /// the window clicked, as every model does. A click over no window
    /// changes nothing.
    pub(crate) fn follow_press(&mut self, hit: Option<&Hit>) {
        if let Some(focus) = policy(self) {
            let over = hit.and_then(|hit| hit.window.as_ref()).and_then(id_of);
            let decision = focus.pressed(over, self.focused_surface());
            self.apply_focus(decision);
        }
    }

    /// The monitor under `at`, by name: where a mouse binding acts.
    fn monitor_at(&self, at: Point<f64, Logical>) -> Option<String> {
        self.space.output_under(at).next().map(Output::name)
    }

    /// A person's click, for a test: see [`crate::Command::Click`].
    pub(crate) fn stand_in_click(&mut self, at: (i32, i32), button: Button, mods: Mods) {
        self.stand_in_press(at, button, mods);
        self.stand_in_release(button);
    }

    /// A person's button going down and staying down, for a test: see
    /// [`crate::Command::Press`].
    pub(crate) fn stand_in_press(&mut self, at: (i32, i32), button: Button, mods: Mods) {
        let Some(Pointed { pointer, at, hit }) = self.stand_in_at(at) else {
            return;
        };
        let code = button.code();
        // Down whoever takes it, as on a seat. Not `person_used_seat`:
        // headless keeps no clock of the person, so a test's agent is never
        // refused for a hand that has stopped.
        self.person_button(code, true);
        // And a turn of the person's attention (#257), as on a seat.
        let serial = SERIAL_COUNTER.next_serial();
        self.attention
            .pressed(serial, Instant::now(), crate::origin::boot_ticks());
        let time = self.now_ms();
        // Focus and raise before the press is delivered, and before a
        // binding, as a seat does: the menu it is past closed first.
        self.press_past_menu(hit.as_ref());
        self.focus_pressed_layer(hit.as_ref());
        self.follow_press(hit.as_ref());
        if !self.bound_press(hit.as_ref(), at, code, mods, time) {
            self.stand_in_button(&pointer, serial, code, ButtonState::Pressed, time);
        }
    }

    /// A person's button coming back up where the pointer is, for a test:
    /// see [`crate::Command::Release`].
    pub(crate) fn stand_in_release(&mut self, button: Button) {
        if !self.can_stand_in() {
            return;
        }
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        let code = button.code();
        self.person_button(code, false);
        let time = self.now_ms();
        if !self.bound_release(code) {
            let serial = SERIAL_COUNTER.next_serial();
            self.stand_in_button(&pointer, serial, code, ButtonState::Released, time);
        }
    }

    /// A stand-in's button event no binding took, to whatever has the
    /// pointer: a grab, or the client under it.
    fn stand_in_button(
        &mut self,
        pointer: &PointerHandle<Self>,
        serial: Serial,
        code: u32,
        state: ButtonState,
        time: u32,
    ) {
        let event = ButtonEvent {
            serial,
            time,
            button: code,
            state,
        };
        let held = self.menu_held();
        pointer.button(self, &event);
        pointer.frame(self);
        self.after_menu_button(held);
    }

    /// A person's mouse moving by `by`, for a test: see
    /// [`crate::Command::Motion`]. Unaccelerated, so both of the motions a
    /// client may read are `by`.
    pub(crate) fn stand_in_motion(&mut self, by: (i32, i32)) {
        if !self.can_stand_in() {
            return;
        }
        let Some(from) = self.pointer.as_ref().map(PointerHandle::current_location) else {
            return;
        };
        let delta = Point::<i32, Logical>::from(by).to_f64();
        let relative = RelativeMotionEvent {
            delta,
            delta_unaccel: delta,
            utime: self.now_us(),
        };
        let time = self.now_ms();
        if let Some(Travelled { hit }) =
            self.travel(from + delta, Some(&relative), time, |_, _| None)
        {
            self.follow_pointer(hit.as_ref());
            self.settle_hold();
            #[cfg(feature = "capture")]
            self.flush_screencopy_for_pointer();
        }
    }

    /// A person's scroll, for a test: see [`crate::Command::Scroll`].
    pub(crate) fn stand_in_scroll(&mut self, at: (i32, i32), v120: (i32, i32), mods: Mods) {
        let Some(Pointed { pointer, at, hit }) = self.stand_in_at(at) else {
            return;
        };
        // 15 pixels a notch, as libinput and every toolkit assume.
        let axes = [v120.0, v120.1].map(|v120| Scrolled {
            v120: Some(f64::from(v120)),
            pixels: f64::from(v120) * 15.0 / 120.0,
            lifted: false,
        });
        if self.bound_scroll(hit.as_ref(), at, mods, axes) {
            return;
        }
        let mut frame = AxisFrame::new(self.now_ms()).source(AxisSource::Wheel);
        for (axis, (v120, scrolled)) in [Axis::Horizontal, Axis::Vertical]
            .into_iter()
            .zip([v120.0, v120.1].into_iter().zip(axes))
        {
            if v120 != 0 {
                frame = frame.value(axis, scrolled.pixels).v120(axis, v120);
            }
        }
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    /// Bring the pointer to `at` for a stand-in, giving it to the client
    /// under it, as a hand moving there would. `None` when there is no
    /// pointer, or nothing could be found under it.
    ///
    /// Without the clients' buffers smithay knows no surface's size, so
    /// every point would look like the empty desktop. Headless keeps them
    /// only with `capture`, and without it a stand-in is refused in the log
    /// rather than carried out against a desk that seems empty.
    fn stand_in_at(&mut self, at: (i32, i32)) -> Option<Pointed> {
        if !self.can_stand_in() {
            return None;
        }
        let pointer = self.pointer.clone()?;
        let from = pointer.current_location();
        let to = Point::<i32, Logical>::from(at).to_f64();
        // A hand goes where a window holding the pointer lets it, as a
        // tablet's does on a seat: nowhere while it is locked, when the
        // press lands where the pointer is.
        let hold = self.hold();
        let Some(at) = hold
            .as_ref()
            .map_or(Some(to), |hold| hold.moves_to(from, to))
        else {
            return Some(Pointed {
                pointer,
                at: from,
                hit: hold.as_ref().map(Hold::hit),
            });
        };
        let hit = hold.as_ref().map(Hold::hit).or_else(|| under(self, at));
        let focus = hit
            .as_ref()
            .and_then(|hit| Some((hit.surface.clone()?.into(), hit.origin)));
        let event = MotionEvent {
            location: at,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.now_ms(),
        };
        pointer.motion(self, focus, &event);
        pointer.frame(self);
        self.follow_pointer(hit.as_ref());
        self.settle_hold();
        #[cfg(feature = "capture")]
        self.flush_screencopy_for_pointer();
        Some(Pointed { pointer, at, hit })
    }

    /// Whether a stand-in can find what is under the pointer: only with the
    /// clients' buffers (see `stand_in_at`). Refused in the log otherwise.
    fn can_stand_in(&self) -> bool {
        let can = self.backend.keeps_buffers();
        if !can {
            tracing::warn!("a stand-in for the mouse needs the `capture` feature headless");
        }
        can
    }
}

/// What [`Compositor::travel`] found where it took the pointer.
pub(crate) struct Travelled {
    /// What the pointer is over now, for the focus policy.
    pub(crate) hit: Option<Hit>,
}

impl Compositor {
    /// Take the pointer to `to`, as a hand moving the mouse does, on either
    /// backend: kept on the outputs (see `backend::pointer`), given to what
    /// is under it there, with the cursor that goes with that.
    ///
    /// `edge` is asked first where the pointer came to rest, and may send it
    /// somewhere else instead: the seat's edge flip, which carries it round
    /// the desk. `relative` is the motion as the device reported it, and
    /// goes to the client under the pointer whether the pointer could follow
    /// it or not: pinned against the edge of the screen, a game turning its
    /// camera still turns. Smithay keeps one pointer focus for both, so they
    /// are given the same one.
    ///
    /// A window holding the pointer ([`crate::hold`]) has its say first.
    /// Locked, the pointer goes nowhere and no client may be told it moved:
    /// only the mouse's own motion goes, to that window. Confined, it goes
    /// as far inside the window's region as it can. Either way it stays the
    /// window's, whatever is drawn over it.
    ///
    /// No focus policy is asked and nothing is drawn: what follows from the
    /// motion is the caller's. `None` when the pointer did not move: there
    /// is none, or a window holds it in place.
    pub(crate) fn travel(
        &mut self,
        to: Point<f64, Logical>,
        relative: Option<&RelativeMotionEvent>,
        time: u32,
        edge: impl FnOnce(&mut Self, Point<f64, Logical>) -> Option<Point<f64, Logical>>,
    ) -> Option<Travelled> {
        let pointer = self.pointer.clone()?;
        let hold = self.hold();
        let to = match &hold {
            Some(hold) => match hold.moves_to(pointer.current_location(), to) {
                Some(to) => to,
                None => {
                    if let Some(relative) = relative {
                        let focus = Some((hold.surface.clone().into(), hold.origin));
                        pointer.relative_motion(self, focus, relative);
                        pointer.frame(self);
                    }
                    return None;
                }
            },
            None => to,
        };
        let mut at = self.on_the_desk(to);
        if let Some(arrived) = edge(self, at) {
            at = arrived;
        }
        let hit = match &hold {
            Some(hold) => Some(hold.hit()),
            None => under(self, at),
        };
        // Over a frame no client has the pointer: the one it left is told so,
        // and the compositor picks the cursor.
        // Over nothing at all, no client is drawing the cursor either, and a
        // resize arrow left over from a frame must not stay. While a grab
        // holds the pointer, the cursor is the grab's: a resize keeps its
        // arrow even when the window lags behind and the pointer runs out
        // over something else.
        if !pointer.is_grabbed() {
            match hit.as_ref() {
                Some(Hit {
                    frame: Some(part), ..
                }) => self.cursor = CursorImageStatus::Named(cursor_for(*part)),
                None => self.cursor = CursorImageStatus::default_named(),
                Some(_) => {}
            }
        }
        let focus = hit
            .as_ref()
            .and_then(|hit| Some((hit.surface.clone()?.into(), hit.origin)));
        pointer.motion(
            self,
            focus.clone(),
            &MotionEvent {
                location: at,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        if let Some(relative) = relative {
            pointer.relative_motion(self, focus, relative);
        }
        pointer.frame(self);
        Some(Travelled { hit })
    }
}

impl Compositor {
    /// Give the pointer to whatever is under it now, though it has not
    /// moved: a layer surface came up under a still pointer, or went from
    /// under it; the session locked, a lock surface came up, or the session
    /// unlocked. Without this a hand had to nudge the mouse before a click
    /// reached the menu or pie it had just opened, and the click went to
    /// the window beneath; a closed menu left the window it uncovered
    /// without the pointer until the mouse moved; and a click at rest on
    /// the lock screen went to the window it hid (#82).
    ///
    /// Only when what is under the pointer changed, so a panel's clock
    /// committing each second moves nothing. Never under a grab, which
    /// owns the pointer, nor while a window holds it ([`crate::hold`]): a
    /// notification popping up over a game must not take its mouselook. And
    /// only with the clients' buffers: without them every point looks like
    /// the empty desktop (see `stand_in_at`). The focus policy is not
    /// asked, since the pointer went nowhere: the keyboard stays where it
    /// is.
    pub(crate) fn repoint(&mut self) {
        if !self.backend.keeps_buffers() {
            return;
        }
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        if pointer.is_grabbed() || self.hold().is_some() {
            return;
        }
        let at = pointer.current_location();
        let hit = under(self, at);
        let now = hit.as_ref().and_then(|hit| hit.surface.clone());
        let was = pointer
            .current_focus()
            .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()));
        if now == was {
            return;
        }
        match hit.as_ref() {
            Some(Hit {
                frame: Some(part), ..
            }) => self.cursor = CursorImageStatus::Named(cursor_for(*part)),
            None => self.cursor = CursorImageStatus::default_named(),
            Some(_) => {}
        }
        let event = MotionEvent {
            location: at,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.now_ms(),
        };
        pointer.motion(
            self,
            hit.and_then(|hit| Some((hit.surface?.into(), hit.origin))),
            &event,
        );
        pointer.frame(self);
        self.backend.redraw();
    }
}

impl Compositor {
    /// Let go of the keyboard and pointer, whatever holds them, as a hand
    /// lifted from the mouse would: a window's menu closes as a click outside
    /// it does ([`Self::close_menu`]), and a grab of the pointer ends: a
    /// button held down in a client, a move or a resize, which finish as
    /// they end (see `shell::grabs`), or a drag and drop.
    ///
    /// The pointer is first told nothing is under it, so a drag and drop is
    /// cancelled rather than dropped wherever it was, and the grab, ended,
    /// has no client to give the pointer back to: [`Self::repoint`] gives it
    /// to whatever is under it now.
    ///
    /// Never from inside one of smithay's pointer or keyboard callbacks,
    /// which hold the locks this goes through.
    pub(crate) fn let_go(&mut self) {
        self.close_menu();
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        if !pointer.is_grabbed() {
            return;
        }
        let event = MotionEvent {
            location: pointer.current_location(),
            serial: SERIAL_COUNTER.next_serial(),
            time: self.now_ms(),
        };
        pointer.motion(self, None, &event);
        pointer.unset_grab(self, event.serial, event.time);
        pointer.frame(self);
    }
}

/// The cursor for a part of a frame: a resize arrow at an edge.
pub(crate) fn cursor_for(part: Part) -> CursorIcon {
    match part {
        Part::Edge(edges) => crate::shell::resize_cursor(edges),
        _ => CursorIcon::Default,
    }
}

/// Where a stand-in brought the pointer, and what it found there.
struct Pointed {
    pointer: PointerHandle<Compositor>,
    at: Point<f64, Logical>,
    hit: Option<Hit>,
}
