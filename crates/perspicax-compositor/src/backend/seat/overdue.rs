//! The frame callbacks no render sent.
//!
//! A window is told it may draw again as its monitor is rendered (see the
//! seat's "Frames"). A window no monitor draws -- parked on another
//! workspace, minimized, behind a tab, or off every monitor -- is never told
//! that way, nor is one on a monitor whose frame never came back. GTK,
//! Chromium and games wait for the word with no limit. So once a second,
//! every window that has waited that long is told anyway, as niri,
//! cosmic-comp and anvil do: a parked window draws a frame a second, and no
//! window waits forever, whatever kept it waiting. Nor does a window or a
//! panel its client unmapped (see `shell::withdrawn` and `layers`).
//!
//! A window on screen should never need this, since its monitor tells it as
//! it is drawn. One that did is named in the log, at debug: something kept
//! its monitor from telling it, as nothing did for #99.

use std::time::Duration;

use smithay::{
    desktop::utils::send_frames_surface_tree,
    reexports::calloop::{
        LoopHandle,
        timer::{TimeoutAction, Timer},
    },
    wayland::compositor::{SurfaceAttributes, TraversalAction, with_surface_tree_downward},
};

use crate::{Error, backend::Running, framed::Framed, shell, state::Compositor};

/// How often the timer looks.
const EVERY: Duration = Duration::from_secs(1);

/// How long a window waits to be told it may draw before the timer tells it.
/// A little under [`EVERY`], so a tick that comes late still finds it due.
/// A render passes it too, with every window it tells, which is how the
/// timer knows which windows a render has told lately.
pub(super) const OVERDUE: Duration = Duration::from_millis(995);

/// Look once a second, for as long as the session runs.
pub(super) fn arm(handle: &LoopHandle<'static, Compositor>) -> Result<(), Error> {
    handle
        .insert_source(Timer::from_duration(EVERY), |_, (), state| {
            release(state);
            TimeoutAction::ToDuration(EVERY)
        })
        .map(drop)
        .map_err(|error| Error::EventLoop(error.to_string()))
}

/// Tell every window, on screen or parked, that has waited [`OVERDUE`] that
/// it may draw.
fn release(state: &mut Compositor) {
    let Running::Seat(session) = &state.backend else {
        return;
    };
    // Away on another VT, no monitor is drawn, and none is expected to be.
    let watching = session.active;
    // Any monitor: what decides is how long each window has waited.
    let Some(output) = state.space.outputs().next().cloned() else {
        return;
    };
    let now = state.started_at().elapsed();
    for window in state.space.elements() {
        let waited = waits(window);
        window.send_frame(&output, now, Some(OVERDUE), |_, _| None);
        if watching && waited && !waits(window) {
            tracing::debug!(
                window = ?shell::id_of(window),
                "a window on screen waited a second to be told it may draw"
            );
        }
    }
    for window in &state.parked {
        window.send_frame(&output, now, Some(OVERDUE), |_, _| None);
    }
    // And one its client unmapped, which may wait for the word before it
    // asks for its window again.
    for withdrawn in &state.withdrawn {
        send_frames_surface_tree(
            withdrawn.toplevel.wl_surface(),
            &output,
            now,
            Some(OVERDUE),
            |_, _| None,
        );
    }
    // A panel its client unmapped is in no monitor's layers, so no monitor
    // tells it either.
    for withdrawn in &state.withdrawn_layers {
        send_frames_surface_tree(
            withdrawn.surface.wl_surface(),
            &output,
            now,
            Some(OVERDUE),
            |_, _| None,
        );
    }
}

/// Whether `window` waits to be told it may draw: a frame callback is
/// outstanding on its surface, or on one of its subsurfaces.
fn waits(window: &Framed) -> bool {
    let Some(surface) = shell::surface_of(window) else {
        return false;
    };
    let mut waits = false;
    with_surface_tree_downward(
        &surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            waits |= !states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .is_empty();
        },
        |_, _, &()| true,
    );
    waits
}
