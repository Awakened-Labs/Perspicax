//! Resistance: a window being moved stops at the edge of a screen or a panel,
//! and goes past only once it is pushed far enough, as Fluxbox and Openbox
//! hold it.
//!
//! An edge holds a window only as the window's frame crosses it outward, from
//! the part of a monitor its panels leave free to beyond it. Moving a window
//! back in from off the screen, or over a panel from beyond it, is never held.
//! While the frame is past the edge by no more than the edge's strength it sits
//! flush against it, and the pointer moves on ahead. Pushed further, the window
//! goes back under the pointer, where it was grabbed, and moves freely until it
//! next crosses an edge from inside.
//!
//! None of that needs remembering. Where the frame is now says whether it was
//! inside: a held frame is flush, which is still inside, so the next motion
//! holds it again until the push is enough. [`resist`] is a function of where
//! the frame is and where the pointer would put it, as Openbox's is.
//!
//! A monitor's own side is split as edge flipping splits it
//! ([`crate::edge_at`]). Where another monitor lies beyond, it is a seam, which
//! holds with a strength of its own, off unless asked for, so that a window
//! dragged from one monitor to the next does not stop on the way. The rest is
//! an outer edge of the desk.

use crate::{Direction, Rect};

/// How hard edges hold a window being moved: how many pixels past an edge its
/// frame can be pushed and still be held flush against it. 0 holds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Resistance {
    /// At an outer edge of the desk, or a panel's edge.
    pub edges: i32,
    /// Where two monitors meet.
    pub seams: i32,
}

/// One stretch of one side of a monitor's usable area, that holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Edge {
    /// The way out across it: `Right` for the right side of an area.
    outward: Direction,
    /// Where it is: an x for a left or right side, a y for a top or bottom.
    /// Far sides are exclusive, as everywhere in this crate, so a right side
    /// is at `x + w`.
    at: i32,
    /// How far along it reaches, `[from, to)`.
    from: i32,
    to: i32,
    strength: i32,
}

/// Where the frame of a window being moved goes, as its top-left corner:
/// where the pointer would put it, or flush against an edge it is crossing
/// outward that still holds it.
///
/// `placed` is the frame where it is now, and `free` the same frame where the
/// pointer would take it. `monitors` is each monitor's whole rect, with the
/// part of it its panels leave free.
///
/// Each axis is held on its own, by the edges beside where the window goes on
/// the other. That is decided once each way and once more: the window's rows
/// as the pointer would put them pick its column, its column picks its row,
/// and its row picks its column again. Taking the rows from where the window
/// is would miss an edge it moves beside on the way; taking them from where
/// the pointer would put it could hold a window by a panel on the monitor it
/// was just held back from.
#[must_use]
pub fn resist(
    placed: Rect,
    free: Rect,
    monitors: &[(Rect, Rect)],
    resistance: Resistance,
) -> (i32, i32) {
    let edges = edges(monitors, resistance);
    let rows = |y: i32| (y, y + free.h);
    let columns = |x: i32| (x, x + free.w);
    let x = along(Axis::Across, placed, free, rows(free.y), &edges);
    let y = along(Axis::Down, placed, free, columns(x), &edges);
    let x = along(Axis::Across, placed, free, rows(y), &edges);
    (x, y)
}

#[derive(Debug, Clone, Copy)]
enum Axis {
    Across,
    Down,
}

/// Where the frame goes along one axis, held by the edges that reach
/// `beside`, the span the window covers along the other.
fn along(axis: Axis, placed: Rect, free: Rect, beside: (i32, i32), edges: &[Edge]) -> i32 {
    let (was, would, size, back, ahead) = match axis {
        Axis::Across => (placed.x, free.x, free.w, Direction::Left, Direction::Right),
        Axis::Down => (placed.y, free.y, free.h, Direction::Up, Direction::Down),
    };
    let reaches = |edge: &&Edge| edge.from < beside.1 && beside.0 < edge.to;
    // Of the edges the window's leading side is crossing outward, from at or
    // inside one to past it, those it has not been pushed far enough past;
    // and of those, the first it came to.
    if would > was {
        edges
            .iter()
            .filter(|edge| edge.outward == ahead)
            .filter(reaches)
            .filter(|edge| {
                let (before, after) = (was + size, would + size);
                before <= edge.at && edge.at < after && after - edge.at <= edge.strength
            })
            .map(|edge| edge.at)
            .min()
            .map_or(would, |at| at - size)
    } else if would < was {
        edges
            .iter()
            .filter(|edge| edge.outward == back)
            .filter(reaches)
            .filter(|edge| was >= edge.at && edge.at > would && edge.at - would <= edge.strength)
            .map(|edge| edge.at)
            .max()
            .unwrap_or(would)
    } else {
        would
    }
}

/// The edges of every monitor's usable area that hold, each side cut into
/// stretches where another monitor begins or ends beyond it.
fn edges(monitors: &[(Rect, Rect)], resistance: Resistance) -> Vec<Edge> {
    let mut edges = Vec::new();
    for &(whole, usable) in monitors {
        if usable.w <= 0 || usable.h <= 0 {
            continue;
        }
        for outward in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            let ((near, far), (from, to)) = extents(usable, outward);
            let ((whole_near, whole_far), _) = extents(whole, outward);
            // Where this side is, whether it is the monitor's own rather than
            // a panel's, and the line of pixels just beyond it.
            let (at, own, line) = match outward {
                Direction::Left | Direction::Up => (near, near == whole_near, near - 1),
                Direction::Right | Direction::Down => (far, far == whole_far, far),
            };
            // The monitors that line runs through, as stretches along this
            // side. Beyond a panel's edge is the panel, on this monitor.
            let beyond: Vec<(i32, i32)> = monitors
                .iter()
                .map(|&(other, _)| extents(other, outward))
                .filter(|&((near, far), _)| own && near <= line && line < far)
                .map(|(_, along)| along)
                .collect();
            for (from, to, seam) in split(from, to, &beyond) {
                let strength = if seam {
                    resistance.seams
                } else {
                    resistance.edges
                };
                if strength > 0 {
                    edges.push(Edge {
                        outward,
                        at,
                        from,
                        to,
                        strength,
                    });
                }
            }
        }
    }
    edges
}

/// How far `rect` reaches across a side facing `outward`, and along it: its
/// columns then its rows for a left or right side, its rows then its columns
/// for a top or bottom.
fn extents(rect: Rect, outward: Direction) -> ((i32, i32), (i32, i32)) {
    let (columns, rows) = ((rect.x, rect.x + rect.w), (rect.y, rect.y + rect.h));
    match outward {
        Direction::Left | Direction::Right => (columns, rows),
        Direction::Up | Direction::Down => (rows, columns),
    }
}

/// `[from, to)` cut where the stretches in `beyond` begin and end: each piece,
/// and whether it is beyond any of them. Neighbouring pieces alike are one.
fn split(from: i32, to: i32, beyond: &[(i32, i32)]) -> Vec<(i32, i32, bool)> {
    let mut cuts: Vec<i32> = beyond
        .iter()
        .flat_map(|&(start, end)| [start, end])
        .filter(|&cut| from < cut && cut < to)
        .chain([from, to])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut pieces: Vec<(i32, i32, bool)> = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let covered = beyond.iter().any(|&(a, b)| a <= start && end <= b);
        match pieces.last_mut() {
            Some(last) if last.2 == covered => last.1 = end,
            _ => pieces.push((start, end, covered)),
        }
    }
    pieces
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

    /// Minimal's: screens and panels hold, seams do not.
    const MINIMAL: Resistance = Resistance {
        edges: 20,
        seams: 0,
    };

    /// One monitor, with no panel.
    fn alone() -> [(Rect, Rect); 1] {
        [(FHD, FHD)]
    }

    /// One monitor, with a 32 px panel along its top.
    fn panel_on_top() -> [(Rect, Rect); 1] {
        [(FHD, Rect::new(0, 32, 1920, 1048))]
    }

    /// Two monitors side by side, A then B.
    fn pair() -> [(Rect, Rect); 2] {
        let b = Rect::new(1920, 0, 1920, 1080);
        [(FHD, FHD), (b, b)]
    }

    /// A 2560x1440 monitor, with a 1920x1080 one to its right, 180 down.
    fn mixed() -> [(Rect, Rect); 2] {
        let (left, right) = (
            Rect::new(0, 0, 2560, 1440),
            Rect::new(2560, 180, 1920, 1080),
        );
        [(left, left), (right, right)]
    }

    /// A 400x300 frame at `(x, y)`.
    fn frame(x: i32, y: i32) -> Rect {
        Rect::new(x, y, 400, 300)
    }

    /// Moving a 400x300 frame from `from` to where the pointer would put it.
    fn drag(
        from: (i32, i32),
        to: (i32, i32),
        monitors: &[(Rect, Rect)],
        resistance: Resistance,
    ) -> (i32, i32) {
        resist(
            frame(from.0, from.1),
            frame(to.0, to.1),
            monitors,
            resistance,
        )
    }

    #[test]
    fn a_frame_pushed_past_the_edge_holds_flush_until_pushed_past_its_strength() {
        // The frame's right side starts at 1900; the screen's is at 1920.
        let push = |x| drag((1500, 400), (x, 400), &alone(), MINIMAL);
        assert_eq!(push(1530), (1520, 400), "10 px past");
        assert_eq!(push(1540), (1520, 400), "20 px past");
        assert_eq!(push(1541), (1541, 400), "21 px past: under the pointer");
        assert_eq!(push(1545), (1545, 400), "25 px past");
    }

    #[test]
    fn a_strength_of_one_holds_one_pixel() {
        let one = Resistance { edges: 1, seams: 0 };
        let push = |x| drag((1519, 400), (x, 400), &alone(), one);
        assert_eq!(push(1520), (1520, 400), "flush, not past");
        assert_eq!(push(1521), (1520, 400), "1 px past");
        assert_eq!(push(1522), (1522, 400), "2 px past");
    }

    #[test]
    fn nothing_holds_a_window_moved_inward() {
        let monitors = alone();
        let inward = |from, to| drag(from, to, &monitors, MINIMAL);
        assert_eq!(
            inward((-200, 400), (-180, 400)),
            (-180, 400),
            "from the left"
        );
        assert_eq!(
            inward((1700, 400), (1690, 400)),
            (1690, 400),
            "from the right"
        );
        assert_eq!(inward((800, -100), (800, -90)), (800, -90), "from above");
        assert_eq!(inward((800, 900), (800, 890)), (800, 890), "from below");
    }

    #[test]
    fn a_window_moved_over_a_panel_from_beyond_it_is_not_held() {
        assert_eq!(
            drag((800, 0), (800, -5), &panel_on_top(), MINIMAL),
            (800, -5),
            "already over the panel, going on up"
        );
        assert_eq!(
            drag((800, -100), (800, 10), &panel_on_top(), MINIMAL),
            (800, 10),
            "coming down over it"
        );
    }

    #[test]
    fn a_panel_holds_a_titlebar_dragged_up_against_it() {
        let up = |y| drag((800, 40), (800, y), &panel_on_top(), MINIMAL);
        assert_eq!(up(25), (800, 32), "7 px past its edge");
        assert_eq!(up(11), (800, 11), "21 px past it");
    }

    #[test]
    fn a_seam_is_crossed_freely_unless_it_has_a_strength() {
        let rightward = |resistance| drag((1500, 400), (1530, 400), &pair(), resistance);
        assert_eq!(rightward(MINIMAL), (1530, 400));
        let seams = Resistance {
            edges: 20,
            seams: 20,
        };
        assert_eq!(rightward(seams), (1520, 400));
        assert_eq!(
            drag((1920, 400), (1910, 400), &pair(), seams),
            (1920, 400),
            "and from the other side"
        );
    }

    #[test]
    fn beside_a_shorter_monitor_the_seam_is_crossed_and_the_strips_above_and_below_hold() {
        let push = |y| drag((2100, y), (2170, y), &mixed(), MINIMAL);
        assert_eq!(push(500), (2170, 500), "beside the seam");
        assert_eq!(push(1300), (2160, 1300), "below the shorter monitor");
        assert_eq!(push(-100), (2160, -100), "above it");
        assert_eq!(
            push(100),
            (2160, 100),
            "beside both: part would leave the desk"
        );
    }

    #[test]
    fn of_two_stretches_where_one_side_meets_the_other_the_stronger_holds() {
        let both = Resistance {
            edges: 20,
            seams: 10,
        };
        let push = |y, x| drag((2100, y), (x, y), &mixed(), both);
        assert_eq!(push(100, 2175), (2160, 100), "15 px: the outer strip holds");
        assert_eq!(
            push(500, 2175),
            (2175, 500),
            "15 px: the seam alone does not"
        );
        assert_eq!(push(500, 2165), (2160, 500), "5 px: the seam alone does");
    }

    #[test]
    fn a_window_wider_than_the_screen_is_held_by_the_side_that_crosses() {
        let wide = |x| Rect::new(x, 400, 2500, 300);
        assert_eq!(
            resist(wide(-600), wide(-570), &alone(), MINIMAL),
            (-580, 400),
            "its right side crosses the screen's"
        );
        assert_eq!(
            resist(wide(-300), wide(-290), &alone(), MINIMAL),
            (-290, 400),
            "both sides already beyond"
        );
        assert_eq!(
            resist(wide(-300), wide(-310), &alone(), MINIMAL),
            (-310, 400)
        );
    }

    #[test]
    fn of_two_edges_crossed_at_once_the_first_that_still_holds_holds() {
        // A above C, C 10 px wider: two right edges, at 1000 and 1010.
        let (a, c) = (Rect::new(0, 0, 1000, 800), Rect::new(0, 800, 1010, 800));
        let monitors = [(a, a), (c, c)];
        let band = |x| Rect::new(x, 700, 400, 200);
        assert_eq!(
            resist(band(590), band(605), &monitors, MINIMAL),
            (600, 700),
            "5 px past A's"
        );
        assert_eq!(
            resist(band(590), band(625), &monitors, MINIMAL),
            (610, 700),
            "25 px past A's, 15 past C's"
        );
    }

    #[test]
    fn a_held_window_follows_the_pointer_back_and_stays_held_while_still_past() {
        let held = (1520, 400);
        assert_eq!(drag(held, (1510, 400), &alone(), MINIMAL), (1510, 400));
        assert_eq!(drag(held, (1525, 400), &alone(), MINIMAL), held);
    }

    #[test]
    fn a_window_pushed_free_is_held_again_only_after_coming_back_inside() {
        assert_eq!(
            drag((1545, 400), (1535, 400), &alone(), MINIMAL),
            (1535, 400),
            "back a little, still past the edge"
        );
        assert_eq!(
            drag((1535, 400), (1515, 400), &alone(), MINIMAL),
            (1515, 400)
        );
        assert_eq!(
            drag((1515, 400), (1530, 400), &alone(), MINIMAL),
            (1520, 400),
            "out again from inside"
        );
    }

    #[test]
    fn a_corner_holds_on_both_sides_and_one_side_holds_while_sliding_along_it() {
        assert_eq!(
            drag((1500, 760), (1530, 790), &alone(), MINIMAL),
            (1520, 780)
        );
        assert_eq!(
            drag((1520, 400), (1530, 300), &alone(), MINIMAL),
            (1520, 300)
        );
    }

    #[test]
    fn a_held_window_moved_off_the_end_of_the_stretch_holding_it_goes_free() {
        assert_eq!(
            drag((2160, 1300), (2170, 900), &mixed(), MINIMAL),
            (2170, 900),
            "from beside the strip below the shorter monitor to beside the seam"
        );
    }

    #[test]
    fn the_rows_that_decide_a_hold_are_where_the_window_goes() {
        // B has a panel along its top; A has none.
        let b = Rect::new(1920, 0, 1920, 1080);
        let monitors = [(FHD, FHD), (b, Rect::new(1920, 32, 1920, 1048))];
        let narrow = |x, y| Rect::new(x, y, 415, 300);
        let seams = Resistance {
            edges: 20,
            seams: 20,
        };
        assert_eq!(
            resist(narrow(1500, 40), narrow(1510, 30), &monitors, seams),
            (1505, 30),
            "held at the seam, all on A: B's panel does not hold it"
        );
        assert_eq!(
            resist(narrow(1500, 40), narrow(1510, 30), &monitors, MINIMAL),
            (1510, 32),
            "over the seam into B, under B's panel: the panel holds it"
        );
    }

    #[test]
    fn the_same_motion_again_changes_nothing() {
        let once = drag((1500, 400), (1530, 400), &alone(), MINIMAL);
        assert_eq!(drag(once, (1530, 400), &alone(), MINIMAL), once);
    }

    #[test]
    fn with_no_strength_nothing_holds() {
        assert_eq!(
            drag((1500, 400), (1530, 400), &alone(), Resistance::default()),
            (1530, 400)
        );
    }

    #[test]
    fn a_panel_side_is_one_edge_and_the_monitors_own_sides_are_outer() {
        let edge = |outward, at, from, to| Edge {
            outward,
            at,
            from,
            to,
            strength: 20,
        };
        assert_eq!(
            edges(&panel_on_top(), MINIMAL),
            [
                edge(Direction::Left, 0, 32, 1080),
                edge(Direction::Right, 1920, 32, 1080),
                edge(Direction::Up, 32, 0, 1920),
                edge(Direction::Down, 1080, 0, 1920),
            ]
        );
    }

    #[test]
    fn a_side_is_cut_where_the_monitor_beyond_it_begins_and_ends() {
        let both = Resistance {
            edges: 20,
            seams: 5,
        };
        let stretches = |resistance, outward, at| -> Vec<(i32, i32, i32)> {
            edges(&mixed(), resistance)
                .into_iter()
                .filter(|edge| edge.outward == outward && edge.at == at)
                .map(|edge| (edge.from, edge.to, edge.strength))
                .collect()
        };
        assert_eq!(
            stretches(both, Direction::Right, 2560),
            [(0, 180, 20), (180, 1260, 5), (1260, 1440, 20)],
            "the taller monitor's right side"
        );
        assert_eq!(
            stretches(both, Direction::Left, 2560),
            [(180, 1260, 5)],
            "the shorter one's left side, all seam"
        );
        assert_eq!(
            stretches(MINIMAL, Direction::Right, 2560),
            [(0, 180, 20), (1260, 1440, 20)],
            "a seam of no strength is left out"
        );
        assert_eq!(stretches(MINIMAL, Direction::Left, 2560), []);
    }

    #[test]
    fn a_panel_along_a_seam_holds_as_a_panel() {
        // B's panel runs down its left side, along the seam with A.
        let b = Rect::new(1920, 0, 1920, 1080);
        let monitors = [(FHD, FHD), (b, Rect::new(1968, 0, 1872, 1080))];
        assert_eq!(
            drag((1980, 400), (1960, 400), &monitors, MINIMAL),
            (1968, 400),
            "out of B's usable area, over its panel"
        );
        assert_eq!(
            drag((1500, 400), (1530, 400), &monitors, MINIMAL),
            (1530, 400),
            "out of A, over the seam"
        );
    }

    #[test]
    fn a_monitor_with_no_usable_area_has_no_edges() {
        assert!(edges(&[(FHD, Rect::new(0, 0, 0, 1080))], MINIMAL).is_empty());
    }
}
