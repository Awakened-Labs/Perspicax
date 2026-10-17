//! The volume shown on the screen: a box in the menus' colours, with an
//! edge, of the speaker as the panel draws it, the output's name, the
//! volume in percent, and a track lit in the accent as far as it, past the
//! full width for none. Muted, the bar is faint. Every pixel is opaque.

use perspicax_config::{Palette, Role};
use tiny_skia::PixmapMut;

use super::{colour, fill, scaled, text::Text};
use crate::layout::{
    Measure as _, Rect,
    osd::{Indicator, TALL, WIDE, lay_out},
};

/// Its edge.
const EDGE: i32 = 1;

/// Draw `indicator` on `canvas`, at `scale`.
pub(crate) fn paint(
    indicator: &Indicator,
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    palette: &Palette,
) {
    let px = |rect: Rect| scaled(rect, scale);
    let (ink, faint, accent, rule) = (
        colour(palette, Role::MenuInk),
        colour(palette, Role::MenuHint),
        colour(palette, Role::Accent),
        colour(palette, Role::MenuRule),
    );
    let (wide, tall) = (WIDE as i32, TALL as i32);
    fill(
        canvas,
        px(Rect::new(0, 0, wide, tall)),
        colour(palette, Role::MenuEdge),
    );
    let within = Rect::new(EDGE, EDGE, wide - 2 * EDGE, tall - 2 * EDGE);
    fill(canvas, px(within), colour(palette, Role::Menu));
    let placed = lay_out();
    let level = indicator.level;
    super::sound::speaker(canvas, px(placed.glyph), Some(level), ink, faint);
    let size = text.size() * scale as f32;
    text.write(canvas, &indicator.name, px(placed.name), size, ink);
    let words = format!("{}%", level.percent);
    let words_wide = (text.width(&words).ceil() as i32 + 2).min(placed.percent.w);
    let at = Rect::new(
        placed.percent.right() - words_wide,
        placed.percent.y,
        words_wide,
        placed.percent.h,
    );
    text.write(
        canvas,
        &words,
        px(at),
        size,
        if level.muted { faint } else { ink },
    );
    fill(canvas, px(placed.bar), rule);
    let reach = placed.bar.w * level.percent.min(100) as i32 / 100;
    let lit = if level.muted { faint } else { accent };
    fill(
        canvas,
        px(Rect {
            w: reach,
            ..placed.bar
        }),
        lit,
    );
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::model::sound::Level;

    fn drawn(percent: u32, muted: bool) -> Pixmap {
        let mut picture = Pixmap::new(WIDE, TALL).expect("a picture");
        let indicator = Indicator {
            name: "Speakers".to_owned(),
            level: Level { percent, muted },
        };
        paint(
            &indicator,
            &mut picture.as_mut(),
            1,
            &mut Text::without_fonts(),
            &Palette::default(),
        );
        picture
    }

    fn pixel(picture: &Pixmap, x: i32, y: i32) -> [u8; 4] {
        let at = ((y * picture.width() as i32 + x) * 4) as usize;
        picture.data()[at..at + 4].try_into().expect("four bytes")
    }

    #[test]
    fn the_bar_is_lit_as_far_as_the_volume_and_faint_while_muted() {
        let bar = lay_out().bar;
        let middle = bar.y + bar.h / 2;
        let palette = Palette::default();
        let accent = colour(&palette, Role::Accent);
        let rule = colour(&palette, Role::MenuRule);
        let half = drawn(50, false);
        assert_eq!(pixel(&half, bar.x + 1, middle), accent);
        assert_eq!(pixel(&half, bar.x + bar.w / 2 - 2, middle), accent);
        assert_eq!(
            pixel(&half, bar.x + bar.w / 2 + 2, middle),
            rule,
            "the track beyond"
        );
        let loud = drawn(150, false);
        assert_eq!(pixel(&loud, bar.right() - 1, middle), accent, "full");
        assert_eq!(
            pixel(&loud, bar.right() + 1, middle),
            colour(&palette, Role::Menu),
            "and no further"
        );
        let muted = drawn(50, true);
        assert_eq!(
            pixel(&muted, bar.x + 1, middle),
            colour(&palette, Role::MenuHint)
        );
        assert_eq!(
            pixel(&half, 0, 0),
            colour(&palette, Role::MenuEdge),
            "opaque to its corner"
        );
    }
}
