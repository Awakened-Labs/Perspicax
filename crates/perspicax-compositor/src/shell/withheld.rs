//! Windows that open without the keys (#259).
//!
//! A program maps a window, and the window used to take the keyboard,
//! whatever the person was doing: typing a password at a terminal's `sudo`
//! prompt, say, the rest of which, Enter and all, went to whichever program
//! had mapped something. #187, #256 and #257 closed the ways a program could
//! ask to be brought forward with the keys; a map was the way left.
//!
//! So on a seat a window takes the keys as it opens only when the person
//! asked for it ([`perspicax_policy::Attention::asked_for`]): a window of the
//! program they are using, a dialog say; anything, while nothing has the
//! keys; or one their latest press reached or started -- a dock clicked, a
//! binding, an Enter at a terminal. A token is the other way to be asked
//! for, granted as it is spent once the window is open
//! (`request_activation`), which is why a window withheld is in the space.
//!
//! Any other window opens without them, as Mutter and KWin open one: just
//! beneath the window in use, in sight but behind what the person is typing
//! into, not told it is active, and marked as wanting attention until it
//! has the keys. It has them only when the person gives them to it: a
//! click, Alt+Tab, the taskbar, a token. When the window in use closes, the
//! keys pass it over for the window beneath ([`Compositor::window_for_keys`]).
//!
//! Headless, with nobody at the seat, every window takes the keys as it
//! opens, as M2's contract has it: nobody is typing anything to take.
//!
//! A launcher, a layer over the windows that wants every key, is asked the
//! same as it comes to want them (`layers`, #267), but never for nothing
//! having the keys: it keeps them from every window while it is up (#131).

use std::{cell::RefCell, time::Instant};

use perspicax_node::SurfaceId;
use perspicax_policy::{Attention, Process, Turn};
use smithay::{
    reexports::wayland_server::{
        Client, Resource as _, backend::ClientId, protocol::wl_surface::WlSurface,
    },
    utils::Serial,
};

use super::{Placement, id_of, placement};
use crate::{
    framed::Framed,
    state::{ClientState, Compositor},
};

/// One of the policy's answers to whether the person asked for something a
/// client shows: [`Attention::asked_for`] for a window, or
/// [`Attention::asked_of`] for a launcher.
type Rule =
    fn(&Attention<Serial, ClientId>, &ClientId, Option<&ClientId>, Option<Turn>, Instant) -> bool;

/// Whether `window` opened without the keys, and has not had them since.
/// Only looks, as `is_on_top` does: a window with no record never did.
pub(crate) fn wants_attention(window: &Framed) -> bool {
    window
        .user_data()
        .get::<RefCell<Placement>>()
        .is_some_and(|cell| cell.borrow().wanting)
}

impl Compositor {
    /// Whether the person asked for a window `surface`'s client maps now:
    /// see the module's doc. Always, with nobody at the seat.
    pub(crate) fn asked_for(&self, surface: &WlSurface) -> bool {
        self.client_asked_for(surface.client().as_ref())
    }

    /// The same, of a window `client` maps: Xwayland, for an X11 window,
    /// which may have no surface yet. Nobody asked for a window of a client
    /// gone.
    pub(crate) fn client_asked_for(&self, client: Option<&Client>) -> bool {
        self.asked_by(client, Attention::asked_for)
    }

    /// Whether the person asked for a launcher `surface`'s client shows, as
    /// it comes to want every key (#267): as for a window, but never for
    /// nothing having the keys. Always, with nobody at the seat.
    pub(crate) fn launcher_asked_for(&self, surface: &WlSurface) -> bool {
        self.asked_by(surface.client().as_ref(), Attention::asked_of)
    }

    /// Whether the person asked for what `client` shows, by `rule`: one of
    /// the policy's two, for a window or for a launcher. Always, with nobody
    /// at the seat; never, of a client gone.
    fn asked_by(&self, client: Option<&Client>, rule: Rule) -> bool {
        if !self.backend.has_person() {
            return true;
        }
        let Some(client) = client else {
            return false;
        };
        let holder = self
            .keyboard_focus()
            .and_then(|held| held.client())
            .map(|holder| holder.id());
        let started_on = client
            .get_data::<ClientState>()
            .and_then(ClientState::started_on);
        rule(
            &self.attention,
            &client.id(),
            holder.as_ref(),
            started_on,
            Instant::now(),
        )
    }

    /// What a program started under `surface` descends from, the person's
    /// press having reached it: its client's process. For an X11 window,
    /// Xwayland being one client for every X client, the process XRes named
    /// for it, once it has.
    pub(crate) fn root_under(&self, surface: &WlSurface, client: &Client) -> Option<Process> {
        if let Some(ours) = client.get_data::<ClientState>() {
            return ours.process();
        }
        // Xwayland: the one client with smithay's data rather than ours.
        #[cfg(feature = "xwayland")]
        return self.x11_process(surface);
        #[cfg(not(feature = "xwayland"))]
        {
            let _ = surface;
            None
        }
    }

    /// Open `window` without the keys: beneath the window in use, told it
    /// is not active, and marked as wanting attention.
    pub(crate) fn open_without_keys(&mut self, window: &Framed) {
        placement(window, |placement| placement.wanting = true);
        self.put_beneath(window);
        self.show_active(self.window_in_use());
        tracing::info!(
            surface = ?id_of(window).map(|id| id.0),
            "opened without the keys: the person did not ask for it"
        );
    }

    /// `window`'s program asked to bring it forward with the keys, and was
    /// refused: a token stale, an X time nothing vouched for. It wants
    /// attention, as one that opened without them does, unless it is the
    /// window in use, which has them already.
    pub(crate) fn refused_keys(&self, window: &Framed) {
        if id_of(window).is_some_and(|id| self.window_in_use() == Some(id)) {
            return;
        }
        placement(window, |placement| placement.wanting = true);
    }

    /// The window `id` has the keys: it wants attention no longer. Asked
    /// while the keyboard is moving, so it only marks.
    pub(crate) fn heeded(&self, id: SurfaceId) {
        if let Some(window) = self.window_for_id(id)
            && wants_attention(&window)
        {
            placement(&window, |placement| placement.wanting = false);
        }
    }
}
