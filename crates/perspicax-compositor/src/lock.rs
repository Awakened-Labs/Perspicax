//! The session lock (swaylock), and idle (swayidle, and a video player
//! keeping the screen on).
//!
//! # While locked
//!
//! Nothing but the lock surfaces is drawn, hit-tested or given the keyboard
//! or the pointer, and no binding runs: a launcher binding that opened a
//! terminal over the lock screen would be an unlock. Only the escape hatches
//! still work, and both leave the session rather than bypass the lock. On a
//! seat, beneath the lock surfaces, the compositor draws the lock screen's
//! own colour and logo (`crate::backdrop`), which show wherever the lock
//! client leaves its surface clear, and before it has drawn at all.
//!
//! The keyboard goes to the lock's own surfaces and nothing else. One gate,
//! [`Compositor::may_take_keyboard`], stands wherever anything is given it,
//! a window, a launcher or a lock surface (#127). A menu's grab, the one
//! other way to it, is refused while locked (#82). Any client may still open
//! a window behind the lock, ask for one to be brought forward, or close the
//! one in front, and on any desktop each of those moves the keyboard. Here
//! none does: whatever took it would get the password as it is typed, and
//! Enter would send it. Unlocked, the window on top has the keyboard, often
//! the one that opened meanwhile.
//!
//! The pointer is taken from the windows the moment the session locks, not
//! at the next motion: a hand at rest, or a tap of the touchpad to wake the
//! screen, would otherwise click in the window under it, unseen (#82). The
//! lock surface under it has it as soon as it comes up, and the window under
//! it has it back as soon as the session unlocks.
//!
//! Agents are locked out too. The lock surfaces are published at the top of
//! the stack, so every node beneath is honestly judged occluded, and an act is
//! refused outright ([`crate::ActError::Locked`]). A person who locked their
//! screen has walked away, and nobody should be driving their applications.
//!
//! # Who locks
//!
//! Only a program `[protocols] session-lock` admits may lock (#178): by
//! default, the copy of `[shell] lock`'s program the system installed. A lock
//! program is typed the password, so any client that could lock could put up
//! a lock screen of its own, over perspicax's own backdrop, and be typed the
//! password instead. A client the rule does not admit is not shown the
//! global, and one that bound it before a reload narrowed the rule is
//! refused its lock as a second lock is, told `finished`. A reload never
//! takes the session from the lock already holding it.
//!
//! # Who unlocks
//!
//! One lock holds the session, and only it may unlock it (#126). Otherwise
//! any program with a Wayland socket could lock and unlock straight away,
//! and open a session its owner locked and walked away from. A second lock while the holder's client lives is refused, and
//! its lock surfaces cover nothing: it is told `finished`, as swaylock is
//! when swayidle starts a second one. Its `unlock_and_destroy` is the
//! protocol error the protocol says it is, and unlocks nothing.
//!
//! A holder whose client goes without unlocking -- swaylock crashed, or was
//! killed -- leaves the session locked, as ext-session-lock requires, over
//! only the backdrop. The next lock to ask takes it over: that is how a
//! person recovers their session, by starting a lock program again from
//! another terminal.
//!
//! ## When to delete the dispatch below
//!
//! smithay 0.7.0 unlocks for any lock's `unlock_and_destroy`, and does not
//! say which lock a lock surface was asked for on. So perspicax answers
//! `ext_session_lock_v1` itself, and smithay only what it lets through, as
//! [`crate::sizes`] does for sizes (#155). Smithay master has both:
//! Smithay/smithay@8d33fec9 ("Don't allow an unrelated client to unlock the
//! session"), with 70f04625 and fea33c8f. Once a release carries them, the
//! unlock check and [`AskedOn`] go, and so does `delegate_session_lock!`'s
//! stand-in, if what [`Compositor::lock_gone`] does has found another home:
//! upstream tells the handler nothing when the holder goes.

use std::sync::{Mutex, PoisonError};

use perspicax_node::{Origin, SurfaceId};
use perspicax_policy::Protocol;
use smithay::{
    input::pointer::CursorImageStatus,
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::{
            ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
            ext_session_lock_v1::{self, ExtSessionLockV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, Resource,
            backend::ClientId,
            delegate_dispatch, delegate_global_dispatch,
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        },
    },
    utils::IsAlive as _,
    wayland::{
        compositor::with_states,
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        seat::WaylandFocus as _,
        session_lock::{
            ExtLockSurfaceUserData, LockSurface, SessionLockHandler, SessionLockManagerGlobalData,
            SessionLockManagerState, SessionLockState, SessionLocker,
        },
    },
};

use crate::{
    chord_capture::{Heard, Reason},
    focus::FocusTarget,
    state::{ClientState, Compositor},
};

/// The session is locked: by which lock, and the surfaces covering each
/// output.
#[derive(Debug)]
pub(crate) struct Locked {
    /// The lock holding the session, the one that may unlock it.
    pub(crate) holder: ExtSessionLockV1,
    /// The holder's lock surfaces, and only the holder's.
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
    /// Locked by `holder`, and nothing covering any output yet.
    fn by(holder: ExtSessionLockV1) -> Self {
        Self {
            holder,
            surfaces: Vec::new(),
        }
    }

    /// The lock surface covering this output.
    pub(crate) fn on(&self, output: &Output) -> Option<&Cover> {
        self.surfaces.iter().find(|cover| &cover.output == output)
    }

    /// Whether `surface` is one of the holder's lock surfaces, and still
    /// there.
    pub(crate) fn covers(&self, surface: &WlSurface) -> bool {
        surface.alive()
            && self
                .surfaces
                .iter()
                .any(|cover| cover.surface.wl_surface() == surface)
    }

    /// Whether a key sent to `target` reaches the lock screen: one of the
    /// holder's lock surfaces, still there. An X11 window never does.
    pub(crate) fn hears(&self, target: &FocusTarget) -> bool {
        match target {
            FocusTarget::Wayland(surface) => self.covers(surface),
            #[cfg(feature = "xwayland")]
            FocusTarget::X11(_) => false,
        }
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
    ///
    /// Whatever holds the keyboard or the pointer lets go first: a window's
    /// open menu closes, and a drag ends where it is ([`Self::let_go`]).
    /// Otherwise the menu kept the keys from the lock screen, and Enter
    /// chose its item unseen.
    ///
    /// The pointer leaves every window now, before any lock surface has come
    /// to take it, and the cursor is the compositor's arrow until the lock
    /// client asks for its own. Smithay puts the arrow back as a client loses
    /// the pointer, but a frame's resize arrow belongs to no client, and
    /// would otherwise stay drawn over the lock.
    ///
    /// Already locked, the lock is refused while the holder's client lives,
    /// and takes the session over once it has gone. See the module's "Who
    /// unlocks".
    ///
    /// A lock from a client `[protocols] session-lock` does not admit is
    /// refused first, whether to lock or to take a session over: it bound the
    /// global before a reload narrowed the rule. See "Who locks".
    fn lock(&mut self, confirmation: SessionLocker) {
        let lock = confirmation.ext_session_lock().clone();
        let admitted = lock
            .client()
            .is_some_and(|client| self.gate.admits(Protocol::SessionLock, &client));
        if !admitted {
            tracing::warn!(
                origin = ?origin(&lock),
                "refused a lock: [protocols] session-lock does not admit it"
            );
            if let Some(client) = lock.client() {
                self.gate.note_if_a_lock(&client);
            }
            // Which tells it `finished`.
            drop(confirmation);
            return;
        }
        if let Some(locked) = self.lock.as_mut() {
            if locked.holder.is_alive() {
                tracing::warn!(
                    origin = ?origin(&lock),
                    "refused a lock: another lock holds the session"
                );
                // Which tells it `finished`.
                drop(confirmation);
                return;
            }
            tracing::info!(
                origin = ?origin(&lock),
                "session lock taken over from a lock client that went without unlocking"
            );
            locked.holder = lock;
            confirmation.lock();
            self.publish_facts();
            return;
        }
        tracing::info!("session locked");
        // Before the keyboard goes, which would call it unfocused.
        self.end_capture(Heard::Cancelled(Reason::Locked));
        self.lock = Some(Locked::by(lock));
        // A switch being drawn is not finished behind the lock, nor after it.
        self.backend.settle_transitions();
        self.let_go();
        self.clear_focus();
        self.cursor = CursorImageStatus::default_named();
        self.repoint();
        confirmation.lock();
        self.backend.redraw();
        self.publish_facts();
    }

    /// Unlock, by the holder: no other lock's unlock gets this far. The
    /// window under the pointer has it back at once. As the lock surface
    /// loses it, smithay puts the compositor's arrow back, so the cursor the
    /// lock client asked for, which for swaylock is none at all, goes too,
    /// until the window sets its own.
    fn unlock(&mut self) {
        tracing::info!("session unlocked");
        self.lock = None;
        self.focus_top_window();
        self.repoint();
        self.backend.redraw();
        self.publish_facts();
    }

    /// A lock surface for one output: sized to cover it, given the keyboard
    /// if it is the first, and the pointer if the pointer is on its output,
    /// whichever came first. swaylock hides the cursor as the pointer
    /// arrives, so until it does the cursor is drawn over the lock.
    ///
    /// A lock surface of a lock that does not hold the session is sized like
    /// any other, but covers nothing: the protocol has a refused lock's
    /// surfaces go undisplayed.
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
        if !asked_on(&wl_surface).is_some_and(|lock| self.holds(&lock)) {
            return;
        }
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
        self.repoint();
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

    /// Whether the keys are where they belong: anywhere while the session is
    /// unlocked. Locked, on one of the holder's lock surfaces, with no
    /// menu's grab to move them on the way out, or on nothing while none is
    /// up (#197). See the module's "While locked".
    pub(crate) fn keys_with_the_lock(&self) -> bool {
        let (Some(locked), Some(keyboard)) = (&self.lock, &self.keyboard) else {
            return true;
        };
        if keyboard.is_grabbed() {
            return false;
        }
        match keyboard.current_focus() {
            Some(target) => locked.hears(&target),
            None => !locked.surfaces.iter().any(|cover| cover.surface.alive()),
        }
    }

    /// Put the keys where they belong while locked, if they are not there
    /// (#197): on the lock screen on the pointer's monitor, else on the
    /// first still up, else on nothing. A menu's grab goes first, since
    /// smithay keeps the keys on the menu whatever is given them while it
    /// holds them.
    pub(crate) fn keep_keys_on_lock(&mut self) {
        if self.keys_with_the_lock() {
            return;
        }
        self.let_go();
        match self.cover_for_keys() {
            Some(cover) => self.focus_plain(cover),
            None => self.clear_focus(),
        }
        if !self.keys_with_the_lock() {
            tracing::warn!("the keys could not be given back to the lock screen");
        }
    }

    /// The lock surface the keys go to: the one on the monitor the pointer
    /// is on, where the person is, else the first still up.
    fn cover_for_keys(&self) -> Option<WlSurface> {
        let locked = self.lock.as_ref()?;
        let up = |cover: &&Cover| cover.surface.alive();
        self.pointer
            .as_ref()
            .and_then(|pointer| self.space.output_under(pointer.current_location()).next())
            .and_then(|output| locked.on(output))
            .filter(up)
            .or_else(|| locked.surfaces.iter().find(up))
            .map(|cover| cover.surface.wl_surface().clone())
    }

    /// Whether `lock` holds the session.
    fn holds(&self, lock: &ExtSessionLockV1) -> bool {
        self.lock
            .as_ref()
            .is_some_and(|locked| locked.holder == *lock)
    }

    /// The holder's client went without unlocking. The session stays locked,
    /// as ext-session-lock requires. What it drew went with it, so nothing
    /// covers any output but the backdrop, and whichever lock takes the
    /// session over starts afresh: its first lock surface has the keyboard.
    fn lock_gone(&mut self) {
        tracing::warn!(
            "the lock client went without unlocking: the session stays locked until another lock takes it over"
        );
        if let Some(locked) = self.lock.as_mut() {
            locked.surfaces.clear();
        }
        self.repoint();
        self.backend.redraw();
        self.publish_facts();
    }
}

/// The process that asked for `object`, for the log: a lock, or a surface.
fn origin(object: &impl Resource) -> Origin {
    object
        .client()
        .and_then(|client| client.get_data::<ClientState>().map(ClientState::origin))
        .unwrap_or_default()
}

/// The process whose window or surface `target` is, for the log.
pub(crate) fn origin_of(target: &FocusTarget) -> Origin {
    target
        .wl_surface()
        .map(|surface| origin(&*surface))
        .unwrap_or_default()
}

/// The lock a lock surface was asked for on, kept on its `wl_surface`:
/// smithay 0.7's `new_surface` is handed the surface alone. Noted as the
/// request passes, before smithay answers it.
struct AskedOn(Mutex<Option<ExtSessionLockV1>>);

fn note_asked_on(surface: &WlSurface, lock: &ExtSessionLockV1) {
    with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe(|| AskedOn(Mutex::new(None)));
        if let Some(asked) = states.data_map.get::<AskedOn>() {
            *asked.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(lock.clone());
        }
    });
}

fn asked_on(surface: &WlSurface) -> Option<ExtSessionLockV1> {
    with_states(surface, |states| {
        let asked = states.data_map.get::<AskedOn>()?;
        asked
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    })
}

// `delegate_session_lock!`, but for `ext_session_lock_v1`.

delegate_global_dispatch!(Compositor: [ExtSessionLockManagerV1: SessionLockManagerGlobalData] => SessionLockManagerState);
delegate_dispatch!(Compositor: [ExtSessionLockManagerV1: ()] => SessionLockManagerState);
delegate_dispatch!(Compositor: [ExtSessionLockSurfaceV1: ExtLockSurfaceUserData] => SessionLockManagerState);

/// A lock's requests: smithay's answers, but only the holder's
/// `unlock_and_destroy` reaches smithay, which unlocks for any lock's.
/// Another's is ext-session-lock's `invalid_unlock`, "unlock requested but
/// locked event was never sent", which ends its client. Each lock surface
/// asked for is noted with its lock on the way, for `new_surface`.
impl Dispatch<ExtSessionLockV1, SessionLockState> for Compositor {
    fn request(
        state: &mut Self,
        client: &Client,
        lock: &ExtSessionLockV1,
        request: ext_session_lock_v1::Request,
        data: &SessionLockState,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match &request {
            ext_session_lock_v1::Request::UnlockAndDestroy if !state.holds(lock) => {
                tracing::warn!(
                    origin = ?origin(lock),
                    "refused an unlock: that lock does not hold the session"
                );
                lock.post_error(
                    ext_session_lock_v1::Error::InvalidUnlock,
                    "this lock does not hold the session",
                );
                return;
            }
            ext_session_lock_v1::Request::GetLockSurface { surface, .. } => {
                note_asked_on(surface, lock);
            }
            _ => {}
        }
        <SessionLockManagerState as Dispatch<ExtSessionLockV1, SessionLockState, Self>>::request(
            state, client, lock, request, data, display, data_init,
        );
    }

    /// A lock gone. The holder's own `unlock_and_destroy` has already
    /// unlocked by now, since a destructor is answered before it is
    /// destroyed: one still holding the session went without unlocking.
    fn destroyed(
        state: &mut Self,
        client: ClientId,
        lock: &ExtSessionLockV1,
        data: &SessionLockState,
    ) {
        if state.holds(lock) {
            state.lock_gone();
        }
        <SessionLockManagerState as Dispatch<ExtSessionLockV1, SessionLockState, Self>>::destroyed(
            state, client, lock, data,
        );
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
        self.inhibitors.retain(WlSurface::is_alive);
        let inhibited = !self.inhibitors.is_empty();
        self.idle.set_is_inhibited(inhibited);
    }
}

smithay::delegate_idle_notify!(Compositor);
smithay::delegate_idle_inhibit!(Compositor);
