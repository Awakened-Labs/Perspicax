//! The mixer: a box in the menus' colours, with an edge, holding for the
//! output and for the input a title, then a button with the device's glyph
//! (the speaker, or the microphone), a slider, and the volume in percent,
//! then a row for each device listed: a round mark, filled in the accent
//! for the one the sound is on, the device's glyph, and its name, faint if
//! nothing is plugged into it, the card's part of it cut short before its
//! jack's. A side's fold is an arrow, pointing down
//! while it is unfolded, and how many it holds.
//!
//! A slider is a thin track, the accent along it as far as the volume, and
//! a round knob there; muted, the accent and the knob are faint. The
//! control the keyboard is on is ringed in the accent, once a key is used.
//! A side with no device says so in place of its slider. What is scrolled
//! past the room inside the edge is cut off there. Every pixel of the box
//! is opaque, and the rest of the surface clear.

use perspicax_config::{Palette, Role};
use tiny_skia::{FillRule, Pixmap, PixmapMut, PixmapPaint, Stroke, Transform};

use super::{colour, fill, rounded, scaled, solid, text::Text};
use crate::{
    layout::{
        Measure as _, Rect,
        mixer::{BORDER, BUTTON, Control, Listed, knob_at, track},
    },
    mixer::View,
    model::sound::Side,
};

/// How thick a slider's track is, how big its knob, and the ring round the
/// control the keyboard is on.
const TRACK: i32 = 4;
const KNOB: f32 = 7.0;
const RING: f32 = 2.0;
/// How big a mute button's glyph is, and a listed device's.
const GLYPH: i32 = 22;
const MARK: i32 = 18;
/// How big a listed device's round mark is, its dot when chosen, and the
/// fold's arrow.
const DOT: f32 = 6.0;
const CHOSEN: f32 = 3.0;
const ARROW: f32 = 4.0;
/// The room between a row's glyph and its words, and the least of a
/// device's name kept beside a long jack's.
const SPACE: i32 = 8;
const DEVICE: i32 = 64;

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

/// Draws what is inside the mixer's edge on a picture of just that.
struct Pen<'p, 't> {
    canvas: PixmapMut<'p>,
    /// The picture's top left corner on the surface, in logical pixels.
    origin: (i32, i32),
    scale: u32,
    text: &'t mut Text,
    colours: Colours,
}

impl Pen<'_, '_> {
    /// `rect`, on the surface, as the picture's pixels.
    fn px(&self, rect: Rect) -> Rect {
        let (x, y) = self.origin;
        scaled(
            Rect::new(rect.x - x, rect.y - y, rect.w, rect.h),
            self.scale,
        )
    }

    /// The point `(x, y)`, on the surface, as the picture's pixels.
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let s = self.scale as f32;
        (
            (x - self.origin.0 as f32) * s,
            (y - self.origin.1 as f32) * s,
        )
    }

    /// Write `words` into `place`: how wide they came to, in logical
    /// pixels.
    fn write(&mut self, words: &str, place: Rect, ink: [u8; 4]) -> i32 {
        let size = self.text.size() * self.scale as f32;
        let place = self.px(place);
        let wide = self.text.write(&mut self.canvas, words, place, size, ink);
        (wide / self.scale as f32).ceil() as i32
    }

    fn circle(&mut self, at: (f32, f32), radius: f32, ink: [u8; 4], stroke: Option<f32>) {
        let (x, y) = self.point(at.0, at.1);
        let s = self.scale as f32;
        let mut circle = tiny_skia::PathBuilder::new();
        circle.push_circle(x, y, radius * s);
        let Some(circle) = circle.finish() else {
            return;
        };
        match stroke {
            Some(width) => {
                let stroke = Stroke {
                    width: width * s,
                    ..Stroke::default()
                };
                self.canvas
                    .stroke_path(&circle, &solid(ink), &stroke, Transform::identity(), None);
            }
            None => self.canvas.fill_path(
                &circle,
                &solid(ink),
                FillRule::Winding,
                Transform::identity(),
                None,
            ),
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
    canvas.fill(tiny_skia::Color::TRANSPARENT);
    let rect = view.placed.rect;
    fill(canvas, scaled(rect, scale), colours.edge);
    let within = Rect::new(
        rect.x + BORDER,
        rect.y + BORDER,
        rect.w - 2 * BORDER,
        rect.h - 2 * BORDER,
    );
    fill(canvas, scaled(within, scale), colours.ground);
    // What is inside is drawn on a picture of its own, laid in where it
    // shows: what is scrolled past the edge of it is cut off there.
    let inside = view.placed.inside;
    let whole = scaled(inside, scale);
    let Some(mut picture) = Pixmap::new(whole.w.max(1) as u32, whole.h.max(1) as u32) else {
        return;
    };
    let mut pen = Pen {
        canvas: picture.as_mut(),
        origin: (inside.x, inside.y),
        scale,
        text,
        colours,
    };
    within_edge(view, &mut pen);
    canvas.draw_pixmap(
        whole.x,
        whole.y,
        picture.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// Everything inside the edge: the titles, the controls and the
/// percentages.
fn within_edge(view: &View<'_>, pen: &mut Pen<'_, '_>) {
    let (ink, faint, accent) = (pen.colours.ink, pen.colours.faint, pen.colours.accent);
    for &(side, title) in &view.placed.titles {
        let device = view.mixed.on(side).map(|master| master.name.as_str());
        let words = match (side, device) {
            (Side::Output, Some(name)) => format!("Output \u{2014} {name}"),
            (Side::Input, Some(name)) => format!("Input \u{2014} {name}"),
            (Side::Output, None) => "Output".to_owned(),
            (Side::Input, None) => "Input".to_owned(),
        };
        pen.write(&words, title, ink);
    }
    for &(control, place) in &view.placed.controls {
        match control {
            Control::Mute(side) => {
                let level = view.mixed.on(side).map(|master| master.level);
                let at = pen.px(Rect::new(
                    place.x + (place.w - GLYPH) / 2,
                    place.y + (place.h - GLYPH) / 2,
                    GLYPH,
                    GLYPH,
                ));
                match side {
                    Side::Output => super::sound::speaker(&mut pen.canvas, at, level, ink, faint),
                    Side::Input => super::sound::mic(&mut pen.canvas, at, level, ink, faint),
                }
            }
            Control::Volume(side) => slider(view, side, place, pen),
            Control::Choose(side, number) => {
                if let Some(listed) = view.mixed.find(side, number) {
                    device(listed, place, pen);
                }
            }
            Control::Fold(side) => {
                let (count, open) = (
                    view.placed.folded[side.at()],
                    view.placed.unfolded[side.at()],
                );
                fold(count, open, place, pen);
            }
        }
        if view.ring == Some(control) {
            ring(pen, place, accent);
        }
    }
    for &(side, place) in &view.placed.percents {
        let Some(percent) = view.shown[side.at()] else {
            continue;
        };
        let words = format!("{percent}%");
        let wide = (pen.text.width(&words).ceil() as i32 + 2).min(place.w);
        let at = Rect::new(place.right() - wide, place.y, wide, place.h);
        pen.write(&words, at, ink);
    }
}

/// `side`'s slider at `place`: its track, the accent as far as its volume,
/// and its knob there; or, with no device on that side, words saying so.
fn slider(view: &View<'_>, side: Side, place: Rect, pen: &mut Pen<'_, '_>) {
    let (Some(master), Some(percent)) = (view.mixed.on(side), view.shown[side.at()]) else {
        let words = match side {
            Side::Output => "No sound server",
            Side::Input => "No microphone",
        };
        pen.write(words, place, pen.colours.faint);
        return;
    };
    let along = track(place);
    let middle = along.y + along.h / 2;
    let line = pen.px(Rect::new(along.x, middle - TRACK / 2, along.w, TRACK));
    fill(&mut pen.canvas, line, pen.colours.rule);
    let knob = knob_at(place, percent, view.mixed.most(side));
    let lit = if master.level.muted {
        pen.colours.faint
    } else {
        pen.colours.accent
    };
    let lit_line = pen.px(Rect::new(
        along.x,
        middle - TRACK / 2,
        knob - along.x,
        TRACK,
    ));
    fill(&mut pen.canvas, lit_line, lit);
    pen.circle((knob as f32, middle as f32), KNOB, lit, None);
}

/// A device listed, at `place`: its round mark, filled if the sound is on
/// it, its glyph, and its name.
fn device(listed: &Listed, place: Rect, pen: &mut Pen<'_, '_>) {
    let ink = if listed.connected {
        pen.colours.ink
    } else {
        pen.colours.faint
    };
    let middle = place.y as f32 + place.h as f32 / 2.0;
    let centre = (place.x as f32 + BUTTON as f32 / 2.0, middle);
    if listed.chosen {
        pen.circle(centre, DOT, pen.colours.accent, Some(1.5));
        pen.circle(centre, CHOSEN, pen.colours.accent, None);
    } else {
        pen.circle(centre, DOT, ink, Some(1.5));
    }
    let glyph = Rect::new(
        place.x + BUTTON + SPACE / 2,
        place.y + (place.h - MARK) / 2,
        MARK,
        MARK,
    );
    let at = pen.px(glyph);
    super::sound::device(&mut pen.canvas, at, listed.kind, ink);
    let words = Rect::new(
        glyph.right() + SPACE,
        place.y,
        place.right() - glyph.right() - SPACE,
        place.h,
    );
    let label = listed.label();
    let Some(jack) = listed
        .jack
        .as_ref()
        .filter(|_| pen.text.width(&label) > words.w as f32)
    else {
        pen.write(&label, words, ink);
        return;
    };
    // A card's jacks are told apart by the jack, not the card: the card's
    // name is cut short first.
    let tail = format!("\u{2014} {jack}");
    let wide = pen.text.width(&tail).ceil() as i32;
    let device = (words.w - wide - SPACE / 2).max(DEVICE);
    let device = pen.write(
        &listed.device,
        Rect::new(words.x, words.y, device, words.h),
        ink,
    );
    let x = words.x + device + SPACE / 2;
    pen.write(
        &tail,
        Rect::new(x, words.y, words.right() - x, words.h),
        ink,
    );
}

/// A side's fold, at `place`: an arrow, pointing right while what it
/// holds is folded away and down while it is not, and how many it holds.
fn fold(count: usize, open: bool, place: Rect, pen: &mut Pen<'_, '_>) {
    let faint = pen.colours.faint;
    let (x, y) = (
        place.x as f32 + BUTTON as f32 / 2.0,
        place.y as f32 + place.h as f32 / 2.0,
    );
    let corners = if open {
        [
            (x - ARROW, y - ARROW / 2.0),
            (x + ARROW, y - ARROW / 2.0),
            (x, y + ARROW / 2.0),
        ]
    } else {
        [
            (x - ARROW / 2.0, y - ARROW),
            (x + ARROW / 2.0, y),
            (x - ARROW / 2.0, y + ARROW),
        ]
    };
    let mut arrow = tiny_skia::PathBuilder::new();
    for (at, &(x, y)) in corners.iter().enumerate() {
        let (x, y) = pen.point(x, y);
        if at == 0 {
            arrow.move_to(x, y);
        } else {
            arrow.line_to(x, y);
        }
    }
    arrow.close();
    if let Some(arrow) = arrow.finish() {
        pen.canvas.fill_path(
            &arrow,
            &solid(faint),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let words = Rect::new(
        place.x + BUTTON + SPACE / 2,
        place.y,
        place.w - BUTTON - SPACE / 2,
        place.h,
    );
    pen.write(&format!("Not connected ({count})"), words, faint);
}

/// A ring just inside `place`, for the control the keyboard is on.
fn ring(pen: &mut Pen<'_, '_>, place: Rect, accent: [u8; 4]) {
    let s = pen.scale as f32;
    let (x, y) = pen.point(place.x as f32, place.y as f32);
    let inset = RING / 2.0 * s;
    let Some(outer) = rounded(
        x + inset,
        y + inset,
        place.w as f32 * s - 2.0 * inset,
        place.h as f32 * s - 2.0 * inset,
        4.0 * s,
    ) else {
        return;
    };
    let stroke = Stroke {
        width: RING * s,
        ..Stroke::default()
    };
    pen.canvas
        .stroke_path(&outer, &solid(accent), &stroke, Transform::identity(), None);
}

#[cfg(test)]
mod tests {
    use perspicax_config::Edge;

    use super::*;
    use crate::{
        layout::mixer::{Anchor, Master, Mixed, number},
        mixer::{Event, State},
        model::sound::{Kind, Level},
    };

    fn listed(key: &str, chosen: bool) -> Listed {
        Listed {
            key: key.to_owned(),
            device: key.to_owned(),
            jack: None,
            kind: Kind::Speaker,
            connected: true,
            chosen,
        }
    }

    /// A mixer with more outputs than it has room for, open on a monitor
    /// 400 by 240 with a panel along its bottom.
    fn opened() -> State {
        let master = |name: &str| Master {
            name: name.to_owned(),
            level: Level {
                percent: 40,
                muted: false,
            },
        };
        let mut outputs = vec![listed("speakers", true), listed("headset", false)];
        outputs.extend((0..6).map(|at| listed(&format!("usb {at}"), false)));
        let mut state = State::default();
        state.update(Event::Open {
            output: "DP-1".to_owned(),
            anchor: Anchor {
                button: Rect::new(300, 200, 100, 40),
                edge: Edge::Bottom,
                area: Rect::new(0, 0, 400, 200),
            },
            mixed: Mixed {
                output: Some(master("Speakers")),
                input: Some(master("Microphone")),
                outputs,
                step: 5,
                limit: 100,
                ..Mixed::default()
            },
        });
        state
    }

    fn drawn(state: &State, scale: u32) -> Pixmap {
        let mut picture = Pixmap::new(400 * scale, 240 * scale).expect("a picture");
        paint(
            &state.view().expect("open"),
            &mut picture.as_mut(),
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
    fn the_device_the_sound_is_on_is_marked_in_the_accent() {
        let state = opened();
        let picture = drawn(&state, 2);
        let view = state.view().expect("open");
        let colours = Colours::of(&Palette::default());
        let centre = |key: &str| {
            let control = Control::Choose(Side::Output, number(key));
            let row = view.placed.of(control).expect("listed");
            (2 * (row.x + BUTTON / 2), 2 * (row.y + row.h / 2))
        };
        let (x, y) = centre("speakers");
        assert_eq!(pixel(&picture, x, y), colours.accent);
        let (x, y) = centre("headset");
        assert_eq!(pixel(&picture, x, y), colours.ground, "a ring, empty");
    }

    #[test]
    fn what_is_scrolled_past_the_edge_is_cut_off_there() {
        let mut state = opened();
        let view = state.view().expect("open");
        let row = view
            .placed
            .of(Control::Choose(Side::Output, number("speakers")))
            .expect("listed");
        state.update(Event::Motion((f64::from(row.x + 2), f64::from(row.y + 2))));
        state.update(Event::Wheel(1));
        let view = state.view().expect("open");
        let (inside, rect) = (view.placed.inside, view.placed.rect);
        assert!(view.placed.scroll > 0, "scrolled");
        let mute = view.placed.of(Control::Mute(Side::Output)).expect("placed");
        assert!(
            mute.y < inside.y && mute.bottom() > inside.y,
            "the speaker cut across the top"
        );
        let picture = drawn(&state, 2);
        let colours = Colours::of(&Palette::default());
        for y in 2 * (rect.y + BORDER)..2 * inside.y {
            for x in 2 * mute.x..2 * mute.right() {
                assert_eq!(pixel(&picture, x, y), colours.ground, "at {x}, {y}");
            }
        }
        for y in 0..2 * rect.y {
            assert_eq!(pixel(&picture, 2 * mute.x, y)[3], 0, "clear above it");
        }
        let speaker_ink = (2 * mute.x..2 * mute.right())
            .flat_map(|x| (2 * inside.y..2 * mute.bottom()).map(move |y| (x, y)))
            .any(|(x, y)| pixel(&picture, x, y) == colours.ink);
        assert!(speaker_ink, "what is in sight of it drawn");
    }
}
