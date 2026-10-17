//! The mixer: a box in the menus' colours, with an edge, holding for the
//! output and for the input a title, then a button with the device's glyph
//! (the speaker, or the microphone), a slider, and the volume in percent,
//! then a row for each device listed: a round mark, filled in the accent
//! for the one the sound is on, the device's glyph, and its name, faint if
//! nothing is plugged into it, the card's part of it cut short before its
//! jack's, or the name the person gave it. Along the foot of a device's row
//! that is metered runs a thin faint track, the accent along it as far as
//! it is loud. At
//! the row's end, three dots unfold under it, set in, what tells it apart,
//! faint; a field with its name in; for an output, a button that plays the
//! test sound on it, faint and saying to choose it while it does not play;
//! and the buttons that hide it and its card, in the accent. A side's
//! folds, of what has nothing plugged in and of what is hidden, are each an
//! arrow, pointing down while it is unfolded, and how many it holds. Under
//! "Applications", each program's row is such an arrow, the program's icon
//! and name, and, faint at its end, what it plays on; then its own button,
//! slider and percentage; unfolded, a row for each output it can be moved
//! to, marked for the one it plays on.
//!
//! A slider is a thin track, the accent along it as far as the volume, and
//! a round knob there; muted, the accent and the knob are faint. The
//! control the keyboard is on is ringed in the accent, once a key is used.
//! A side with no device says so in place of its slider. What is scrolled
//! past the room inside the edge is cut off there. Every pixel of the box
//! is opaque, and the rest of the surface clear.

use std::collections::HashMap;

use perspicax_config::{Palette, Role};
use tiny_skia::{FillRule, Pixmap, PixmapMut, PixmapPaint, Stroke, Transform};

use super::{colour, fill, icons::Images, rounded, scaled, solid, text::Text};
use crate::{
    layout::{
        Measure as _, Rect,
        mixer::{BORDER, BUTTON, Control, Heading, Listed, Program, knob_at, track},
    },
    mixer::View,
    model::sound::{Side, Whose, loudness},
};

/// How thick a slider's track is, how big its knob, and the ring round the
/// control the keyboard is on.
const TRACK: i32 = 4;
const KNOB: f32 = 7.0;
const RING: f32 = 2.0;
/// How big a mute button's glyph is, and a listed device's or program's.
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
/// How far apart the dots of the button at a device's row's end are.
const DOTS: f32 = 5.0;
/// How thick a meter is.
const METER: i32 = 2;

/// The mixer's colours, from the theme, as fills take them.
struct Colours {
    ground: [u8; 4],
    ink: [u8; 4],
    edge: [u8; 4],
    rule: [u8; 4],
    faint: [u8; 4],
    accent: [u8; 4],
    /// Behind a field typed in.
    typed: [u8; 4],
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
            typed: colour(palette, Role::MenuTyped),
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
    images: &'t mut Images,
    colours: Colours,
    /// The peak of each source metered, by name.
    peaks: &'t HashMap<String, f32>,
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
/// its size, the sources metered at `peaks`.
pub(crate) fn paint(
    view: &View<'_>,
    canvas: &mut PixmapMut<'_>,
    (scale, palette): (u32, &Palette),
    text: &mut Text,
    images: &mut Images,
    peaks: &HashMap<String, f32>,
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
        images,
        colours,
        peaks,
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
    for &(heading, title) in &view.placed.titles {
        let words = match heading {
            Heading::Side(side) => {
                let named = match side {
                    Side::Output => "Output",
                    Side::Input => "Input",
                };
                match view.mixed.on(side) {
                    Some(master) => format!("{named} \u{2014} {}", master.name),
                    None => named.to_owned(),
                }
            }
            Heading::Programs => "Applications".to_owned(),
        };
        pen.write(&words, title, ink);
    }
    for &(control, place) in &view.placed.controls {
        match control {
            Control::Mute(whose) => {
                let level = view.mixed.level(whose);
                let at = pen.px(Rect::new(
                    place.x + (place.w - GLYPH) / 2,
                    place.y + (place.h - GLYPH) / 2,
                    GLYPH,
                    GLYPH,
                ));
                match whose {
                    Whose::Side(Side::Input) => {
                        super::sound::mic(&mut pen.canvas, at, level, ink, faint);
                    }
                    Whose::Side(Side::Output) | Whose::App(_) => {
                        super::sound::speaker(&mut pen.canvas, at, level, ink, faint);
                    }
                }
            }
            Control::Volume(whose) => slider(view, whose, place, pen),
            Control::Choose(side, number) => {
                if let Some(listed) = view.mixed.find(side, number) {
                    device(listed, listed.chosen, place, pen);
                }
            }
            Control::Fold(side) => {
                let count = view.placed.unplugged[side.at()];
                let open = view.placed.unfolded.unplugged[side.at()];
                fold(&format!("Not connected ({count})"), open, place, pen);
            }
            Control::Hidden(side) => {
                let count = view.placed.hidden[side.at()];
                let open = view.placed.unfolded.hidden[side.at()];
                fold(&format!("Hidden ({count})"), open, place, pen);
            }
            Control::More(side, number) => {
                let open = view.placed.unfolded.more == Some((side, number));
                dots(place, if open { ink } else { faint }, pen);
            }
            Control::Rename(side, number) => {
                if let Some(listed) = view.mixed.find(side, number) {
                    let typed = view
                        .naming
                        .filter(|&(naming, _)| naming == control)
                        .map(|(_, typed)| typed);
                    field(listed, typed, place, pen);
                }
            }
            Control::Test(side, number) => {
                match view.mixed.find(side, number).map(|listed| listed.live) {
                    Some(Some(_)) => button("Test sound", place, pen),
                    Some(None) => {
                        let at =
                            Rect::new(place.x + SPACE / 2, place.y, place.w - SPACE / 2, place.h);
                        pen.write("Choose it to hear it", at, faint);
                    }
                    None => {}
                }
            }
            Control::Hide(side, number) => {
                if let Some(listed) = view.mixed.find(side, number) {
                    let words = if listed.hidden { "Unhide" } else { "Hide" };
                    button(words, place, pen);
                }
            }
            Control::HideCard(side, number) => {
                let card = view
                    .mixed
                    .find(side, number)
                    .and_then(|listed| listed.card.as_ref());
                if let Some(card) = card {
                    let does = if card.hidden { "Unhide" } else { "Hide" };
                    button(&format!("{does} all of {}", card.name), place, pen);
                }
            }
            Control::Plays(stream) => {
                if let Some(program) = view.mixed.program(stream) {
                    plays(view, program, place, pen);
                }
            }
            Control::Move(stream, sink) => {
                let program = view.mixed.program(stream);
                let listed = view.mixed.sinks.iter().find(|&&(had, _)| had == sink);
                if let (Some(program), Some((_, listed))) = (program, listed) {
                    device(listed, program.sink == sink, place, pen);
                }
            }
        }
        if view.ring == Some(control) {
            ring(pen, place, accent);
        }
    }
    for (line, place) in &view.placed.details {
        pen.write(line, *place, faint);
    }
    for &(whose, place) in &view.placed.percents {
        let Some(percent) = view.shown(whose) else {
            continue;
        };
        let words = format!("{percent}%");
        let wide = (pen.text.width(&words).ceil() as i32 + 2).min(place.w);
        let at = Rect::new(place.right() - wide, place.y, wide, place.h);
        pen.write(&words, at, ink);
    }
}

/// The slider of `whose` at `place`: its track, the accent as far as its
/// volume, and its knob there; or, with no device on a side, words saying
/// so.
fn slider(view: &View<'_>, whose: Whose, place: Rect, pen: &mut Pen<'_, '_>) {
    let (Some(level), Some(percent)) = (view.mixed.level(whose), view.shown(whose)) else {
        let words = match whose {
            Whose::Side(Side::Output) => "No sound server",
            Whose::Side(Side::Input) => "No microphone",
            Whose::App(_) => return,
        };
        pen.write(words, place, pen.colours.faint);
        return;
    };
    let along = track(place);
    let middle = along.y + along.h / 2;
    let line = pen.px(Rect::new(along.x, middle - TRACK / 2, along.w, TRACK));
    fill(&mut pen.canvas, line, pen.colours.rule);
    let knob = knob_at(place, percent, view.mixed.most(whose));
    let lit = if level.muted {
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

/// A program's row, at `place`: its arrow, its icon, if it has one, its
/// name, and, faint at the end, what it plays on.
fn plays(view: &View<'_>, program: &Program, place: Rect, pen: &mut Pen<'_, '_>) {
    let open = view.placed.unfolded.programs.contains(&program.stream);
    arrow(open, place, pen);
    let icon = Rect::new(
        place.x + BUTTON + SPACE / 2,
        place.y + (place.h - MARK) / 2,
        MARK,
        MARK,
    );
    let at = pen.px(icon);
    let found = program
        .icon
        .as_deref()
        .and_then(|name| pen.images.get(name, MARK as u32, pen.scale));
    if let Some(image) = found {
        super::icons::draw(&mut pen.canvas, image, at);
    }
    let words = Rect::new(
        icon.right() + SPACE,
        place.y,
        place.right() - icon.right() - SPACE,
        place.h,
    );
    let on = view
        .mixed
        .playing_on(program)
        .map_or("", |listed| listed.short.as_str());
    // What it plays on keeps to the end, in at most half the room.
    let wide = (pen.text.width(on).ceil() as i32).min(words.w / 2);
    let start = words.right() - wide;
    pen.write(
        on,
        Rect::new(start, words.y, wide, words.h),
        pen.colours.faint,
    );
    let name = Rect::new(words.x, words.y, start - SPACE - words.x, words.h);
    pen.write(&program.name, name, pen.colours.ink);
}

/// A device listed, at `place`: its round mark, filled if it is `chosen`,
/// its glyph, and its name.
fn device(listed: &Listed, chosen: bool, place: Rect, pen: &mut Pen<'_, '_>) {
    let ink = if listed.connected {
        pen.colours.ink
    } else {
        pen.colours.faint
    };
    let middle = place.y as f32 + place.h as f32 / 2.0;
    let centre = (place.x as f32 + BUTTON as f32 / 2.0, middle);
    if chosen {
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
    let peak = listed
        .meter
        .as_ref()
        .and_then(|source| pen.peaks.get(source));
    if let Some(&peak) = peak {
        // A faint track the width of the name, the accent along it as far
        // as it is loud.
        let room = place.right() - SPACE - (glyph.right() + SPACE);
        let reach = (loudness(peak) * room as f32).round() as i32;
        let track = Rect::new(
            glyph.right() + SPACE,
            place.bottom() - METER - 1,
            room,
            METER,
        );
        let lit = pen.px(Rect { w: reach, ..track });
        let track = pen.px(track);
        fill(&mut pen.canvas, track, pen.colours.rule);
        fill(&mut pen.canvas, lit, pen.colours.accent);
    }
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
        .filter(|_| listed.name.is_none() && pen.text.width(&label) > words.w as f32)
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

/// A fold, at `place`: its arrow, and `words`, faint.
fn fold(words: &str, open: bool, place: Rect, pen: &mut Pen<'_, '_>) {
    arrow(open, place, pen);
    let words_at = Rect::new(
        place.x + BUTTON + SPACE / 2,
        place.y,
        place.w - BUTTON - SPACE / 2,
        place.h,
    );
    pen.write(words, words_at, pen.colours.faint);
}

/// Three dots across the middle of `place`, in `ink`.
fn dots(place: Rect, ink: [u8; 4], pen: &mut Pen<'_, '_>) {
    let middle = place.y as f32 + place.h as f32 / 2.0;
    let centre = place.x as f32 + place.w as f32 / 2.0;
    for at in [-1.0, 0.0, 1.0] {
        pen.circle((centre + at * DOTS, middle), 1.5, ink, None);
    }
}

/// The field `listed` is named in, at `place`: what is `typed` in it, with
/// a caret, while it is being named; else its name, or faint, what the
/// field is for.
fn field(listed: &Listed, typed: Option<&str>, place: Rect, pen: &mut Pen<'_, '_>) {
    let ground = Rect::new(place.x, place.y + 2, place.w, place.h - 4);
    let at = pen.px(ground);
    fill(&mut pen.canvas, at, pen.colours.typed);
    let words = Rect::new(ground.x + SPACE / 2, ground.y, ground.w - SPACE, ground.h);
    match (typed, &listed.name) {
        (Some(typed), _) => {
            let wide = pen.write(typed, words, pen.colours.ink);
            let caret = Rect::new(
                (words.x + wide + 1).min(words.right()),
                words.y + 5,
                1,
                words.h - 10,
            );
            let caret = pen.px(caret);
            fill(&mut pen.canvas, caret, pen.colours.ink);
        }
        (None, Some(name)) => {
            pen.write(name, words, pen.colours.ink);
        }
        (None, None) => {
            pen.write("Name it\u{2026}", words, pen.colours.faint);
        }
    }
}

/// A button of `words` alone, in the accent, at `place`.
fn button(words: &str, place: Rect, pen: &mut Pen<'_, '_>) {
    let words_at = Rect::new(place.x + SPACE / 2, place.y, place.w - SPACE / 2, place.h);
    pen.write(words, words_at, pen.colours.accent);
}

/// An arrow at the start of `place`, faint, pointing down if what it
/// unfolds is `open`, and right if it is folded away.
fn arrow(open: bool, place: Rect, pen: &mut Pen<'_, '_>) {
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
            &solid(pen.colours.faint),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
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
        mixer::{Act, Event, State},
        model::sound::{Kind, Level},
    };

    fn listed(key: &str, chosen: bool) -> Listed {
        Listed {
            key: key.to_owned(),
            device: key.to_owned(),
            jack: None,
            short: key.to_owned(),
            kind: Kind::Speaker,
            live: None,
            meter: None,
            connected: true,
            chosen,
            name: None,
            hidden: false,
            card: None,
            details: Vec::new(),
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
        drawn_with(state, scale, &HashMap::new())
    }

    fn drawn_with(state: &State, scale: u32, peaks: &HashMap<String, f32>) -> Pixmap {
        let mut picture = Pixmap::new(400 * scale, 240 * scale).expect("a picture");
        paint(
            &state.view().expect("open"),
            &mut picture.as_mut(),
            (scale, &Palette::default()),
            &mut Text::without_fonts(),
            &mut Images::default(),
            peaks,
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
    fn a_programs_outputs_mark_the_one_it_plays_on() {
        let mut state = State::default();
        state.update(Event::Open {
            output: "DP-1".to_owned(),
            anchor: Anchor {
                button: Rect::new(300, 200, 100, 40),
                edge: Edge::Bottom,
                area: Rect::new(0, 0, 400, 200),
            },
            mixed: Mixed {
                programs: vec![Program {
                    stream: 40,
                    name: "Firefox".to_owned(),
                    icon: None,
                    level: Level {
                        percent: 30,
                        muted: false,
                    },
                    sink: 1,
                }],
                sinks: vec![(0, listed("speakers", true)), (1, listed("headset", false))],
                step: 5,
                limit: 100,
                ..Mixed::default()
            },
        });
        state.update(Event::Act(Control::Plays(40), Act::Click));
        // Scrolled to, at the foot of a mixer too tall for the room.
        state.update(Event::Act(Control::Move(40, 1), Act::Focus));
        let picture = drawn(&state, 1);
        let view = state.view().expect("open");
        let colours = Colours::of(&Palette::default());
        let centre = |sink| {
            let row = view.placed.of(Control::Move(40, sink)).expect("unfolded");
            (row.x + BUTTON / 2, row.y + row.h / 2)
        };
        let (x, y) = centre(1);
        assert_eq!(pixel(&picture, x, y), colours.accent, "where it plays");
        let (x, y) = centre(0);
        assert_eq!(
            pixel(&picture, x, y),
            colours.ground,
            "not where the sound is"
        );
    }

    #[test]
    fn a_field_being_typed_in_has_a_caret_and_the_ground_typed_on() {
        let mut state = opened();
        let speakers = number("speakers");
        state.update(Event::Act(
            Control::More(Side::Output, speakers),
            Act::Click,
        ));
        let field = Control::Rename(Side::Output, speakers);
        state.update(Event::Act(field, Act::Focus));
        let colours = Colours::of(&Palette::default());
        let place = |state: &State| {
            state
                .view()
                .expect("open")
                .placed
                .of(field)
                .expect("placed")
        };
        let at = place(&state);
        let ground = (at.right() - 2, at.y + at.h / 2);
        let caret = (at.x + SPACE / 2 + 1, at.y + at.h / 2);
        let picture = drawn(&state, 1);
        assert_eq!(pixel(&picture, ground.0, ground.1), colours.typed);
        assert_eq!(
            pixel(&picture, caret.0, caret.1),
            colours.typed,
            "no caret yet"
        );
        state.update(Event::Key(crate::mixer::Key::Press));
        let picture = drawn(&state, 1);
        assert_eq!(
            pixel(&picture, caret.0, caret.1),
            colours.ink,
            "typing in it"
        );
    }

    #[test]
    fn a_metered_devices_row_has_a_bar_along_its_foot_as_far_as_it_is_loud() {
        let mut state = opened();
        let view = state.view().expect("open");
        let mut mixed = view.mixed.clone();
        mixed.outputs[0].meter = Some("speakers.monitor".to_owned());
        state.update(Event::Heard(mixed));
        let view = state.view().expect("open");
        let row = view
            .placed
            .of(Control::Choose(Side::Output, number("speakers")))
            .expect("listed");
        let colours = Colours::of(&Palette::default());
        let foot = row.bottom() - 2;
        let start = row.x + BUTTON + SPACE / 2 + MARK + SPACE + 1;
        let end = row.right() - SPACE - 2;
        let quiet = drawn(&state, 1);
        assert_ne!(pixel(&quiet, start, foot), colours.accent, "nothing heard");
        let peaks = |peak| HashMap::from([("speakers.monitor".to_owned(), peak)]);
        let full = drawn_with(&state, 1, &peaks(1.0));
        assert_eq!(pixel(&full, start, foot), colours.accent);
        assert_eq!(pixel(&full, end, foot), colours.accent, "all the way");
        let half = drawn_with(&state, 1, &peaks(0.1));
        assert_eq!(pixel(&half, start, foot), colours.accent);
        assert_eq!(
            pixel(&half, end, foot),
            colours.rule,
            "20 dB down, two thirds, the track beyond"
        );
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
        let mute = view
            .placed
            .of(Control::Mute(Whose::Side(Side::Output)))
            .expect("placed");
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
