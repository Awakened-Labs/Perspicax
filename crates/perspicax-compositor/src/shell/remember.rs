//! A program's next window opens where its last one closed (#89).
//!
//! A Wayland window cannot ask where to open: xdg-shell has no request for a
//! position, and a program that saved its own place has nowhere to put it.
//! So the place is kept here instead, one per program, in
//! `perspicax_policy::Places`. It is taken as a window goes, closed or
//! unmapped by its client, and given to the next window of its program as
//! that one opens. An X11 window gets it as it maps. A Wayland window gets it
//! at its first picture, because it names its program only after it has been
//! placed. Either way it is then settled onto its monitor, as any new window
//! is ([`Compositor::settle_new`]).
//!
//! Before it, a place the window asks for itself: an X11 window's
//! USPosition or PPosition (see `crate::xwayland`). xdg-session-management
//! would be the next to ask, if this compositor spoke it. After it, the
//! cascade.
//!
//! Only with a person at the seat, as everything that moves a window is (see
//! the module doc of `crate::shell`), and only for a program's main windows.
//! A dialog opens over its own window, wherever that is, and is never what
//! its program is remembered by.

use perspicax_policy::{Spot, clear_of};
use smithay::{
    output::Output,
    utils::{Logical, Point, Rectangle},
};

use super::{extent_size, id_of, is_main, placement, places_itself};
use crate::{framed::Framed, state::Compositor, toplevels::app_id};

impl Compositor {
    /// `window` is going: where it was is its program's place now. Where it
    /// would be put back if it stayed: the rect it is restored to if it fills
    /// its monitor or is snapped, so that the next one opens where this one
    /// unmaximizes to; where it is; or where it was parked, minimized or on a
    /// hidden workspace. A tab behind another is where its group's front is.
    ///
    /// Only a window that opened in front of a person (`settled`), so none
    /// that closed before it ever drew. Its monitor goes by name, and its
    /// corner from that monitor's own.
    pub(crate) fn remember_place(&mut self, window: &Framed) {
        if !placement(window, |placement| placement.settled) {
            return;
        }
        let Some(app) = self.remembered_app(window) else {
            return;
        };
        let front = self
            .behind_tab(window)
            .and_then(|front| self.any_window(front));
        let kept = front.as_ref().unwrap_or(window);
        let (restore, parked) = placement(kept, |placement| (placement.restore, placement.parked));
        let Some(mut rect) = restore
            .or_else(|| self.extent(kept))
            .or_else(|| Some(Rectangle::new(parked?, extent_size(kept))))
        else {
            return;
        };
        // A window that has not drawn has no size, and its corner is what
        // places it.
        rect.size.w = rect.size.w.max(1);
        rect.size.h = rect.size.h.max(1);
        let output = self.output_at(rect).or_else(|| {
            let at = self.unpark(kept, rect.loc);
            self.output_at(Rectangle::new(at, rect.size))
        });
        let Some((output, origin)) = output.and_then(|output| {
            let origin = self.space.output_geometry(&output)?.loc;
            Some((output, origin))
        }) else {
            return;
        };
        let at = rect.loc - origin;
        self.places.remember(
            &app,
            Spot {
                output: output.name(),
                at: (at.x, at.y),
            },
        );
    }

    /// Where a window that is opening goes, if its program's last window
    /// closed somewhere, and on which monitor: that one, by name, if it is
    /// still plugged in, or else the one under the pointer, at the same
    /// corner. Stepped aside from any window of the same program already
    /// there, so that a second does not open exactly on the first. Moves
    /// nothing.
    ///
    /// Only before it is settled, and not for a window that opened filling
    /// its monitor, snapped or in a tab group: those have a place already,
    /// as [`Self::settle_new`] leaves them.
    pub(crate) fn remembered_place(
        &self,
        window: &Framed,
    ) -> Option<(Point<i32, Logical>, Output)> {
        if placement(window, |placement| placement.settled)
            || Self::filling(window).is_some()
            || Self::is_snapped(window)
            || self.is_tabbed(window)
        {
            return None;
        }
        let app = self.remembered_app(window)?;
        let spot = self.places.recall(&app)?;
        let output = self
            .space
            .outputs()
            .find(|output| output.name() == spot.output)
            .cloned()
            .or_else(|| self.output_under_pointer())?;
        let origin = self.space.output_geometry(&output)?.loc;
        let taken: Vec<(i32, i32)> = self
            .space
            .elements()
            .filter(|other| *other != window && app_id(other).as_deref() == Some(app.as_str()))
            .filter_map(|other| self.space.element_location(other))
            .map(|at| (at.x, at.y))
            .collect();
        let at = clear_of((origin.x + spot.at.0, origin.y + spot.at.1), &taken);
        Some((at.into(), output))
    }

    /// A Wayland window's first picture, the first time it can be known by
    /// its program: put where that program's last window closed, if it did,
    /// and on the workspace that monitor shows. Which monitor, for
    /// [`Self::settle_new`] to settle it on.
    ///
    /// It was cascaded at `get_toplevel`, before it said what it was. That
    /// step of the cascade is given back, as an X11 window placed this way
    /// never takes one, unless another window has been placed since.
    pub(crate) fn place_remembered(&mut self, window: &Framed) -> Option<Output> {
        let (at, output) = self.remembered_place(window)?;
        self.space.map_element(window.clone(), at, false);
        self.window_moved(window);
        if let Some(step) = placement(window, |placement| placement.cascaded.take())
            && self.placed == step.wrapping_add(1)
        {
            self.placed = step;
        }
        tracing::info!(
            surface = ?id_of(window).map(|id| id.0),
            ?at,
            output = output.name(),
            "opened where its program's last window closed"
        );
        Some(output)
    }

    /// The person changed what is remembered: a program no longer to be is
    /// forgotten now, and every one if nothing is to be. Its history goes
    /// with the wish, rather than lying in wait for the switch to go back.
    #[cfg(feature = "seat")]
    pub(crate) fn forget_unremembered(&mut self) {
        let backend = &self.backend;
        if self.places.retain(|app| backend.remembers(app)) {
            tracing::info!("forgot where the programs no longer remembered last closed");
        }
    }

    /// The program `window` is remembered by, if it is: its app id, for a
    /// main window that does not place itself, unless the person said not to.
    fn remembered_app(&self, window: &Framed) -> Option<String> {
        if places_itself(window) || !is_main(window) {
            return None;
        }
        app_id(window).filter(|app| self.backend.remembers(app))
    }
}
