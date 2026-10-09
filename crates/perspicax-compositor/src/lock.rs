//! The session lock (swaylock), and idle (swayidle, and a video player
//! keeping the screen on).
//!
//! # While locked
//!
//! Nothing but the lock surfaces is drawn, hit-tested or given the keyboard,
//! and no binding runs: a launcher binding that opened a terminal over the
//! lock screen would be an unlock. Only the escape hatches still work, and
//! both leave the session rather than bypass the lock.
//!
//! Agents are locked out too. The lock surfaces are published at the top of
//! the stack, so every node beneath is honestly judged occluded, and an act is
//! refused outright ([`crate::ActError::Locked`]). A person who locked their
//! screen has walked away, and nobody should be driving their applications.

use perspicax_node::SurfaceId;
use smithay::{
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    wayland::{
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker},
    },
};

use crate::state::Compositor;

/// The session is locked, and these are the surfaces covering each output.
#[derive(Debug, Default)]
pub(crate) struct Locked {
    pub(crate) surfaces: Vec<Cover>,
}

/// One lock surface, the output it covers, and its id for the facts.
#[derive(Debug)]
pub(crate) struct Cover {
    pub(crate) output: Output,
    pub(crate) surface: LockSurface,
    pub(crate) id: SurfaceId,
}

impl Locked {
    /// The lock surface covering this output.
    pub(crate) fn on(&self, output: &Output) -> Option<&Cover> {
        self.surfaces.iter().find(|cover| &cover.output == output)
    }
}

impl SessionLockHandler for Compositor {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.lock_manager
    }

    /// Lock. Confirmed at once: from the next frame on, nothing but lock
    /// surfaces is drawn. The frame already on the screen stays there for
    /// at most one refresh, which is the gap between confirming and the
    /// first frame that could prove the screen is covered.
    fn lock(&mut self, confirmation: SessionLocker) {
        tracing::info!("session locked");
        self.lock = Some(Locked::default());
        self.clear_focus();
        confirmation.lock();
        self.backend.redraw();
        self.publish_facts();
    }

    fn unlock(&mut self) {
        tracing::info!("session unlocked");
        self.lock = None;
        self.focus_top_window();
        self.backend.redraw();
        self.publish_facts();
    }

    /// A lock surface for one output: sized to cover it, and given the
    /// keyboard if it is the first.
    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let Some(output) = Output::from_resource(&output) else {
            return;
        };
        let Some(area) = self.space.output_geometry(&output) else {
            return;
        };
        surface.with_pending_state(|pending| {
            pending.size = Some(
                (
                    u32::try_from(area.size.w).unwrap_or(0),
                    u32::try_from(area.size.h).unwrap_or(0),
                )
                    .into(),
            );
        });
        surface.send_configure();
        let wl_surface = surface.wl_surface().clone();
        let id = self.mint_surface_id();
        let Some(locked) = self.lock.as_mut() else {
            return;
        };
        let first = locked.surfaces.is_empty();
        locked.surfaces.push(Cover {
            output,
            surface,
            id,
        });
        if first {
            self.focus_plain(wl_surface);
        }
        self.backend.redraw();
        self.publish_facts();
    }
}

impl Compositor {
    /// The lock surface at a point, if the session is locked.
    pub(crate) fn lock_surface_at(
        &self,
        at: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) -> Option<(
        WlSurface,
        smithay::utils::Point<f64, smithay::utils::Logical>,
    )> {
        let locked = self.lock.as_ref()?;
        let output = self.space.output_under(at).next()?;
        let cover = locked.on(output)?;
        let origin = self.space.output_geometry(output)?.loc.to_f64();
        Some((cover.surface.wl_surface().clone(), origin))
    }
}

impl IdleNotifierHandler for Compositor {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle
    }
}

impl IdleInhibitHandler for Compositor {
    /// A surface (a video playing, a presentation) asks the session not to go
    /// idle while it is up.
    fn inhibit(&mut self, surface: WlSurface) {
        self.inhibitors.push(surface);
        self.update_inhibition();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.inhibitors.retain(|inhibitor| inhibitor != &surface);
        self.update_inhibition();
    }
}

impl Compositor {
    /// Idle is inhibited while any inhibiting surface is still alive. A dead
    /// one is dropped here rather than trusted to have said goodbye: a
    /// crashed video player must not keep the screen on forever.
    fn update_inhibition(&mut self) {
        use smithay::reexports::wayland_server::Resource as _;
        self.inhibitors.retain(WlSurface::is_alive);
        let inhibited = !self.inhibitors.is_empty();
        self.idle.set_is_inhibited(inhibited);
    }
}

smithay::delegate_session_lock!(Compositor);
smithay::delegate_idle_notify!(Compositor);
smithay::delegate_idle_inhibit!(Compositor);
smithay::delegate_layer_shell!(Compositor);
