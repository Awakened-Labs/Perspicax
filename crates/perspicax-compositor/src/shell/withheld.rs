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
//! A dialog another program opens for a window of the program in use is
//! asked for too: the portal's file chooser, opened for Firefox's window by
//! another program, D-Bus started, that the person's press never reached
//! (#235, #268). Its parent is named through xdg-foreign (`crate::foreign`)
//! only after `get_toplevel`, where the rest is judged, so it is judged again
//! at its first picture, as it opens ([`Compositor::asked_through_parent`]),
//! and given the keys then as a token spent on it would give them. A
//! program names only its own windows to another, so no program but the one
//! in use can have a dialog opened with the keys this way.
//!
//! And a prompt a daemon draws for them, in the moment they may be waiting
//! on it: pinentry for the gpg-agent their `git commit -S` asked, a polkit
//! agent's dialog for `pkexec`, a keyring's unlock (#268). The person asked
//! for each through another program, and the daemon was started at login or
//! by D-Bus, never by their press. So a window of a program `[focus]
//! prompts` names takes the keys as it opens just after an Enter or a click,
//! with nothing pressed since ([`Compositor::awaited`]), and never while they
//! type: one opening mid-password opens without them, as any window does.
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
//! And so is a taskbar, as it asks to bring a window forward, minimize it or
//! close it (`toplevels::wlr`, #270): what it does to the window in use moves
//! the keys. So is a pager, as it asks to switch workspace (`pager`, #278): a
//! switch parks the window in use, and the keys go to the window on top where
//! it lands.

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
/// client shows or does: [`Attention::asked_for`] for a window, or
/// [`Attention::asked_of`] for a launcher, a taskbar or a pager.
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
    /// gone. Or a prompt they may be waiting on, if `client` is a program
    /// that draws one ([`Self::awaited`]); Xwayland never is, being every X
    /// client at once, and an X11 prompt is known once XRes names its
    /// process.
    pub(crate) fn client_asked_for(&self, client: Option<&Client>) -> bool {
        if self.asked_by(client, Attention::asked_for) {
            return true;
        }
        let Some(state) = client.and_then(|client| client.get_data::<ClientState>()) else {
            return false;
        };
        let origin = state.origin();
        let exe = state.file().of(&origin);
        let awaited = self.awaited(exe);
        if awaited {
            tracing::info!(
                exe = exe.map(|exe| exe.path),
                "a prompt opened just after the person's Enter or click: it takes the keys"
            );
        }
        awaited
    }

    /// Whether a window of a program running `exe` is a prompt the person
    /// may be waiting on (#268): a program `[focus] prompts` names, opening
    /// it just after their Enter or click, with nothing pressed since
    /// ([`Attention::awaits`]). pinentry, for the gpg-agent their `git
    /// commit -S` asked, descends from no program their press reached.
    ///
    /// Never while the session is locked. A prompt that opens behind the
    /// lock is asked for by nobody looking at it, and one told it was would
    /// be handed the keys at unlock, where the person may type anything.
    pub(crate) fn awaited(&self, exe: Option<perspicax_policy::Exe<'_>>) -> bool {
        self.lock.is_none()
            && self.attention.awaits(Instant::now())
            && self.backend.prompts().admits(exe)
    }

    /// Whether the person asked for a launcher `surface`'s client shows, as
    /// it comes to want every key (#267): as for a window, but never for
    /// nothing having the keys. Always, with nobody at the seat.
    pub(crate) fn launcher_asked_for(&self, surface: &WlSurface) -> bool {
        self.asked_of(surface.client().as_ref())
    }

    /// Whether the person asked something of `client`: that a launcher it
    /// shows take the keys, that a window be done what its taskbar asks
    /// (#270), or that the workspace its pager asks for be shown (#278). As a
    /// window is asked for, but never for nothing having the keys. Always,
    /// with nobody at the seat; never, of a client gone.
    pub(crate) fn asked_of(&self, client: Option<&Client>) -> bool {
        self.asked_by(client, Attention::asked_of)
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

    /// Whether the person asked for `window`, which opened without the keys,
    /// through the window it hangs from: a dialog another program opened for
    /// a window of the program they are using, as the portal opens a file
    /// chooser for Firefox's (#235, #268). Its own program's dialogs were
    /// asked for as they opened, through it. Only an xdg window's parent is
    /// another program's; an X11 window's is Xwayland's, as it is.
    pub(crate) fn asked_through_parent(&self, window: &Framed) -> bool {
        if !wants_attention(window) {
            return false;
        }
        self.hangs_from(window)
            .and_then(|parent| parent.toplevel()?.wl_surface().client())
            .is_some_and(|client| self.asked_of(Some(&client)))
    }

    /// Give `window`, which opened without the keys, the keys as it opens
    /// after all: raised, and focused, as a token spent on it would.
    pub(crate) fn heed_as_it_opens(&mut self, window: &Framed) {
        let (Some(id), Some(surface)) = (
            id_of(window),
            window
                .toplevel()
                .map(|toplevel| toplevel.wl_surface().clone()),
        ) else {
            return;
        };
        tracing::info!(
            surface = id.0,
            "opened for the window in use: it takes the keys after all"
        );
        self.space.raise_element(window, false);
        self.focus_surface(surface, id);
        self.backend.redraw();
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
