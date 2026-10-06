//! Window titles, shaped and rasterised for the titlebar, with the
//! titlebar's buttons beside them.
//!
//! cosmic-text does the text: it shapes complex scripts and, when the first
//! font has no glyph for a character, falls back across the system's fonts,
//! so a title in Japanese or ending in an emoji is written rather than boxed.
//! Finding the fonts is a scan of the system's font directories, done once
//! when the seat starts. It is the one slow thing here, and headless never
//! does it, because headless draws nothing.
//!
//! The buttons are drawn as shapes rather than as characters from a font:
//! a font that lacks a ballot X would leave a window that cannot be closed
//! from its frame.
//!
//! A title is drawn again only when what it was drawn from changes: its text,
//! the room it has, the colour it is inked in, or the whole scale of the
//! output it shows on. Everything else is a cached buffer, and its unchanged
//! id is what lets the damage tracker leave the titlebar alone.

use cosmic_text::{
    Align, Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics,
    Shaping, SwashCache, Wrap,
};
use perspicax_policy::{FrameButton, Rect, buttons_in, tab_rects};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};

use crate::framed::{Framed, Title, TitleKey};

/// Room left either side of the title, in logical pixels.
const PADDING: i32 = 8;

/// The text's size, as a share of the titlebar's height.
const TEXT_SHARE: f32 = 0.5;

/// How opaque the label of a tab behind the front one is, out of 255.
const BEHIND: u8 = 150;

/// The fonts, and the glyphs already rasterised from them.
pub(super) struct Titles {
    fonts: FontSystem,
    glyphs: SwashCache,
}

impl Titles {
    pub(super) fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            glyphs: SwashCache::new(),
        }
    }

    /// Make sure `window` has its titlebar drawn at the whole scale `scale`
    /// in its ink and the theme's `family`: `labels`, one per tab, with
    /// `front` the one in front. Does nothing if it already has.
    pub(super) fn prepare(
        &mut self,
        window: &Framed,
        labels: Vec<String>,
        front: usize,
        scale: i32,
        family: &perspicax_policy::Family,
    ) {
        let (Some(area), Some(ink)) = (window.title_at(), window.ink()) else {
            return;
        };
        let key = TitleKey {
            labels,
            front,
            size: (area.w, area.h),
            ink,
            family: family.clone(),
            scale,
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
        let buttons = buttons_in(bar);
        for (button, place) in &buttons {
            let place = Rect::new(
                place.x * key.scale,
                place.y * key.scale,
                place.w * key.scale,
                place.h * key.scale,
            );
            draw_button(&mut pixels, width, *button, place, ink, key.scale);
        }

        // One label per tab, in the same places the pointer finds the tabs.
        // With more than one, the tabs behind are written fainter, the one in
        // front is underlined, and a thin rule parts each from the next.
        let tabs = tab_rects(bar, key.labels.len());
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
        let line = place.h as f32;
        let mut text = Buffer::new(&mut self.fonts, Metrics::new(line * TEXT_SHARE, line));
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
        text.draw(
            &mut self.fonts,
            &mut self.glyphs,
            ink,
            |x, y, w, h, colour| {
                for row in (place.y + y).max(place.y)..(place.y + y + h as i32).min(bottom) {
                    for column in (left + x).max(left)..(left + x + w as i32).min(right) {
                        let at = ((row * width + column) * 4) as usize;
                        over(&mut pixels[at..at + 4], colour);
                    }
                }
            },
        );
    }
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

/// Draw `button`'s symbol, centred in `place`, in pixels of a buffer
/// `width` wide: a cross to close, a square to maximize, a bar to minimize.
/// Each pixel is inked by how much of it the shape covers, so the diagonals
/// of the cross are smooth.
fn draw_button(
    pixels: &mut [u8],
    width: i32,
    button: FrameButton,
    place: Rect,
    ink: Color,
    scale: i32,
) {
    let half = place.h as f32 * 0.2;
    let stroke = 1.5 * scale as f32;
    let (cx, cy) = (
        place.x as f32 + place.w as f32 / 2.0,
        place.y as f32 + place.h as f32 / 2.0,
    );
    // How far a pixel's centre is from the shape's line, as `x, y` from the
    // middle of the button.
    let distance = |x: f32, y: f32| match button {
        FrameButton::Close if x.abs() <= half && y.abs() <= half => {
            (x - y).abs().min((x + y).abs()) / std::f32::consts::SQRT_2
        }
        FrameButton::Maximize if x.abs() <= half && y.abs() <= half => half - x.abs().max(y.abs()),
        FrameButton::Minimize if x.abs() <= half => (y - half).abs(),
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
        if titles.fonts.db().is_empty() {
            return;
        }
        let key = |text: &str| TitleKey {
            labels: vec![text.to_owned()],
            front: 0,
            size: (200, 24),
            ink: perspicax_policy::Colour::rgb(0xff, 0xff, 0xff),
            family: perspicax_policy::Family::SansSerif,
            scale: 2,
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

        // Three buttons of the bar's height at the right, and the padding
        // before them, which a long title stops short of.
        let buttons = 3 * height;
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
        };
        let (pixels, (width, height)) = titles.pixels(&key);
        let tabs = tab_rects(Rect::new(0, 0, 200, 24), 2);
        let underlined = |tab: Rect| {
            let row = height - 1;
            (tab.x + PADDING..tab.x + tab.w - PADDING)
                .all(|column| pixels[((row * width + column) * 4 + 3) as usize] == 255)
        };
        assert!(underlined(tabs[1]), "the front tab");
        assert!(!underlined(tabs[0]), "not the one behind");
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
