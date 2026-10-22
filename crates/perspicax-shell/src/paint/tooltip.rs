//! A panel's tooltip: its words on a box in the menus' colours, with an
//! edge. Every pixel is opaque.

use perspicax_config::{Palette, Role};
use tiny_skia::PixmapMut;

use super::{colour, fill, scaled, text::Text};
use crate::layout::{Rect, tooltip::BORDER, tooltip::words as words_in};

/// Draw a tip `size` big saying `words` on `canvas`, at `scale`.
pub(crate) fn paint(
    words: &str,
    (width, height): (i32, i32),
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    palette: &Palette,
) {
    let px = |rect: Rect| scaled(rect, scale);
    fill(
        canvas,
        px(Rect::new(0, 0, width, height)),
        colour(palette, Role::MenuEdge),
    );
    let within = Rect::new(BORDER, BORDER, width - 2 * BORDER, height - 2 * BORDER);
    fill(canvas, px(within), colour(palette, Role::Menu));
    let size = text.size() * scale as f32;
    text.write(
        canvas,
        words,
        px(words_in((width, height))),
        size,
        colour(palette, Role::MenuInk),
    );
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::layout::tooltip::TALL;

    fn pixel(picture: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * picture.width() + x) * 4) as usize;
        picture.data()[at..at + 4].try_into().expect("four bytes")
    }

    #[test]
    fn a_tip_is_opaque_in_the_menus_colours_with_an_edge() {
        let palette = Palette::default();
        let (edge, inside) = (
            colour(&palette, Role::MenuEdge),
            colour(&palette, Role::Menu),
        );
        for scale in [1, 2] {
            let (wide, tall) = (80 * scale, TALL as u32 * scale);
            let mut picture = Pixmap::new(wide, tall).expect("a picture");
            paint(
                "Terminal",
                (80, TALL),
                &mut picture.as_mut(),
                scale,
                &mut Text::without_fonts(),
                &palette,
            );
            for corner in [(0, 0), (wide - 1, 0), (0, tall - 1), (wide - 1, tall - 1)] {
                assert_eq!(
                    pixel(&picture, corner.0, corner.1),
                    edge,
                    "{scale}: {corner:?}"
                );
            }
            assert_eq!(pixel(&picture, scale - 1, scale - 1), edge, "{scale}: edge");
            assert_eq!(pixel(&picture, scale, scale), inside, "{scale}: inside");
            assert_eq!(
                pixel(&picture, wide / 2, tall / 2),
                inside,
                "{scale}: no fonts, no words"
            );
        }
    }
}
