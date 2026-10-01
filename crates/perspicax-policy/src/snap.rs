//! Snapping: dragging a window against an edge of the desk to give it half
//! or a quarter of a monitor, or the whole of one, as Windows does.
//!
//! Like edge flipping, only an *outer* edge snaps. Between two monitors the
//! pointer passes through, and a window being dragged across must not snap
//! to the seam on the way.
//!
//! The same zones are reached from the keyboard, Logo and an arrow, stepping
//! between them the way Windows steps ([`keyed`]).

use crate::{Direction, Rect, edge_at};

/// Where a snapped window goes on its monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// The whole monitor: maximized.
    Top,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// How snapping by dragging behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapping {
    /// Dragging a window to an outer edge snaps it.
    pub drag: bool,
    /// How near the edge, in pixels, the pointer has to be. The pointer stops
    /// at an outer edge, so this only makes the target forgiving.
    pub threshold: i32,
}

impl Default for Snapping {
    fn default() -> Self {
        Self {
            drag: true,
            threshold: 4,
        }
    }
}

/// The share of a side, from either end, that is a corner rather than a
/// half: an eighth, so a corner is easy to hit and a half is the rest.
const CORNER: i32 = 8;

impl Zone {
    /// The rectangle this zone is of `area`, the part of the monitor panels
    /// leave free. Halves and quarters split odd sizes so that the two halves
    /// together cover the area exactly.
    #[must_use]
    pub fn rect(self, area: Rect) -> Rect {
        let (half_w, half_h) = (area.w / 2, area.h / 2);
        let (left, right) = ((area.x, half_w), (area.x + half_w, area.w - half_w));
        let (top, bottom) = ((area.y, half_h), (area.y + half_h, area.h - half_h));
        let make = |(x, w): (i32, i32), (y, h): (i32, i32)| Rect::new(x, y, w, h);
        let whole_h = (area.y, area.h);
        match self {
            Self::Top => area,
            Self::Left => make(left, whole_h),
            Self::Right => make(right, whole_h),
            Self::TopLeft => make(left, top),
            Self::TopRight => make(right, top),
            Self::BottomLeft => make(left, bottom),
            Self::BottomRight => make(right, bottom),
        }
    }
}

/// The zone a window dragged with the pointer at `pointer` would snap to on
/// `output`, given every output's rect: `None` away from the outer edges of
/// the desk.
///
/// The side edges give a half, or a quarter in the top or bottom eighth. The
/// top edge maximizes, or gives a quarter in the leftmost or rightmost
/// eighth. The bottom edge gives nothing but its corners, because dragging a
/// window to the bottom of the screen is how a person moves it out of the
/// way, not a request for a shape.
#[must_use]
pub fn zone(pointer: (f64, f64), output: Rect, outputs: &[Rect], threshold: i32) -> Option<Zone> {
    let (x, y) = pointer;
    let near = f64::from(threshold.max(1));
    let (left, top) = (f64::from(output.x), f64::from(output.y));
    let (right, bottom) = (
        f64::from(output.x + output.w),
        f64::from(output.y + output.h),
    );
    // Whether the edge on that side, at this point along it, is an outer
    // edge: the pointer moved onto it would be against the edge of the desk.
    let outer = |direction: Direction| {
        let onto = match direction {
            Direction::Left => (left, y),
            Direction::Right => (right - 0.5, y),
            Direction::Up => (x, top),
            Direction::Down => (x, bottom - 0.5),
        };
        edge_at(onto, outputs) == Some(direction)
    };
    let at_left = x < left + near && outer(Direction::Left);
    let at_right = x >= right - near && outer(Direction::Right);
    let at_top = y < top + near && outer(Direction::Up);
    let at_bottom = y >= bottom - near && outer(Direction::Down);
    let (corner_w, corner_h) = (f64::from(output.w / CORNER), f64::from(output.h / CORNER));
    let high = y < top + corner_h;
    let low = y >= bottom - corner_h;
    let leftish = x < left + corner_w;
    let rightish = x >= right - corner_w;
    Some(if at_left {
        if high {
            Zone::TopLeft
        } else if low {
            Zone::BottomLeft
        } else {
            Zone::Left
        }
    } else if at_right {
        if high {
            Zone::TopRight
        } else if low {
            Zone::BottomRight
        } else {
            Zone::Right
        }
    } else if at_top {
        if leftish {
            Zone::TopLeft
        } else if rightish {
            Zone::TopRight
        } else {
            Zone::Top
        }
    } else if at_bottom && leftish {
        Zone::BottomLeft
    } else if at_bottom && rightish {
        Zone::BottomRight
    } else {
        return None;
    })
}

/// What Logo and an arrow does to a window snapped to `current` (or not
/// snapped, `None`), as Windows does it: `Some` zone to snap to, or `None`
/// to put the window back as it was.
///
/// Left and right take a window to that half, and back from the other half
/// to where it was. Up and down move a half to its quarter and a quarter to
/// its half; up from nothing maximizes, and down from maximized restores.
#[must_use]
pub fn keyed(current: Option<Zone>, direction: Direction) -> Option<Zone> {
    use Direction::{Down, Left, Right, Up};
    use Zone::{BottomLeft, BottomRight, Top, TopLeft, TopRight};
    match (current, direction) {
        (None | Some(Top), Left) => Some(Zone::Left),
        (None | Some(Top), Right) => Some(Zone::Right),
        (None, Up) => Some(Top),
        (Some(Zone::Left), Up) => Some(TopLeft),
        (Some(Zone::Right), Up) => Some(TopRight),
        (Some(Zone::Left), Down) => Some(BottomLeft),
        (Some(Zone::Right), Down) => Some(BottomRight),
        (Some(TopLeft | BottomLeft), Up | Down) | (Some(TopLeft | BottomLeft), Left) => {
            Some(Zone::Left)
        }
        (Some(TopRight | BottomRight), Up | Down) | (Some(TopRight | BottomRight), Right) => {
            Some(Zone::Right)
        }
        (Some(TopLeft), Right) => Some(TopRight),
        (Some(BottomLeft), Right) => Some(BottomRight),
        (Some(TopRight), Left) => Some(TopLeft),
        (Some(BottomRight), Left) => Some(BottomLeft),
        (Some(Zone::Left), Left) => Some(Zone::Left),
        (Some(Zone::Right), Right) => Some(Zone::Right),
        (Some(Top) | None, Down) | (Some(Zone::Left), Right) | (Some(Zone::Right), Left) => None,
        (Some(Top), Up) => Some(Top),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    fn alone(pointer: (f64, f64)) -> Option<Zone> {
        zone(pointer, FHD, &[FHD], 4)
    }

    #[test]
    fn the_sides_snap_to_halves_and_the_top_maximizes() {
        assert_eq!(alone((0.0, 540.0)), Some(Zone::Left));
        assert_eq!(alone((1919.5, 540.0)), Some(Zone::Right));
        assert_eq!(alone((960.0, 0.0)), Some(Zone::Top));
    }

    #[test]
    fn the_corners_snap_to_quarters() {
        assert_eq!(alone((0.0, 20.0)), Some(Zone::TopLeft));
        assert_eq!(alone((1919.5, 1079.5)), Some(Zone::BottomRight));
        assert_eq!(
            alone((100.0, 0.0)),
            Some(Zone::TopLeft),
            "along the top, near the left"
        );
        assert_eq!(alone((1900.0, 1079.5)), Some(Zone::BottomRight));
    }

    #[test]
    fn the_middle_of_the_bottom_and_of_the_screen_do_not_snap() {
        assert_eq!(alone((960.0, 1079.5)), None);
        assert_eq!(alone((960.0, 540.0)), None);
        assert_eq!(alone((10.0, 540.0)), None, "near, not at, the edge");
    }

    #[test]
    fn the_seam_between_two_monitors_does_not_snap() {
        let right = Rect::new(1920, 0, 1920, 1080);
        let desk = [FHD, right];
        assert_eq!(zone((1919.5, 540.0), FHD, &desk, 4), None);
        assert_eq!(zone((1920.0, 540.0), right, &desk, 4), None);
        assert_eq!(zone((3839.5, 540.0), right, &desk, 4), Some(Zone::Right));
    }

    #[test]
    fn zones_split_the_free_area_exactly() {
        let area = Rect::new(0, 32, 1921, 1048);
        assert_eq!(Zone::Left.rect(area), Rect::new(0, 32, 960, 1048));
        assert_eq!(Zone::Right.rect(area), Rect::new(960, 32, 961, 1048));
        assert_eq!(Zone::BottomRight.rect(area), Rect::new(960, 556, 961, 524));
        assert_eq!(Zone::Top.rect(area), area);
    }

    #[test]
    fn logo_and_the_arrows_step_as_windows_does() {
        use Direction::{Down, Left, Right, Up};
        assert_eq!(keyed(None, Left), Some(Zone::Left));
        assert_eq!(keyed(Some(Zone::Left), Up), Some(Zone::TopLeft));
        assert_eq!(keyed(Some(Zone::TopLeft), Down), Some(Zone::Left));
        assert_eq!(keyed(Some(Zone::Left), Right), None, "back as it was");
        assert_eq!(keyed(None, Up), Some(Zone::Top));
        assert_eq!(keyed(Some(Zone::Top), Down), None);
        assert_eq!(
            keyed(Some(Zone::BottomLeft), Right),
            Some(Zone::BottomRight)
        );
    }
}
