//! Where each monitor sits in the global space.
//!
//! A person describes a desk, not coordinates: "the laptop is below the big
//! screen, pushed 300 pixels right". Coordinates follow from the sizes, and
//! the sizes are only known once each monitor is lit at a mode and a scale.
//! So the config says how monitors relate, and [`arrange`] turns that into
//! positions each time the set of lit monitors changes.
//!
//! Everything is in logical pixels, the mode divided by the scale, because
//! that is the space windows and the pointer live in. A 4K monitor at scale 2
//! is 1920 wide here, and the monitor beside it starts at 1920.

use crate::Rect;

/// Which side of another monitor this one is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    LeftOf,
    RightOf,
    Above,
    Below,
}

/// How one monitor is placed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Place {
    /// To the right of everything placed so far, top-aligned with the
    /// global origin.
    #[default]
    Auto,
    /// Its top-left corner at exactly this point.
    At(i32, i32),
    /// Against one side of the monitor named `of`, with the edges aligned
    /// (tops for left and right, lefts for above and below) and then moved
    /// `offset` along the edge they share. A positive offset is down for
    /// [`Side::LeftOf`] and [`Side::RightOf`], right for [`Side::Above`] and
    /// [`Side::Below`].
    Beside { side: Side, of: String, offset: i32 },
}

/// One monitor to place: its name, its size in logical pixels, and how the
/// config says to place it.
#[derive(Debug, Clone, Copy)]
pub struct Screen<'a> {
    pub name: &'a str,
    pub size: (i32, i32),
    pub place: &'a Place,
}

/// The top-left corner of every screen, in the order given.
///
/// Absolute positions first, then everything placed beside something already
/// placed, repeatedly, so chains (C below B, B right of A) resolve in any
/// order. When nothing more can be placed that way, the first remaining screen
/// in the order given goes to the right of everything so far, and the loop
/// carries on. That covers [`Place::Auto`], a screen beside one that is not
/// connected (which monitors are plugged in changes all the time, so that is
/// not an error), and a cycle the config should have refused.
///
/// Last, the whole arrangement is moved so the topmost and leftmost edges are
/// at 0. A monitor left of the first one would otherwise sit at a negative x,
/// and Xwayland and absolute pointing devices assume the space starts at the
/// origin.
#[must_use]
pub fn arrange(screens: &[Screen<'_>]) -> Vec<(i32, i32)> {
    let mut at: Vec<Option<(i32, i32)>> = screens
        .iter()
        .map(|screen| match screen.place {
            Place::At(x, y) => Some((*x, *y)),
            _ => None,
        })
        .collect();

    while at.iter().any(Option::is_none) {
        let mut progressed = false;
        for (i, screen) in screens.iter().enumerate() {
            if at[i].is_some() {
                continue;
            }
            let Place::Beside { side, of, offset } = screen.place else {
                continue;
            };
            let anchor = screens
                .iter()
                .position(|other| other.name == of)
                .and_then(|j| {
                    at[j].map(|(x, y)| Rect::new(x, y, screens[j].size.0, screens[j].size.1))
                });
            if let Some(anchor) = anchor {
                at[i] = Some(beside(anchor, screen.size, *side, *offset));
                progressed = true;
            }
        }
        if progressed {
            continue;
        }
        // Nothing more is placed relative to something placed. The first
        // unplaced screen that is not waiting on another one goes to the
        // right of everything, and becomes something the rest can be placed
        // beside. Only a cycle leaves every unplaced screen waiting, and then
        // the first of them breaks it.
        let waiting = |i: usize| match screens[i].place {
            Place::Beside { of, .. } => screens.iter().any(|other| other.name == of),
            _ => false,
        };
        let unplaced = || (0..screens.len()).filter(|&i| at[i].is_none());
        if let Some(next) = unplaced()
            .find(|&i| !waiting(i))
            .or_else(|| unplaced().next())
        {
            let right = at
                .iter()
                .zip(screens)
                .filter_map(|(at, screen)| at.map(|(x, _)| x + screen.size.0))
                .max()
                .unwrap_or(0);
            at[next] = Some((right, 0));
        }
    }

    let placed: Vec<(i32, i32)> = at.into_iter().map(Option::unwrap_or_default).collect();
    let min_x = placed.iter().map(|&(x, _)| x).min().unwrap_or(0);
    let min_y = placed.iter().map(|&(_, y)| y).min().unwrap_or(0);
    placed
        .into_iter()
        .map(|(x, y)| (x - min_x, y - min_y))
        .collect()
}

fn beside(anchor: Rect, (w, h): (i32, i32), side: Side, offset: i32) -> (i32, i32) {
    match side {
        Side::RightOf => (anchor.x + anchor.w, anchor.y + offset),
        Side::LeftOf => (anchor.x - w, anchor.y + offset),
        Side::Below => (anchor.x + offset, anchor.y + anchor.h),
        Side::Above => (anchor.x + offset, anchor.y - h),
    }
}

/// The first two outputs that overlap, by index. Not refused, because two
/// absolute positions a person wrote may mean a mirror they want, but worth a
/// warning: a window on the overlap is on both, and the pointer cannot tell
/// which.
#[must_use]
pub fn overlapping(outputs: &[Rect]) -> Option<(usize, usize)> {
    (0..outputs.len()).find_map(|i| {
        (i + 1..outputs.len())
            .find(|&j| intersects(outputs[i], outputs[j]))
            .map(|j| (i, j))
    })
}

/// Where a window that is on no output should go: inside the output nearest
/// to it, as close to where it was as fits. `None` for a window any part of
/// which is already on an output, which is left where the person put it.
///
/// What an unplugged monitor leaves behind. Its windows keep their
/// coordinates, which now point at nothing; without this, they are open,
/// focused, and nowhere anyone can see or reach them.
#[must_use]
pub fn rescue(window: Rect, outputs: &[Rect]) -> Option<(i32, i32)> {
    if outputs.iter().any(|&output| intersects(window, output)) {
        return None;
    }
    let centre = (
        i64::from(window.x) + i64::from(window.w) / 2,
        i64::from(window.y) + i64::from(window.h) / 2,
    );
    let nearest = outputs
        .iter()
        .min_by_key(|output| distance_squared(centre, **output))?;
    let x = window
        .x
        .clamp(nearest.x, nearest.x + (nearest.w - window.w).max(0));
    let y = window
        .y
        .clamp(nearest.y, nearest.y + (nearest.h - window.h).max(0));
    Some((x, y))
}

/// Whether two rectangles share any area. Touching edges do not count:
/// monitors side by side touch and do not overlap.
#[must_use]
pub fn intersects(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn distance_squared((px, py): (i64, i64), rect: Rect) -> i64 {
    let (x, y, w, h) = (
        i64::from(rect.x),
        i64::from(rect.y),
        i64::from(rect.w),
        i64::from(rect.h),
    );
    let dx = (x - px).max(0).max(px - (x + w));
    let dy = (y - py).max(0).max(py - (y + h));
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    const QHD: (i32, i32) = (2560, 1440);
    const FHD: (i32, i32) = (1920, 1080);

    fn beside_of(side: Side, of: &str, offset: i32) -> Place {
        Place::Beside {
            side,
            of: of.to_owned(),
            offset,
        }
    }

    fn screen<'a>(name: &'a str, size: (i32, i32), place: &'a Place) -> Screen<'a> {
        Screen { name, size, place }
    }

    #[test]
    fn unplaced_monitors_line_up_left_to_right_in_the_order_found() {
        let auto = Place::Auto;
        let at = arrange(&[screen("DP-1", QHD, &auto), screen("HDMI-A-1", FHD, &auto)]);
        assert_eq!(at, [(0, 0), (2560, 0)]);
    }

    #[test]
    fn a_smaller_monitor_right_of_a_bigger_one_is_lowered_by_its_offset() {
        let auto = Place::Auto;
        let right = beside_of(Side::RightOf, "DP-1", 180);
        let at = arrange(&[screen("DP-1", QHD, &auto), screen("HDMI-A-1", FHD, &right)]);
        assert_eq!(at, [(0, 0), (2560, 180)], "centred: (1440 - 1080) / 2");
    }

    #[test]
    fn a_monitor_left_of_the_first_moves_everything_right_of_it() {
        let auto = Place::Auto;
        let left = beside_of(Side::LeftOf, "DP-1", -100);
        let at = arrange(&[screen("DP-1", FHD, &auto), screen("eDP-1", QHD, &left)]);
        assert_eq!(
            at,
            [(2560, 100), (0, 0)],
            "normalized so nothing is negative"
        );
    }

    #[test]
    fn a_laptop_below_a_monitor_is_offset_along_the_bottom_edge() {
        let auto = Place::Auto;
        let below = beside_of(Side::Below, "DP-1", 320);
        let at = arrange(&[screen("eDP-1", FHD, &below), screen("DP-1", QHD, &auto)]);
        assert_eq!(at, [(320, 1440), (0, 0)]);
    }

    #[test]
    fn a_chain_resolves_whatever_order_it_is_written_in() {
        // An L: C above B, B right of A, listed backwards.
        let auto = Place::Auto;
        let b = beside_of(Side::RightOf, "A", 0);
        let c = beside_of(Side::Above, "B", 0);
        let at = arrange(&[
            screen("C", FHD, &c),
            screen("B", FHD, &b),
            screen("A", FHD, &auto),
        ]);
        assert_eq!(at, [(1920, 0), (1920, 1080), (0, 1080)]);
    }

    #[test]
    fn a_monitor_beside_one_that_is_unplugged_is_placed_automatically() {
        let auto = Place::Auto;
        let right = beside_of(Side::RightOf, "DP-2", 0);
        let at = arrange(&[screen("DP-1", FHD, &auto), screen("HDMI-A-1", FHD, &right)]);
        assert_eq!(at, [(0, 0), (1920, 0)]);
    }

    #[test]
    fn a_cycle_still_places_every_monitor() {
        let a = beside_of(Side::RightOf, "B", 0);
        let b = beside_of(Side::RightOf, "A", 0);
        let at = arrange(&[screen("A", FHD, &a), screen("B", FHD, &b)]);
        assert_eq!(at, [(0, 0), (1920, 0)]);
    }

    #[test]
    fn an_absolute_position_anchors_relative_ones() {
        let fixed = Place::At(1000, 500);
        let below = beside_of(Side::Below, "DP-1", 0);
        let at = arrange(&[screen("DP-1", FHD, &fixed), screen("eDP-1", FHD, &below)]);
        assert_eq!(at, [(0, 0), (0, 1080)], "normalized to the origin");
    }

    #[test]
    fn side_by_side_is_not_an_overlap_and_a_shared_area_is() {
        let a = Rect::new(0, 0, 1920, 1080);
        assert_eq!(overlapping(&[a, Rect::new(1920, 0, 1920, 1080)]), None);
        assert_eq!(
            overlapping(&[a, Rect::new(1900, 0, 1920, 1080)]),
            Some((0, 1))
        );
    }

    #[test]
    fn a_window_on_any_output_is_not_rescued() {
        let outputs = [Rect::new(0, 0, 1920, 1080)];
        assert_eq!(rescue(Rect::new(1800, 100, 400, 300), &outputs), None);
    }

    #[test]
    fn a_window_left_on_an_unplugged_monitor_comes_onto_the_nearest() {
        let outputs = [Rect::new(0, 0, 1920, 1080)];
        assert_eq!(
            rescue(Rect::new(2500, 200, 800, 600), &outputs),
            Some((1120, 200)),
            "against the right edge, at the same height"
        );
    }

    #[test]
    fn a_rescued_window_bigger_than_the_output_keeps_its_top_left_on_it() {
        let outputs = [Rect::new(0, 0, 1280, 1024)];
        assert_eq!(
            rescue(Rect::new(3000, 0, 2560, 1440), &outputs),
            Some((0, 0))
        );
    }

    #[test]
    fn with_no_outputs_there_is_nowhere_to_rescue_to() {
        assert_eq!(rescue(Rect::new(0, 0, 10, 10), &[]), None);
    }
}
