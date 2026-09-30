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
//! The pointer is confined to the outputs ([`super::super::pointer`]),
//! hit-tested against the stack, and every motion and press is put to the
//! focus policy ([`perspicax_policy::Focus`]), whose decision is then carried
//! out here.

use perspicax_node::SurfaceId;
use perspicax_policy::{Action, Button, Change, Decision, Drag, Mods, cycle, edges_near};
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
    desktop::{Window, WindowSurfaceType},
    input::{
        keyboard::{FilterResult, Keycode, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, GrabStartData, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
};

use super::{
    super::{
        Running,
        hatch::{self, Hatch},
        pointer,
    },
    settings,
};
use crate::{shell::id_of, state::Compositor};

/// Linux button codes, from `linux/input-event-codes.h`.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

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
            if let Running::Seat(session) = &mut state.backend {
                settings::configure(&mut device, &session.settings.pointer);
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
            if pressed == KeyState::Released {
                return match session.swallowed.iter().position(|&k| k == keycode) {
                    Some(at) => {
                        session.swallowed.swap_remove(at);
                        FilterResult::Intercept(Taken::Release)
                    }
                    None => FilterResult::Forward,
                };
            }
            let raw = keysym.raw_syms();
            let taken = hatch::classify(modifiers, keysym.modified_sym(), &raw)
                .map(Taken::Hatch)
                .or_else(|| {
                    let mut syms = raw.clone();
                    syms.push(keysym.modified_sym());
                    session
                        .settings
                        .bindings
                        .resolve(mods(modifiers), &syms)
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
        Some(Taken::Bound(action)) => perform(state, &action),
        Some(Taken::Release) | None => {}
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

/// Carry out a binding.
fn perform(state: &mut Compositor, action: &Action) {
    match action {
        Action::Close => {
            let window = state
                .focused_surface()
                .and_then(|id| state.window_for_id(id));
            if let Some(toplevel) = window.as_ref().and_then(Window::toplevel) {
                toplevel.send_close();
            }
        }
        Action::Spawn(command) => {
            let launch = state.launch.clone();
            if let Running::Seat(session) = &mut state.backend {
                session.spawn(launch.as_ref(), command);
            }
        }
        Action::MoveToOutput(towards) => {
            if let Some(window) = state
                .focused_surface()
                .and_then(|id| state.window_for_id(id))
            {
                state.move_to_output(&window, *towards);
            }
        }
        Action::Reload => settings::reload(state),
        Action::CycleFocus => {
            // Minimized windows count as below the bottom of the stack, so
            // cycling reaches them first and brings them back: without a
            // taskbar (W4), this is how a minimized window returns.
            let stack: Vec<SurfaceId> = state
                .minimized
                .iter()
                .chain(state.space.elements())
                .filter_map(id_of)
                .collect();
            if let Some(next) = cycle(&stack) {
                apply(
                    state,
                    Decision {
                        focus: Change::To(next),
                        raise: Some(next),
                    },
                );
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
    let at = pointer::confine(to, &outputs);
    let under = under(state, at);
    let over = under.as_ref().and_then(|(window, ..)| id_of(window));
    handle.motion(
        state,
        under.map(|(_, surface, origin)| (surface, origin)),
        &MotionEvent {
            location: at,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    handle.frame(state);

    if let Some(focus) = policy(state) {
        let decision = focus.pointer_over(over, state.focused_surface());
        apply(state, decision);
    }
    // The cursor itself moved, whatever the policy decided.
    state.backend.redraw();
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
        let over = under(state, at).map(|(window, ..)| window);
        // Focus and raise before the press is delivered, so the client
        // receives its click already on top and focused, as it would under
        // any desktop.
        if let Some(focus) = policy(state) {
            let decision = focus.pressed(over.as_ref().and_then(id_of), state.focused_surface());
            apply(state, decision);
        }
        // With the drag modifier held, the press is the compositor's: it
        // starts a move or resize, and the client never sees it.
        if let (Some(window), Some(drag)) = (over, drag(state, code)) {
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
    // Delivered to the client, or to the grab just started, which is how the
    // grab learns which button to wait for the release of.
    handle.button(state, &event);
    handle.frame(state);
}

/// Whether pressing this button, with the modifiers held now, is a drag.
fn drag(state: &Compositor, code: u32) -> Option<Drag> {
    let Running::Seat(session) = &state.backend else {
        return None;
    };
    let held = state.keyboard.as_ref()?.modifier_state();
    let button = match code {
        BTN_LEFT => Button::Left,
        BTN_RIGHT => Button::Right,
        BTN_MIDDLE => Button::Middle,
        _ => return None,
    };
    session.settings.bindings.drag(mods(&held), button)
}

/// A scroll. Continuous amounts where the device reports them; otherwise the
/// wheel's 120ths, at the 15 pixels a detent that libinput and every toolkit
/// assume.
fn axis(state: &mut Compositor, event: &impl PointerAxisEvent<LibinputInputBackend>) {
    let Some(handle) = state.pointer.clone() else {
        return;
    };
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

/// Carry out a focus decision.
fn apply(state: &mut Compositor, decision: Decision<SurfaceId>) {
    if decision == Decision::none() {
        return;
    }
    let targets = [
        match decision.focus {
            Change::To(id) => Some(id),
            _ => None,
        },
        decision.raise,
    ];
    let parked: Vec<_> = targets
        .into_iter()
        .flatten()
        .filter_map(|id| state.minimized_with(id))
        .collect();
    for window in parked {
        state.restore(&window);
    }
    match decision.focus {
        Change::Keep => {}
        Change::To(id) => {
            let surface = state.window_for_id(id).and_then(|window| {
                window
                    .toplevel()
                    .map(|toplevel| toplevel.wl_surface().clone())
            });
            if let Some(surface) = surface {
                state.focus_surface(surface, id);
            }
        }
        Change::Clear => {
            if let Some(keyboard) = state.keyboard.clone() {
                keyboard.set_focus(state, None, SERIAL_COUNTER.next_serial());
            }
        }
    }
    if let Some(window) = decision.raise.and_then(|id| state.window_for_id(id)) {
        state.space.raise_element(&window, false);
        state.backend.redraw();
    }
    // Focus and stacking are both facts the index judges against.
    state.publish_facts();
}

/// The topmost window at `at`, the surface of it there (a subsurface or the
/// toplevel itself), and that surface's origin in global space -- the pair
/// `PointerHandle::motion` wants.
fn under(
    state: &Compositor,
    at: Point<f64, Logical>,
) -> Option<(Window, WlSurface, Point<f64, Logical>)> {
    let (window, location) = state.space.element_under(at)?;
    let (surface, offset) = window.surface_under(at - location.to_f64(), WindowSurfaceType::ALL)?;
    Some((window.clone(), surface, (location + offset).to_f64()))
}

fn policy(state: &Compositor) -> Option<perspicax_policy::Focus> {
    match &state.backend {
        Running::Seat(session) => Some(session.settings.focus),
        Running::Headless { .. } => None,
    }
}

/// The bounding box of every output.
fn extent(state: &Compositor) -> Option<Rectangle<i32, Logical>> {
    state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .reduce(|a, b| a.merge(b))
}

fn mods(modifiers: &ModifiersState) -> Mods {
    Mods {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        logo: modifiers.logo,
    }
}
