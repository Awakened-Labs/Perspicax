//! Window titles, shaped and rasterised for the titlebar, with the
//! titlebar's buttons beside them.
//!
//! cosmic-text does the text: it shapes complex scripts and, when the first
//! font has no glyph for a character, falls back across the system's fonts,
//! so a title in Japanese or ending in an emoji is written rather than boxed.
//! Finding the fonts is a scan of the system's font directories, done once
//! when the seat starts. It is the one slow thing here, so it runs on a
//! thread of its own while the seat comes up: the monitors are dark until the
//! first frame, and the first frame needs no fonts. The first title written
//! waits for the scan, if it is still going. Headless never does it, because
//! headless draws nothing.
//!
//! The buttons are drawn as shapes rather than as characters from a font:
//! a font that lacks a ballot X would leave a window that cannot be closed
//! from its frame.
//!
//! A title is drawn again only when what it was drawn from changes: its text,
//! the room it has, the colour it is inked in, or the whole scale of the
//! output it shows on. Everything else is a cached buffer, and its unchanged
//! id is what lets the damage tracker leave the titlebar alone.

use std::thread::JoinHandle;

use cosmic_text::{
    Align, Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics,
    Shaping, SwashCache, Wrap, fontdb,
};
use perspicax_policy::{FrameButton, Rect, buttons_in, tab_rects};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};

use crate::{
    framed::{Framed, Title, TitleKey},
    state::Compositor,
};

/// Room left either side of the title, in logical pixels.
const PADDING: i32 = 8;

/// The text's size, as a share of the titlebar's height.
const TEXT_SHARE: f32 = 0.5;

/// How opaque the label of a tab behind the front one is, out of 255.
const BEHIND: u8 = 150;

/// What a window's titlebar shows that only the compositor can say,
/// gathered before drawing, while it can still be asked.
pub(super) struct Bar {
    /// One label per tab, the window's own title when it is in no group.
    pub(super) labels: Vec<String>,
    /// Which tab is in front.
    pub(super) front: usize,
    /// Whether it is on every workspace.
    pub(super) sticky: bool,
    /// Whether it is maximized, as its maximize button means it.
    pub(super) maximized: bool,
}

/// The fonts, and the glyphs already rasterised from them.
pub(super) struct Titles {
    fonts: Fonts,
    glyphs: SwashCache,
}

impl Titles {
    /// Start finding the system's fonts, and return without waiting for them.
    pub(super) fn new() -> Self {
        Self {
            fonts: Fonts::find(),
            glyphs: SwashCache::new(),
        }
    }

    /// Make sure `window` has its titlebar drawn at the whole scale `scale`
    /// in its ink and the theme's `family`: `bar`'s labels, and `buttons` at
    /// the bar's two ends, each toggle showing its state. Does nothing if it
    /// already has.
    pub(super) fn prepare(
        &mut self,
        window: &Framed,
        bar: Bar,
        scale: i32,
        family: &perspicax_policy::Family,
        buttons: perspicax_policy::Buttons,
    ) {
        let (Some(area), Some(ink)) = (window.title_at(), window.ink()) else {
            return;
        };
        let key = TitleKey {
            labels: bar.labels,
            front: bar.front,
            size: (area.w, area.h),
            ink,
            family: family.clone(),
            scale,
            buttons,
            shades: Compositor::is_server_decorated(window),
            shaded: crate::shell::is_shaded(window),
            sticky: bar.sticky,
            maximized: bar.maximized,
        };
        if window.has_title(&key) {
            return;
        }
        let image = self.rasterise(&key);
        window.put_title(Title { key, image });
    }

    /// Draw `key`'s text into a buffer its size times its scale.
    fn rasterise(&mut self, key: &TitleKey) -> MemoryRenderBuffer {
        let (pixels, size) = self.pixels(key);
        MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            size,
            key.scale,
            Transform::Normal,
            None,
        )
    }

    /// `key`'s titlebar as premultiplied ARGB8888, and its size in pixels.
    fn pixels(&mut self, key: &TitleKey) -> (Vec<u8>, (i32, i32)) {
        let (width, height) = (
            (key.size.0 * key.scale).max(1),
            (key.size.1 * key.scale).max(1),
        );
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let ink = Color::rgb(key.ink.r, key.ink.g, key.ink.b);

        // The buttons, in pixels: the same places the pointer finds them.
        let bar = Rect::new(0, 0, key.size.0, key.size.1);
        let buttons = buttons_in(bar, &key.buttons, key.shades);
        for (button, place) in &buttons {
            let place = Rect::new(
                place.x * key.scale,
                place.y * key.scale,
                place.w * key.scale,
                place.h * key.scale,
            );
            draw_button(
                &mut pixels,
                width,
                *button,
                place,
                ink,
                key.scale,
                key.toggles(),
            );
        }

        // One label per tab, in the same places the pointer finds the tabs.
        // With more than one, the tabs behind are written fainter, the one in
        // front is underlined, and a thin rule parts each from the next.
        let tabs = tab_rects(bar, key.labels.len(), &key.buttons, key.shades);
        let grouped = tabs.len() > 1;
        for (n, (place, label)) in tabs.iter().zip(&key.labels).enumerate() {
            let place = Rect::new(
                place.x * key.scale,
                place.y * key.scale,
                place.w * key.scale,
                place.h * key.scale,
            );
            let front = n == key.front;
            let shade = if grouped && !front { BEHIND } else { 255 };
            let inked = Color::rgba(ink.r(), ink.g(), ink.b(), shade);
            self.write(&mut pixels, width, label, place, inked, key);
            if grouped && front {
                let rule = Rect::new(
                    place.x,
                    place.y + place.h - 2 * key.scale,
                    place.w,
                    2 * key.scale,
                );
                fill(&mut pixels, width, rule, ink);
            }
            if grouped && n > 0 {
                let rule = Rect::new(place.x, place.y + place.h / 4, key.scale, place.h / 2);
                fill(
                    &mut pixels,
                    width,
                    rule,
                    Color::rgba(ink.r(), ink.g(), ink.b(), BEHIND),
                );
            }
        }
        (pixels, (width, height))
    }

    /// Write `label` into `place`, a rect of a buffer `width` pixels wide,
    /// inside its padding, cut short with an ellipsis if it does not fit, at
    /// `key`'s scale and in its family.
    fn write(
        &mut self,
        pixels: &mut [u8],
        width: i32,
        label: &str,
        place: Rect,
        ink: Color,
        key: &TitleKey,
    ) {
        let padding = PADDING * key.scale;
        let room = place.w - 2 * padding;
        if room <= 0 {
            return;
        }
        let fonts = self.fonts.found();
        // With no fonts at all cosmic-text has nothing to shape with, and
        // panics rather than say so.
        if fonts.db().is_empty() {
            return;
        }
        let line = place.h as f32;
        let mut text = Buffer::new(fonts, Metrics::new(line * TEXT_SHARE, line));
        text.set_wrap(Wrap::None);
        text.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
        text.set_size(Some(room as f32), Some(line));
        text.set_text(
            label,
            &Attrs::new().family(family(&key.family)),
            Shaping::Advanced,
            Some(Align::Left),
        );
        let (left, right) = (place.x + padding, place.x + padding + room);
        let bottom = (place.y + place.h).min(pixels.len() as i32 / 4 / width.max(1));
        text.draw(fonts, &mut self.glyphs, ink, |x, y, w, h, colour| {
            for row in (place.y + y).max(place.y)..(place.y + y + h as i32).min(bottom) {
                for column in (left + x).max(left)..(left + x + w as i32).min(right) {
                    let at = ((row * width + column) * 4) as usize;
                    over(&mut pixels[at..at + 4], colour);
                }
            }
        });
    }
}

/// The system's fonts, found on a thread of their own and waited for only
/// when a title is first written.
struct Fonts {
    /// The scan, until it is waited for.
    finding: Option<JoinHandle<FontSystem>>,
    /// What it found: none until then, and none for good if it panicked.
    found: FontSystem,
}

impl Fonts {
    /// Start the scan. Where no thread can be had, it is done here and now.
    fn find() -> Self {
        match std::thread::Builder::new()
            .name("perspicax-fonts".to_owned())
            .spawn(FontSystem::new)
        {
            Ok(finding) => Self {
                finding: Some(finding),
                found: none(),
            },
            Err(error) => {
                tracing::warn!(%error, "could not start a thread to find the fonts; finding them now");
                Self {
                    finding: None,
                    found: FontSystem::new(),
                }
            }
        }
    }

    /// The fonts, once the scan is done, waiting for it if need be. A scan
    /// that panicked leaves none: titles are written without their text, and
    /// the buttons, which need no font, are still drawn.
    fn found(&mut self) -> &mut FontSystem {
        if let Some(finding) = self.finding.take() {
            match finding.join() {
                Ok(fonts) => self.found = fonts,
                Err(_) => tracing::error!("finding the fonts failed; titles go without their text"),
            }
        }
        &mut self.found
    }
}

/// No fonts at all. The locale only chooses among fallback fonts, and there
/// are none to choose among.
fn none() -> FontSystem {
    FontSystem::new_with_locale_and_db(String::from("en-US"), fontdb::Database::new())
}

/// The theme's family as cosmic-text names it. A named family missing from
/// the system falls back, glyph by glyph, to whatever has the character.
fn family(family: &perspicax_policy::Family) -> Family<'_> {
    match family {
        perspicax_policy::Family::SansSerif => Family::SansSerif,
        perspicax_policy::Family::Serif => Family::Serif,
        perspicax_policy::Family::Monospace => Family::Monospace,
        perspicax_policy::Family::Named(name) => Family::Name(name),
    }
}

/// The window's toggles, as its buttons show them.
#[derive(Debug, Clone, Copy)]
struct Toggles {
    /// Rolled up to its titlebar.
    shaded: bool,
    /// On every workspace.
    stuck: bool,
    maximized: bool,
}

impl TitleKey {
    fn toggles(&self) -> Toggles {
        Toggles {
            shaded: self.shaded,
            stuck: self.sticky,
            maximized: self.maximized,
        }
    }
}

/// Draw `button`'s symbol, centred in `place`, in pixels of a buffer
/// `width` wide: a cross to close, a square to maximize, a bar along the
/// bottom to minimize, one along the top to roll up, and a pin, a ring on a
/// short stem, to keep the window on every workspace.
///
/// Each toggle shows its state, as `toggles` has it. Rolled up already, a
/// chevron under the shade button's bar points down, the way it unrolls.
/// On every workspace, the pin's head is filled. Maximized, two squares, one
/// behind the other, restore it. Each pixel is inked by how much of it the
/// shape covers, so the diagonals of the cross and the chevron, and the
/// pin's ring, are smooth.
fn draw_button(
    pixels: &mut [u8],
    width: i32,
    button: FrameButton,
    place: Rect,
    ink: Color,
    scale: i32,
    toggles: Toggles,
) {
    let half = place.h as f32 * 0.2;
    let stroke = 1.5 * scale as f32;
    let (cx, cy) = (
        place.x as f32 + place.w as f32 / 2.0,
        place.y as f32 + place.h as f32 / 2.0,
    );
    // The chevron's arms, from either side of it down to its point.
    let (arm, point) = ((half * 0.7, -half * 0.25), (0.0, half * 0.6));
    // The pin's ring, above the middle, and how big round it is.
    let (ring, round) = (-half * 0.3, half * 0.7);
    // How far inside a square's edge a point is, as maximize's square is
    // drawn, or nowhere for a point outside it. Restore's two are offset
    // from the middle, the one in front down and to the left.
    let inside = |x: f32, y: f32, (cx, cy): (f32, f32), size: f32| {
        let reach = (x - cx).abs().max((y - cy).abs());
        if reach <= size {
            size - reach
        } else {
            f32::INFINITY
        }
    };
    let (front, back, size) = (
        (-half * 0.25, half * 0.25),
        (half * 0.25, -half * 0.25),
        half * 0.75,
    );
    // How far a pixel's centre is from the shape's line, as `x, y` from the
    // middle of the button.
    let distance = |x: f32, y: f32| match button {
        FrameButton::Close if x.abs() <= half && y.abs() <= half => {
            (x - y).abs().min((x + y).abs()) / std::f32::consts::SQRT_2
        }
        // Behind the front square, the one behind is hidden.
        FrameButton::Maximize if toggles.maximized => match inside(x, y, front, size) {
            over if over.is_finite() => over,
            _ => inside(x, y, back, size),
        },
        FrameButton::Maximize if x.abs() <= half && y.abs() <= half => half - x.abs().max(y.abs()),
        FrameButton::Minimize if x.abs() <= half => (y - half).abs(),
        FrameButton::Shade if x.abs() <= half && toggles.shaded => (y + half)
            .abs()
            .min(segment((x, y), (-arm.0, arm.1), point))
            .min(segment((x, y), arm, point)),
        FrameButton::Shade if x.abs() <= half => (y + half).abs(),
        FrameButton::Sticky if x.abs() <= half && y.abs() <= half => {
            let edge = x.hypot(y - ring) - round;
            let head = if toggles.stuck {
                edge.max(0.0)
            } else {
                edge.abs()
            };
            head.min(segment((x, y), (0.0, ring + round), (0.0, half)))
        }
        _ => f32::INFINITY,
    };
    for row in place.y..place.y + place.h {
        for column in place.x..(place.x + place.w).min(width) {
            let (x, y) = (column as f32 + 0.5 - cx, row as f32 + 0.5 - cy);
            let cover = (stroke / 2.0 + 0.5 - distance(x, y)).clamp(0.0, 1.0);
            if cover > 0.0 {
                let alpha = (cover * f32::from(ink.a())) as u8;
                let at = ((row * width + column) * 4) as usize;
                over(
                    &mut pixels[at..at + 4],
                    Color::rgba(ink.r(), ink.g(), ink.b(), alpha),
                );
            }
        }
    }
}

/// How far `at` is from the line from `from` to `to`, ends and all.
fn segment(at: (f32, f32), from: (f32, f32), to: (f32, f32)) -> f32 {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let along = ((at.0 - from.0) * dx + (at.1 - from.1) * dy) / (dx * dx + dy * dy);
    let along = along.clamp(0.0, 1.0);
    (at.0 - from.0 - along * dx).hypot(at.1 - from.1 - along * dy)
}

/// Lay `colour` over every pixel of `place`.
fn fill(pixels: &mut [u8], width: i32, place: Rect, colour: Color) {
    let height = pixels.len() as i32 / 4 / width.max(1);
    for row in place.y.max(0)..(place.y + place.h).min(height) {
        for column in place.x.max(0)..(place.x + place.w).min(width) {
            let at = ((row * width + column) * 4) as usize;
            over(&mut pixels[at..at + 4], colour);
        }
    }
}

/// Lay `colour` over one ARGB8888 pixel, premultiplied: the renderer blends
/// premultiplied alpha, and a straight-alpha glyph edge would come out dark.
/// The bytes are little-endian, so blue comes first.
fn over(pixel: &mut [u8], colour: Color) {
    let alpha = u32::from(colour.a());
    let keep = 255 - alpha;
    let source = [colour.b(), colour.g(), colour.r()];
    for (channel, value) in pixel.iter_mut().zip(source) {
        let premultiplied = u32::from(value) * alpha / 255;
        *channel = (premultiplied + u32::from(*channel) * keep / 255) as u8;
    }
    pixel[3] = (alpha + u32::from(pixel[3]) * keep / 255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against whatever fonts this machine has: a container with none draws
    /// nothing, and that is not what this test is about.
    #[test]
    fn a_title_is_written_inside_its_padding_and_cut_short_when_long() {
        let mut titles = Titles::new();
        if titles.fonts.found().db().is_empty() {
            return;
        }
        let key = |text: &str| TitleKey {
            labels: vec![text.to_owned()],
            front: 0,
            size: (200, 24),
            ink: perspicax_policy::Colour::rgb(0xff, 0xff, 0xff),
            family: perspicax_policy::Family::SansSerif,
            scale: 2,
            buttons: perspicax_policy::Buttons::default(),
            shades: true,
            shaded: false,
            sticky: false,
            maximized: false,
        };
        let (pixels, (width, height)) = titles.pixels(&key("Terminal — foot"));
        assert_eq!((width, height), (400, 48), "drawn at twice the size");
        let inked = |column: i32| {
            (0..height).any(|row| pixels[((row * width + column) * 4 + 3) as usize] > 0)
        };
        assert!((0..width).any(inked), "something was written");
        assert!(
            (0..PADDING * 2).all(|column| !inked(column)),
            "nothing in the left padding"
        );

        // Four buttons of the bar's height at the right, and the padding
        // before them, which a long title stops short of.
        let buttons = 4 * height;
        let (long, _) = titles.pixels(&key(&"a very long title ".repeat(20)));
        let column_inked = |pixels: &[u8], column: i32| {
            (0..height).any(|row| pixels[((row * width + column) * 4 + 3) as usize] > 0)
        };
        let gap = (width - buttons - PADDING * 2)..(width - buttons);
        assert!(
            gap.clone().all(|column| !column_inked(&long, column)),
            "a long title stops before the buttons' padding"
        );
        let close = (width - height)..width;
        assert!(
            close.clone().any(|column| column_inked(&long, column)),
            "and the close button is drawn in the corner"
        );
    }

    #[test]
    fn the_fonts_used_are_the_ones_the_thread_found_waited_for_once() {
        let mut fonts = Fonts {
            finding: Some(std::thread::spawn(|| {
                FontSystem::new_with_locale_and_db(String::from("la"), fontdb::Database::new())
            })),
            found: none(),
        };
        assert_eq!(fonts.found().locale(), "la", "the thread's, not none");
        assert!(fonts.finding.is_none(), "waited for once");
    }

    /// A scan that panicked leaves no fonts, as a machine with none has none.
    #[test]
    fn with_no_fonts_a_title_is_its_buttons_alone() {
        let mut titles = Titles {
            fonts: Fonts {
                finding: Some(std::thread::spawn(|| {
                    panic!("a scan that falls over, on purpose")
                })),
                found: none(),
            },
            glyphs: SwashCache::new(),
        };
        let key = TitleKey {
            labels: vec!["Terminal — foot".to_owned()],
            front: 0,
            size: (200, 24),
            ink: perspicax_policy::Colour::rgb(0xff, 0xff, 0xff),
            family: perspicax_policy::Family::SansSerif,
            scale: 1,
            buttons: perspicax_policy::Buttons::default(),
            shades: true,
            shaded: false,
            sticky: false,
            maximized: false,
        };
        let (pixels, (width, height)) = titles.pixels(&key);
        let inked = |column: i32| {
            (0..height).any(|row| pixels[((row * width + column) * 4 + 3) as usize] > 0)
        };
        let buttons = 4 * height;
        assert!((0..width - buttons).all(|column| !inked(column)), "no text");
        assert!(
            ((width - height)..width).any(inked),
            "and the close button, which needs no font"
        );
    }

    /// The underline is drawn without fonts, so this runs anywhere.
    #[test]
    fn the_tab_in_front_is_underlined_and_the_others_are_not() {
        let mut titles = Titles::new();
        let key = TitleKey {
            labels: vec!["one".to_owned(), "two".to_owned()],
            front: 1,
            size: (200, 24),
            ink: perspicax_policy::Colour::rgb(0xff, 0xff, 0xff),
            family: perspicax_policy::Family::SansSerif,
            scale: 1,
            buttons: perspicax_policy::Buttons::default(),
            shades: true,
            shaded: false,
            sticky: false,
            maximized: false,
        };
        let (pixels, (width, height)) = titles.pixels(&key);
        let tabs = tab_rects(
            Rect::new(0, 0, 200, 24),
            2,
            &perspicax_policy::Buttons::default(),
            true,
        );
        let underlined = |tab: Rect| {
            let row = height - 1;
            (tab.x + PADDING..tab.x + tab.w - PADDING)
                .all(|column| pixels[((row * width + column) * 4 + 3) as usize] == 255)
        };
        assert!(underlined(tabs[1]), "the front tab");
        assert!(!underlined(tabs[0]), "not the one behind");
    }

    /// The shade button is a bar along the top, the other way up from
    /// minimize's. Rolled up, a chevron under it points down, and nothing
    /// else on the bar changes. Buttons need no fonts, so this runs anywhere.
    #[test]
    fn the_shade_button_is_a_bar_along_the_top_and_points_down_while_rolled_up() {
        let mut titles = Titles {
            fonts: Fonts {
                finding: None,
                found: none(),
            },
            glyphs: SwashCache::new(),
        };
        let key = |shaded| TitleKey {
            labels: vec!["Terminal — foot".to_owned()],
            front: 0,
            size: (200, 24),
            ink: perspicax_policy::Colour::rgb(0xff, 0xff, 0xff),
            family: perspicax_policy::Family::SansSerif,
            scale: 1,
            buttons: perspicax_policy::Buttons::default(),
            shades: true,
            shaded,
            sticky: false,
            maximized: false,
        };
        let (unrolled, (width, _)) = titles.pixels(&key(false));
        let (rolled, _) = titles.pixels(&key(true));
        let (_, place) = buttons_in(
            Rect::new(0, 0, 200, 24),
            &perspicax_policy::Buttons::default(),
            true,
        )
        .into_iter()
        .find(|(button, _)| *button == FrameButton::Shade)
        .expect("a shade button");
        let inked = |pixels: &[u8], row: i32| -> Vec<i32> {
            (place.x..place.x + place.w)
                .filter(|column| pixels[((row * width + column) * 4 + 3) as usize] > 0)
                .collect()
        };
        let middle = place.y + place.h / 2;
        let (top, bottom) = (place.y..middle, middle..place.y + place.h);
        assert!(top.clone().any(|row| !inked(&unrolled, row).is_empty()));
        assert!(
            bottom.clone().all(|row| inked(&unrolled, row).is_empty()),
            "a bar along the top, and nothing under it"
        );
        let lowest = bottom
            .rev()
            .find(|row| !inked(&rolled, *row).is_empty())
            .expect("rolled up, something under the bar");
        let centre = place.x + place.w / 2;
        assert!(
            inked(&rolled, lowest)
                .iter()
                .all(|column| (column - centre).abs() <= 2),
            "its point at the bottom, in the middle: {:?}",
            inked(&rolled, lowest)
        );
        let outside = |pixels: &[u8]| -> Vec<u8> {
            pixels
                .chunks(4)
                .enumerate()
                .filter(|(n, _)| {
                    let column = i32::try_from(*n).unwrap() % width;
                    column < place.x || column >= place.x + place.w
                })
                .flat_map(|(_, pixel)| pixel.to_vec())
                .collect()
        };
        assert!(
            outside(&rolled) == outside(&unrolled),
            "the rest is as it was"
        );

        // A strip of tabs over a frame its client draws has no shade button.
        let (strip, _) = titles.pixels(&TitleKey {
            shades: false,
            ..key(false)
        });
        assert!(
            (place.y..place.y + place.h).all(|row| inked(&strip, row).is_empty()),
            "no shade button on a strip of tabs"
        );
    }

    /// minimal's bar: sticky at the left, the other four at the right.
    fn fluxbox() -> perspicax_policy::Buttons {
        use FrameButton::*;
        perspicax_policy::Buttons::new(&[Sticky], &[Shade, Minimize, Maximize, Close])
            .expect("five buttons, once each")
    }

    /// No fonts, so all that is drawn is the buttons.
    fn fontless() -> Titles {
        Titles {
            fonts: Fonts {
                finding: None,
                found: none(),
            },
            glyphs: SwashCache::new(),
        }
    }

    fn bar(buttons: perspicax_policy::Buttons) -> TitleKey {
        TitleKey {
            labels: vec!["Terminal — foot".to_owned()],
            front: 0,
            size: (200, 24),
            ink: perspicax_policy::Colour::rgb(0xff, 0xff, 0xff),
            family: perspicax_policy::Family::SansSerif,
            scale: 1,
            buttons,
            shades: true,
            shaded: false,
            sticky: false,
            maximized: false,
        }
    }

    /// Against whatever fonts this machine has, as the first test is: the
    /// title is written after the buttons at the left, and their padding.
    #[test]
    fn a_title_starts_after_the_left_hand_buttons() {
        let mut titles = Titles::new();
        if titles.fonts.found().db().is_empty() {
            return;
        }
        let (pixels, (width, height)) = titles.pixels(&bar(fluxbox()));
        let inked = |column: i32| {
            (0..height).any(|row| pixels[((row * width + column) * 4 + 3) as usize] > 0)
        };
        assert!((0..height).any(inked), "the sticky button, at the left end");
        assert!(
            (height..height + PADDING).all(|column| !inked(column)),
            "then the padding before the title"
        );
        assert!(
            (height + PADDING..width - 4 * height).any(inked),
            "then the title"
        );
    }

    /// Buttons need no fonts, so this runs anywhere.
    #[test]
    fn a_sticky_button_is_drawn_where_the_pointer_finds_it() {
        let mut titles = fontless();
        let found = |buttons| {
            buttons_in(Rect::new(0, 0, 200, 24), &buttons, true)
                .into_iter()
                .find(|(button, _)| *button == FrameButton::Sticky)
                .map(|(_, place)| place)
        };
        let place = found(fluxbox()).expect("minimal's bar has one");
        let inked = |pixels: &[u8], width: i32| {
            (place.y..place.y + place.h).any(|row| {
                (place.x..place.x + place.w)
                    .any(|column| pixels[((row * width + column) * 4 + 3) as usize] > 0)
            })
        };
        let (pinned, (width, _)) = titles.pixels(&bar(fluxbox()));
        assert!(inked(&pinned, width), "the pin, where the pointer finds it");
        assert_eq!(found(perspicax_policy::Buttons::default()), None);
        let (plain, _) = titles.pixels(&bar(perspicax_policy::Buttons::default()));
        assert!(!inked(&plain, width), "and nothing there on classic's bar");
    }

    /// What of a bar `width` pixels wide is outside `place`, byte by byte.
    fn outside(pixels: &[u8], width: i32, place: Rect) -> Vec<u8> {
        pixels
            .chunks(4)
            .enumerate()
            .filter(|(n, _)| {
                let n = i32::try_from(*n).unwrap();
                let (column, row) = (n % width, n / width);
                column < place.x
                    || column >= place.x + place.w
                    || row < place.y
                    || row >= place.y + place.h
            })
            .flat_map(|(_, pixel)| pixel.to_vec())
            .collect()
    }

    /// How much ink there is in `place`.
    fn ink_in(pixels: &[u8], width: i32, place: Rect) -> u32 {
        (place.y..place.y + place.h)
            .flat_map(|row| (place.x..place.x + place.w).map(move |column| (row, column)))
            .map(|(row, column)| u32::from(pixels[((row * width + column) * 4 + 3) as usize]))
            .sum()
    }

    fn place_of(buttons: perspicax_policy::Buttons, wanted: FrameButton) -> Rect {
        buttons_in(Rect::new(0, 0, 200, 24), &buttons, true)
            .into_iter()
            .find(|(button, _)| *button == wanted)
            .map(|(_, place)| place)
            .expect("on the bar")
    }

    /// On every workspace, the pin's head is filled: more ink, where the
    /// pin is, and none of the rest of the bar changes. Buttons need no
    /// fonts, so this runs anywhere.
    #[test]
    fn a_stuck_window_fills_its_sticky_button_and_nothing_else_changes() {
        let mut titles = fontless();
        let (loose, (width, _)) = titles.pixels(&bar(fluxbox()));
        let (stuck, _) = titles.pixels(&TitleKey {
            sticky: true,
            ..bar(fluxbox())
        });
        let place = place_of(fluxbox(), FrameButton::Sticky);
        assert!(
            ink_in(&stuck, width, place) > ink_in(&loose, width, place) * 3 / 2,
            "filled: {} against {}",
            ink_in(&stuck, width, place),
            ink_in(&loose, width, place)
        );
        assert!(
            outside(&stuck, width, place) == outside(&loose, width, place),
            "the rest is as it was"
        );
    }

    /// Maximized, the maximize button shows restore, two squares, the one
    /// behind showing above and to the right of the one in front, and none
    /// of the rest of the bar changes.
    #[test]
    fn a_maximized_window_shows_restore() {
        let mut titles = fontless();
        let buttons = perspicax_policy::Buttons::default();
        let (plain, (width, _)) = titles.pixels(&bar(buttons));
        let (restore, _) = titles.pixels(&TitleKey {
            maximized: true,
            ..bar(buttons)
        });
        let place = place_of(buttons, FrameButton::Maximize);
        assert!(restore != plain, "a shape of its own");
        assert!(
            outside(&restore, width, place) == outside(&plain, width, place),
            "the rest is as it was"
        );
        // Just right of the middle is the right-hand edge of restore's
        // square in front, which maximize's one square, wider, has none of.
        let inked = |pixels: &[u8]| {
            let (row, column) = (place.y + place.h / 2, place.x + place.w / 2 + 1);
            pixels[((row * width + column) * 4 + 3) as usize] > 0
        };
        assert!(inked(&restore), "the front square's edge");
        assert!(!inked(&plain), "inside maximize's square");
    }

    #[test]
    fn a_glyph_edge_is_premultiplied_and_a_full_one_replaces() {
        let mut pixel = [0u8; 4];
        over(&mut pixel, Color::rgba(255, 255, 255, 128));
        assert_eq!(pixel, [128, 128, 128, 128], "half white, premultiplied");
        over(&mut pixel, Color::rgba(255, 0, 0, 255));
        assert_eq!(pixel, [0, 0, 255, 255], "opaque red over it, blue first");
    }
}
