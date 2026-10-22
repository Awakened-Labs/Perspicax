//! A panel's tooltip: how big it is, and where it goes beside the bar.
//!
//! A tip is one line of words in a box [`TALL`] high, as wide as its words
//! with room either side, and no wider than the monitor less a [`MARGIN`]
//! each side: a title wider than that is cut short. It stands [`GAP`] off
//! the bar, above a panel along the bottom and below one along the top,
//! centred on the part it is for and kept inside the monitor.
//!
//! It is worked out in the panel surface's own coordinates, as everything
//! else on the bar is. The panel is the whole width of its monitor, flush
//! with its edge, so those are the monitor's own but for how far the panel
//! is from the monitor's top, which only the edge's margin needs.

use perspicax_config::Edge;

use super::{Measure, Rect};

/// How tall a tip is: room for the tallest font a theme may ask for.
pub(crate) const TALL: i32 = 28;
/// Its edge, and the room between that and its words, either side.
pub(crate) const BORDER: i32 = 1;
const PAD: i32 = 8;
/// How far it stands off the bar, and in from the monitor's sides.
pub(crate) const GAP: i32 = 4;
pub(crate) const MARGIN: i32 = 4;

/// Where the tip saying `words` goes, for the part at `part` of a panel
/// `size` big along `edge`, in the panel's own coordinates.
pub(crate) fn place(
    words: &str,
    part: Rect,
    (width, height): (i32, i32),
    edge: Edge,
    measure: &mut impl Measure,
) -> Rect {
    // A pixel or two over what the words measure, so that rounding never
    // cuts the last letter short.
    let wanted = measure.width(words).ceil() as i32 + 2 + 2 * (BORDER + PAD);
    let wide = wanted.min(width - 2 * MARGIN).max(2 * (BORDER + PAD));
    let x = (part.x + part.w / 2 - wide / 2).clamp(MARGIN, (width - MARGIN - wide).max(MARGIN));
    let y = match edge {
        Edge::Bottom => -GAP - TALL,
        Edge::Top => height + GAP,
    };
    Rect::new(x, y, wide, TALL)
}

/// How far `tip`, placed beside a panel `height` high along `edge`, is from
/// its monitor's left side and from the edge the panel is along: the margins
/// of a layer surface anchored there.
pub(crate) fn margins(tip: Rect, height: i32, edge: Edge) -> (i32, i32) {
    match edge {
        Edge::Bottom => (tip.x, height - tip.bottom()),
        Edge::Top => (tip.x, tip.y),
    }
}

/// Where a tip `size` big writes its words, in its own coordinates.
pub(crate) fn words((width, height): (i32, i32)) -> Rect {
    let inset = BORDER + PAD;
    Rect::new(
        inset,
        BORDER,
        (width - 2 * inset).max(0),
        (height - 2 * BORDER).max(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Monospace;

    const PANEL: (i32, i32) = (1280, 40);
    /// A task 36 pixels wide, beside the start button.
    const TASK: Rect = Rect::new(40, 0, 36, 40);

    fn placed(words: &str, part: Rect, edge: Edge) -> Rect {
        place(words, part, PANEL, edge, &mut Monospace(8.0))
    }

    #[test]
    fn a_tip_stands_above_a_bottom_panel_and_below_a_top_one() {
        let above = placed("Terminal", TASK, Edge::Bottom);
        assert_eq!((above.y, above.bottom()), (-GAP - TALL, -GAP));
        let below = placed("Terminal", TASK, Edge::Top);
        assert_eq!(below.y, 40 + GAP);
        assert_eq!(
            (above.x, above.w),
            (below.x, below.w),
            "only the side differs"
        );
    }

    #[test]
    fn a_tip_is_centred_on_its_part_and_kept_inside_the_monitor() {
        let tip = placed("Terminal", Rect::new(600, 0, 36, 40), Edge::Bottom);
        assert_eq!(tip.x + tip.w / 2, 618, "centred");
        let left = placed("A rather long title", TASK, Edge::Bottom);
        assert_eq!(left.x, MARGIN, "not past the left side");
        let right = placed(
            "A rather long title",
            Rect::new(1250, 0, 30, 40),
            Edge::Bottom,
        );
        assert_eq!(right.right(), 1280 - MARGIN, "nor past the right");
    }

    #[test]
    fn a_tip_is_as_wide_as_its_words_and_a_title_wider_than_the_monitor_is_cut_to_it() {
        let tip = placed("Terminal", TASK, Edge::Bottom);
        assert_eq!(tip.w, 8 * 8 + 2 + 2 * (BORDER + PAD));
        assert!(words((tip.w, tip.h)).w >= 8 * 8, "room for every letter");
        let long = "x".repeat(400);
        let cut = placed(&long, Rect::new(600, 0, 36, 40), Edge::Bottom);
        assert_eq!((cut.x, cut.w), (MARGIN, 1280 - 2 * MARGIN));
        let none = placed("", TASK, Edge::Bottom);
        assert!(none.w >= 2 * (BORDER + PAD), "{none:?}");
    }

    #[test]
    fn the_margins_put_it_where_it_was_placed() {
        let above = placed("Terminal", TASK, Edge::Bottom);
        assert_eq!(
            margins(above, 40, Edge::Bottom),
            (above.x, 40 + GAP),
            "its foot that far above the monitor's"
        );
        let below = placed("Terminal", TASK, Edge::Top);
        assert_eq!(margins(below, 40, Edge::Top), (below.x, 40 + GAP));
    }

    #[test]
    fn its_words_sit_inside_its_edge_and_padding() {
        assert_eq!(
            words((100, TALL)),
            Rect::new(BORDER + PAD, BORDER, 100 - 2 * (BORDER + PAD), TALL - 2)
        );
        assert_eq!(words((4, TALL)).w, 0, "never less than none");
    }
}
