//! The person's keyboard and pointer, from libinput.
//!
//! Everything here is real input, and all of it is marked as such
//! ([`Compositor::person_used_seat`]) so an agent's act can see it would race
//! the person. For a key, the order of authority is:
//!
//! 1. the escape hatches ([`super::super::hatch`]), which nothing can rebind;
//! 2. the bindings ([`perspicax_policy::Bindings`]);
//! 3. the focused client.
//!
//! A tap of the Logo key is a binding too, but one known only once the key
//! comes back up with nothing pressed in between
//! ([`perspicax_policy::LogoTap`]). Its press and release still reach the
//! client, as any modifier's do.
//!
//! A binding is found under any layout: a key is matched by what it makes
//! there, and by what it makes in the keymap's Latin layout and, for
//! AZERTY's number row, with Shift ([`perspicax_policy::candidates`]).
//!
//! Every keyboard's lock keys are lit to match xkb's state, including one
//! plugged in later, or brought back from another VT, where something else
//! may have lit them differently.
//!
//! The pointer is confined to the outputs ([`super::super::pointer`]),
//! hit-tested against the stack, and every motion and press is put to the
//! focus policy ([`perspicax_policy::Focus`]), whose decision is then carried
//! out here. For a button or the wheel, the order of authority is:
//!
//! 1. the `[mouse]` bindings ([`crate::mouse`], shared with headless);
//! 2. what the compositor does with a press itself: a frame's buttons, edges
//!    and titlebar, a tab picked up with the middle button, the drag
//!    modifier, and the wheel flipping workspaces over the desktop;
//! 3. the client under the pointer.

use std::time::Duration;

use perspicax_policy::{
    Action, Button, Drag, FrameButton, Mods, Part, Resting, arrival, candidates, edge_at,
    edges_near, is_double, is_logo,
};
use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent as _, Axis, AxisSource, ButtonState, Event as _, InputEvent,
            KeyState, KeyboardKeyEvent as _, PointerAxisEvent, PointerButtonEvent as _,
            PointerMotionEvent as _,
        },
        libinput::LibinputInputBackend,
        session::Session as _,
    },
    input::{
        keyboard::{FilterResult, KeyboardHandle, Keycode, LedState, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, CursorImageStatus, GrabStartData, MotionEvent, PointerHandle,
        },
    },
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        input::{Device, DeviceCapability, Led},
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
};

use super::{
    super::{
        Running,
        hatch::{self, Hatch},
        pointer,
    },
    Session, settings,
};
use crate::{
    framed::Framed,
    mouse::{Hit, Scrolled, cursor_for, policy, under},
    shell::{covers_panels, id_of},
    state::Compositor,
};

const BTN_LEFT: u32 = Button::Left.code();
const BTN_MIDDLE: u32 = Button::Middle.code();

/// A key the compositor kept for itself.
enum Taken {
    Hatch(Hatch),
    Bound(Action),
    /// The release of a key whose press was taken: swallowed, nothing to do.
    Release,
}

/// One libinput event.
pub(super) fn handle(state: &mut Compositor, event: InputEvent<LibinputInputBackend>) {
    match event {
        InputEvent::Keyboard { event } => {
            state.person_used_seat();
            key(state, event.key_code(), event.state(), event.time_msec());
        }
        InputEvent::PointerMotion { event } => {
            state.person_used_seat();
            let Some(from) = state
                .pointer
                .as_ref()
                .map(|pointer| pointer.current_location())
            else {
                return;
            };
            moved(state, from + event.delta(), event.time_msec());
        }
        InputEvent::PointerMotionAbsolute { event } => {
            state.person_used_seat();
            // A tablet or a virtual machine's absolute pointer spans every
            // output at once, so it is mapped onto their bounding box.
            let Some(extent) = extent(state) else {
                return;
            };
            let to = event.position_transformed(extent.size) + extent.loc.to_f64();
            moved(state, to, event.time_msec());
        }
        InputEvent::PointerButton { event } => {
            state.person_used_seat();
            button(state, event.button_code(), event.state(), event.time_msec());
        }
        InputEvent::PointerAxis { event } => {
            state.person_used_seat();
            axis(state, &event);
        }
        InputEvent::DeviceAdded { mut device } => {
            // Coming back from another VT, libinput adds every device again,
            // so this also relights the keyboards after whatever lit them
            // there.
            let leds = state.keyboard.as_ref().map(KeyboardHandle::led_state);
            if let Running::Seat(session) = &mut state.backend {
                settings::configure(&mut device, &session.settings.pointer);
                if let Some(leds) = leds {
                    light(&mut device, leds);
                }
                session.devices.push(device);
            }
        }
        InputEvent::DeviceRemoved { device } => {
            if let Running::Seat(session) = &mut state.backend {
                session.devices.retain(|known| known != &device);
            }
        }
        _ => {}
    }
}

/// One key. Escape hatches first, then bindings, then the focused client.
fn key(state: &mut Compositor, keycode: Keycode, pressed: KeyState, time: u32) {
    let Some(keyboard) = state.keyboard.clone() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    // A tap of the Logo key, finished by this release: done once the client
    // has seen the release, so it never believes the key is still down.
    let mut tapped = None;
    let taken = keyboard.input(
        state,
        keycode,
        pressed,
        serial,
        time,
        |state, modifiers, keysym| {
            let Running::Seat(session) = &mut state.backend else {
                return FilterResult::Forward;
            };
            let raw = keysym.raw_syms();
            let logo = raw.iter().copied().any(is_logo);
            let locked = state.lock.is_some();
            if pressed == KeyState::Released {
                if session.logo_tap.release(logo) {
                    tapped = session
                        .settings
                        .bindings
                        .tap()
                        .filter(|action| !locked || action.while_locked())
                        .cloned();
                }
                return match session.swallowed.iter().position(|&k| k == keycode) {
                    Some(at) => {
                        session.swallowed.swap_remove(at);
                        FilterResult::Intercept(Taken::Release)
                    }
                    None => FilterResult::Forward,
                };
            }
            session.logo_tap.press(logo, mods(modifiers));
            // While locked, only the escape hatches and the layout keys: a
            // binding that opened a terminal over the lock screen would be an
            // unlock, while a password is typed in a layout too.
            let taken = hatch::classify(modifiers, keysym.modified_sym(), &raw)
                .map(Taken::Hatch)
                .or_else(|| {
                    let layout = modifiers.serialized.layout_effective;
                    let syms = candidates(
                        keysym.modified_sym(),
                        &raw,
                        keysym.raw_latin_sym_or_raw_current_sym(),
                        state.keys.shifted(layout, keycode),
                    );
                    session
                        .settings
                        .bindings
                        .resolve(mods(modifiers), &syms)
                        .filter(|action| !locked || action.while_locked())
                        .cloned()
                        .map(Taken::Bound)
                });
            match taken {
                Some(taken) => {
                    session.swallowed.push(keycode);
                    FilterResult::Intercept(taken)
                }
                None => FilterResult::Forward,
            }
        },
    );

    match taken {
        Some(Taken::Hatch(hatch)) => escape(state, hatch),
        Some(Taken::Bound(action)) => state.perform(&action),
        Some(Taken::Release) | None => {}
    }
    if let Some(action) = tapped {
        state.perform(&action);
    }
}

fn escape(state: &mut Compositor, hatch: Hatch) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    match hatch {
        Hatch::Exit => {
            tracing::info!("Ctrl+Alt+Backspace: ending the session");
            session.exit = true;
        }
        Hatch::Vt(vt) => {
            tracing::info!(vt, "switching VT");
            if let Err(error) = session.seat.change_vt(vt) {
                tracing::warn!(vt, %error, "could not switch VT");
            }
        }
    }
}

/// The pointer moved to `to`, before confinement.
fn moved(state: &mut Compositor, to: Point<f64, Logical>, time: u32) {
    let Some(handle) = state.pointer.clone() else {
        return;
    };
    let outputs: Vec<_> = state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .collect();
    let mut at = pointer::confine(to, &outputs);
    if let Some(arrived) = rest_at_edge(state, at) {
        at = arrived;
    }
    let under = under(state, at);
    // The focus policy is about windows. Over a panel it has nothing to say:
    // passing it the panel as "no window" would make strict focus drop the
    // keyboard every time the pointer crossed the taskbar.
    let over_layer = under.as_ref().is_some_and(|hit| hit.window.is_none());
    let over = under
        .as_ref()
        .and_then(|hit| hit.window.as_ref())
        .and_then(id_of);
    // Over a frame no client has the pointer: the one it left is told so,
    // and the compositor picks the cursor.
    // Over nothing at all, no client is drawing the cursor either, and a
    // resize arrow left over from a frame must not stay. While a grab holds
    // the pointer, the cursor is the grab's: a resize keeps its arrow even
    // when the window lags behind and the pointer runs out over something
    // else.
    if !handle.is_grabbed() {
        match under.as_ref() {
            Some(Hit {
                frame: Some(part), ..
            }) => state.cursor = CursorImageStatus::Named(cursor_for(*part)),
            None => state.cursor = CursorImageStatus::default_named(),
            Some(_) => {}
        }
    }
    handle.motion(
        state,
        under.and_then(|hit| Some((hit.surface?.into(), hit.origin))),
        &MotionEvent {
            location: at,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    handle.frame(state);

    if let Some(focus) = policy(state)
        && !over_layer
    {
        let decision = focus.pointer_over(over, state.focused_surface());
        state.apply_focus(decision);
    }
    // The cursor itself moved, whatever the policy decided.
    state.backend.redraw();
    #[cfg(feature = "capture")]
    state.flush_screencopy_for_pointer();
}

/// A button went down or up at the pointer's current location.
fn button(state: &mut Compositor, code: u32, pressed: ButtonState, time: u32) {
    let Some(handle) = state.pointer.clone() else {
        return;
    };
    let at = handle.current_location();
    let event = ButtonEvent {
        serial: SERIAL_COUNTER.next_serial(),
        time,
        button: code,
        state: pressed,
    };
    if pressed == ButtonState::Pressed {
        // A click at the edge of the screen is a click, not a flip, and a
        // click with Logo held is not a tap of it.
        if let Running::Seat(session) = &mut state.backend {
            session.dwell.cancel();
            session.logo_tap.interrupt();
        }
        let hit = under(state, at);
        // A panel or launcher that takes the keyboard on a click gets it,
        // without anything being raised: layers stack by layer, not by click.
        if let Some(surface) = hit
            .as_ref()
            .filter(|hit| hit.window.is_none() && hit.takes_focus)
            .and_then(|hit| hit.surface.clone())
        {
            state.focus_plain(surface);
        }
        let frame = hit.as_ref().and_then(|hit| hit.frame);
        let over = hit.as_ref().and_then(|hit| hit.window.clone());
        // Focus and raise before the press is delivered, so the client
        // receives its click already on top and focused, as it would under
        // any desktop. And before a binding, so one that acts on a window
        // acts on the window clicked.
        if let Some(focus) = policy(state) {
            let decision = focus.pressed(over.as_ref().and_then(id_of), state.focused_surface());
            state.apply_focus(decision);
        }
        // A press a binding took is no half of a titlebar's double-click.
        if state.bound_press(hit.as_ref(), at, code, held(state), time) {
            if let Running::Seat(session) = &mut state.backend {
                session.title_press = None;
            }
            return;
        }
        // A press on a frame is the compositor's, and no client sees it. The
        // middle button on a title or a tab picks the tab up, to drop on
        // another window's titlebar or away from its own.
        if let (Some(window), Some(part), BTN_LEFT) = (over.as_ref(), frame, code) {
            pressed_frame(state, window, part, at, time, event.serial);
        } else if let (Some(window), Some(Part::Title | Part::Tab(_)), BTN_MIDDLE) =
            (over.as_ref(), frame, code)
        {
            let tab = match frame {
                Some(Part::Tab(n)) => state.tab_at(window, n),
                _ => None,
            }
            .unwrap_or_else(|| window.clone());
            let start = GrabStartData {
                focus: None,
                button: code,
                location: at,
            };
            state.start_tab_drag(&tab, start, event.serial);
        }
        // With the drag modifier held, the press is the compositor's: it
        // starts a move or resize, and the client never sees it.
        else if let (Some(window), Some(drag)) = (over, drag(state, code)) {
            let start = GrabStartData {
                focus: None,
                button: code,
                location: at,
            };
            match drag {
                Drag::Move => state.start_move(&window, start, event.serial),
                Drag::Resize => {
                    let Some(bounds) = state.space.element_geometry(&window) else {
                        return;
                    };
                    let inside = (at - bounds.loc.to_f64()).to_i32_round();
                    let edges = edges_near((inside.x, inside.y), (bounds.size.w, bounds.size.h));
                    state.start_resize(&window, edges, start, event.serial);
                }
            }
        }
    }
    if pressed == ButtonState::Released {
        if state.bound_release(code) {
            return;
        }
        if code == BTN_LEFT {
            released_frame(state, at);
        }
    }
    // Delivered to the client, or to the grab just started, which is how the
    // grab learns which button to wait for the release of.
    handle.button(state, &event);
    handle.frame(state);
}

/// The left button went down on `window`'s frame.
///
/// The titlebar moves the window, or with a second press soon after,
/// maximizes it. An edge resizes. A button waits for the release, so a press
/// dragged off a button before it is let go does nothing, as everywhere else.
fn pressed_frame(
    state: &mut Compositor,
    window: &Framed,
    part: Part,
    at: Point<f64, Logical>,
    time: u32,
    serial: smithay::utils::Serial,
) {
    let start = GrabStartData {
        focus: None,
        button: BTN_LEFT,
        location: at,
    };
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    let id = id_of(window);
    let press = (time, (at.x, at.y));
    let within = session.settings.pointer.double_click_ms;
    let double = session
        .title_press
        .take()
        .is_some_and(|(was, first)| was == id && is_double(first, press, within));
    match part {
        Part::Title if double => state.toggle_maximize(window),
        Part::Title => {
            session.title_press = Some((id, press));
            state.start_move(window, start, serial);
        }
        Part::Edge(edges) => state.start_resize(window, edges, start, serial),
        Part::Button(button) => session.button_press = Some((window.clone(), button)),
        // A tab behind comes to the front, and the press then moves the
        // group, as a press on a title does.
        Part::Tab(n) => {
            let tab = state.tab_at(window, n).unwrap_or_else(|| window.clone());
            state.activate_tab(&tab);
            state.start_move(&tab, start, serial);
        }
    }
}

/// The left button came up: if it went down on a frame's button and comes up
/// on the same one, that button does its job.
fn released_frame(state: &mut Compositor, at: Point<f64, Logical>) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    let Some((window, button)) = session.button_press.take() else {
        return;
    };
    if state.frame_part(&window, at) != Some(Part::Button(button)) {
        return;
    }
    match button {
        FrameButton::Close => Compositor::close(&window),
        FrameButton::Maximize => state.toggle_maximize(&window),
        FrameButton::Minimize => state.minimize(&window),
    }
}

/// Whether pressing this button, with the modifiers held now, is a drag.
fn drag(state: &Compositor, code: u32) -> Option<Drag> {
    let Running::Seat(session) = &state.backend else {
        return None;
    };
    session
        .settings
        .bindings
        .drag(held(state), Button::from_code(code))
}

/// The modifiers held now.
fn held(state: &Compositor) -> Mods {
    state
        .keyboard
        .as_ref()
        .map(|keyboard| mods(&keyboard.modifier_state()))
        .unwrap_or_default()
}

/// A scroll. Continuous amounts where the device reports them; otherwise the
/// wheel's 120ths, at the 15 pixels a detent that libinput and every toolkit
/// assume.
fn axis(state: &mut Compositor, event: &impl PointerAxisEvent<LibinputInputBackend>) {
    let Some(handle) = state.pointer.clone() else {
        return;
    };
    // Logo held while scrolling is a gesture, not a tap.
    if let Running::Seat(session) = &mut state.backend {
        session.logo_tap.interrupt();
    }
    let at = handle.current_location();
    let scrolled = [Axis::Horizontal, Axis::Vertical].map(|axis| Scrolled {
        v120: event.amount_v120(axis),
        pixels: event.amount(axis).unwrap_or(0.0),
        lifted: event.source() == AxisSource::Finger && event.amount(axis) == Some(0.0),
    });
    let hit = under(state, at);
    if state.bound_scroll(hit.as_ref(), at, held(state), scrolled) {
        return;
    }
    if scroll_flips(state, event) {
        return;
    }
    let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
    for direction in [Axis::Horizontal, Axis::Vertical] {
        let discrete = event.amount_v120(direction);
        let amount = event
            .amount(direction)
            .unwrap_or_else(|| discrete.unwrap_or(0.0) * 15.0 / 120.0);
        if amount != 0.0 {
            frame = frame
                .relative_direction(direction, event.relative_direction(direction))
                .value(direction, amount);
            if let Some(discrete) = discrete {
                // libinput reports v120 steps as whole numbers in an f64.
                frame = frame.v120(direction, discrete as i32);
            }
        }
        // A finger lifted off a touchpad: tell the client the scroll ended,
        // so kinetic scrolling can take over.
        if event.source() == AxisSource::Finger && event.amount(direction) == Some(0.0) {
            frame = frame.stop(direction);
        }
    }
    handle.axis(state, frame);
    handle.frame(state);
}

/// Milliseconds since the session started: the clock the edge dwell runs on.
fn clock(state: &Compositor) -> u64 {
    u64::try_from(state.started_at().elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The pointer is at `at`: feed the edge dwell, and flip the workspace if it
/// has rested against an outer edge of the desk long enough. Returns where
/// the pointer goes instead, across the desk, when it flipped.
///
/// Off while a button is held, unless that button is dragging a window and
/// the config says a drag flips too; then the window goes with it. Off over
/// the fullscreen window in use, and behind the lock screen.
fn rest_at_edge(state: &mut Compositor, at: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
    let Running::Seat(session) = &state.backend else {
        return None;
    };
    let flipping = session.settings.flipping;
    let carrying = state.dragging.clone();
    let edge = if flipping.edge_flips(resting(state, at, carrying.is_some())) {
        edge_at((at.x, at.y), &state.output_rects())
    } else {
        None
    };
    let now = clock(state);
    let Running::Seat(session) = &mut state.backend else {
        return None;
    };
    let flip = session.dwell.feed(edge, now);
    arm_dwell(session, now);
    let direction = flip?;

    let output = state.space.output_under(at).next()?.clone();
    if !state.flip(&output.name(), direction, carrying.as_ref()) {
        return None;
    }
    // The pointer is about to come round to the far side of the desk: a
    // jump, to a window being moved, and no edge it lands beside holds it.
    state.desk_jumps = state.desk_jumps.wrapping_add(1);
    // Spanning, the desk is one big screen and the pointer comes round to
    // its far side. Per output, only this monitor flipped, and the pointer
    // comes round to the far side of this monitor.
    let rects = match state.workspaces.shape().mode {
        perspicax_policy::Mode::Spanning => state.output_rects(),
        perspicax_policy::Mode::PerOutput => {
            vec![crate::shell::rect(state.space.output_geometry(&output)?)]
        }
    };
    Some(arrival((at.x, at.y), direction, &rects).into())
}

/// How the pointer at `at` is resting against an edge, for flipping;
/// `carrying` when a window is being dragged.
fn resting(state: &Compositor, at: Point<f64, Logical>, carrying: bool) -> Resting {
    let grabbed = state
        .pointer
        .as_ref()
        .is_some_and(PointerHandle::is_grabbed);
    if state.lock.is_some() {
        Resting::Locked
    } else if state
        .space
        .element_under(at)
        .is_some_and(|(window, _)| covers_panels(window))
    {
        // Windows alone: a notification on the overlay layer, over the
        // game's edge, does not hand the edge back.
        Resting::OverFullscreen
    } else if !grabbed {
        Resting::Free
    } else if carrying {
        Resting::Carrying
    } else {
        Resting::Held
    }
}

/// Arm a timer for when the edge dwell is due, if it is due and no timer is
/// armed for that moment already. A pointer resting against an edge sends no
/// motion, so without this the rest would never be noticed to have lasted.
fn arm_dwell(session: &mut super::Session, now: u64) {
    let due = session.dwell.due();
    if due.is_none() || due == session.dwell_armed {
        return;
    }
    session.dwell_armed = due;
    let wait = Duration::from_millis(due.unwrap_or(now).saturating_sub(now));
    let armed = session
        .handle
        .insert_source(Timer::from_duration(wait), |_, (), state| {
            if let Running::Seat(session) = &mut state.backend {
                session.dwell_armed = None;
            }
            if let Some(at) = state.pointer.as_ref().map(PointerHandle::current_location) {
                // Through `moved`, so a flip warps the pointer exactly as a
                // motion that flipped would.
                let time = u32::try_from(clock(state)).unwrap_or(u32::MAX);
                moved(state, at, time);
            }
            TimeoutAction::Drop
        });
    if let Err(error) = armed {
        tracing::warn!(%error, "could not arm the edge-flip timer");
    }
}

/// A scroll over the desktop, with scroll flipping on: step the workspace
/// under the pointer once per notch, down and right forward, up and left
/// back. Returns whether the scroll was the compositor's, in which case no
/// client hears of it. A `[mouse]` wheel binding is looked for first, so
/// binding the wheel over the desktop turns this round.
fn scroll_flips(
    state: &mut Compositor,
    event: &impl PointerAxisEvent<LibinputInputBackend>,
) -> bool {
    let Running::Seat(session) = &state.backend else {
        return false;
    };
    if !session.settings.flipping.scroll || state.lock.is_some() {
        return false;
    }
    let Some(pointer) = state.pointer.clone() else {
        return false;
    };
    if pointer.is_grabbed() {
        return false;
    }
    let at = pointer.current_location();
    if !state.over_desktop(at) {
        return false;
    }
    let Some(output) = state.space.output_under(at).next().map(Output::name) else {
        return false;
    };
    let Running::Seat(session) = &mut state.backend else {
        return false;
    };
    let mut notches = 0;
    for axis in [Axis::Vertical, Axis::Horizontal] {
        if event.source() == AxisSource::Finger && event.amount(axis) == Some(0.0) {
            session.notches.reset();
        }
        notches += session
            .notches
            .feed(event.amount_v120(axis), event.amount(axis).unwrap_or(0.0));
    }
    for _ in 0..notches.unsigned_abs() {
        state.scroll_workspace(&output, notches > 0);
    }
    true
}

/// The bounding box of every output.
fn extent(state: &Compositor) -> Option<Rectangle<i32, Logical>> {
    state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .reduce(|a, b| a.merge(b))
}

impl Session {
    /// Light every keyboard's lock keys as xkb has them.
    pub(crate) fn light(&mut self, leds: LedState) {
        for device in &mut self.devices {
            light(device, leds);
        }
    }
}

/// Light one device's lock keys, if it is a keyboard.
fn light(device: &mut Device, leds: LedState) {
    if device.has_capability(DeviceCapability::Keyboard) {
        device.led_update(lit(leds));
    }
}

/// libinput's LEDs for xkb's: a light the keymap has no indicator for is
/// off.
fn lit(leds: LedState) -> Led {
    [
        (leds.num, Led::NUMLOCK),
        (leds.caps, Led::CAPSLOCK),
        (leds.scroll, Led::SCROLLLOCK),
    ]
    .into_iter()
    .filter(|&(on, _)| on == Some(true))
    .fold(Led::empty(), |lit, (_, led)| lit | led)
}

fn mods(modifiers: &ModifiersState) -> Mods {
    Mods {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        logo: modifiers.logo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_is_lit_only_when_xkb_says_it_is_on() {
        let leds = LedState {
            num: Some(false),
            caps: Some(true),
            scroll: None,
        };
        assert_eq!(lit(leds), Led::CAPSLOCK);
        assert_eq!(lit(LedState::default()), Led::empty());
    }
}
