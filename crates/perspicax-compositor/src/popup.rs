//! Where a popup is drawn: a menu, a combo list or a tooltip, hung from the
//! window or panel that opened it.
//!
//! A popup is a surface of its own, with a tree of its own, and it belongs to
//! its window through xdg-shell rather than through `wl_subsurface`. So
//! [`damage::root_of`](crate::damage::root_of), which follows subsurfaces up
//! to their window, stops at a popup. What a popup draws is its window's all
//! the same: a menu's accessible nodes hang off its window's, and the index
//! joins them to the window's surface. So it is counted against that window,
//! moved by where the popup is drawn in it (issue #42). A panel's menu is the
//! panel's in the same way.
//!
//! # Where smithay draws it
//!
//! A popup is placed by its positioner, relative to its parent's window
//! geometry. A submenu's parent is a menu, so the offsets add up down the
//! chain to the window or panel at the bottom of it. Smithay draws the popup's
//! surface that far from the corner of the window's geometry, pushed back by
//! the popup's own geometry -- the shadow it draws around itself. A panel's
//! popups are drawn from the panel's own corner, since a layer surface has no
//! window geometry. [`hung`] is that sum, so damage lands where a seat draws
//! the menu, including past the window's edge. A seat, and headless with
//! `capture`, check the two agree at every commit in a debug build
//! ([`check_hung_as_drawn`]).

use std::sync::PoisonError;

use perspicax_node::Vec2;
use smithay::{
    desktop::{PopupKind, find_popup_root_surface, get_popup_toplevel_coords},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
    wayland::{
        compositor::{get_role, with_states},
        shell::xdg::{XDG_TOPLEVEL_ROLE, XdgPopupSurfaceData},
    },
};

/// Where a popup hangs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hung {
    /// The surface of the window or panel at the bottom of its chain of
    /// popups.
    pub(crate) from: WlSurface,
    /// Where the popup's surface begins, in `from`'s coordinates.
    pub(crate) at: Vec2,
}

/// Where `popup` hangs: from which window or panel, and where in it smithay
/// draws the popup's surface. `None` for a popup with no parent yet -- a
/// panel's, before the panel has claimed it -- or one whose parent is gone.
///
/// Read from the popup's role state alone, which smithay keeps until a
/// destroyed popup's handler has returned, so a menu going away can still be
/// placed where it was.
pub(crate) fn hung(popup: &PopupKind) -> Option<Hung> {
    let PopupKind::Xdg(xdg) = popup else {
        return None;
    };
    let from = find_popup_root_surface(popup).ok()?;
    let own = with_states(xdg.wl_surface(), |states| {
        states.data_map.get::<XdgPopupSurfaceData>().map(|data| {
            data.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .current
                .geometry
                .loc
        })
    })?;
    let at = frame(&from) + get_popup_toplevel_coords(popup) + own - popup.geometry().loc;
    Some(Hung {
        from,
        at: Vec2::new(f64::from(at.x), f64::from(at.y)),
    })
}

/// Where the popups of the window or panel whose surface is `from` are
/// measured from, in its coordinates: a window's geometry, which is the
/// smithay `Window::geometry()` that draws it, and a panel's own corner.
fn frame(from: &WlSurface) -> Point<i32, Logical> {
    if get_role(from) == Some(XDG_TOPLEVEL_ROLE) {
        crate::geometry::window_geometry(from).map_or_else(Point::default, |geometry| geometry.loc)
    } else {
        Point::default()
    }
}

/// In a debug build, that a popup hangs where smithay's renderer draws it,
/// and that it draws it once. `frame` is where smithay measures the popups
/// of `hung.from` from: a window's `Window::geometry()` corner, a panel's
/// own. Every live test built with `capture` runs through here, so a
/// placement that drifted from smithay's would fail them rather than
/// misplace a menu's damage on a seat.
#[cfg(any(feature = "seat", feature = "capture"))]
pub(crate) fn check_hung_as_drawn(frame: Point<i32, Logical>, popup: &PopupKind, hung: &Hung) {
    let drawn: Vec<_> = smithay::desktop::PopupManager::popups_for_surface(&hung.from)
        .filter(|(drawn, _)| drawn == popup)
        .map(|(drawn, location)| frame + location - drawn.geometry().loc)
        .collect();
    // Filed under its window or panel by `PopupManager::commit` before this
    // is asked, at its first commit, so a popup that hangs is always drawn.
    debug_assert_eq!(drawn.len(), 1, "a popup is drawn once");
    if let Some(drawn) = drawn.first() {
        debug_assert_eq!(
            Vec2::new(f64::from(drawn.x), f64::from(drawn.y)),
            hung.at,
            "a popup's damage is placed where smithay draws it"
        );
    }
}
