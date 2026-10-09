//! Where a pie's icons go, which one the pointer is pointing at, and how
//! much each grows as it points at it: worked out from how many there are,
//! the pie's size and where it opened, with no drawing.
//!
//! The icons sit round a ring, the first at the top and the rest clockwise,
//! spun round by whole places. The pie is a square `size` across, centred
//! where it opened but kept wholly inside the room it has, and every icon
//! fits in it at its biggest.
//!
//! Pointing is by angle alone, as Fitts would have it: past a small dead
//! middle, the icon pointed at is the one in that direction however far off
//! the pointer is, out to the screen's edge. An icon grows the nearer the
//! pointer's direction is to its own, so the one pointed at is biggest and
//! its neighbours a little bigger than the rest.

use std::f64::consts::{PI, TAU};

use super::Rect;

/// How much bigger than at rest an icon pointed straight at is drawn.
pub(crate) const ZOOM: f64 = 0.5;
/// Room left between neighbouring icons at rest, as a share of an icon.
const GAP: f64 = 0.15;
/// The pie's margin, as a share of its size.
const MARGIN: f64 = 0.02;
/// The biggest an icon is at rest, as a share of the pie's size.
const CAP: f64 = 0.2;
/// The smallest an icon is at rest, in logical pixels.
const SMALLEST: f64 = 16.0;
/// The dead middle, as a share of the ring's radius: nothing is pointed at
/// from inside it.
const DEAD: f64 = 0.4;
/// How far round the ring an icon's growing reaches, in places.
const REACH: f64 = 1.5;

/// A pie of some icons, laid out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ring {
    /// Its middle, on the surface.
    pub(crate) centre: (f64, f64),
    /// How far across the whole pie is.
    pub(crate) side: f64,
    /// How far from the middle each icon's middle is.
    pub(crate) radius: f64,
    /// How big an icon is at rest.
    pub(crate) icon: f64,
    /// How many icons.
    pub(crate) places: usize,
}

impl Ring {
    /// `places` icons round a pie `size` across, opened `at` a point, kept
    /// inside `area`.
    pub(crate) fn new(places: usize, size: u32, at: (i32, i32), area: Rect) -> Self {
        let side = f64::from(size)
            .min(f64::from(area.w))
            .min(f64::from(area.h))
            .max(0.0);
        let half = side / 2.0;
        let clamp = |at: i32, low: i32, extent: i32| {
            let (low, high) = (f64::from(low) + half, f64::from(low + extent) - half);
            f64::from(at).clamp(low, high.max(low))
        };
        let centre = (clamp(at.0, area.x, area.w), clamp(at.1, area.y, area.h));
        let outer = half - MARGIN * side;
        let icon = if places < 2 {
            CAP * side
        } else {
            let gap = (PI / places as f64).sin();
            (2.0 * gap * outer / (1.0 + GAP + gap * (1.0 + ZOOM))).clamp(SMALLEST, CAP * side)
        };
        Self {
            centre,
            side,
            radius: (outer - icon * (1.0 + ZOOM) / 2.0).max(0.0),
            icon,
            places,
        }
    }

    /// The square the pie is drawn in, in whole pixels, rounded out.
    pub(crate) fn square(&self) -> Rect {
        let half = self.side / 2.0;
        let (x, y) = (
            (self.centre.0 - half).floor(),
            (self.centre.1 - half).floor(),
        );
        let (right, bottom) = ((self.centre.0 + half).ceil(), (self.centre.1 + half).ceil());
        Rect::new(x as i32, y as i32, (right - x) as i32, (bottom - y) as i32)
    }

    /// The angle of place `place`: the top is the first, and on clockwise.
    fn angle(&self, place: usize) -> f64 {
        -PI / 2.0 + TAU * place as f64 / self.places as f64
    }

    /// The place icon `item` is in, spun round `spin` places.
    fn place_of(&self, item: usize, spin: i32) -> usize {
        let places = self.places as i64;
        (item as i64 + i64::from(spin)).rem_euclid(places) as usize
    }

    /// Which way, from the middle, `pointer` is, if it is out of the dead
    /// middle.
    fn pointing(&self, pointer: (f64, f64)) -> Option<f64> {
        let (dx, dy) = (pointer.0 - self.centre.0, pointer.1 - self.centre.1);
        (dx.hypot(dy) >= DEAD * self.radius && self.places > 0).then(|| dy.atan2(dx))
    }

    /// Whether the icons grow with the pointer at `pointer`: whether it is
    /// out of the dead middle.
    pub(crate) fn zooms(&self, pointer: Option<(f64, f64)>) -> bool {
        pointer.and_then(|pointer| self.pointing(pointer)).is_some()
    }

    /// The icon `pointer` points at, spun round `spin` places, if any.
    pub(crate) fn pointed(&self, pointer: (f64, f64), spin: i32) -> Option<usize> {
        let towards = self.pointing(pointer)?;
        let places = self.places as f64;
        let place = ((towards + PI / 2.0) * places / TAU).round() as i64;
        let place = place.rem_euclid(self.places as i64);
        Some((place - i64::from(spin)).rem_euclid(self.places as i64) as usize)
    }

    /// How much bigger than at rest icon `item` is drawn, with the pointer
    /// at `pointer`.
    pub(crate) fn zoom(&self, item: usize, spin: i32, pointer: Option<(f64, f64)>) -> f64 {
        let Some(towards) = pointer.and_then(|pointer| self.pointing(pointer)) else {
            return 1.0;
        };
        let apart = (towards - self.angle(self.place_of(item, spin)) + PI).rem_euclid(TAU) - PI;
        let reach = REACH * TAU / self.places as f64;
        1.0 + ZOOM * (1.0 - apart.abs() / reach).clamp(0.0, 1.0)
    }

    /// Where icon `item` is drawn, spun round `spin` places, with the
    /// pointer at `pointer`: its middle and how big it is.
    pub(crate) fn place(
        &self,
        item: usize,
        spin: i32,
        pointer: Option<(f64, f64)>,
    ) -> ((f64, f64), f64) {
        let angle = self.angle(self.place_of(item, spin));
        (
            (
                self.centre.0 + self.radius * angle.cos(),
                self.centre.1 + self.radius * angle.sin(),
            ),
            self.icon * self.zoom(item, spin, pointer),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0, 0, 1920, 1048);

    fn ring(places: usize) -> Ring {
        Ring::new(places, 512, (960, 500), SCREEN)
    }

    #[test]
    fn icons_shrink_as_more_share_the_ring_and_always_fit_in_the_pie() {
        let sizes: Vec<_> = [1, 2, 6, 12, 20]
            .map(|places| ring(places).icon.round() as i32)
            .to_vec();
        assert_eq!(sizes, [102, 102, 102, 83, 56]);
        for places in [1, 2, 6, 12, 20, 60] {
            let ring = ring(places);
            let biggest = ring.icon * (1.0 + ZOOM);
            assert!(
                ring.radius + biggest / 2.0 <= ring.side / 2.0 + 1e-9,
                "{places} icons stick out"
            );
            assert!(ring.icon >= SMALLEST);
        }
        // Neighbours at rest do not touch.
        let twelve = ring(12);
        let ((x0, y0), _) = twelve.place(0, 0, None);
        let ((x1, y1), _) = twelve.place(1, 0, None);
        assert!((x1 - x0).hypot(y1 - y0) > twelve.icon);
    }

    #[test]
    fn the_first_is_at_the_top_and_the_rest_go_clockwise() {
        let four = ring(4);
        let at = |item| {
            let ((x, y), _) = four.place(item, 0, None);
            ((x - 960.0).round() as i32, (y - 500.0).round() as i32)
        };
        let r = four.radius.round() as i32;
        assert_eq!(
            [at(0), at(1), at(2), at(3)],
            [(0, -r), (r, 0), (0, r), (-r, 0)]
        );
        // Spun one place on, the first is where the second was.
        let ((x, y), _) = four.place(0, 1, None);
        assert_eq!(
            ((x - 960.0).round() as i32, (y - 500.0).round() as i32),
            (r, 0)
        );
    }

    #[test]
    fn pointing_is_by_direction_out_to_the_screens_edge_but_not_from_the_middle() {
        let four = ring(4);
        assert_eq!(four.pointed((960.0, 0.0), 0), Some(0), "straight up");
        assert_eq!(four.pointed((1919.0, 500.0), 0), Some(1), "the right edge");
        assert_eq!(four.pointed((960.0, 1047.0), 0), Some(2));
        assert_eq!(four.pointed((0.0, 500.0), 0), Some(3));
        assert_eq!(four.pointed((960.0, 500.0), 0), None, "the middle");
        assert_eq!(
            four.pointed((960.0, 500.0 - 0.39 * four.radius), 0),
            None,
            "just inside the dead middle"
        );
        // A sector's edges: 45 degrees either side of up is still up, just.
        let edge = |degrees: f64| {
            let angle = (-90.0 + degrees).to_radians();
            (960.0 + 300.0 * angle.cos(), 500.0 + 300.0 * angle.sin())
        };
        assert_eq!(four.pointed(edge(44.9), 0), Some(0));
        assert_eq!(four.pointed(edge(45.1), 0), Some(1));
        assert_eq!(four.pointed(edge(-44.9), 0), Some(0));
        assert_eq!(four.pointed(edge(-45.1), 0), Some(3));
        // Spun one place on, up points at the last.
        assert_eq!(four.pointed((960.0, 0.0), 1), Some(3));
        assert_eq!(four.pointed((960.0, 0.0), -1), Some(1));
        assert_eq!(four.pointed((960.0, 0.0), 5), Some(3));
    }

    #[test]
    fn a_pie_with_nothing_in_it_points_at_nothing() {
        let none = ring(0);
        assert_eq!(none.pointed((960.0, 0.0), 0), None);
        assert!((none.zoom(0, 0, Some((960.0, 0.0))) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_icon_pointed_at_grows_most_and_less_the_further_round() {
        let twelve = ring(12);
        let up = Some((960.0, 0.0));
        let zooms: Vec<f64> = (0..12).map(|item| twelve.zoom(item, 0, up)).collect();
        assert!((zooms[0] - (1.0 + ZOOM)).abs() < 1e-9);
        assert!(zooms[1] < zooms[0] && zooms[1] > 1.0);
        assert!((zooms[1] - zooms[11]).abs() < 1e-9, "the same either side");
        assert!((zooms[2] - 1.0).abs() < 1e-9 && (zooms[6] - 1.0).abs() < 1e-9);
        // From the middle, nothing grows.
        assert!((twelve.zoom(0, 0, Some((960.0, 500.0))) - 1.0).abs() < 1e-9);
        assert!((twelve.zoom(0, 0, None) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_pie_opened_near_an_edge_is_kept_inside_its_room() {
        let corner = Ring::new(6, 512, (5, 1040), SCREEN);
        assert_eq!(corner.centre, (256.0, 1048.0 - 256.0));
        assert_eq!(corner.square(), Rect::new(0, 536, 512, 512));
        // Room smaller than the pie: the pie shrinks to it.
        let small = Ring::new(6, 512, (100, 100), Rect::new(0, 32, 400, 300));
        assert_eq!(small.side, 300.0);
        assert_eq!(small.square(), Rect::new(0, 32, 300, 300));
    }
}
