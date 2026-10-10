//! A window holding the pointer: a game's mouselook.
//!
//! A game turns its camera by how far the mouse goes, and needs the pointer
//! to stay put while it does, or the pointer runs off the window, and into
//! the edge of the screen, before the camera has turned far. So a client may
//! ask, with `zwp_pointer_constraints_v1`, to *lock* the pointer where it is
//! over one of its surfaces, or to *confine* it to a region of one. Xwayland
//! asks the same for an X game: a pointer grab confined to a window (Wine's
//! `ClipCursor`) becomes a confine, and a hidden pointer warped back to the
//! middle (Wine's and SDL2's mouselook) becomes a lock -- which Xwayland
//! only does when this and the relative pointer are both advertised.
//!
//! smithay keeps the requests. What this module decides is when one takes
//! hold, what the pointer does while it holds, and when it lets go.
//!
//! # When a window holds the pointer
//!
//! Only with a person at the seat, never behind the lock screen, and only
//! the window in use ([`Compositor::window_in_use`]): its surface has the
//! pointer, and its window is on screen and has the keyboard. A window the
//! person has left cannot reach out and take the pointer, and one they
//! leave lets go of it.
//!
//! It takes hold only where the client asked: with the pointer in the region
//! it named, on the part of its surface that takes input, and on the surface
//! itself rather than on something drawn over it.
//!
//! # What holding does
//!
//! Locked, the pointer goes nowhere and no client is told it moved, while
//! the mouse's own motion still reaches the window ([`Compositor::travel`]).
//! Confined, it moves, but only inside the region, sliding along its edge.
//! Either way it stays the window's: what is drawn over the window does not
//! take it, and no edge of the desk flips the workspace.
//!
//! # Derived, never stored
//!
//! Which window holds the pointer is asked of smithay each time and never
//! kept here. smithay forgets a constraint without telling anyone: when the
//! client destroys it, when a oneshot one lets go, and when Xwayland swaps a
//! confine for a lock. It also lets go of one itself when its surface loses
//! the pointer, so a constraint that holds is only ever on the surface that
//! has the pointer now, and that is the one place [`Compositor::hold`] looks.

use std::borrow::Cow;

use smithay::{
    delegate_pointer_constraints,
    desktop::space::SpaceElement,
    input::pointer::PointerHandle,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::{
        compositor::{RectangleKind, RegionAttributes, SurfaceAttributes, with_states},
        pointer_constraints::{
            PointerConstraint, PointerConstraintsHandler, with_pointer_constraint,
        },
        seat::WaylandFocus,
    },
};

use crate::{
    backend::pointer::{distance, nearest_on},
    damage,
    framed::Framed,
    geometry,
    mouse::{Hit, under},
    shell,
    state::Compositor,
};

/// How a window holds the pointer.
#[derive(Debug, Clone)]
pub(crate) enum Kind {
    /// Where it is.
    Locked,
    /// Inside this region of the surface, or anywhere on it.
    Confined(Option<RegionAttributes>),
}

/// The window holding the pointer, and how.
#[derive(Debug, Clone)]
pub(crate) struct Hold {
    /// The surface that asked: the window's own, or one of its subsurfaces.
    pub(crate) surface: WlSurface,
    pub(crate) window: Framed,
    /// Where `surface` begins, in global space.
    pub(crate) origin: Point<f64, Logical>,
    pub(crate) kind: Kind,
}

impl Hold {
    /// The held surface, as what the pointer is over: it stays over it,
    /// whatever is drawn there.
    pub(crate) fn hit(&self) -> Hit {
        Hit {
            window: Some(self.window.clone()),
            takes_focus: false,
            surface: Some(self.surface.clone()),
            origin: self.origin,
            frame: None,
        }
    }

    /// Where the pointer goes, moved from `from` toward `to`, as this hold
    /// lets it: nowhere while locked (`None`), and while confined, as far
    /// inside the region as it can get.
    pub(crate) fn moves_to(
        &self,
        from: Point<f64, Logical>,
        to: Point<f64, Logical>,
    ) -> Option<Point<f64, Logical>> {
        match &self.kind {
            Kind::Locked => None,
            Kind::Confined(region) => {
                // A surface showing nothing has nowhere to be confined to.
                let Some(area) = Area::of(&self.surface, region.as_ref()) else {
                    return Some(from);
                };
                Some(area.confine(from - self.origin, to - self.origin) + self.origin)
            }
        }
    }
}

impl Compositor {
    /// The window holding the pointer now, if one is.
    ///
    /// Behind the lock screen nothing holds it, whatever smithay has yet to
    /// be told: the person must reach the lock screen at once.
    pub(crate) fn hold(&self) -> Option<Hold> {
        if self.lock.is_some() {
            return None;
        }
        let pointer = self.pointer.as_ref()?;
        let surface = pointed_at(pointer)?;
        let kind = with_pointer_constraint(&surface, pointer, |constraint| {
            constraint
                .filter(|constraint| constraint.is_active())
                .map(|constraint| kind_of(&constraint))
        })?;
        let (window, origin) = self.placed(&surface)?;
        Some(Hold {
            surface,
            window,
            origin,
            kind,
        })
    }

    /// Let the constraint on the surface under the pointer take hold, or let
    /// go, as the rule in this module's comment says it may now. Idempotent:
    /// called wherever something it reads may have changed, and a no-op
    /// where nothing did.
    ///
    /// Never from inside one of smithay's pointer or keyboard callbacks (a
    /// grab, `focus_changed`), which hold the locks this reads through.
    pub(crate) fn settle_hold(&mut self) {
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        let Some(surface) = pointed_at(&pointer) else {
            return;
        };
        // Read, and let go at once: what is asked below locks this
        // surface's state too.
        let Some((active, region)) = with_pointer_constraint(&surface, &pointer, |constraint| {
            constraint.map(|constraint| (constraint.is_active(), constraint.region().cloned()))
        }) else {
            return;
        };
        let may = self.may_hold(&surface, &pointer, active, region.as_ref());
        if may == active {
            return;
        }
        with_pointer_constraint(&surface, &pointer, |constraint| match constraint {
            Some(constraint) if may && !constraint.is_active() => constraint.activate(),
            // Only one that holds: smithay tells the client it let go
            // whether it held or not, and drops a oneshot one either way.
            Some(constraint) if !may && constraint.is_active() => constraint.deactivate(),
            _ => {}
        });
        tracing::debug!(held = may, "a window's hold on the pointer");
    }

    /// Whether the constraint on `surface` may hold the pointer: one that
    /// already does, wherever in its region the pointer is; one that does
    /// not yet, only with the pointer in its region, on that surface.
    fn may_hold(
        &self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        active: bool,
        region: Option<&RegionAttributes>,
    ) -> bool {
        if !self.backend.has_person() || self.lock.is_some() {
            return false;
        }
        let Some((window, origin)) = self.placed(surface) else {
            return false;
        };
        let in_use = shell::id_of(&window).is_some_and(|id| self.window_in_use() == Some(id));
        if !in_use {
            return false;
        }
        if active {
            return true;
        }
        // Smithay's idea of what has the pointer lags a window raised over
        // it without the pointer moving, so what is drawn there is asked.
        let at = pointer.current_location();
        under(self, at).and_then(|hit| hit.surface).as_ref() == Some(surface)
            && Area::of(surface, region).is_some_and(|area| area.contains(at - origin))
    }

    /// Where `surface` is, on a window on screen: the window, and where the
    /// surface begins in global space, as [`under`] finds it there. `None`
    /// for a surface of no window on screen -- a layer, a popup, a window
    /// parked -- none of which ever holds the pointer.
    fn placed(&self, surface: &WlSurface) -> Option<(Framed, Point<f64, Logical>)> {
        let (root, at) = damage::root_of(surface);
        let window = self
            .space
            .elements()
            .find(|window| shell::is_toplevel_of(window, &root))?
            .clone();
        let location = self.space.element_location(&window)?;
        // Where smithay draws the window's surface: its geometry pushed back
        // by the geometry's own offset into the surface.
        let drawn = location - SpaceElement::geometry(&window).loc;
        Some((window, drawn.to_f64() + Point::from((at.x, at.y))))
    }
}

impl PointerConstraintsHandler for Compositor {
    /// A client asked to hold the pointer. It takes hold at once if it may
    /// now, and otherwise when it may.
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        self.settle_hold();
    }

    /// Where a client holding the pointer in place draws it: the pointer is
    /// put there, telling no client, so that when the hold ends the pointer
    /// is where the person last saw it. Xwayland's warp emulation sends one
    /// with every motion of an X game's warped pointer, and does not keep it
    /// inside the window, so it is kept inside the surface and on the desk
    /// here.
    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        let Some(hold) = self.hold() else {
            return;
        };
        if hold.surface != *surface || !matches!(hold.kind, Kind::Locked) {
            return;
        }
        let Some(size) = geometry::shown(surface) else {
            return;
        };
        let on_surface = nearest_on(location, Rectangle::from_size(size));
        pointer.set_location(self.on_the_desk(on_surface + hold.origin));
        self.backend.redraw();
        #[cfg(feature = "capture")]
        self.flush_screencopy_for_pointer();
    }
}

delegate_pointer_constraints!(Compositor);

/// The surface that has the pointer, while it lives.
fn pointed_at(pointer: &PointerHandle<Compositor>) -> Option<WlSurface> {
    pointer
        .current_focus()?
        .wl_surface()
        .map(Cow::into_owned)
        .filter(IsAlive::alive)
}

fn kind_of(constraint: &PointerConstraint) -> Kind {
    match constraint {
        PointerConstraint::Locked(_) => Kind::Locked,
        PointerConstraint::Confined(confined) => Kind::Confined(confined.region().cloned()),
    }
}

/// Where on its surface a constraint may hold the pointer, in the surface's
/// coordinates: on the picture the surface shows, where it takes input,
/// inside the region its client named.
#[derive(Debug, Clone)]
struct Area {
    size: Size<i32, Logical>,
    /// The input region and the constraint's, whichever there are: a point
    /// is in the area only if it is in all of them.
    regions: Vec<RegionAttributes>,
}

impl Area {
    /// The area of the constraint with `region` on `surface`. `None` while
    /// the surface shows nothing.
    fn of(surface: &WlSurface, region: Option<&RegionAttributes>) -> Option<Self> {
        let size = geometry::shown(surface)?;
        let input = with_states(surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .input_region
                .clone()
        });
        Some(Self {
            size,
            regions: input.into_iter().chain(region.cloned()).collect(),
        })
    }

    /// Whether `at` is in the area: the pixel it is on is.
    fn contains(&self, at: Point<f64, Logical>) -> bool {
        let pixel = at.to_i32_floor();
        Rectangle::from_size(self.size).contains(pixel)
            && self.regions.iter().all(|region| region.contains(pixel))
    }

    /// Where a pointer confined here goes when the mouse moves it from
    /// `from` toward `to`.
    ///
    /// Straight there if it can. If not, as far toward it as the part of
    /// the area it is in reaches: up to the edge it met, and along it, as
    /// the pointer slides along the edge of the screen. If `from` is outside
    /// already -- the region shrank under it, or the window moved -- to the
    /// nearest point inside, so it is never stranded outside what it is
    /// confined to. With no area at all, it stays where it is.
    fn confine(&self, from: Point<f64, Logical>, to: Point<f64, Logical>) -> Point<f64, Logical> {
        let to = nearest_on(to, Rectangle::from_size(self.size));
        if self.contains(to) {
            return to;
        }
        let pieces = self.pieces();
        let nearest = |pieces: &mut dyn Iterator<Item = &Rectangle<i32, Logical>>| {
            pieces
                .map(|&piece| nearest_on(to, piece))
                .filter(|&at| self.contains(at))
                .min_by(|a, b| distance(to, *a).total_cmp(&distance(to, *b)))
        };
        if !self.contains(from) {
            return nearest(&mut pieces.iter()).unwrap_or(from);
        }
        // Never across a gap into another part: only within the one it is
        // in, or failing that (a cut-out in the way), along one axis.
        let pixel = from.to_i32_floor();
        nearest(&mut pieces.iter().filter(|piece| piece.contains(pixel)))
            .or_else(|| {
                [(to.x, from.y).into(), (from.x, to.y).into()]
                    .into_iter()
                    .find(|&at| self.contains(at))
            })
            .unwrap_or(from)
    }

    /// The rectangles the area is made of: the surface, cut down by each
    /// region's added rectangles in turn. What a region takes away again is
    /// left to [`Self::contains`].
    fn pieces(&self) -> Vec<Rectangle<i32, Logical>> {
        self.regions
            .iter()
            .fold(vec![Rectangle::from_size(self.size)], |pieces, region| {
                pieces
                    .iter()
                    .flat_map(|piece| {
                        region
                            .rects
                            .iter()
                            .filter(|(kind, _)| matches!(kind, RectangleKind::Add))
                            .filter_map(|(_, rect)| piece.intersection(*rect))
                    })
                    .collect()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// x, y, width and height, as the protocol sends a rectangle.
    type Sides = (i32, i32, i32, i32);

    fn region(rects: &[(RectangleKind, Sides)]) -> RegionAttributes {
        RegionAttributes {
            rects: rects
                .iter()
                .map(|&(kind, (x, y, w, h))| (kind, Rectangle::new((x, y).into(), (w, h).into())))
                .collect(),
        }
    }

    /// A 400x300 surface, confined to a 100x100 square at (50, 50).
    fn square() -> Area {
        Area {
            size: (400, 300).into(),
            regions: vec![region(&[(RectangleKind::Add, (50, 50, 100, 100))])],
        }
    }

    fn at(x: f64, y: f64) -> Point<f64, Logical> {
        (x, y).into()
    }

    #[test]
    fn a_move_inside_the_area_goes_straight_there() {
        assert_eq!(
            square().confine(at(60.0, 60.0), at(80.5, 90.0)),
            at(80.5, 90.0)
        );
    }

    #[test]
    fn a_move_out_of_the_area_slides_along_its_edge() {
        // Right and down, out through the right side: to the edge, and the
        // down part of the move along it.
        assert_eq!(
            square().confine(at(140.0, 60.0), at(200.0, 70.0)),
            at(149.0, 70.0)
        );
        // Straight out of the bottom: it stays where it was.
        assert_eq!(
            square().confine(at(60.0, 149.0), at(60.0, 400.0)),
            at(60.0, 149.0)
        );
    }

    #[test]
    fn a_fast_mouse_still_reaches_the_edge() {
        // Five pixels from the edge, moved ten: on the edge, not short of it.
        assert_eq!(
            square().confine(at(145.0, 60.0), at(155.0, 60.0)),
            at(149.0, 60.0)
        );
    }

    #[test]
    fn a_pointer_stopped_by_a_gap_stays_in_its_own_part_of_the_area() {
        let two = Area {
            size: (400, 300).into(),
            regions: vec![region(&[
                (RectangleKind::Add, (0, 0, 100, 100)),
                (RectangleKind::Add, (200, 0, 100, 100)),
            ])],
        };
        // Into the gap: to the edge of its own part, though the other one
        // is as near.
        assert_eq!(two.confine(at(90.0, 50.0), at(150.0, 50.0)), at(99.0, 50.0));
    }

    #[test]
    fn with_no_region_the_whole_surface_is_the_area_and_the_far_edges_are_its_last_pixels() {
        let whole = Area {
            size: (400, 300).into(),
            regions: Vec::new(),
        };
        assert_eq!(
            whole.confine(at(10.0, 10.0), at(1000.0, 1000.0)),
            at(399.0, 299.0)
        );
        assert_eq!(whole.confine(at(10.0, 10.0), at(-5.0, 20.0)), at(0.0, 20.0));
    }

    #[test]
    fn a_pointer_left_outside_by_a_shrinking_region_comes_back_to_the_nearest_point_in_it() {
        assert_eq!(
            square().confine(at(300.0, 100.0), at(301.0, 100.0)),
            at(149.0, 100.0)
        );
    }

    #[test]
    fn a_cut_out_of_the_region_is_not_in_the_area() {
        // The square with its middle taken out: a ring.
        let ring = Area {
            size: (400, 300).into(),
            regions: vec![region(&[
                (RectangleKind::Add, (50, 50, 100, 100)),
                (RectangleKind::Subtract, (75, 75, 50, 50)),
            ])],
        };
        assert!(!ring.contains(at(100.0, 100.0)));
        assert!(ring.contains(at(60.0, 100.0)));
        // Into the hole from the left: it stops short, sliding nowhere.
        assert_eq!(
            ring.confine(at(60.0, 100.0), at(100.0, 100.0)),
            at(60.0, 100.0)
        );
    }

    #[test]
    fn the_input_region_bounds_the_area_too() {
        let both = Area {
            size: (400, 300).into(),
            regions: vec![
                region(&[(RectangleKind::Add, (0, 0, 120, 300))]),
                region(&[(RectangleKind::Add, (50, 50, 100, 100))]),
            ],
        };
        assert!(both.contains(at(110.0, 60.0)));
        assert!(!both.contains(at(130.0, 60.0)), "outside the input region");
    }

    #[test]
    fn with_no_area_at_all_the_pointer_stays_put() {
        let none = Area {
            size: (400, 300).into(),
            regions: vec![region(&[])],
        };
        assert_eq!(none.confine(at(10.0, 10.0), at(20.0, 20.0)), at(10.0, 10.0));
    }
}
