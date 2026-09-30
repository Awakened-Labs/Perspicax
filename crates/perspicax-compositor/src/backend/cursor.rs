//! The arrow the compositor draws when no client is drawing its own.
//!
//! Read once from the person's Xcursor theme (`XCURSOR_THEME`,
//! `XCURSOR_SIZE`, the same variables every toolkit reads), and drawn from
//! then on as one texture. If there is no theme at all, a plain arrow is
//! generated rather than drawing nothing: a session whose pointer is
//! invisible looks like one whose mouse is dead.
//!
//! One image, not the full set of named shapes. A client that wants a text
//! beam or a resize arrow still gets it by attaching its own cursor surface,
//! which GTK and Qt both do. Named shapes from the compositor itself arrive
//! with `cursor-shape-v1`.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Logical, Point, Transform},
};
use xcursor::{CursorTheme, parser::parse_xcursor};

/// The size asked of the theme when `XCURSOR_SIZE` says nothing. 24 is the
/// Xcursor default, and what every toolkit falls back to as well.
const DEFAULT_SIZE: u32 = 24;

/// The compositor's own pointer image.
pub(crate) struct Cursor {
    pub(crate) image: MemoryRenderBuffer,
    /// The pixel that is "the pointer", from the image's top-left.
    pub(crate) hotspot: Point<i32, Logical>,
}

impl Cursor {
    /// The theme's default arrow, or a generated one.
    pub(crate) fn load() -> Self {
        let theme = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".to_owned());
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|size| size.parse().ok())
            .unwrap_or(DEFAULT_SIZE);
        themed(&theme, size).unwrap_or_else(|| {
            tracing::warn!(%theme, "no Xcursor theme found; drawing a plain arrow");
            generated()
        })
    }
}

/// The theme's `default` cursor at the nominal size nearest `size`.
fn themed(theme: &str, size: u32) -> Option<Cursor> {
    let theme = CursorTheme::load(theme);
    let path = theme
        .load_icon("default")
        .or_else(|| theme.load_icon("left_ptr"))?;
    let images = parse_xcursor(&std::fs::read(path).ok()?)?;
    let image = images
        .iter()
        .min_by_key(|image| image.size.abs_diff(size))?;
    let dimensions = (
        i32::try_from(image.width).ok()?,
        i32::try_from(image.height).ok()?,
    );
    let hotspot = (
        i32::try_from(image.xhot).ok()?,
        i32::try_from(image.yhot).ok()?,
    );
    // Xcursor stores each pixel as a little-endian ARGB word, so the bytes in
    // file order are B, G, R, A: DRM's `Argb8888`.
    Some(Cursor {
        image: buffer(&image.pixels_rgba, dimensions),
        hotspot: hotspot.into(),
    })
}

/// A white arrow with a black outline, drawn into the same byte order the
/// themed path uses.
fn generated() -> Cursor {
    const SIDE: usize = 16;
    let mut pixels = vec![0_u8; SIDE * SIDE * 4];
    for y in 0..SIDE {
        // A right triangle down the left edge, its tip at the hotspot. The
        // last rows narrow again to give it a point rather than a flat base.
        let width = if y < 12 { y + 1 } else { SIDE - y };
        for x in 0..width {
            let edge = x == 0 || x + 1 == width || y + 1 == SIDE;
            let shade = if edge { 0 } else { 255 };
            let at = (y * SIDE + x) * 4;
            pixels[at..at + 4].copy_from_slice(&[shade, shade, shade, 255]);
        }
    }
    Cursor {
        image: buffer(&pixels, (16, 16)),
        hotspot: (0, 0).into(),
    }
}

fn buffer(pixels: &[u8], size: (i32, i32)) -> MemoryRenderBuffer {
    MemoryRenderBuffer::from_slice(pixels, Fourcc::Argb8888, size, 1, Transform::Normal, None)
}
