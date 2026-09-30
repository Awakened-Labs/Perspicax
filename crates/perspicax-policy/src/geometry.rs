//! Where windows go and how big they get.
//!
//! Every function here is plain arithmetic on rectangles, kept out of the
//! compositor so it can be tested without one. The compositor's part is to
//! feed in the rectangles it has and apply the answer. It carries out the
//! xdg-shell protocol around each decision, and none of these functions needs
//! to know that exists.

/// A rectangle in the compositor's global space, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    #[must_use]
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
}

/// Which edges of a window are being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

/// How far a new window is offset from the one before it.
const CASCADE: i32 = 32;

/// Where the `nth` new window on `output` goes: a cascade from the output's
/// top-left corner, so two windows opened in a row are never exactly on top of
/// each other.
///
/// The cascade wraps before it reaches the middle of the output. Unbounded,
/// the fortieth window of a long session would open off the bottom-right of
/// the screen. The wrap is a whole number of steps, so a wrapped window lands
/// exactly on the first one's position, not one pixel off it.
#[must_use]
pub fn place(nth: u32, output: Rect) -> (i32, i32) {
    let span = (output.w.min(output.h) / 2).max(CASCADE);
    let steps = u32::try_from(span / CASCADE).unwrap_or(1).max(1);
    let offset = i32::try_from(nth % steps).unwrap_or(0) * CASCADE;
    (output.x + offset, output.y + offset)
}

/// The size a window resized from `start` should be, with the pointer moved
/// `(dx, dy)` since the drag began.
///
/// `min` and `max` are what the client declared, `0` meaning "no limit" as it
/// does in `xdg_toplevel`. Nothing gets smaller than 1x1, whatever the client
/// says, because a window of zero size cannot be clicked to be resized back.
#[must_use]
pub fn resize(
    start: Rect,
    edges: Edges,
    (dx, dy): (i32, i32),
    min: (i32, i32),
    max: (i32, i32),
) -> (i32, i32) {
    let w = match (edges.left, edges.right) {
        (true, _) => start.w - dx,
        (_, true) => start.w + dx,
        _ => start.w,
    };
    let h = match (edges.top, edges.bottom) {
        (true, _) => start.h - dy,
        (_, true) => start.h + dy,
        _ => start.h,
    };
    (bound(w, min.0, max.0), bound(h, min.1, max.1))
}

fn bound(value: i32, min: i32, max: i32) -> i32 {
    let max = if max <= 0 { i32::MAX } else { max };
    value.min(max).max(min.max(1))
}

/// Where a window resized from `start` should sit, now that it has committed
/// `size`.
///
/// Dragging the right or bottom edge leaves the top-left corner where it was.
/// Dragging the left or top edge has to keep the *opposite* edge still, which
/// means moving the window by however much it changed size. That can only be
/// done once the client has said what size it actually chose, which may not
/// be the size it was asked for. A terminal rounds to whole character cells.
#[must_use]
pub fn anchor(start: Rect, edges: Edges, size: (i32, i32)) -> (i32, i32) {
    let x = if edges.left {
        start.x + (start.w - size.0)
    } else {
        start.x
    };
    let y = if edges.top {
        start.y + (start.h - size.1)
    } else {
        start.y
    };
    (x, y)
}

/// Which edges a modifier-drag at `at`, relative to the window's top-left,
/// should resize.
///
/// Split the window into thirds each way and take the edges of the third the
/// pointer is in, the way Openbox and KWin do. The corners resize diagonally,
/// the middle of each side resizes that side, and the very centre resizes the
/// bottom-right because it has to resize something.
#[must_use]
pub fn edges_near(at: (i32, i32), size: (i32, i32)) -> Edges {
    let (x, y) = at;
    let (w, h) = (size.0.max(1), size.1.max(1));
    let mut edges = Edges {
        left: x < w / 3,
        right: x > w - w / 3,
        top: y < h / 3,
        bottom: y > h - h / 3,
    };
    if edges == Edges::default() {
        edges.right = true;
        edges.bottom = true;
    }
    edges
}

/// Where a maximized window goes when it is dragged out of maximized.
///
/// Its restored size is smaller than the output it filled, so it is placed
/// under the pointer at the same relative position across its width. Grab a
/// maximized window a quarter of the way along its titlebar, and the restored
/// window hangs from the pointer a quarter of the way along. The top edge
/// stays where it was, so the titlebar is still under the pointer.
#[must_use]
pub fn unmaximized_at(pointer: (f64, f64), maximized: Rect, restored: (i32, i32)) -> (i32, i32) {
    let fraction = if maximized.w > 0 {
        ((pointer.0 - f64::from(maximized.x)) / f64::from(maximized.w)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // A pixel position, well inside i32.
    let x = (pointer.0 - fraction * f64::from(restored.0)).round() as i32;
    (x, maximized.y)
}

/// The output next to `from` in the row, the way `towards` points,
/// wrapping at either end. Outputs are ordered left to right by their left
/// edge, then top to bottom, which is the order a person sees them in. `None`
/// with fewer than two outputs, since there is nowhere to go.
#[must_use]
pub fn neighbour(outputs: &[Rect], from: usize, towards: crate::Towards) -> Option<usize> {
    if outputs.len() < 2 || from >= outputs.len() {
        return None;
    }
    let mut order: Vec<usize> = (0..outputs.len()).collect();
    order.sort_by_key(|&i| (outputs[i].x, outputs[i].y));
    let at = order.iter().position(|&i| i == from)?;
    let len = order.len();
    let next = match towards {
        crate::Towards::Next => (at + 1) % len,
        crate::Towards::Previous => (at + len - 1) % len,
    };
    Some(order[next])
}

/// Where a window at `window` on output `from` goes on output `to`: at the
/// same distance from the output's top-left corner, pulled back inside if
/// the new output is too small to hold it there. The same offset rather than
/// the same proportion, because a window of fixed size scaled by position
/// lands somewhere the eye does not expect.
#[must_use]
pub fn carry(window: Rect, from: Rect, to: Rect) -> (i32, i32) {
    let dx = (window.x - from.x).clamp(0, (to.w - window.w).max(0));
    let dy = (window.y - from.y).clamp(0, (to.h - window.h).max(0));
    (to.x + dx, to.y + dy)
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

    fn edges(top: bool, bottom: bool, left: bool, right: bool) -> Edges {
        Edges {
            top,
            bottom,
            left,
            right,
        }
    }

    #[test]
    fn the_first_windows_cascade_from_the_outputs_corner() {
        assert_eq!(place(0, FHD), (0, 0));
        assert_eq!(place(2, FHD), (64, 64));
        let right = Rect::new(1920, 0, 2560, 1440);
        assert_eq!(place(1, right), (1952, 32));
    }

    #[test]
    fn the_cascade_wraps_onto_the_first_position_before_the_middle() {
        // 1080 / 2 = 540, which is 16 whole steps of 32.
        assert_eq!(place(16, FHD), (0, 0));
        assert_eq!(place(15, FHD), (480, 480));
    }

    #[test]
    fn a_tiny_output_still_cascades_by_at_least_one_step() {
        assert_eq!(place(1, Rect::new(0, 0, 40, 40)), (0, 0));
    }

    #[test]
    fn dragging_the_right_edge_grows_the_width_only() {
        let start = Rect::new(100, 100, 400, 300);
        let size = resize(
            start,
            edges(false, false, false, true),
            (50, 20),
            (0, 0),
            (0, 0),
        );
        assert_eq!(size, (450, 300));
    }

    #[test]
    fn dragging_the_top_left_corner_out_grows_both_ways() {
        let start = Rect::new(100, 100, 400, 300);
        let size = resize(
            start,
            edges(true, false, true, false),
            (-50, -20),
            (0, 0),
            (0, 0),
        );
        assert_eq!(size, (450, 320));
    }

    #[test]
    fn a_resize_respects_what_the_client_declared() {
        let start = Rect::new(0, 0, 400, 300);
        let grown = resize(
            start,
            edges(false, true, false, true),
            (500, 500),
            (0, 0),
            (600, 0),
        );
        assert_eq!(grown, (600, 800), "max width 600, height unbounded");
        let shrunk = resize(
            start,
            edges(false, true, false, true),
            (-390, -290),
            (200, 150),
            (0, 0),
        );
        assert_eq!(shrunk, (200, 150));
    }

    #[test]
    fn nothing_is_resized_below_one_pixel() {
        let start = Rect::new(0, 0, 10, 10);
        let size = resize(
            start,
            edges(false, true, false, true),
            (-100, -100),
            (0, 0),
            (0, 0),
        );
        assert_eq!(size, (1, 1));
    }

    #[test]
    fn a_left_edge_resize_keeps_the_right_edge_still_at_the_committed_size() {
        let start = Rect::new(100, 100, 400, 300);
        // Asked for 450 wide; the client chose 448 (whole cells, say).
        assert_eq!(
            anchor(start, edges(false, false, true, false), (448, 300)),
            (52, 100)
        );
    }

    #[test]
    fn a_bottom_right_resize_never_moves_the_window() {
        let start = Rect::new(100, 100, 400, 300);
        assert_eq!(
            anchor(start, edges(false, true, false, true), (10, 10)),
            (100, 100)
        );
    }

    #[test]
    fn a_drag_near_a_corner_resizes_diagonally() {
        assert_eq!(
            edges_near((5, 5), (300, 300)),
            edges(true, false, true, false)
        );
        assert_eq!(
            edges_near((295, 295), (300, 300)),
            edges(false, true, false, true)
        );
    }

    #[test]
    fn a_drag_near_the_middle_of_a_side_resizes_that_side() {
        assert_eq!(
            edges_near((150, 5), (300, 300)),
            edges(true, false, false, false)
        );
    }

    #[test]
    fn a_drag_in_the_centre_resizes_the_bottom_right() {
        assert_eq!(
            edges_near((150, 150), (300, 300)),
            edges(false, true, false, true)
        );
    }

    #[test]
    fn an_unmaximized_window_keeps_the_pointers_place_along_its_titlebar() {
        // A quarter of the way along a 1920-wide maximized window.
        let at = unmaximized_at((480.0, 10.0), FHD, (800, 600));
        assert_eq!(at, (280, 0), "480 - 800/4");
    }

    #[test]
    fn the_next_output_is_to_the_right_and_wraps() {
        let row = [
            Rect::new(1920, 0, 1920, 1080),
            FHD,
            Rect::new(3840, 0, 1280, 1024),
        ];
        assert_eq!(neighbour(&row, 1, crate::Towards::Next), Some(0));
        assert_eq!(
            neighbour(&row, 2, crate::Towards::Next),
            Some(1),
            "wraps to the leftmost"
        );
        assert_eq!(
            neighbour(&row, 1, crate::Towards::Previous),
            Some(2),
            "wraps to the rightmost"
        );
    }

    #[test]
    fn one_output_has_no_neighbour() {
        assert_eq!(neighbour(&[FHD], 0, crate::Towards::Next), None);
    }

    #[test]
    fn a_carried_window_keeps_its_offset_from_the_corner() {
        let right = Rect::new(1920, 0, 2560, 1440);
        assert_eq!(carry(Rect::new(100, 50, 800, 600), FHD, right), (2020, 50));
    }

    #[test]
    fn a_carried_window_is_pulled_back_onto_a_smaller_output() {
        let small = Rect::new(1920, 0, 1280, 1024);
        assert_eq!(
            carry(Rect::new(1000, 800, 800, 600), FHD, small),
            (2400, 424)
        );
    }
}
