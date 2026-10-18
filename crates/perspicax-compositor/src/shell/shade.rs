//! A window rolled up to its titlebar, and back: Fluxbox's and Openbox's
//! shade.
//!
//! Rolled up, a window keeps its place and its size, and only its frame is
//! drawn: the titlebar, with the border round it. Its client is not told,
//! for xdg-shell has no such state, and is not resized. A client asked to be
//! a strip would lay itself out again, lose its place in what it shows, and
//! draw twice; left alone, it is put back exactly as it was. Nor is it drawn
//! or hit: a click where it was lands on whatever is under it (see
//! `crate::framed`). Not drawn, it is not told it may draw either, beyond
//! the once a second a parked window is told (`backend::seat::overdue`), and
//! it holds no pointer (`crate::hold`), so a game rolled up lets go of it.
//!
//! A tab group rolls up as one: the shade is the group's, and the tab
//! coming forward takes it (`shell::tabs`). A tab taken out of the group
//! comes out unrolled.
//!
//! Only a window whose frame this compositor draws has a titlebar to roll up
//! to. One that draws its own, a GTK 4 window's headerbar say, is left as it
//! is: where its headerbar ends, and how far its shadow reaches, is its
//! client's business, and a guess would cut either.
//!
//! It keeps the keyboard, as in Fluxbox and Openbox, so keys still reach the
//! hidden client. Its menus close, and it opens none until it is unrolled.
//! Going fullscreen unrolls it ([`Compositor::fill`]), and nothing else
//! does: maximized, snapped, minimized, sent to another workspace or made
//! sticky, it stays rolled up.
//!
//! An X11 window is rolled up the same way, and is not told: smithay has no
//! `_NET_WM_STATE_SHADED` to tell it with, and so an X11 client's own asking
//! to be rolled up goes unheard.
//!
//! An agent is told the window is `shaded`, and is refused its nodes
//! (`perspicax_index::host`): what the person rolled up, the person unrolls.

use super::{Fill, id_of, is_shaded, placement, surface_of};
use crate::{framed::Framed, state::Compositor};

impl Compositor {
    /// Roll `window` up to its titlebar, or unroll it. Nothing if it is so
    /// already, and nothing for a window with no titlebar of ours to roll up
    /// to: one that draws its own frame, or a fullscreen one.
    pub(crate) fn shade(&mut self, window: &Framed, rolled: bool) {
        if rolled == is_shaded(window) {
            return;
        }
        if rolled
            && !(Self::is_server_decorated(window)
                && Self::filling(window) != Some(Fill::Fullscreen))
        {
            tracing::debug!(
                surface = id_of(window).map(|id| id.0),
                "no titlebar of ours to roll this window up to"
            );
            return;
        }
        if rolled && let Some(surface) = surface_of(window) {
            // Its open menu closes as a click outside it would, and the
            // keyboard goes back to the window, which keeps it.
            if self.menu_opened_from(&surface) {
                self.dismiss_menu();
            }
            // And every other popup of it, with nothing to hang from.
            crate::popup::dismiss_all(&surface);
        }
        placement(window, |placement| placement.shaded = rolled);
        // The pointer, to what the client uncovered, or back to the client;
        // and a game rolled up lets go of it.
        self.repoint();
        self.settle_hold_later();
        self.backend.redraw();
        self.publish_facts();
    }

    /// Roll `window` up to its titlebar, or unroll it if it is rolled up.
    pub(crate) fn toggle_shade(&mut self, window: &Framed) {
        self.shade(window, !is_shaded(window));
    }
}
