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
use perspicax_policy::{FrameButton, Rect, buttons_in};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};

use crate::framed::{Framed, Title, TitleKey};

/// Room left either side of the title, in logical pixels.
const PADDING: i32 = 8;

/// The text's size, as a share of the titlebar's height.
const TEXT_SHARE: f32 = 0.5;

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

    /// Make sure `window` has its title, `text`, drawn at the whole scale
    /// `scale` in the ink its titlebar reads best in. Does nothing if it
    /// already has.
    pub(super) fn prepare(&mut self, window: &Framed, text: &str, scale: i32) {
        let (Some(area), Some(ink)) = (window.title_at(), window.ink()) else {
            return;
        };
        let key = TitleKey {
            text: text.to_owned(),
            size: (area.w, area.h),
            ink,
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
        let taken: i32 = buttons.iter().map(|(_, place)| place.w * key.scale).sum();

        let padding = PADDING * key.scale;
        let line = height as f32;
        let mut text = Buffer::new(&mut self.fonts, Metrics::new(line * TEXT_SHARE, line));
        text.set_wrap(Wrap::None);
        text.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
        let room = width - taken - 2 * padding;
        if room <= 0 {
            return (pixels, (width, height));
        }
        text.set_size(Some(room as f32), Some(line));
        text.set_text(
            &key.text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            Some(Align::Left),
        );

        let limit = padding + room;
        text.draw(
            &mut self.fonts,
            &mut self.glyphs,
            ink,
            |x, y, w, h, colour| {
                for row in y.max(0)..(y + h as i32).min(height) {
                    for column in (x + padding).max(0)..(x + padding + w as i32).min(limit) {
                        let at = ((row * width + column) * 4) as usize;
                        over(&mut pixels[at..at + 4], colour);
                    }
                }
            },
        );
        (pixels, (width, height))
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
            text: text.to_owned(),
            size: (200, 24),
            ink: perspicax_policy::Colour::rgb(0xff, 0xff, 0xff),
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

    #[test]
    fn a_glyph_edge_is_premultiplied_and_a_full_one_replaces() {
        let mut pixel = [0u8; 4];
        over(&mut pixel, Color::rgba(255, 255, 255, 128));
        assert_eq!(pixel, [128, 128, 128, 128], "half white, premultiplied");
        over(&mut pixel, Color::rgba(255, 0, 0, 255));
        assert_eq!(pixel, [0, 0, 255, 255], "opaque red over it, blue first");
    }
}
