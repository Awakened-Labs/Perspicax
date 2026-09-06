//! Synthetic input, dispatched into this compositor's own seat.
//!
//! This is the arrow no library outside a compositor can draw. An external tool
//! asks an application to please activate a widget and hopes the toolkit agrees;
//! here the events go onto the same `wl_pointer` and `wl_keyboard` every real
//! device would use, so focus, grabs and z-order are correct by construction
//! rather than by a convention every client has to honour.
//!
//! Two things follow from that and are worth stating, because both are
//! properties of the mechanism rather than choices made here:
//!
//! - **A client cannot tell.** There is no flag on a Wayland input event saying
//!   where it came from, so nothing downstream can refuse it. That is what makes
//!   this work on stock GTK and Qt with no cooperation, and it is why the gate
//!   in `perspicax-index` has to hold: it is the only place the question can be asked.
//! - **Coordinates arrive window-relative and leave global.** An accessibility
//!   bridge cannot know where its window is -- measured, not assumed, and
//!   recorded as risk #1 -- so every rect an agent names is relative to its own
//!   surface, and the compositor supplies the origin. That translation happens
//!   here, once, in [`Compositor::act`].

use std::time::Instant;

use perspicax_index::{Action, PointerButton};
use perspicax_node::{Rect, SurfaceId};
use smithay::{
    backend::input::{Axis, AxisSource, ButtonState, KeyState},
    desktop::Window,
    input::{
        keyboard::{FilterResult, KeyboardHandle, Keycode, Keysym, xkb},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    utils::SERIAL_COUNTER,
};

use crate::state::Compositor;

/// Linux button codes, from `linux/input-event-codes.h`. Wayland carries these
/// verbatim rather than an enum of its own, so the numbers are the protocol.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// The shifted layout level. Levels 0 and 1 are unmodified and shifted on every
/// layout, and this crate goes no further: reaching level 2 means guessing which
/// of `AltGr`, `ISO_Level3_Shift` or a compose sequence this particular keymap
/// wants. A character we cannot type without guessing is refused by name.
const LEVEL_SHIFTED: u32 = 1;

/// Why an action could not be dispatched.
///
/// These sit below `perspicax_index::Refusal`, which answers the different
/// question of whether an act should be *allowed*. Everything here is a
/// statement about this compositor's ability to carry it out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActError {
    /// No surface with this id is mapped. It was destroyed, or never existed.
    #[error("surface {0} is not mapped")]
    NoSuchSurface(u64),
    /// The seat has no keyboard, which means xkb could not compile a keymap.
    #[error("the seat has no keyboard: xkb compiled no keymap")]
    NoKeyboard,
    /// The character has no key on this layout, at a level we will press
    /// without guessing. Named rather than skipped: typing most of a string and
    /// reporting success is the failure an agent cannot detect.
    #[error("no key on this layout produces {0:?}")]
    Untypeable(char),
    /// No compositor loop answered. It has not started, it has stopped, or it
    /// is wedged; from outside those look the same and an agent can do nothing
    /// different about any of them.
    #[error("no compositor loop answered")]
    Unreachable,
}

/// What the compositor did, as it alone can report it.
///
/// Focus before and after are here rather than in the receipt because only the
/// seat knows them, and they are the cheapest evidence that an act landed
/// somewhere real: an activation that moves focus to the surface it named has
/// done something a client noticed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatched {
    /// When the last event of this action left the compositor.
    pub at: Instant,
    /// Which surface held keyboard focus before the action.
    pub focus_before: Option<SurfaceId>,
    /// Which surface holds it after.
    pub focus_after: Option<SurfaceId>,
}

/// A character's key, and whether it needs shift held.
type Stroke = (Keycode, bool);

/// Which key produces which character, on this seat's keymap.
///
/// Built once, because it is a pure function of the layout, and built by
/// *reversal*: `KeyboardHandle::input` takes a keycode, while an agent asks to
/// type a string. Everything between those two facts is this table.
pub(crate) struct Keys {
    /// Every character the layout can produce without guessing at a modifier.
    strokes: std::collections::HashMap<char, Stroke>,
    /// The physical shift key, so a shifted character can hold it down. Pressed
    /// as a real key rather than by asserting a modifier state, because that is
    /// what a keyboard does and what smithay's modifier tracking follows.
    shift: Option<Keycode>,
}

impl Keys {
    /// Walk a keymap and record what each key produces.
    ///
    /// The keymap is compiled from the same empty RMLVO names as
    /// `XkbConfig::default()`, which is what the seat was built with, so the
    /// two cannot disagree about what is on the keyboard.
    pub(crate) fn from_default_layout() -> Self {
        // Empty RMLVO names, which is exactly what `XkbConfig::default()`
        // passes: libxkbcommon then resolves its own defaults, so the table and
        // the seat cannot end up describing different keyboards.
        let (rules, model, layout, variant) = ("", "", "", "");
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let Some(keymap) = xkb::Keymap::new_from_names(
            &context,
            rules,
            model,
            layout,
            variant,
            None,
            xkb::COMPILE_NO_FLAGS,
        ) else {
            tracing::warn!("no keymap: typing will refuse every character");
            return Self {
                strokes: std::collections::HashMap::new(),
                shift: None,
            };
        };
        Self::from_keymap(&keymap)
    }

    /// The reversal itself, separated so a test can drive it with a keymap it
    /// chose rather than whichever one this machine happens to default to.
    fn from_keymap(keymap: &xkb::Keymap) -> Self {
        let mut strokes = std::collections::HashMap::new();
        let mut shift = None;

        for raw in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
            let key = Keycode::new(raw);
            let levels = keymap.num_levels_for_key(key, 0);

            for level in 0..levels {
                let Some(&sym) = keymap.key_get_syms_by_level(key, 0, level).first() else {
                    continue;
                };

                if sym == Keysym::Shift_L {
                    shift.get_or_insert(key);
                }

                // Above shifted, the modifier needed stops being knowable
                // without guessing. See `LEVEL_SHIFTED`.
                if level > LEVEL_SHIFTED {
                    continue;
                }

                let Some(ch) = char::from_u32(xkb::keysym_to_utf32(sym)) else {
                    continue;
                };
                // Control characters have keysyms and are not text. `\n` is the
                // exception worth keeping: an agent asking to type a newline
                // means Return, and Return is a key.
                if ch.is_control() && ch != '\n' {
                    continue;
                }

                // First key wins. A layout can produce one character from
                // several keys -- the numeric keypad being the obvious case --
                // and the lower keycode is the main-block one.
                strokes.entry(ch).or_insert((key, level == LEVEL_SHIFTED));
            }
        }

        Self { strokes, shift }
    }

    /// The key that produces this character, if any.
    fn stroke(&self, ch: char) -> Option<Stroke> {
        self.strokes.get(&ch).copied()
    }

    /// How many distinct characters this layout can produce. Diagnostics, and
    /// the assertion a test about the table's completeness needs.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.strokes.len()
    }
}

impl Compositor {
    /// Carry out one action against one surface.
    ///
    /// The surface is named rather than inferred: `perspicax-index` resolved a
    /// node to a surface through the join, and re-deriving it here from a
    /// coordinate would be the compositor second-guessing an attribution that
    /// was made with better evidence.
    pub(crate) fn act(
        &mut self,
        surface: SurfaceId,
        action: &Action,
    ) -> Result<Dispatched, ActError> {
        let window = self
            .window_for_id(surface)
            .ok_or(ActError::NoSuchSurface(surface.0))?;
        let focus_before = self.focused_surface();

        match action {
            Action::Focus => self.act_focus(&window, surface),
            Action::Click { at, button } => self.act_click(&window, *at, *button),
            Action::Scroll { at, dx, dy } => self.act_scroll(&window, *at, *dx, *dy),
            Action::Type { text } => self.act_type(text),
        }?;

        Ok(Dispatched {
            at: Instant::now(),
            focus_before,
            focus_after: self.focused_surface(),
        })
    }

    /// Give this surface the keyboard.
    fn act_focus(&mut self, window: &Window, id: SurfaceId) -> Result<(), ActError> {
        let Some(toplevel) = window.toplevel() else {
            return Err(ActError::NoSuchSurface(id.0));
        };
        let wl_surface = toplevel.wl_surface().clone();
        self.focus_surface(wl_surface, id);
        Ok(())
    }

    /// Move the pointer onto a rect and press a button on it.
    ///
    /// `at` is window-relative, because that is the only coordinate space an
    /// accessibility bridge can be trusted in. The centre is used rather than a
    /// corner: a corner is shared with whatever is next to it, and a widget's
    /// centre is the part of it the widget certainly owns.
    fn act_click(
        &mut self,
        window: &Window,
        at: Rect,
        button: PointerButton,
    ) -> Result<(), ActError> {
        let (global, origin) = self.point_in(window, at);
        let Some(pointer) = self.pointer.clone() else {
            return Ok(());
        };
        let Some(toplevel) = window.toplevel() else {
            return Ok(());
        };
        let focus = Some((toplevel.wl_surface().clone(), origin.into()));
        let time = self.now_ms();

        pointer.motion(
            self,
            focus.clone(),
            &MotionEvent {
                location: global.into(),
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);

        let code = match button {
            PointerButton::Left => BTN_LEFT,
            PointerButton::Middle => BTN_MIDDLE,
            PointerButton::Right => BTN_RIGHT,
        };
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pointer.button(
                self,
                &ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: self.now_ms(),
                    button: code,
                    state,
                },
            );
            pointer.frame(self);
        }
        Ok(())
    }

    /// Scroll at a point, in surface-local units.
    fn act_scroll(&mut self, window: &Window, at: Rect, dx: f64, dy: f64) -> Result<(), ActError> {
        let (global, origin) = self.point_in(window, at);
        let Some(pointer) = self.pointer.clone() else {
            return Ok(());
        };
        let Some(toplevel) = window.toplevel() else {
            return Ok(());
        };
        let time = self.now_ms();

        pointer.motion(
            self,
            Some((toplevel.wl_surface().clone(), origin.into())),
            &MotionEvent {
                location: global.into(),
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);

        // `AxisSource::Wheel` rather than `Finger`: a client may treat finger
        // scrolling as kinetic and keep moving after the events stop, which
        // would leave the surface in a state no receipt could describe.
        let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
        if dx != 0.0 {
            frame = frame.value(Axis::Horizontal, dx);
        }
        if dy != 0.0 {
            frame = frame.value(Axis::Vertical, dy);
        }
        pointer.axis(self, frame);
        pointer.frame(self);
        Ok(())
    }

    /// Type a string on the seat's keyboard.
    ///
    /// Whatever holds keyboard focus receives this. The action carries no
    /// surface for the same reason a keyboard has no target: focus is the
    /// addressing mechanism, and an agent that wants a different one asks for
    /// `Focus` first.
    fn act_type(&mut self, text: &str) -> Result<(), ActError> {
        let keyboard = self.keyboard.clone().ok_or(ActError::NoKeyboard)?;

        // Resolve every character before pressing anything. A string that is
        // half typeable is not half typed: an application left holding the
        // first three letters of a word is worse off than one left untouched,
        // and an agent cannot undo what it cannot see.
        let strokes = text
            .chars()
            .map(|ch| {
                self.keys
                    .stroke(ch)
                    .map(|stroke| (ch, stroke))
                    .ok_or(ActError::Untypeable(ch))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let shift = self.keys.shift;
        let mut held = false;

        for (_, (key, wants_shift)) in strokes {
            if wants_shift != held
                && let Some(shift_key) = shift
            {
                let state = if wants_shift {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                self.press(&keyboard, shift_key, state);
                held = wants_shift;
            }
            self.press(&keyboard, key, KeyState::Pressed);
            self.press(&keyboard, key, KeyState::Released);
        }

        // Never leave a modifier down. A stuck shift turns every later act into
        // a different one, and the client has no way to notice.
        if held && let Some(shift_key) = shift {
            self.press(&keyboard, shift_key, KeyState::Released);
        }
        Ok(())
    }

    /// One key event, with no compositor-level filtering.
    ///
    /// The filter closure is where a real compositor implements its own key
    /// bindings; this one has none, and forwarding everything is what makes the
    /// events indistinguishable from a device's.
    fn press(&mut self, keyboard: &KeyboardHandle<Self>, key: Keycode, state: KeyState) {
        let time = self.now_ms();
        keyboard.input::<(), _>(
            self,
            key,
            state,
            SERIAL_COUNTER.next_serial(),
            time,
            |_, _, _| FilterResult::Forward,
        );
    }

    /// Where to put the pointer for a window-relative rect: the rect's centre
    /// in global space, and the window's own origin in global space.
    ///
    /// # The second number is the window's origin, and getting that wrong is silent
    ///
    /// [`PointerHandle::motion`] takes the pointer's location in compositor
    /// space *and* the focus target's origin in compositor space, and works out
    /// the surface-local coordinate by subtracting the second from the first.
    /// Handing it the surface-local point instead -- which is the intuitive
    /// reading of "focus", and what this function returned until M3 slice 5 --
    /// makes it subtract the offset twice, so the client is told the pointer is
    /// at `global - local` inside its surface.
    ///
    /// Nothing catches that, and the reason is worth writing down. A window at
    /// the origin has `global == local`, so the client is told `(0, 0)`: it
    /// receives a real enter, a real press and a real release, at the top-left
    /// corner of its window rather than on the widget. Every test that asserts
    /// "the click reached the surface" passes. It was found by clicking a real
    /// GTK button and asking the application, which reported that it had not
    /// been clicked while the receipt reported `DamageWitness::Quiet` --
    /// the receipt being right about it is the whole argument for receipts.
    ///
    /// [`PointerHandle::motion`]: smithay::input::pointer::PointerHandle::motion
    fn point_in(&self, window: &Window, at: Rect) -> ((f64, f64), (f64, f64)) {
        let local = (at.x0 + (at.x1 - at.x0) / 2.0, at.y0 + (at.y1 - at.y0) / 2.0);
        let origin = self
            .space
            .element_location(window)
            .map_or((0.0, 0.0), |point| (f64::from(point.x), f64::from(point.y)));
        let global = (local.0 + origin.0, local.1 + origin.1);
        (global, origin)
    }

    /// Milliseconds since this compositor started, which is the clock every
    /// other event it sends is stamped with.
    fn now_ms(&self) -> u32 {
        u32::try_from(self.started_at().elapsed().as_millis() % u128::from(u32::MAX))
            .unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap() -> xkb::Keymap {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        xkb::Keymap::new_from_names(&context, "", "", "us", "", None, xkb::COMPILE_NO_FLAGS)
            .expect("a us layout compiles")
    }

    #[test]
    fn every_ascii_printable_has_a_key() {
        let keys = Keys::from_keymap(&keymap());
        for ch in ' '..='~' {
            assert!(keys.stroke(ch).is_some(), "no key produces {ch:?}");
        }
    }

    #[test]
    fn case_is_the_shift_level_of_one_key() {
        let keys = Keys::from_keymap(&keymap());
        let (lower, lower_shift) = keys.stroke('a').expect("a is typeable");
        let (upper, upper_shift) = keys.stroke('A').expect("A is typeable");
        assert_eq!(lower, upper, "one key, two levels");
        assert!(!lower_shift);
        assert!(upper_shift);
    }

    #[test]
    fn a_character_the_layout_cannot_make_is_refused_by_name() {
        let keys = Keys::from_keymap(&keymap());
        // Not on a US layout at a level we will press without guessing.
        assert_eq!(keys.stroke('\u{4e2d}'), None);
    }

    #[test]
    fn the_layout_has_a_shift_key() {
        let keys = Keys::from_keymap(&keymap());
        assert!(
            keys.shift.is_some(),
            "shift is how the second level is reached"
        );
    }

    #[test]
    fn a_newline_is_a_key_and_other_control_characters_are_not() {
        let keys = Keys::from_keymap(&keymap());
        assert!(keys.stroke('\n').is_some(), "Return produces a newline");
        assert_eq!(keys.stroke('\u{7}'), None, "bell is not text");
    }

    #[test]
    fn the_table_covers_a_whole_layout_rather_than_a_corner_of_one() {
        let keys = Keys::from_keymap(&keymap());
        // 95 printable ASCII, and a US layout adds little else at levels 0-1.
        assert!(keys.len() >= 95, "only {} characters mapped", keys.len());
    }
}
