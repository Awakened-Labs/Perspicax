//! A panel: which monitors have one, the strip of the monitor it takes, and
//! where on it each thing it holds sits.
//!
//! A panel runs the whole width of its monitor, along the top or the
//! bottom. The start button is a square as tall as the panel, and the clock
//! is as wide as the time it shows. The taskbar takes whatever room is left;
//! a panel without one keeps its last item at the right end, and the rest at
//! the left.

use perspicax_config::{Edge, Item, PanelOutputs};

use super::{Measure, Rect};

/// Room either side of the clock's time.
pub(crate) const CLOCK_PAD: i32 = 10;

/// Which of `monitors`, each a connector name and where its top-left corner
/// is on the desk, have a panel by `rule`, in the order given. The first
/// monitor is the leftmost, the topmost of those level with it.
pub(crate) fn chosen<'a>(rule: &PanelOutputs, monitors: &[(&'a str, (i32, i32))]) -> Vec<&'a str> {
    match rule {
        PanelOutputs::All => monitors.iter().map(|&(name, _)| name).collect(),
        PanelOutputs::First => monitors
            .iter()
            .min_by_key(|&&(_, at)| at)
            .map(|&(name, _)| name)
            .into_iter()
            .collect(),
        PanelOutputs::Named(names) => monitors
            .iter()
            .map(|&(name, _)| name)
            .filter(|name| names.iter().any(|named| named == name))
            .collect(),
    }
}

/// The strip of `monitor` a panel `height` high along `edge` takes.
#[cfg_attr(
    not(any(feature = "menus", test)),
    expect(
        dead_code,
        reason = "where menus keep off; no menus, nothing to keep off it"
    )
)]
pub(crate) fn strip(edge: Edge, height: i32, monitor: Rect) -> Rect {
    let height = height.min(monitor.h);
    match edge {
        Edge::Top => Rect::new(monitor.x, monitor.y, monitor.w, height),
        Edge::Bottom => Rect::new(monitor.x, monitor.bottom() - height, monitor.w, height),
    }
}

/// Where each of `items` sits on a panel `size` big, in its own logical
/// pixels, with the clock showing `time`.
pub(crate) fn lay_out(
    items: &[Item],
    time: &str,
    (width, height): (i32, i32),
    measure: &mut impl Measure,
) -> Vec<(Item, Rect)> {
    let widths: Vec<i32> = items
        .iter()
        .map(|item| match item {
            Item::Start => height,
            Item::Clock => measure.width(time).ceil() as i32 + 2 * CLOCK_PAD,
            // The taskbar's width is what the others leave.
            Item::Taskbar => 0,
            // Nothing to draw, so no room taken.
            Item::Pager | Item::Tray => 0,
        })
        .collect();
    let room = (width - widths.iter().sum::<i32>()).max(0);
    let stretch = items
        .iter()
        .position(|&item| item == Item::Taskbar)
        .unwrap_or(items.len().saturating_sub(1));
    let mut x = 0;
    items
        .iter()
        .zip(widths)
        .enumerate()
        .map(|(at, (&item, mut w))| {
            if at == stretch {
                match item {
                    Item::Taskbar => w = room,
                    _ => x += room,
                }
            }
            let rect = Rect::new(x, 0, w, height);
            x += w;
            (item, rect)
        })
        .collect()
}

/// What on a laid-out panel is at `point`, if anything is.
#[cfg_attr(
    not(any(feature = "menus", test)),
    expect(
        dead_code,
        reason = "a start button to press; no menus, nothing to open"
    )
)]
pub(crate) fn item_at(placed: &[(Item, Rect)], point: (f64, f64)) -> Option<(Item, Rect)> {
    placed
        .iter()
        .copied()
        .find(|(_, rect)| rect.contains(point))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Monospace;

    const PANEL: (i32, i32) = (1280, 40);

    fn laid(items: &[Item]) -> Vec<(Item, Rect)> {
        lay_out(items, "14:05", PANEL, &mut Monospace(8.0))
    }

    #[test]
    fn the_start_button_sits_first_and_the_clock_last() {
        let placed = laid(&[Item::Start, Item::Taskbar, Item::Pager, Item::Clock]);
        let clock = 5 * 8 + 2 * CLOCK_PAD;
        assert_eq!(placed[0], (Item::Start, Rect::new(0, 0, 40, 40)), "square");
        assert_eq!(
            placed[1],
            (Item::Taskbar, Rect::new(40, 0, 1280 - 40 - clock, 40)),
            "the room between"
        );
        assert_eq!(
            placed[3],
            (Item::Clock, Rect::new(1280 - clock, 0, clock, 40)),
            "against the right end"
        );
    }

    #[test]
    fn without_a_taskbar_the_last_item_keeps_the_right_end() {
        let placed = laid(&[Item::Start, Item::Clock]);
        assert_eq!(placed[0].1.x, 0);
        assert_eq!(placed[1].1.right(), 1280);

        let placed = laid(&[Item::Clock, Item::Start]);
        assert_eq!(placed[0].1.x, 0, "the clock first, at the left");
        assert_eq!(placed[1].1, Rect::new(1240, 0, 40, 40));
    }

    #[test]
    fn a_point_finds_the_item_under_it() {
        let placed = laid(&[Item::Start, Item::Taskbar, Item::Clock]);
        assert_eq!(
            item_at(&placed, (20.0, 20.0)).map(|(item, _)| item),
            Some(Item::Start)
        );
        assert_eq!(
            item_at(&placed, (1279.0, 1.0)).map(|(item, _)| item),
            Some(Item::Clock)
        );
        assert_eq!(item_at(&placed, (1300.0, 20.0)), None);
    }

    #[test]
    fn outputs_first_puts_one_panel_on_the_first_monitor() {
        let monitors = [
            ("HDMI-A-1", (1920, 0)),
            ("DP-1", (0, 0)),
            ("DP-2", (0, 1080)),
        ];
        assert_eq!(
            chosen(&PanelOutputs::First, &monitors),
            ["DP-1"],
            "the leftmost, and the topmost of those"
        );
        assert_eq!(
            chosen(&PanelOutputs::All, &monitors),
            ["HDMI-A-1", "DP-1", "DP-2"]
        );
        assert_eq!(
            chosen(
                &PanelOutputs::Named(vec!["DP-2".to_owned(), "DP-9".to_owned()]),
                &monitors
            ),
            ["DP-2"],
            "a name with no monitor is no panel"
        );
        assert!(chosen(&PanelOutputs::First, &[]).is_empty());
    }

    #[test]
    fn a_panel_takes_a_strip_along_its_edge() {
        let monitor = Rect::new(0, 0, 1280, 800);
        assert_eq!(
            strip(Edge::Bottom, 40, monitor),
            Rect::new(0, 760, 1280, 40)
        );
        assert_eq!(strip(Edge::Top, 32, monitor), Rect::new(0, 0, 1280, 32));
    }
}
