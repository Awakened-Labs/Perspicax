//! The sound's glyphs, drawn in code in the panel's ink, as Perspicax's mark
//! is: an icon theme's symbolic icons are drawn dark for a light panel, and
//! nothing here recolours them, so a theme's speaker would be lost on a
//! dark one.
//!
//! The speaker is a box and a cone opening to the right, with up to three
//! waves beyond it, more the louder the sound, and a cross in place of the
//! waves while it is muted. With no sound server to ask, it is faint, and
//! crossed out.
//!
//! An output is drawn as what it is: a speaker without its waves,
//! headphones, headphones with a microphone at the end of a boom, or a
//! screen on a stand.

use std::f32::consts::FRAC_PI_4;

use tiny_skia::{FillRule, LineCap, LineJoin, PathBuilder, PixmapMut, Stroke, Transform};

use super::solid;
use crate::{
    layout::Rect,
    model::sound::{Kind, Level},
};

/// The glyphs are drawn on a square this many units on a side, as a
/// symbolic icon is.
const UNITS: f32 = 24.0;
/// The speaker: its box, and its cone's mouth at the right.
const BOX: (f32, f32, f32, f32) = (2.5, 9.0, 4.5, 6.0);
const CONE: [(f32, f32); 4] = [(7.0, 9.0), (12.0, 4.5), (12.0, 19.5), (7.0, 15.0)];
/// Each wave's radius about the cone's mouth, softest first.
const WAVES: [f32; 3] = [3.5, 6.5, 9.5];
/// The cross beside the cone, from corner to corner.
const CROSS: ((f32, f32), (f32, f32)) = ((15.0, 9.0), (21.0, 15.0));
/// How heavy a wave and the cross are.
const LINE: f32 = 1.75;
/// How far right an output's speaker sits, to stand in the middle of its
/// square without the waves beside it.
const ALONE: f32 = 4.5;

/// How many waves `percent` shows: none for silence, then one more each
/// third of the way to 100%.
pub(crate) fn waves(percent: u32) -> usize {
    match percent {
        0 => 0,
        1..=33 => 1,
        34..=66 => 2,
        _ => 3,
    }
}

/// A speaker filling `place`, a square in pixels, in `ink`, showing
/// `level`; or with `None`, with no sound server to ask, faint and crossed
/// out, in `faint`.
pub(crate) fn speaker(
    canvas: &mut PixmapMut<'_>,
    place: Rect,
    level: Option<Level>,
    ink: [u8; 4],
    faint: [u8; 4],
) {
    let k = place.w as f32 / UNITS;
    let through = Transform::from_row(k, 0.0, 0.0, k, place.x as f32, place.y as f32);
    let paint = solid(if level.is_some() { ink } else { faint });
    let stroke = Stroke {
        width: LINE,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let (x, y, w, h) = BOX;
    let mut body = PathBuilder::new();
    body.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h).expect("a box"));
    let [first, rest @ ..] = CONE;
    body.move_to(first.0, first.1);
    for (x, y) in rest {
        body.line_to(x, y);
    }
    body.close();
    if let Some(body) = body.finish() {
        canvas.fill_path(&body, &paint, FillRule::Winding, through, None);
    }
    let mut lines = PathBuilder::new();
    match level {
        Some(level) if !level.muted => {
            let mouth = (CONE[1].0, UNITS / 2.0);
            for radius in &WAVES[..waves(level.percent)] {
                arc(&mut lines, mouth, *radius);
            }
        }
        _ => {
            let ((left, top), (right, bottom)) = CROSS;
            lines.move_to(left, top);
            lines.line_to(right, bottom);
            lines.move_to(right, top);
            lines.line_to(left, bottom);
        }
    }
    if let Some(lines) = lines.finish() {
        canvas.stroke_path(&lines, &paint, &stroke, through, None);
    }
}

/// What an output of `kind` is, filling `place`, a square in pixels, in
/// `ink`.
pub(crate) fn device(canvas: &mut PixmapMut<'_>, place: Rect, kind: Kind, ink: [u8; 4]) {
    let k = place.w as f32 / UNITS;
    let through = Transform::from_row(k, 0.0, 0.0, k, place.x as f32, place.y as f32);
    let paint = solid(ink);
    let stroke = Stroke {
        width: LINE,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let mut filled = PathBuilder::new();
    let mut lines = PathBuilder::new();
    match kind {
        Kind::Speaker | Kind::Other => {
            let (x, y, w, h) = BOX;
            filled.push_rect(tiny_skia::Rect::from_xywh(x + ALONE, y, w, h).expect("a box"));
            let [first, rest @ ..] = CONE;
            filled.move_to(first.0 + ALONE, first.1);
            for (x, y) in rest {
                filled.line_to(x + ALONE, y);
            }
            filled.close();
        }
        Kind::Headphones | Kind::Headset => {
            // The band over the head, and a cup at each ear.
            lines.move_to(5.0, 14.0);
            lines.cubic_to(5.0, 3.5, 19.0, 3.5, 19.0, 14.0);
            for x in [3.0, 16.5] {
                filled.push_rect(tiny_skia::Rect::from_xywh(x, 12.5, 4.5, 7.0).expect("a cup"));
            }
            if kind == Kind::Headset {
                // A boom from the left cup, round to the mouth.
                lines.move_to(5.5, 19.5);
                lines.quad_to(6.5, 22.0, 11.0, 21.5);
                filled.push_circle(12.0, 21.4, 1.4);
            }
        }
        Kind::Display => {
            // A screen, and its stand.
            filled.push_rect(tiny_skia::Rect::from_xywh(3.0, 4.0, 18.0, 12.0).expect("a screen"));
            lines.move_to(12.0, 16.0);
            lines.line_to(12.0, 19.5);
            lines.move_to(8.0, 19.5);
            lines.line_to(16.0, 19.5);
        }
    }
    if let Some(filled) = filled.finish() {
        canvas.fill_path(&filled, &paint, FillRule::Winding, through, None);
    }
    if let Some(lines) = lines.finish() {
        canvas.stroke_path(&lines, &paint, &stroke, through, None);
    }
}

/// A quarter of a circle of `radius` about `centre`, facing right: from 45°
/// above the middle to 45° below, as one cubic curve.
fn arc(path: &mut PathBuilder, (cx, cy): (f32, f32), radius: f32) {
    let (half, reach) = (FRAC_PI_4, 4.0 / 3.0 * (FRAC_PI_4 / 2.0).tan());
    let point = |angle: f32| (cx + radius * angle.cos(), cy + radius * angle.sin());
    let tangent = |angle: f32| (-angle.sin() * radius * reach, angle.cos() * radius * reach);
    let (start, end) = (point(-half), point(half));
    let (out, back) = (tangent(-half), tangent(half));
    path.move_to(start.0, start.1);
    path.cubic_to(
        start.0 + out.0,
        start.1 + out.1,
        end.0 - back.0,
        end.1 - back.1,
        end.0,
        end.1,
    );
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;

    const INK: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
    const FAINT: [u8; 4] = [0x80, 0x80, 0x80, 0xff];

    /// A 24-pixel speaker showing `level`, as its pixels.
    fn drawn(level: Option<Level>) -> Pixmap {
        let mut picture = Pixmap::new(24, 24).expect("a picture");
        speaker(
            &mut picture.as_mut(),
            Rect::new(0, 0, 24, 24),
            level,
            INK,
            FAINT,
        );
        picture
    }

    /// Whether anything is drawn at `(x, y)`.
    fn inked(picture: &Pixmap, x: u32, y: u32) -> bool {
        picture.pixel(x, y).expect("in the picture").alpha() > 0
    }

    fn level(percent: u32, muted: bool) -> Option<Level> {
        Some(Level { percent, muted })
    }

    /// A 24-pixel output of `kind`, as its pixels.
    fn device_drawn(kind: Kind) -> Pixmap {
        let mut picture = Pixmap::new(24, 24).expect("a picture");
        device(&mut picture.as_mut(), Rect::new(0, 0, 24, 24), kind, INK);
        picture
    }

    #[test]
    fn each_kind_of_output_is_drawn_as_what_it_is() {
        let speaker = device_drawn(Kind::Speaker);
        assert!(
            inked(&speaker, 10, 12) && !inked(&speaker, 3, 12),
            "a speaker, moved right"
        );
        assert!(!inked(&speaker, 21, 12), "with no waves");
        let headphones = device_drawn(Kind::Headphones);
        assert!(
            inked(&headphones, 4, 16) && inked(&headphones, 19, 16),
            "a cup at each ear"
        );
        assert!(inked(&headphones, 12, 6), "the band over the head");
        assert!(!inked(&headphones, 12, 21), "no microphone");
        let headset = device_drawn(Kind::Headset);
        assert!(inked(&headset, 12, 21), "a microphone");
        let display = device_drawn(Kind::Display);
        assert!(
            inked(&display, 12, 10) && inked(&display, 12, 18),
            "a screen on a stand"
        );
        assert!(!inked(&display, 4, 21));
        assert_eq!(
            device_drawn(Kind::Other).data(),
            speaker.data(),
            "anything else, as a speaker"
        );
    }

    #[test]
    fn a_wave_shows_for_each_third_of_the_way_to_full() {
        assert_eq!(
            [0, 1, 33, 34, 66, 67, 100, 150].map(waves),
            [0, 1, 1, 2, 2, 3, 3, 3]
        );
    }

    #[test]
    fn the_waves_reach_further_the_louder_it_is() {
        // On the middle line, where each wave crosses it, right of the cone.
        let (middle, first, last) = (12, 15, 21);
        let silent = drawn(level(0, false));
        assert!(inked(&silent, 5, middle), "the box");
        assert!(!inked(&silent, first, middle) && !inked(&silent, last, middle));
        let quiet = drawn(level(20, false));
        assert!(inked(&quiet, first, middle) && !inked(&quiet, last, middle));
        let loud = drawn(level(90, false));
        assert!(inked(&loud, first, middle) && inked(&loud, last, middle));
    }

    #[test]
    fn muted_or_with_no_server_it_is_crossed_out_and_without_a_server_faint() {
        let muted = drawn(level(90, true));
        let crossing = (18, 12);
        assert!(inked(&muted, crossing.0, crossing.1), "the cross");
        assert!(!inked(&muted, 21, 12), "no wave");
        assert_eq!(
            muted.pixel(5, 12).expect("the box").red(),
            0xff,
            "in the ink"
        );
        let gone = drawn(None);
        assert!(inked(&gone, crossing.0, crossing.1));
        assert_eq!(gone.pixel(5, 12).expect("the box").red(), 0x80, "faint");
    }
}
