//! Windows their clients took off the screen without closing them.
//!
//! xdg-shell lets a client unmap its window by attaching no buffer to it.
//! The window "returns to the state it had right after get_toplevel": it
//! cannot be shown, whatever was being done to it is cancelled, and every
//! state the compositor gave it is discarded. Its client may map it again
//! later, by the steps a new window takes.
//!
//! So to everyone but its client, an unmapped window has closed: it is off
//! the screen, out of the taskbars and the facts, its menus are dismissed,
//! and the keyboard moves on from it. Its toplevel waits here, told nothing
//! but that it may draw, until its client asks for it again.
//!
//! The asking is a new window's: a commit with nothing in it, the configure
//! a new window is sent in answer, then a picture. The window is on screen
//! again from that first picture, and not before, as a new window: placed,
//! given the keyboard, announced to the taskbars, and with an id nothing has
//! seen, so nothing an agent learned of it before is taken for what it is
//! now. A hidden client may commit for reasons of its own, and on screen
//! before it drew, the window would again be a frame around nothing. An X11
//! window its client withdraws and maps again goes and comes back the same
//! way (`crate::xwayland`). So does a panel its client unmaps, but that it is
//! back from the commit that asks for it (`crate::layers`).

use perspicax_node::SurfaceId;
use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::shell::xdg::ToplevelSurface,
};

use super::Fill;
use crate::{damage::Buffer, framed::Framed, state::Compositor};

/// A window its client unmapped, waiting to be asked for again.
pub(crate) struct Withdrawn {
    pub(crate) toplevel: ToplevelSurface,
    /// Maximized or fullscreen, if its client asked for that while it
    /// waited, as a client may ask before it maps: done once it is on screen
    /// again, with a person at the seat.
    pub(crate) wants: Option<Fill>,
}

impl Compositor {
    /// Take a window its client unmapped off the screen, as though it had
    /// closed, and keep its toplevel until it is asked for again.
    ///
    /// Its client still holds it, so nothing that went with a closed window
    /// goes with this one unasked: its menus are dismissed, and the keyboard
    /// is moved on, rather than left to go with a surface that is not
    /// destroyed.
    pub(crate) fn withdraw(&mut self, window: &Framed, id: SurfaceId) {
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        let surface = toplevel.wl_surface().clone();
        // Asked first: a menu dismissed no longer says whose it was.
        let in_use = self.window_in_use() == Some(id);
        // Its open menu closes as a click outside it would, letting go of the
        // keyboard and the pointer. A menu that holds them is the only grab
        // there can be, so no other window's gesture ends with it.
        if self.menu_opened_from(&surface) {
            self.let_go();
        }
        // And every other popup of it goes, so none comes back with it.
        crate::popup::dismiss_all(&surface);
        self.forget_window(window);
        // Back as it was right after get_toplevel: nothing maximized,
        // fullscreen or tiled, no size asked for, and owed the configure a new
        // window is owed. Who draws its frame and what it may do, it keeps:
        // smithay's own unmap keeps them too, and that configure answers its
        // frame again. Smithay resets the configure only where it keeps
        // buffers, and a commit late, so it is reset here on every backend.
        toplevel.with_pending_state(|pending| {
            pending.states = Default::default();
            pending.size = None;
            pending.bounds = None;
            pending.fullscreen_output = None;
        });
        toplevel.reset_initial_configure_sent();
        self.withdrawn.push(Withdrawn {
            toplevel,
            wants: None,
        });
        // The keyboard moves on, if it was this window's, as it does when one
        // closes. Headless it stays where it is, as it does when an X11
        // window is withdrawn: nothing asked for it to move.
        if in_use && self.backend.has_person() {
            self.focus_top_window();
        }
        // And the pointer to what it uncovered.
        self.repoint();
        tracing::info!(surface = id.0, "toplevel unmapped by its client");
    }

    /// A commit from a window its client unmapped: its client asking for it
    /// again, or drawing in it. Each commit before the first picture is
    /// answered, if it is still owed, with the configure a new window is
    /// sent, and the first picture puts the window back on screen as a new
    /// one, filled as it asked to be while it waited. A client that draws
    /// without asking first is configured all the same.
    pub(crate) fn map_again(&mut self, surface: &WlSurface, own: Buffer) {
        let Some(at) = self
            .withdrawn
            .iter()
            .position(|withdrawn| withdrawn.toplevel.wl_surface() == surface)
        else {
            return;
        };
        let toplevel = &self.withdrawn[at].toplevel;
        if !toplevel.is_initial_configure_sent() {
            self.configure_new(toplevel);
        }
        if !matches!(own, Buffer::New(_)) {
            return;
        }
        let Withdrawn { toplevel, wants } = self.withdrawn.remove(at);
        let window = self.open_toplevel(toplevel);
        if let Some(fill) = wants
            && self.backend.has_person()
        {
            self.fill(&window, fill, None);
        }
    }

    /// Whether `surface` is that of a window its client unmapped.
    pub(crate) fn is_withdrawn(&self, surface: &WlSurface) -> bool {
        self.withdrawn
            .iter()
            .any(|withdrawn| withdrawn.toplevel.wl_surface() == surface)
    }

    /// The window its client unmapped whose surface is `surface`.
    pub(crate) fn withdrawn_mut(&mut self, surface: &WlSurface) -> Option<&mut Withdrawn> {
        self.withdrawn
            .iter_mut()
            .find(|withdrawn| withdrawn.toplevel.wl_surface() == surface)
    }

    /// Whether the menu that holds the keyboard and pointer, if one does,
    /// was opened from the window whose surface is `surface`. Not one that
    /// has let go of them: letting go of the pointer then would end another
    /// window's drag.
    pub(super) fn menu_opened_from(&self, surface: &WlSurface) -> bool {
        self.held_menu_root().as_ref() == Some(surface)
    }
}
