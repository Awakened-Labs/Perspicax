//! What is shown on the screen a moment, in a box in the menus' colours,
//! with an edge. Every pixel is opaque.
//!
//! The volume is the speaker as the panel draws it, the output's name, the
//! volume in percent, and a track lit in the accent as far as it, past the
//! full width for none. Muted, the bar is faint.
//!
//! A notice is a "no" sign in the alert's colour, a ring struck through
//! from its top left, beside its headline and why, in the menus' ink.

use perspicax_config::{Palette, Role};
use tiny_skia::{LineCap, PathBuilder, PixmapMut, Stroke, Transform};

use super::{colour, fill, scaled, solid, text::Text};
#[cfg(feature = "audio")]
use crate::layout::{
    Measure as _,
    osd::{Indicator, lay_out},
};
use crate::{
    layout::{
        Rect,
        osd::{Showing, lay_out_notice},
    },
    model::notice::Notice,
};

/// Its edge.
const EDGE: i32 = 1;
/// The "no" sign is drawn on a square this many units on a side: a ring of
/// this radius about its middle, and a line this heavy.
const UNITS: f32 = 32.0;
const RING: f32 = 14.0;
const HEAVY: f32 = 3.0;

/// Draw `showing` on `canvas`, a box `size` logical pixels at `scale`.
pub(crate) fn paint(
    showing: &Showing,
    canvas: &mut PixmapMut<'_>,
    size: (u32, u32),
    scale: u32,
    text: &mut Text,
    palette: &Palette,
) {
    let (wide, tall) = (size.0 as i32, size.1 as i32);
    fill(
        canvas,
        scaled(Rect::new(0, 0, wide, tall), scale),
        colour(palette, Role::MenuEdge),
    );
    let within = Rect::new(EDGE, EDGE, wide - 2 * EDGE, tall - 2 * EDGE);
    fill(canvas, scaled(within, scale), colour(palette, Role::Menu));
    match showing {
        #[cfg(feature = "audio")]
        Showing::Volume(indicator) => volume(indicator, canvas, scale, text, palette),
        Showing::Notice(notice) => notice_in(notice, (wide, tall), canvas, scale, text, palette),
    }
}

/// The volume, on its box.
#[cfg(feature = "audio")]
fn volume(
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

/// A notice, on its box `size` logical pixels.
fn notice_in(
    notice: &Notice,
    size: (i32, i32),
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    palette: &Palette,
) {
    let px = |rect: Rect| scaled(rect, scale);
    let placed = lay_out_notice(size);
    no(canvas, px(placed.glyph), colour(palette, Role::Alert));
    let (ink, size) = (colour(palette, Role::MenuInk), text.size() * scale as f32);
    text.write(canvas, &notice.headline, px(placed.headline), size, ink);
    text.write(canvas, &notice.why, px(placed.why), size, ink);
}

/// A "no" sign filling `place`, a square in pixels, in `ink`.
fn no(canvas: &mut PixmapMut<'_>, place: Rect, ink: [u8; 4]) {
    let k = place.w as f32 / UNITS;
    let through = Transform::from_row(k, 0.0, 0.0, k, place.x as f32, place.y as f32);
    let middle = UNITS / 2.0;
    // The bar meets the ring where the ring is at 45°.
    let reach = RING * std::f32::consts::FRAC_1_SQRT_2;
    let mut sign = PathBuilder::new();
    sign.push_circle(middle, middle, RING);
    sign.move_to(middle - reach, middle - reach);
    sign.line_to(middle + reach, middle + reach);
    let stroke = Stroke {
        width: HEAVY,
        line_cap: LineCap::Butt,
        ..Stroke::default()
    };
    if let Some(sign) = sign.finish() {
        canvas.stroke_path(&sign, &solid(ink), &stroke, through, None);
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::layout::osd::{TALL_NOTICE, WIDE};

    fn drawn(showing: &Showing, (wide, tall): (u32, u32), scale: u32) -> Pixmap {
        let mut picture = Pixmap::new(wide * scale, tall * scale).expect("a picture");
        paint(
            showing,
            &mut picture.as_mut(),
            (wide, tall),
            scale,
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
    fn a_notices_sign_is_in_the_alerts_colour_on_an_opaque_box() {
        let palette = Palette::default();
        let alert = colour(&palette, Role::Alert);
        let showing = Showing::Notice(Notice {
            headline: "Not given to an agent".to_owned(),
            why: "Nobody knows which program drew this window".to_owned(),
        });
        let size = (WIDE + 100, TALL_NOTICE);
        for scale in [1, 2] {
            let picture = drawn(&showing, size, scale);
            let at = |x: i32, y: i32| pixel(&picture, x * scale as i32, y * scale as i32);
            let glyph = lay_out_notice((size.0 as i32, size.1 as i32)).glyph;
            let middle = glyph.y + glyph.h / 2;
            assert_eq!(
                at(glyph.x + glyph.w / 2, middle),
                alert,
                "the bar, at {scale}"
            );
            assert_eq!(at(glyph.x + 2, middle), alert, "the ring, at {scale}");
            assert_eq!(
                at(glyph.x + glyph.w / 2, glyph.y + glyph.h / 4),
                colour(&palette, Role::Menu),
                "inside it, above the bar, at {scale}"
            );
            assert_eq!(
                at(0, 0),
                colour(&palette, Role::MenuEdge),
                "opaque to its corner, at {scale}"
            );
            assert_eq!(
                at(size.0 as i32 - 1, size.1 as i32 - 1),
                colour(&palette, Role::MenuEdge),
                "and to the far one, at {scale}"
            );
        }
    }

    #[cfg(feature = "audio")]
    #[test]
    fn the_bar_is_lit_as_far_as_the_volume_and_faint_while_muted() {
        use crate::{layout::osd::TALL, model::sound::Level};

        let drawn = |percent, muted| {
            let indicator = Indicator {
                name: "Speakers".to_owned(),
                level: Level { percent, muted },
            };
            drawn(&Showing::Volume(indicator), (WIDE, TALL), 1)
        };
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
