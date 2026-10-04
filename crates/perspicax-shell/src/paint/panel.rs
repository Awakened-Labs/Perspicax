//! A panel: a dark bar along the edge of a monitor, with a rule where it
//! meets the desktop. The start button is a grid of four squares, drawn in
//! the highlight's shade while its menu is open; the clock is the time, in
//! the panel's ink. Every pixel of it is opaque.

use perspicax_config::{Edge, Item};
use tiny_skia::PixmapMut;

use super::{fill, scaled, text::Text};
use crate::layout::{Rect, TEXT, panel::CLOCK_PAD};

/// The panel's colours, as premultiplied RGBA. Dark, after Breeze's.
pub(crate) const BAR: [u8; 4] = [0x23, 0x26, 0x29, 0xff];
pub(crate) const INK: [u8; 4] = [0xfc, 0xfc, 0xfc, 0xff];
/// The rule along the edge it shares with the desktop.
pub(crate) const RULE: [u8; 4] = [0x3b, 0x40, 0x45, 0xff];
/// Behind the start button while the start menu is open: the highlight,
/// faint over the bar.
pub(crate) const OPEN: [u8; 4] = [0x2b, 0x4f, 0x63, 0xff];

/// One square of the start button's grid, and the room between them.
const SQUARE: i32 = 7;
const BETWEEN: i32 = 3;

/// What a panel shows.
pub(crate) struct Shown<'a> {
    /// The panel's size, in its own logical pixels.
    pub(crate) size: (i32, i32),
    pub(crate) edge: Edge,
    pub(crate) placed: &'a [(Item, Rect)],
    pub(crate) time: &'a str,
    /// The start menu is open from this panel's button.
    pub(crate) open: bool,
}

/// Draw `shown` on `canvas`, a surface's pixels at `scale` times its size.
pub(crate) fn paint(shown: &Shown<'_>, canvas: &mut PixmapMut<'_>, scale: u32, text: &mut Text) {
    let px = |rect: Rect| scaled(rect, scale);
    let (width, height) = shown.size;
    fill(canvas, px(Rect::new(0, 0, width, height)), BAR);
    let rule = match shown.edge {
        Edge::Bottom => Rect::new(0, 0, width, 1),
        Edge::Top => Rect::new(0, height - 1, width, 1),
    };
    for &(item, place) in shown.placed {
        match item {
            Item::Start => {
                if shown.open {
                    fill(canvas, px(place), OPEN);
                }
                grid(canvas, px(place), scale as i32);
            }
            Item::Clock => {
                let words = Rect::new(
                    place.x + CLOCK_PAD,
                    place.y,
                    place.w - 2 * CLOCK_PAD,
                    place.h,
                );
                text.write(canvas, shown.time, px(words), TEXT * scale as f32, INK);
            }
            Item::Taskbar | Item::Pager | Item::Tray => {}
        }
    }
    fill(canvas, px(rule), RULE);
}

/// Four squares, two by two, centred in `place`, `s` pixels to a logical
/// one.
fn grid(canvas: &mut PixmapMut<'_>, place: Rect, s: i32) {
    let (square, between) = (SQUARE * s, BETWEEN * s);
    let side = 2 * square + between;
    let (left, top) = (
        place.x + (place.w - side) / 2,
        place.y + (place.h - side) / 2,
    );
    for (column, row) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let at = Rect::new(
            left + column * (square + between),
            top + row * (square + between),
            square,
            square,
        );
        fill(canvas, at, INK);
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::layout::{Monospace, panel::lay_out};

    fn painted(open: bool, scale: u32) -> (Pixmap, Vec<(Item, Rect)>) {
        let placed = lay_out(
            &[Item::Start, Item::Taskbar, Item::Clock],
            "14:05",
            (400, 40),
            &mut Monospace(8.0),
        );
        let mut picture = Pixmap::new(400 * scale, 40 * scale).expect("a picture");
        paint(
            &Shown {
                size: (400, 40),
                edge: Edge::Bottom,
                placed: &placed,
                time: "14:05",
                open,
            },
            &mut picture.as_mut(),
            scale,
            &mut Text::without_fonts(),
        );
        (picture, placed)
    }

    fn pixel(picture: &Pixmap, x: i32, y: i32) -> [u8; 4] {
        let at = ((y * picture.width() as i32 + x) * 4) as usize;
        picture.data()[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn a_panel_is_opaque_with_a_rule_along_the_desktop() {
        for scale in [1, 2] {
            let (picture, _) = painted(false, scale);
            assert!(
                picture.data().chunks_exact(4).all(|pixel| pixel[3] == 0xff),
                "every pixel opaque, at {scale}x"
            );
            let s = scale as i32;
            assert_eq!(
                pixel(&picture, 200 * s, 0),
                RULE,
                "the top row, at {scale}x"
            );
            assert_eq!(pixel(&picture, 200 * s, 20 * s), BAR);
        }
    }

    #[test]
    fn the_start_button_is_a_grid_lit_while_its_menu_is_open() {
        let (closed, placed) = painted(false, 1);
        let button = placed[0].1;
        let middle = (button.x + button.w / 2, button.y + button.h / 2);
        // The middle is between the squares; a square's middle is off it.
        let square = (
            middle.0 - BETWEEN / 2 - SQUARE / 2 - 1,
            middle.1 - BETWEEN / 2 - SQUARE / 2 - 1,
        );
        assert_eq!(pixel(&closed, middle.0, middle.1), BAR);
        assert_eq!(pixel(&closed, square.0, square.1), INK);
        assert_eq!(pixel(&closed, button.x + 2, button.y + 2), BAR);

        let (open, _) = painted(true, 1);
        assert_eq!(pixel(&open, button.x + 2, button.y + 2), OPEN);
        assert_eq!(pixel(&open, square.0, square.1), INK);
    }
}
