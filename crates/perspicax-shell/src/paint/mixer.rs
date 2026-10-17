//! The mixer: a box in the menus' colours, with an edge, holding for the
//! output and for the input a title, then a button with the device's glyph
//! (the speaker, or the microphone), a slider, and the volume in percent.
//! A slider is a thin track, the accent along it as far as the volume, and
//! a round knob there; muted, the accent and the knob are faint. The
//! control the keyboard is on is ringed in the accent, once a key is used.
//! A side with no
//! device says so in place of its slider. Every pixel of the box is opaque,
//! and the rest of the surface clear.

use perspicax_config::{Palette, Role};
use tiny_skia::{FillRule, PixmapMut, Transform};

use super::{colour, fill, rounded, scaled, solid, text::Text};
use crate::{
    layout::{
        Measure as _, Rect,
        mixer::{BORDER, Control, knob_at, track},
    },
    mixer::View,
    model::sound::Side,
};

/// How thick a slider's track is, how big its knob, and the ring round the
/// control the keyboard is on.
const TRACK: i32 = 4;
const KNOB: f32 = 7.0;
const RING: f32 = 2.0;
/// How big a mute button's glyph is.
const GLYPH: i32 = 22;

/// The mixer's colours, from the theme, as fills take them.
struct Colours {
    ground: [u8; 4],
    ink: [u8; 4],
    edge: [u8; 4],
    rule: [u8; 4],
    faint: [u8; 4],
    accent: [u8; 4],
}

impl Colours {
    fn of(palette: &Palette) -> Self {
        Self {
            ground: colour(palette, Role::Menu),
            ink: colour(palette, Role::MenuInk),
            edge: colour(palette, Role::MenuEdge),
            rule: colour(palette, Role::MenuRule),
            faint: colour(palette, Role::MenuHint),
            accent: colour(palette, Role::Accent),
        }
    }
}

/// Draw `view` on `canvas`, a monitor's surface's pixels at `scale` times
/// its size.
pub(crate) fn paint(
    view: &View<'_>,
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    palette: &Palette,
) {
    let colours = Colours::of(palette);
    let px = |rect: Rect| scaled(rect, scale);
    canvas.fill(tiny_skia::Color::TRANSPARENT);
    let rect = view.placed.rect;
    fill(canvas, px(rect), colours.edge);
    let inside = Rect::new(
        rect.x + BORDER,
        rect.y + BORDER,
        rect.w - 2 * BORDER,
        rect.h - 2 * BORDER,
    );
    fill(canvas, px(inside), colours.ground);
    let size = text.size() * scale as f32;
    for &(side, title) in &view.placed.titles {
        let device = view.mixed.on(side).map(|master| master.name.as_str());
        let words = match (side, device) {
            (Side::Output, Some(name)) => format!("Output \u{2014} {name}"),
            (Side::Input, Some(name)) => format!("Input \u{2014} {name}"),
            (Side::Output, None) => "Output".to_owned(),
            (Side::Input, None) => "Input".to_owned(),
        };
        text.write(canvas, &words, px(title), size, colours.ink);
    }
    for &(control, place) in &view.placed.controls {
        match control {
            Control::Mute(side) => {
                let level = view.mixed.on(side).map(|master| master.level);
                let at = Rect::new(
                    place.x + (place.w - GLYPH) / 2,
                    place.y + (place.h - GLYPH) / 2,
                    GLYPH,
                    GLYPH,
                );
                match side {
                    Side::Output => {
                        super::sound::speaker(canvas, px(at), level, colours.ink, colours.faint);
                    }
                    Side::Input => {
                        super::sound::mic(canvas, px(at), level, colours.ink, colours.faint);
                    }
                }
            }
            Control::Volume(side) => slider(view, side, place, canvas, scale, text, &colours),
        }
        if view.ring == Some(control) {
            ring(canvas, place, scale, colours.accent);
        }
    }
    for &(side, place) in &view.placed.percents {
        let Some(percent) = view.shown[usize::from(side == Side::Input)] else {
            continue;
        };
        let words = format!("{percent}%");
        let wide = (text.width(&words).ceil() as i32 + 2).min(place.w);
        let at = Rect::new(place.right() - wide, place.y, wide, place.h);
        text.write(canvas, &words, px(at), size, colours.ink);
    }
}

/// `side`'s slider at `place`: its track, the accent as far as its volume,
/// and its knob there; or, with no device on that side, words saying so.
fn slider(
    view: &View<'_>,
    side: Side,
    place: Rect,
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    colours: &Colours,
) {
    let px = |rect: Rect| scaled(rect, scale);
    let (Some(master), Some(percent)) = (
        view.mixed.on(side),
        view.shown[usize::from(side == Side::Input)],
    ) else {
        let words = match side {
            Side::Output => "No sound server",
            Side::Input => "No microphone",
        };
        let size = text.size() * scale as f32;
        text.write(canvas, words, px(place), size, colours.faint);
        return;
    };
    let along = track(place);
    let middle = along.y + along.h / 2;
    fill(
        canvas,
        px(Rect::new(along.x, middle - TRACK / 2, along.w, TRACK)),
        colours.rule,
    );
    let knob = knob_at(place, percent, view.mixed.most(side));
    let lit = if master.level.muted {
        colours.faint
    } else {
        colours.accent
    };
    fill(
        canvas,
        px(Rect::new(
            along.x,
            middle - TRACK / 2,
            knob - along.x,
            TRACK,
        )),
        lit,
    );
    let mut circle = tiny_skia::PathBuilder::new();
    let s = scale as f32;
    circle.push_circle(knob as f32 * s, middle as f32 * s, KNOB * s);
    if let Some(circle) = circle.finish() {
        canvas.fill_path(
            &circle,
            &solid(lit),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// A ring round `place`, for the control the keyboard is on.
fn ring(canvas: &mut PixmapMut<'_>, place: Rect, scale: u32, accent: [u8; 4]) {
    let s = scale as f32;
    let (x, y, w, h) = (
        place.x as f32 * s,
        place.y as f32 * s,
        place.w as f32 * s,
        place.h as f32 * s,
    );
    let Some(outer) = rounded(x, y, w, h, 4.0 * s) else {
        return;
    };
    let stroke = tiny_skia::Stroke {
        width: RING * s,
        ..tiny_skia::Stroke::default()
    };
    canvas.stroke_path(&outer, &solid(accent), &stroke, Transform::identity(), None);
}
