//! The person's mouse, as both backends see it: what the pointer is over,
//! and what a `[mouse]` binding makes of a press or a notch of the wheel
//! there.
//!
//! A seat comes here from libinput, through `backend::seat::input`, which
//! keeps for itself what only a seat does with a press: the focus policy,
//! the frame's own buttons and edges, dragging tabs and windows, and
//! flipping workspaces with the wheel. Headless comes here from a test,
//! through [`crate::Command::Click`] and [`crate::Command::Scroll`], which
//! stand in for a person's hand as [`crate::Command::Perform`] stands in for
//! their keys.
//!
//! An agent never comes here. Its clicks and scrolls go straight to the
//! pointer (`crate::act`), so no binding can be set off by one. An agent
//! that could flip a person's workspaces, or start a program, by clicking an
//! empty spot of the desktop would be reaching past the window it was
//! given.

use perspicax_policy::{Button, Context, Focus, Gesture, Mods, Part, Wheel};
use smithay::{
    backend::input::{Axis, AxisSource, ButtonState},
    desktop::WindowSurfaceType,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, MotionEvent, PointerHandle,
    },
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::seat::WaylandFocus,
};

use crate::{framed::Framed, layers, shell::id_of, state::Compositor};

/// What the pointer is over.
pub(crate) struct Hit {
    /// Set when it is a window, which is all the focus policy decides about.
    pub(crate) window: Option<Framed>,
    /// Set when it is a layer surface that may take the keyboard on a click.
    #[cfg_attr(
        not(feature = "seat"),
        expect(dead_code, reason = "the seat's input path")
    )]
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
        let (window, location) = state.space.element_under(at)?;
        if raised && !crate::shell::covers_panels(window) {
            return None;
        }
        // The client first, so a popup hanging over the titlebar gets its
        // clicks; then the frame around it.
        if let Some((surface, offset)) =
            window.surface_under(at - location.to_f64(), WindowSurfaceType::ALL)
        {
            return Some(Hit {
                window: Some(window.clone()),
                takes_focus: false,
                surface: Some(surface),
                origin: (location + offset).to_f64(),
                frame: None,
            });
        }
        Some(Hit {
            window: Some(window.clone()),
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
/// holds the keyboard and no pointing may move it. Headless has none.
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
    /// Whether the empty desktop is at `at`: no window, and no panel or menu
    /// over the wallpaper. A widget on the bottom layer is part of the
    /// desktop, as the wallpaper is.
    pub(crate) fn over_desktop(&self, at: Point<f64, Logical>) -> bool {
        self.layer_surface_under(&layers::ABOVE, at).is_none()
            && self.space.element_under(at).is_none()
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
        if let Some(focus) = policy(self)
            && let Some(window) = hit.and_then(|hit| hit.window.as_ref())
        {
            let decision = focus.pressed(id_of(window), self.focused_surface());
            self.apply_focus(decision);
        }
        let monitor = self.monitor_at(at);
        for action in &actions {
            self.perform_on(action, monitor.clone());
        }
        true
    }

    /// The monitor under `at`, by name: where a mouse binding acts.
    fn monitor_at(&self, at: Point<f64, Logical>) -> Option<String> {
        self.space.output_under(at).next().map(Output::name)
    }

    /// A person's click, for a test: see [`crate::Command::Click`].
    pub(crate) fn stand_in_click(&mut self, at: (i32, i32), button: Button, mods: Mods) {
        let Some(Pointed { pointer, at, hit }) = self.stand_in_at(at) else {
            return;
        };
        let code = button.code();
        for state in [ButtonState::Pressed, ButtonState::Released] {
            let time = self.now_ms();
            let taken = match state {
                ButtonState::Pressed => self.bound_press(hit.as_ref(), at, code, mods, time),
                ButtonState::Released => self.bound_release(code),
            };
            if !taken {
                let serial = SERIAL_COUNTER.next_serial();
                let event = ButtonEvent {
                    serial,
                    time,
                    button: code,
                    state,
                };
                pointer.button(self, &event);
                pointer.frame(self);
            }
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
        if !self.backend.keeps_buffers() {
            tracing::warn!("a stand-in for the mouse needs the `capture` feature headless");
            return None;
        }
        let pointer = self.pointer.clone()?;
        let at = Point::<i32, Logical>::from(at).to_f64();
        let hit = under(self, at);
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
        #[cfg(feature = "capture")]
        self.flush_screencopy_for_pointer();
        Some(Pointed { pointer, at, hit })
    }
}

impl Compositor {
    /// Give the pointer to whatever is under it now, though it has not
    /// moved: a layer surface came up under a still pointer, or went from
    /// under it. Without this a hand had to nudge the mouse before a click
    /// reached the menu or pie it had just opened, and the click went to
    /// the window beneath; and a closed menu left the window it uncovered
    /// without the pointer until the mouse moved.
    ///
    /// Only when what is under the pointer changed, so a panel's clock
    /// committing each second moves nothing. Never under a grab, which
    /// owns the pointer, and only with the clients' buffers: without them
    /// every point looks like the empty desktop (see `stand_in_at`). The
    /// focus policy is not asked, since the pointer went nowhere: the
    /// keyboard stays where it is.
    pub(crate) fn repoint(&mut self) {
        if !self.backend.keeps_buffers() {
            return;
        }
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        if pointer.is_grabbed() {
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
