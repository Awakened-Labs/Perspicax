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

use std::{collections::HashMap, time::Instant};

use perspicax_index::{Action, PointerButton};
use perspicax_node::{Rect, SurfaceId};
use smithay::{
    backend::input::{Axis, AxisSource, ButtonState, KeyState},
    desktop::LayerSurface,
    input::{
        keyboard::{FilterResult, KeyboardHandle, Keycode, Keysym, xkb},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
};

use crate::{Keymap, framed::Framed, state::Compositor};

/// What input lands on: an application's window, or a layer-shell surface
/// (a panel, a wallpaper, a menu) where it sits in global space.
enum Target {
    Window(Framed),
    Layer(LayerSurface, Rectangle<i32, Logical>),
}

/// Where a pointer event goes: a point in global space, on a surface whose
/// own origin is at `origin` in global space.
struct Aim {
    global: (f64, f64),
    origin: (f64, f64),
    surface: WlSurface,
}

/// Linux button codes, from `linux/input-event-codes.h`. Wayland carries these
/// verbatim rather than an enum of its own, so the numbers are the protocol.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

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
    /// No layout of this keyboard has a key for the character, alone or with
    /// Shift. Named rather than skipped: typing most of a string and
    /// reporting success is the failure an agent cannot detect.
    #[error("no layout of this keyboard produces {0:?}")]
    Untypeable(char),
    /// The person at this seat used the keyboard or pointer moments ago.
    /// Synthetic input now would race theirs: a `Focus` would pull the
    /// keyboard out from under their typing, and a click would jump the
    /// pointer they are holding. Try again once they pause. Never produced
    /// headless, where nobody sits at the seat.
    #[error("the person at this seat is using it; try again when they pause")]
    PersonActive,
    /// The session is locked. The person has walked away, and nothing acts on
    /// their applications until they unlock it.
    #[error("the session is locked")]
    Locked,
    /// Focus was asked for a layer surface that takes no keyboard: a
    /// wallpaper, or a panel that is only ever clicked.
    #[error("surface {0} takes no keyboard focus")]
    TakesNoKeyboard(u64),
    /// No compositor loop answered. It has not started, it has stopped, or it
    /// is wedged; from outside those look the same and an agent can do nothing
    /// different about any of them.
    #[error("no compositor loop answered")]
    Unreachable,
    /// No monitor of this name.
    #[error("no output named {0}")]
    NoSuchOutput(String),
    /// A picture was asked of a build that cannot take one.
    #[error("this build has no `{0}` feature")]
    NotBuilt(&'static str),
    /// The renderer could not draw or read back the picture.
    #[error("the picture could not be taken: {0}")]
    Capture(String),
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

/// Which key produces which character, in every layout of this seat's
/// keymap.
///
/// Built once per keymap, because it is a pure function of it, and built by
/// *reversal*: `KeyboardHandle::input` takes a keycode, while an agent asks to
/// type a string. Everything between those two facts is this table.
///
/// Only Shift is ever held. Reaching further means guessing which of `AltGr`,
/// `ISO_Level3_Shift` or a compose sequence this particular keymap wants, and
/// a character we cannot type without guessing is refused by name.
pub(crate) struct Keys {
    /// For each layout, in the keymap's order, with Caps Lock off and then
    /// on: every character a key makes alone or with Shift, and that key.
    layouts: Vec<[HashMap<char, Stroke>; 2]>,
    /// What each key makes with Shift held, by layout: where a binding finds
    /// AZERTY's digits. See `perspicax_policy::candidates`.
    shifted: HashMap<(u32, Keycode), Keysym>,
    /// The physical shift key, so a shifted character can hold it down. Pressed
    /// as a real key rather than by asserting a modifier state, because that is
    /// what a keyboard does and what smithay's modifier tracking follows.
    shift: Option<Keycode>,
}

impl Keys {
    /// The table for a keymap compiled from these names: the ones the seat's
    /// keyboard was just given. Rebuilt whenever they change, because a
    /// table describing the previous keymap would type the wrong characters
    /// -- `y` for `z` on a German keyboard -- and report success.
    pub(crate) fn new(names: &Keymap) -> Self {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let Some(keymap) = xkb::Keymap::new_from_names(
            &context,
            &names.rules,
            &names.model,
            &names.layout,
            &names.variant,
            names.options.clone(),
            xkb::COMPILE_NO_FLAGS,
        ) else {
            tracing::warn!("no keymap: typing will refuse every character");
            return Self {
                layouts: Vec::new(),
                shifted: HashMap::new(),
                shift: None,
            };
        };
        Self::from_keymap(&keymap)
    }

    /// The reversal itself, separated so a test can drive it with a keymap it
    /// chose rather than whichever one this machine happens to default to.
    ///
    /// Each key is asked what it makes rather than read off its levels, with
    /// an xkb state in each layout, Shift up and down, Caps Lock off and on.
    /// That is how the client will read it, so the key types, the groups a
    /// key without its own falls back to, and what Caps Lock does to each
    /// key are xkb's answer and not a guess made here.
    fn from_keymap(keymap: &xkb::Keymap) -> Self {
        let mask = |name| match keymap.mod_get_index(name) {
            xkb::MOD_INVALID => 0,
            index => 1 << index,
        };
        let (shift_mask, caps_mask) = (mask(xkb::MOD_NAME_SHIFT), mask(xkb::MOD_NAME_CAPS));
        let keys: Vec<Keycode> = (keymap.min_keycode().raw()..=keymap.max_keycode().raw())
            .map(Keycode::new)
            .collect();
        let mut state = xkb::State::new(keymap);
        let mut made = |layout, caps, shifted| -> Vec<Keysym> {
            let depressed = if shifted { shift_mask } else { 0 };
            let locked = if caps { caps_mask } else { 0 };
            state.update_mask(depressed, 0, locked, 0, 0, layout);
            keys.iter().map(|&key| state.key_get_one_sym(key)).collect()
        };

        let mut layouts = Vec::new();
        let mut shifted = HashMap::new();
        for layout in 0..keymap.num_layouts() {
            let mut tables = [HashMap::new(), HashMap::new()];
            for (table, caps) in tables.iter_mut().zip([false, true]) {
                let alone = made(layout, caps, false);
                let with_shift = made(layout, caps, true);
                for ((&key, &plain), &upper) in keys.iter().zip(&alone).zip(&with_shift) {
                    if !caps {
                        shifted.insert((layout, key), upper);
                    }
                    // First key wins. A layout can produce one character
                    // from several keys -- the numeric keypad being the
                    // obvious case -- and the lower keycode is the
                    // main-block one.
                    for (sym, shift) in [(plain, false), (upper, true)] {
                        if let Some(ch) = text(sym) {
                            table.entry(ch).or_insert((key, shift));
                        }
                    }
                }
            }
            layouts.push(tables);
        }
        let shift = keys.iter().copied().find(|&key| {
            keymap
                .key_get_syms_by_level(key, 0, 0)
                .contains(&Keysym::Shift_L)
        });
        Self {
            layouts,
            shifted,
            shift,
        }
    }

    /// The layout and key that type this character: the layout in use if it
    /// has one, else the first that does, with Caps Lock as it is.
    fn stroke(&self, ch: char, active: u32, caps: bool) -> Option<(u32, Stroke)> {
        let find = |(layout, tables): (u32, &[HashMap<char, Stroke>; 2])| {
            tables[usize::from(caps)]
                .get(&ch)
                .map(|&stroke| (layout, stroke))
        };
        let numbered = || (0..).zip(&self.layouts);
        numbered()
            .filter(|&(layout, _)| layout == active)
            .chain(numbered())
            .find_map(find)
    }

    /// What this key makes with Shift held in this layout.
    #[cfg_attr(
        not(any(feature = "seat", test)),
        expect(dead_code, reason = "the seat's bindings")
    )]
    pub(crate) fn shifted(&self, layout: u32, key: Keycode) -> Option<Keysym> {
        self.shifted.get(&(layout, key)).copied()
    }

    /// How many distinct characters this layout can produce, with Caps Lock
    /// off. Diagnostics, and the assertion a test about the table's
    /// completeness needs.
    #[cfg(test)]
    pub(crate) fn len(&self, layout: usize) -> usize {
        self.layouts.get(layout).map_or(0, |tables| tables[0].len())
    }
}

/// The character a keysym types, if it types one. Control characters have
/// keysyms and are not text, with one exception: an agent asking to type a
/// newline means Return. xkb reads Return as `\r` and gives `\n` to the
/// Linefeed key, which no toolkit takes for Enter, so Return is named here.
fn text(sym: Keysym) -> Option<char> {
    if sym == Keysym::Return {
        return Some('\n');
    }
    let ch = char::from_u32(xkb::keysym_to_utf32(sym))?;
    (!ch.is_control()).then_some(ch)
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
        if self.lock.is_some() {
            return Err(ActError::Locked);
        }
        if self.person_is_active() {
            return Err(ActError::PersonActive);
        }
        // A window verb addresses a window wherever it is: a tab behind
        // another is parked, and closing a window on a hidden workspace is
        // still closing it. Input goes only to what is on screen, which may
        // be a panel or a wallpaper as well as a window.
        let target = match action {
            Action::Close | Action::Forward => self.any_window(surface).map(Target::Window),
            _ => self.window_for_id(surface).map(Target::Window).or_else(|| {
                self.layer_by_id(surface)
                    .map(|(layer, placed)| Target::Layer(layer, placed))
            }),
        }
        .ok_or(ActError::NoSuchSurface(surface.0))?;
        let focus_before = self.focused_surface();

        match (action, &target) {
            (Action::Focus, _) => self.act_focus(&target, surface),
            (Action::Click { at, button }, _) => self.act_click(&target, *at, *button),
            (Action::Scroll { at, dx, dy }, _) => self.act_scroll(&target, *at, *dx, *dy),
            (Action::Type { text }, _) => self.act_type(text),
            (Action::Close, Target::Window(window)) => {
                Self::close(window);
                Ok(())
            }
            (Action::Forward, Target::Window(window)) => {
                self.activate_tab(window);
                Ok(())
            }
            // Found by `any_window`, so never a layer.
            (Action::Close | Action::Forward, Target::Layer(..)) => {
                Err(ActError::NoSuchSurface(surface.0))
            }
        }?;

        Ok(Dispatched {
            at: Instant::now(),
            focus_before,
            focus_after: self.focused_surface(),
        })
    }

    /// Give this surface the keyboard: a window, or a layer surface that
    /// takes it.
    fn act_focus(&mut self, target: &Target, id: SurfaceId) -> Result<(), ActError> {
        let wl_surface = match target {
            Target::Window(window) => {
                crate::shell::surface_of(window).ok_or(ActError::NoSuchSurface(id.0))?
            }
            Target::Layer(layer, _) if layer.can_receive_keyboard_focus() => {
                layer.wl_surface().clone()
            }
            Target::Layer(..) => return Err(ActError::TakesNoKeyboard(id.0)),
        };
        self.focus_surface(wl_surface, id);
        Ok(())
    }

    /// Where a rect given in the target's own coordinates is, for a pointer
    /// event: its centre in global space, on which surface, with that
    /// surface's origin.
    fn aim(&self, target: &Target, at: Rect) -> Option<Aim> {
        match target {
            Target::Window(window) => {
                let (global, origin) = self.point_in(window, at);
                Some(Aim {
                    global,
                    origin,
                    surface: crate::shell::surface_of(window)?,
                })
            }
            // A layer surface has no shadow and no frame: it is its own
            // rectangle, placed by the layer map.
            Target::Layer(layer, placed) => {
                let origin = (f64::from(placed.loc.x), f64::from(placed.loc.y));
                let local = centre(at);
                Some(Aim {
                    global: (origin.0 + local.0, origin.1 + local.1),
                    origin,
                    surface: layer.wl_surface().clone(),
                })
            }
        }
    }

    /// Move the pointer onto a rect and press a button on it.
    ///
    /// `at` is relative to the target, the window or the layer surface,
    /// because that is the only coordinate space an accessibility bridge can
    /// be trusted in. Its centre is what is clicked; see [`centre`].
    fn act_click(
        &mut self,
        target: &Target,
        at: Rect,
        button: PointerButton,
    ) -> Result<(), ActError> {
        let Some(pointer) = self.pointer.clone() else {
            return Ok(());
        };
        let Some(Aim {
            global,
            origin,
            surface,
        }) = self.aim(target, at)
        else {
            return Ok(());
        };
        let focus = Some((surface.into(), origin.into()));
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
    fn act_scroll(&mut self, target: &Target, at: Rect, dx: f64, dy: f64) -> Result<(), ActError> {
        let Some(pointer) = self.pointer.clone() else {
            return Ok(());
        };
        let Some(Aim {
            global,
            origin,
            surface,
        }) = self.aim(target, at)
        else {
            return Ok(());
        };
        let time = self.now_ms();

        pointer.motion(
            self,
            Some((surface.into(), origin.into())),
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
        let (active, _) = self.layouts().ok_or(ActError::NoKeyboard)?;
        let caps = keyboard.modifier_state().caps_lock;

        // Resolve every character before pressing anything. A string that is
        // half typeable is not half typed: an application left holding the
        // first three letters of a word is worse off than one left untouched,
        // and an agent cannot undo what it cannot see.
        let strokes = text
            .chars()
            .map(|ch| {
                self.keys
                    .stroke(ch, active, caps)
                    .ok_or(ActError::Untypeable(ch))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let shift = self.keys.shift;
        let mut held = false;
        let mut layout = active;

        for (wanted, (key, wants_shift)) in strokes {
            // A character only another layout has is typed in that layout,
            // as a person would switch to type it. The client hears the
            // switch before the key, in the modifiers smithay sends.
            if wanted != layout {
                self.lock_layout(wanted);
                layout = wanted;
            }
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
        // Nor the person in a layout they did not choose.
        if layout != active {
            self.lock_layout(active);
        }
        Ok(())
    }

    /// One key event, with no compositor-level filtering.
    ///
    /// The filter closure is where the seat backend takes its escape hatches
    /// and key bindings. Synthetic keys bypass it on purpose: an agent typing
    /// text must never end the session, switch VT or close a window by
    /// spelling a chord. Forwarding everything is also what makes the events
    /// indistinguishable from a device's to the client.
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
    /// in global space, and the window's surface origin in global space.
    ///
    /// # The second number is the surface's origin, and getting that wrong is silent
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
    /// It happened a second time, more quietly, on the first real seat. The
    /// window's *geometry* origin was passed where its *surface* origin
    /// belongs -- the two differ by the client-side-decoration shadow, and
    /// only once a renderer gives Smithay real buffer sizes, so headless never
    /// saw it. The click landed just above a zenity button. This time the
    /// receipt said `on_target`, because the near miss repainted the button;
    /// it was caught by the dialog not closing. Damage on target is evidence
    /// that something under the click changed, not that the click landed.
    ///
    /// [`PointerHandle::motion`]: smithay::input::pointer::PointerHandle::motion
    fn point_in(&self, window: &Framed, at: Rect) -> ((f64, f64), (f64, f64)) {
        let local = centre(at);
        let placed = self.space.element_location(window).unwrap_or_default();
        // `at` is relative to the window geometry, which sits at `placed`.
        let global = (local.0 + f64::from(placed.x), local.1 + f64::from(placed.y));
        // The focus origin is the *surface's* origin, which is the geometry's
        // pushed back by the geometry's own offset into the surface -- the
        // client-side-decoration shadow. It is where Smithay itself renders the
        // surface and hit-tests it from. Using the geometry origin instead puts
        // every click short by the shadow's width: on the first real seat that
        // was enough to land a click just above a zenity button and repaint it
        // without pressing it. Headless the offset is zero,
        // because without a renderer Smithay's window geometry is empty.
        let surface = placed - window.geometry().loc;
        (global, (f64::from(surface.x), f64::from(surface.y)))
    }

    /// Milliseconds since this compositor started, which is the clock every
    /// other event it sends is stamped with.
    fn now_ms(&self) -> u32 {
        u32::try_from(self.started_at().elapsed().as_millis() % u128::from(u32::MAX))
            .unwrap_or(u32::MAX)
    }
}

/// A rect's centre. Aimed at rather than a corner: a corner is shared with
/// whatever is next to it, and a widget's centre is the part of it the widget
/// certainly owns.
fn centre(at: Rect) -> (f64, f64) {
    (at.x0 + (at.x1 - at.x0) / 2.0, at.y0 + (at.y1 - at.y0) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap(layout: &str) -> xkb::Keymap {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        xkb::Keymap::new_from_names(&context, "", "", layout, "", None, xkb::COMPILE_NO_FLAGS)
            .expect("the layout compiles")
    }

    fn us() -> Keys {
        Keys::from_keymap(&keymap("us"))
    }

    /// The key and shift for a character in the first layout, Caps Lock off.
    fn plain(keys: &Keys, ch: char) -> Option<Stroke> {
        keys.stroke(ch, 0, false).map(|(_, stroke)| stroke)
    }

    #[test]
    fn every_ascii_printable_has_a_key() {
        let keys = us();
        for ch in ' '..='~' {
            assert!(plain(&keys, ch).is_some(), "no key produces {ch:?}");
        }
    }

    #[test]
    fn case_is_the_shift_level_of_one_key() {
        let keys = us();
        let (lower, lower_shift) = plain(&keys, 'a').expect("a is typeable");
        let (upper, upper_shift) = plain(&keys, 'A').expect("A is typeable");
        assert_eq!(lower, upper, "one key, two levels");
        assert!(!lower_shift);
        assert!(upper_shift);
    }

    #[test]
    fn with_caps_lock_on_a_letter_takes_shift_to_be_small_and_a_digit_does_not() {
        let keys = us();
        let (key, _) = plain(&keys, 'a').expect("a is typeable");
        assert_eq!(keys.stroke('a', 0, true), Some((0, (key, true))));
        assert_eq!(keys.stroke('A', 0, true), Some((0, (key, false))));
        let (one, _) = plain(&keys, '1').expect("1 is typeable");
        assert_eq!(keys.stroke('1', 0, true), Some((0, (one, false))));
    }

    #[test]
    fn a_character_the_layout_cannot_make_is_refused_by_name() {
        // Not on a US layout at a level we will press without guessing.
        assert_eq!(us().stroke('\u{4e2d}', 0, false), None);
    }

    #[test]
    fn the_layout_has_a_shift_key() {
        assert!(
            us().shift.is_some(),
            "shift is how the second level is reached"
        );
    }

    #[test]
    fn a_newline_is_return_and_other_control_characters_are_not_keys() {
        let map = keymap("us");
        let keys = Keys::from_keymap(&map);
        let (key, shift) = plain(&keys, '\n').expect("a newline is typeable");
        assert_eq!(
            map.key_get_syms_by_level(key, 0, 0),
            [Keysym::Return],
            "Return, not Linefeed"
        );
        assert!(!shift);
        assert_eq!(plain(&keys, '\r'), None, "a carriage return is not text");
        assert_eq!(plain(&keys, '\u{7}'), None, "bell is not text");
    }

    #[test]
    fn the_table_covers_a_whole_layout_rather_than_a_corner_of_one() {
        let keys = us();
        // 95 printable ASCII, and a US layout adds little else at levels 0-1.
        assert!(keys.len(0) >= 95, "only {} characters mapped", keys.len(0));
    }

    #[test]
    fn each_layout_of_a_keymap_has_a_table_of_its_own() {
        let keys = Keys::from_keymap(&keymap("us,ru"));
        let (q, _) = plain(&keys, 'q').expect("q is on the first layout");
        assert_eq!(
            keys.stroke('й', 0, false),
            Some((1, (q, false))),
            "й is on the same key, in the second layout"
        );
        assert_eq!(keys.stroke('Й', 0, false), Some((1, (q, true))));
        assert!(keys.len(1) >= 66, "33 letters, both cases");
    }

    #[test]
    fn the_layout_in_use_is_preferred_and_another_used_only_when_it_must() {
        let keys = Keys::from_keymap(&keymap("us,ru"));
        let (space, _) = plain(&keys, ' ').expect("a space");
        assert_eq!(
            keys.stroke(' ', 1, false),
            Some((1, (space, false))),
            "both layouts have a space; the one in use types it"
        );
        assert_eq!(
            keys.stroke('q', 1, false).map(|(layout, _)| layout),
            Some(0),
            "only the first layout has q"
        );
        assert_eq!(
            keys.stroke('\u{4e2d}', 1, false),
            None,
            "and neither has 中"
        );
    }

    #[test]
    fn azerty_types_digits_with_shift_and_a_binding_finds_them_there() {
        let keys = Keys::from_keymap(&keymap("fr"));
        let (one, shifted) = plain(&keys, '1').expect("1 is typeable");
        assert!(shifted, "AZERTY's digits are above its symbols");
        assert_eq!(plain(&keys, '&'), Some((one, false)));
        assert_eq!(keys.shifted(0, one), Some(Keysym::_1));
        let (a, _) = plain(&keys, 'a').expect("a is typeable");
        let us_keys = us();
        assert_eq!(
            plain(&us_keys, 'q').map(|(key, _)| key),
            Some(a),
            "A where Q is"
        );
    }
}
