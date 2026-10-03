//! A cascade of menus: the root menu where it was asked for, and each open
//! submenu beside the item that opened it.
//!
//! A menu opens down and to the right of the point it was asked for, and
//! flips at the edge of the monitor: leftwards near the right edge, upwards
//! near the bottom. A submenu opens to the right of its parent, or to the
//! left where there is no room, and the submenus below it keep going the way
//! it went. A submenu that would run off the bottom slides up until it fits.
//! A menu too tall for the monitor breaks into columns.

use super::{Measure, Rect};

/// One line's height.
pub(crate) const ROW: i32 = 30;
/// A separator's height: a line with room either side.
pub(crate) const SEPARATOR: i32 = 9;
/// Room above the first line and below the last, inside the border.
pub(crate) const PAD: i32 = 4;
/// The border round a menu.
pub(crate) const BORDER: i32 = 1;
/// Room left of the icon and right of the arrow.
pub(crate) const INSET: i32 = 8;
/// An icon's size.
pub(crate) const ICON: i32 = 22;
/// Room between the icon and the label, and between the label and the arrow.
pub(crate) const GAP: i32 = 8;
/// Room for the arrow that marks a submenu.
pub(crate) const ARROW: i32 = 12;
/// The narrowest and widest a menu is drawn. A longer label is cut short.
pub(crate) const MIN_WIDTH: i32 = 180;
pub(crate) const MAX_WIDTH: i32 = 420;
/// The text's size.
pub(crate) const TEXT: f32 = 14.0;

/// One menu of a cascade, as it is to be laid out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shown<'a> {
    /// What a person has typed, shown on a line of its own above the rest.
    pub(crate) header: Option<&'a str>,
    pub(crate) lines: Vec<Line<'a>>,
    /// The line of the menu before it that opened it; `None` for the first.
    pub(crate) from: Option<usize>,
}

/// One line of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Line<'a> {
    Item { label: &'a str, submenu: bool },
    Separator,
}

/// One menu of a cascade, laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placed {
    /// The menu, border and all.
    pub(crate) rect: Rect,
    /// The typed line, if there is one.
    pub(crate) header: Option<Rect>,
    /// Each line's rectangle, in order.
    pub(crate) rows: Vec<Rect>,
    /// Whether it opened leftwards of its parent.
    pub(crate) leftwards: bool,
}

/// Lay out `menus`, the first opened at `anchor` and each other beside the
/// line of the one before that opened it, all inside `area`.
pub(crate) fn cascade(
    menus: &[Shown<'_>],
    anchor: (i32, i32),
    area: Rect,
    measure: &mut impl Measure,
) -> Vec<Placed> {
    let mut placed: Vec<Placed> = Vec::with_capacity(menus.len());
    for menu in menus {
        let (columns, width, height) = size(menu, area, measure);
        let (x, y, leftwards) = match (menu.from, placed.last()) {
            (Some(line), Some(parent)) => {
                let row = parent.rows.get(line).copied().unwrap_or(parent.rect);
                beside(parent, row, (width, height), area)
            }
            _ => at_point(anchor, (width, height), area),
        };
        let rect = Rect::new(x, y, width, height);
        let (header, rows) = lines(menu, rect, &columns);
        placed.push(Placed {
            rect,
            header,
            rows,
            leftwards,
        });
    }
    placed
}

/// Where a menu of `size` goes when opened at `point`: down and to the
/// right of it, flipped to fit. A menu that fits neither way is put against
/// the edge it would cross.
fn at_point(point: (i32, i32), (width, height): (i32, i32), area: Rect) -> (i32, i32, bool) {
    let leftwards = point.0 + width > area.right();
    let x = if leftwards { point.0 - width } else { point.0 };
    let y = if point.1 + height <= area.bottom() {
        point.1
    } else if point.1 - height >= area.y {
        point.1 - height
    } else {
        area.bottom() - height
    };
    (
        x.clamp(area.x, (area.right() - width).max(area.x)),
        y.max(area.y),
        leftwards,
    )
}

/// Where a submenu of `size` goes beside `row` of `parent`: its first line
/// level with the row, on the side the parent opened to while there is
/// room, slid up to fit above the bottom.
fn beside(parent: &Placed, row: Rect, (width, height): (i32, i32), area: Rect) -> (i32, i32, bool) {
    let right = parent.rect.right();
    let left = parent.rect.x - width;
    let fits_right = right + width <= area.right();
    let fits_left = left >= area.x;
    let leftwards = if parent.leftwards {
        fits_left || !fits_right
    } else {
        !fits_right && fits_left
    };
    let x = if leftwards { left } else { right };
    let y = (row.y - PAD - BORDER)
        .min(area.bottom() - height)
        .max(area.y);
    (
        x.clamp(area.x, (area.right() - width).max(area.x)),
        y,
        leftwards,
    )
}

/// How a menu's lines break into columns, each a run of line indices, and
/// how big the menu is: a column's width each, and its tallest column's
/// height.
fn size(
    menu: &Shown<'_>,
    area: Rect,
    measure: &mut impl Measure,
) -> (Vec<(usize, usize)>, i32, i32) {
    let submenus = menu
        .lines
        .iter()
        .any(|line| matches!(line, Line::Item { submenu: true, .. }));
    let arrow = if submenus { GAP + ARROW } else { 0 };
    let widest = menu
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::Item { label, .. } => Some(*label),
            Line::Separator => None,
        })
        .chain(menu.header)
        .map(|text| measure.width(text))
        .fold(0.0_f32, f32::max);
    let column =
        (INSET + ICON + GAP + widest.ceil() as i32 + arrow + INSET).clamp(MIN_WIDTH, MAX_WIDTH);

    let header = if menu.header.is_some() { ROW } else { 0 };
    let room = (area.h - 2 * (PAD + BORDER) - header).max(ROW);
    let mut columns = Vec::new();
    let (mut start, mut filled, mut tallest) = (0, 0, 0);
    for (at, line) in menu.lines.iter().enumerate() {
        let height = height(*line);
        if filled + height > room && at > start {
            columns.push((start, at));
            tallest = tallest.max(filled);
            (start, filled) = (at, 0);
        }
        filled += height;
    }
    columns.push((start, menu.lines.len()));
    tallest = tallest.max(filled);
    let width = column * columns.len() as i32 + 2 * BORDER;
    (columns, width, header + tallest + 2 * (PAD + BORDER))
}

fn height(line: Line<'_>) -> i32 {
    match line {
        Line::Item { .. } => ROW,
        Line::Separator => SEPARATOR,
    }
}

/// The typed line's rectangle and each line's, for a menu at `rect` broken
/// into `columns`.
fn lines(menu: &Shown<'_>, rect: Rect, columns: &[(usize, usize)]) -> (Option<Rect>, Vec<Rect>) {
    let inner = Rect::new(
        rect.x + BORDER,
        rect.y + BORDER + PAD,
        rect.w - 2 * BORDER,
        rect.h - 2 * (BORDER + PAD),
    );
    let header = menu
        .header
        .map(|_| Rect::new(inner.x, inner.y, inner.w, ROW));
    let top = inner.y + header.map_or(0, |header| header.h);
    let width = inner.w / columns.len().max(1) as i32;
    let mut rows = Vec::with_capacity(menu.lines.len());
    for (n, &(start, end)) in columns.iter().enumerate() {
        let x = inner.x + width * n as i32;
        let mut y = top;
        for line in &menu.lines[start..end] {
            let height = height(*line);
            rows.push(Rect::new(x, y, width, height));
            y += height;
        }
    }
    (header, rows)
}

/// How many lines a menu with a typed line above them shows in one column
/// in `area`: no more are shown of what typing finds, so a first letter
/// does not fill the monitor.
pub(crate) fn found_fit(area: Rect) -> usize {
    ((area.h - 2 * (PAD + BORDER) - ROW) / ROW).max(1) as usize
}

/// The menu and line at `point`, if it is on a line.
pub(crate) fn line_at(placed: &[Placed], point: (f64, f64)) -> Option<(usize, usize)> {
    placed.iter().enumerate().rev().find_map(|(level, menu)| {
        menu.rect
            .contains(point)
            .then(|| menu.rows.iter().position(|row| row.contains(point)))
            .flatten()
            .map(|line| (level, line))
    })
}

/// Whether `point` is on any of the menus.
pub(crate) fn on_menus(placed: &[Placed], point: (f64, f64)) -> bool {
    placed.iter().any(|menu| menu.rect.contains(point))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Monospace;

    const SCREEN: Rect = Rect::new(0, 0, 1280, 800);

    fn items(labels: &[&'static str]) -> Vec<Line<'static>> {
        labels
            .iter()
            .map(|&label| Line::Item {
                label,
                submenu: label.ends_with('>'),
            })
            .collect()
    }

    fn root(labels: &[&'static str]) -> Shown<'static> {
        Shown {
            header: None,
            lines: items(labels),
            from: None,
        }
    }

    fn sub(from: usize, count: usize) -> Shown<'static> {
        Shown {
            header: None,
            lines: vec![
                Line::Item {
                    label: "item",
                    submenu: false
                };
                count
            ],
            from: Some(from),
        }
    }

    fn laid(menus: &[Shown<'_>], anchor: (i32, i32)) -> Vec<Placed> {
        cascade(menus, anchor, SCREEN, &mut Monospace(8.0))
    }

    #[test]
    fn a_menu_opens_below_and_right_of_the_point() {
        let menus = laid(&[root(&["Accessories >", "Internet >"])], (100, 50));
        let menu = &menus[0];
        assert_eq!((menu.rect.x, menu.rect.y), (100, 50));
        assert_eq!(menu.rect.h, 2 * ROW + 2 * (PAD + BORDER));
        assert_eq!(menu.rows[0], Rect::new(101, 55, menu.rect.w - 2, ROW));
        assert_eq!(menu.rows[1].y, 55 + ROW);
        assert!(menu.rect.w >= MIN_WIDTH);
    }

    #[test]
    fn a_menu_near_the_right_edge_opens_leftwards() {
        let menus = laid(&[root(&["Accessories >"]), sub(0, 3)], (1250, 50));
        assert_eq!(menus[0].rect.right(), 1250, "its right edge at the point");
        assert!(menus[0].leftwards);
        assert_eq!(
            menus[1].rect.right(),
            menus[0].rect.x,
            "and its submenu opens to its left"
        );
    }

    #[test]
    fn a_menu_near_the_bottom_opens_upwards() {
        let menus = laid(&[root(&["a", "b", "c"])], (100, 790));
        assert_eq!(menus[0].rect.bottom(), 790);
    }

    #[test]
    fn a_submenu_that_would_run_off_the_bottom_slides_up() {
        let menus = laid(&[root(&["a", "b", "Games >"]), sub(2, 10)], (100, 600));
        let submenu = menus[1].rect;
        assert_eq!(submenu.bottom(), SCREEN.bottom(), "against the bottom");
        assert!(
            submenu.y < menus[0].rows[2].y,
            "above the line that opened it"
        );
        assert_eq!(submenu.x, menus[0].rect.right(), "still to the right");

        let short = laid(&[root(&["a", "b", "Games >"]), sub(2, 2)], (100, 100));
        assert_eq!(
            short[1].rows[0].y, short[0].rows[2].y,
            "with room, its first line is level with the line that opened it"
        );
    }

    #[test]
    fn a_menu_taller_than_the_monitor_breaks_into_columns() {
        let short_screen = Rect::new(0, 0, 1280, 6 * ROW + 2 * (PAD + BORDER));
        let long = Shown {
            from: None,
            ..sub(0, 14)
        };
        let menus = cascade(&[long], (0, 0), short_screen, &mut Monospace(8.0));
        let menu = &menus[0];
        assert_eq!(menu.rect.h, short_screen.h);
        assert_eq!(menu.rect.w, 3 * MIN_WIDTH + 2 * BORDER, "three columns");
        assert_eq!(menu.rows[6].x, menu.rows[0].x + MIN_WIDTH);
        assert_eq!(menu.rows[6].y, menu.rows[0].y);
    }

    #[test]
    fn a_long_label_widens_the_menu_up_to_a_point() {
        let label = "a label of forty characters, give or take";
        let wide = laid(&[root(&[label])], (0, 0));
        assert_eq!(
            wide[0].rect.w,
            INSET + ICON + GAP + 8 * label.len() as i32 + INSET + 2 * BORDER
        );
        let long: &'static str = "x".repeat(200).leak();
        let widest = laid(&[root(&[long])], (0, 0));
        assert_eq!(widest[0].rect.w, MAX_WIDTH + 2 * BORDER);
    }

    #[test]
    fn a_point_finds_the_line_under_it_on_the_topmost_menu() {
        let menus = laid(&[root(&["a", "Games >"]), sub(1, 3)], (100, 100));
        let middle = |rect: Rect| {
            (
                f64::from(rect.x + rect.w / 2),
                f64::from(rect.y + rect.h / 2),
            )
        };
        assert_eq!(line_at(&menus, middle(menus[0].rows[1])), Some((0, 1)));
        assert_eq!(line_at(&menus, middle(menus[1].rows[2])), Some((1, 2)));
        assert_eq!(line_at(&menus, (101.0, 101.0)), None, "in the padding");
        assert!(on_menus(&menus, (101.0, 101.0)));
        assert!(!on_menus(&menus, (10.0, 10.0)));
    }
}
