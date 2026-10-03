//! Text, shaped and drawn by cosmic-text, as the compositor draws its
//! window titles: complex scripts shaped, and a glyph the first font lacks
//! found in another, so a menu item in Japanese is written rather than
//! boxed.
//!
//! Finding the fonts is a scan of the system's font folders, the one slow
//! thing here, so it is done once, on a thread of its own as the shell
//! starts, and waited for only when a menu first needs it.

use std::{collections::HashMap, thread::JoinHandle};

use cosmic_text::{
    Align, Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics,
    Shaping, SwashCache, Wrap,
};
use tiny_skia::PixmapMut;

use crate::layout::{Measure, Rect, menu::TEXT};

/// The fonts, found once.
pub(crate) enum Fonts {
    /// Still being found.
    Finding(JoinHandle<FontSystem>),
    Found(Box<Text>),
    /// Taken while the search is waited for.
    Gone,
}

impl Fonts {
    /// Start finding the fonts.
    pub(crate) fn find() -> Self {
        Self::Finding(std::thread::spawn(FontSystem::new))
    }

    /// The fonts, waiting for them if they are still being found.
    pub(crate) fn get(&mut self) -> &mut Text {
        if let Self::Finding(_) | Self::Gone = self {
            let fonts = match std::mem::replace(self, Self::Gone) {
                Self::Finding(search) => search.join().unwrap_or_else(|_| {
                    tracing::warn!("finding the fonts failed; menus are drawn without text");
                    no_fonts()
                }),
                _ => FontSystem::new(),
            };
            *self = Self::Found(Box::new(Text::new(fonts)));
        }
        match self {
            Self::Found(text) => text,
            Self::Finding(_) | Self::Gone => unreachable!("found just above"),
        }
    }
}

/// The fonts, with what has been measured and drawn from them kept.
pub(crate) struct Text {
    fonts: FontSystem,
    glyphs: SwashCache,
    /// Each label's width at the menu's size, measured once.
    widths: HashMap<String, f32>,
}

impl Text {
    fn new(fonts: FontSystem) -> Self {
        if fonts.db().is_empty() {
            tracing::warn!("no fonts are installed; menus are drawn without text");
        }
        Self {
            fonts,
            glyphs: SwashCache::new(),
            widths: HashMap::new(),
        }
    }

    /// No fonts at all: text is measured as nothing and not drawn. For
    /// tests, which must not depend on the fonts a machine has.
    #[cfg(test)]
    pub(crate) fn without_fonts() -> Self {
        Self::new(no_fonts())
    }

    /// Write `text` in `ink` into `place`, a rectangle of `canvas` in its
    /// pixels, `size` pixels high, vertically centred, cut short with an
    /// ellipsis if it does not fit.
    pub(crate) fn write(
        &mut self,
        canvas: &mut PixmapMut<'_>,
        text: &str,
        place: Rect,
        size: f32,
        ink: [u8; 4],
    ) {
        // cosmic-text panics shaping with no font at all: a machine with
        // none gets menus without text rather than no shell.
        if place.w <= 0 || place.h <= 0 || self.fonts.db().is_empty() {
            return;
        }
        let line = place.h as f32;
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, line));
        buffer.set_wrap(Wrap::None);
        buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
        buffer.set_size(Some(place.w as f32), Some(line));
        buffer.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            Some(Align::Left),
        );
        let (width, height) = (canvas.width() as i32, canvas.height() as i32);
        let (right, bottom) = (place.right().min(width), place.bottom().min(height));
        let pixels = canvas.data_mut();
        buffer.draw(
            &mut self.fonts,
            &mut self.glyphs,
            Color::rgba(ink[0], ink[1], ink[2], ink[3]),
            |x, y, w, h, colour| {
                for row in (place.y + y).max(place.y.max(0))..(place.y + y + h as i32).min(bottom) {
                    for column in
                        (place.x + x).max(place.x.max(0))..(place.x + x + w as i32).min(right)
                    {
                        let at = ((row * width + column) * 4) as usize;
                        over(&mut pixels[at..at + 4], colour);
                    }
                }
            },
        );
    }
}

impl Measure for Fonts {
    fn width(&mut self, text: &str) -> f32 {
        self.get().width(text)
    }
}

impl Measure for Text {
    fn width(&mut self, text: &str) -> f32 {
        if let Some(&width) = self.widths.get(text) {
            return width;
        }
        if self.fonts.db().is_empty() {
            return 0.0;
        }
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(TEXT, TEXT * 2.0));
        buffer.set_wrap(Wrap::None);
        buffer.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.fonts, false);
        let width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max);
        self.widths.insert(text.to_owned(), width);
        width
    }
}

/// A font system with no fonts in it, found without looking.
fn no_fonts() -> FontSystem {
    FontSystem::new_with_locale_and_db("en-US".to_owned(), cosmic_text::fontdb::Database::new())
}

/// Lay `colour`, straight alpha, over one premultiplied RGBA pixel.
fn over(pixel: &mut [u8], colour: Color) {
    let alpha = u32::from(colour.a());
    let keep = 255 - alpha;
    for (channel, value) in pixel.iter_mut().zip([colour.r(), colour.g(), colour.b()]) {
        let premultiplied = u32::from(value) * alpha / 255;
        *channel = (premultiplied + u32::from(*channel) * keep / 255) as u8;
    }
    pixel[3] = (alpha + u32::from(pixel[3]) * keep / 255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_no_fonts_text_is_nothing_rather_than_a_panic() {
        let mut text = Text::without_fonts();
        assert_eq!(text.width("Firefox"), 0.0);
        let mut picture = tiny_skia::Pixmap::new(40, 20).expect("a picture");
        text.write(
            &mut picture.as_mut(),
            "Firefox",
            Rect::new(0, 0, 40, 20),
            14.0,
            [0, 0, 0, 255],
        );
        assert!(picture.data().iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_glyph_edge_is_premultiplied_and_a_full_one_replaces() {
        let mut pixel = [0u8; 4];
        over(&mut pixel, Color::rgba(255, 255, 255, 128));
        assert_eq!(pixel, [128, 128, 128, 128], "half white, premultiplied");
        over(&mut pixel, Color::rgba(255, 0, 0, 255));
        assert_eq!(pixel, [255, 0, 0, 255], "opaque red over it");
    }
}
