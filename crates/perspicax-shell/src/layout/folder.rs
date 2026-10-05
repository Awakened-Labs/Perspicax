//! Where the desktop folder's icons go: in a grid from the top-left corner
//! of what the shell's panel leaves of the monitor, filling each column
//! from the top before starting the next, as a file manager's desktop does.
//! Icons past the right edge are not shown.

use super::{Measure, Rect};

/// Each icon's cell, its picture, and its one line of name below it.
pub(crate) const CELL: (i32, i32) = (96, 88);
pub(crate) const ICON: i32 = 48;
/// The room kept between the grid and the edge of what it is laid in.
const MARGIN: i32 = 8;
/// The picture's distance from the top of its cell, and the name's line.
const ABOVE: i32 = 6;
const LINE: i32 = 22;
/// The room either side of a name.
const BESIDE: i32 = 4;
/// The highlight of a selected icon, and where a click finds it: its cell,
/// less this much all round, which leaves a gap between one and the next.
const INSET: i32 = 2;

/// Where one icon is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Spot {
    /// What is highlighted when it is selected, and what a click finds.
    pub(crate) place: Rect,
    pub(crate) picture: Rect,
    /// Its name's line: centred, as wide as the name or the cell allows.
    pub(crate) label: Rect,
}

/// Where each of `names` goes inside `area`, in order, as far as they fit.
pub(crate) fn lay_out<'a>(
    names: impl IntoIterator<Item = &'a str>,
    area: Rect,
    measure: &mut impl Measure,
) -> Vec<Spot> {
    let (width, height) = CELL;
    let rows = ((area.h - 2 * MARGIN) / height).max(1);
    let columns = (area.w - 2 * MARGIN) / width;
    names
        .into_iter()
        .take((rows * columns).max(0) as usize)
        .enumerate()
        .map(|(at, name)| {
            let at = at as i32;
            let cell = Rect::new(
                area.x + MARGIN + at / rows * width,
                area.y + MARGIN + at % rows * height,
                width,
                height,
            );
            let picture = Rect::new(cell.x + (width - ICON) / 2, cell.y + ABOVE, ICON, ICON);
            // A pixel to spare, so a name that just fits is not cut short.
            let wide = (measure.width(name).ceil() as i32 + 1).min(width - 2 * BESIDE);
            let label = Rect::new(
                cell.x + (width - wide) / 2,
                picture.bottom() + 4,
                wide,
                LINE,
            );
            Spot {
                place: Rect::new(
                    cell.x + INSET,
                    cell.y + INSET,
                    width - 2 * INSET,
                    height - 2 * INSET,
                ),
                picture,
                label,
            }
        })
        .collect()
}

/// Which of `spots` is at `point`, if any.
pub(crate) fn at(spots: &[Spot], point: (f64, f64)) -> Option<usize> {
    spots.iter().position(|spot| spot.place.contains(point))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Monospace, usable};

    fn names(count: usize) -> Vec<String> {
        (0..count).map(|at| format!("file {at}")).collect()
    }

    fn laid(count: usize, area: Rect) -> Vec<Spot> {
        let names = names(count);
        lay_out(names.iter().map(String::as_str), area, &mut Monospace(7.0))
    }

    #[test]
    fn icons_fill_columns_and_avoid_the_panel() {
        let monitor = Rect::new(0, 0, 1280, 800);
        let bottom = usable(monitor, Some(Rect::new(0, 760, 1280, 40)));
        let spots = laid(10, bottom);
        // (760 - 16) / 88: eight rows above a 40-pixel panel.
        let tops: Vec<_> = spots
            .iter()
            .map(|spot| (spot.place.x, spot.place.y))
            .collect();
        assert_eq!(&tops[..2], [(10, 10), (10, 98)], "down the first column");
        assert_eq!(tops[7], (10, 626), "to its eighth row");
        assert_eq!(tops[8], (106, 10), "then the top of the next");
        assert!(
            spots.iter().all(|spot| spot.place.bottom() <= 760),
            "all of them clear of the panel"
        );

        let top = usable(monitor, Some(Rect::new(0, 0, 1280, 32)));
        assert_eq!(
            laid(1, top)[0].place.y,
            32 + 8 + 2,
            "below a panel along the top"
        );
    }

    #[test]
    fn icons_past_the_edge_are_not_shown() {
        let small = Rect::new(0, 0, 200, 200);
        // Two rows of one column: (200 - 16) / 88 and (200 - 16) / 96.
        assert_eq!(laid(5, small).len(), 2);
        assert!(
            laid(5, Rect::new(0, 0, 50, 50)).is_empty(),
            "nor in no room"
        );
    }

    #[test]
    fn a_name_is_centred_under_its_picture_and_cut_to_its_cell() {
        let area = Rect::new(0, 0, 400, 400);
        let short = lay_out(["a.txt"], area, &mut Monospace(7.0))[0];
        assert_eq!(short.picture, Rect::new(32, 14, 48, 48));
        assert_eq!(short.label.w, 36, "five letters of seven, and a pixel");
        assert_eq!(
            short.label.x + short.label.w / 2,
            short.picture.x + ICON / 2,
            "centred under it"
        );
        let long = lay_out(["a very long name indeed"], area, &mut Monospace(7.0))[0];
        assert_eq!(
            long.label.w,
            CELL.0 - 2 * BESIDE,
            "as wide as the cell allows"
        );
    }

    #[test]
    fn a_click_finds_the_icon_under_it_and_not_the_gap_between() {
        let spots = laid(2, Rect::new(0, 0, 400, 400));
        assert_eq!(at(&spots, (50.0, 50.0)), Some(0));
        assert_eq!(at(&spots, (50.0, 140.0)), Some(1));
        assert_eq!(at(&spots, (50.0, 96.5)), None, "between the two");
        assert_eq!(at(&spots, (300.0, 50.0)), None, "nor off them");
    }
}
