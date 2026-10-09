//! Pixels: tiny-skia paints straight into the buffer a surface shows, and
//! [`argb`] puts the buffer's bytes in the order Wayland reads them.

#[cfg(feature = "icons")]
pub(crate) mod folder;
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) mod icons;
#[cfg(feature = "menus")]
pub(crate) mod menu;
#[cfg(feature = "panel")]
pub(crate) mod panel;
#[cfg(feature = "pie")]
pub(crate) mod pie;
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) mod text;
#[cfg(feature = "wallpaper")]
pub(crate) mod wallpaper;

#[cfg(feature = "panel")]
pub(crate) use shapes::rounded;
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) use shapes::{fill, scaled, solid};

/// What the menus, the panels and the desktop folder's icons are drawn
/// with: the fonts, and the icon theme's images, each found once and
/// shared, and the theme's colours. Empty in a shell of a wallpaper alone,
/// which draws neither, so that what draws a desktop takes it either way.
pub(crate) struct Kit {
    #[cfg(any(feature = "menus", feature = "panel"))]
    pub(crate) fonts: text::Fonts,
    #[cfg(any(feature = "menus", feature = "panel"))]
    pub(crate) images: icons::Images,
    #[cfg(any(feature = "menus", feature = "panel"))]
    pub(crate) palette: perspicax_config::Palette,
}

/// `role`'s colour in `palette`, as tiny-skia's paint takes it: RGBA with
/// straight alpha.
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) fn colour(palette: &perspicax_config::Palette, role: perspicax_config::Role) -> [u8; 4] {
    let perspicax_config::Rgba { r, g, b, a } = palette[role];
    [r, g, b, a]
}

/// What the menus and the panel are drawn with.
#[cfg(any(feature = "menus", feature = "panel"))]
mod shapes {
    use tiny_skia::{Paint, PixmapMut, Transform};

    use crate::layout::Rect;

    /// `rect` in pixels at `scale`.
    pub(crate) fn scaled(rect: Rect, scale: u32) -> Rect {
        let s = scale as i32;
        Rect::new(rect.x * s, rect.y * s, rect.w * s, rect.h * s)
    }

    /// Paint in `colour`, RGBA with straight alpha.
    pub(crate) fn solid(colour: [u8; 4]) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color_rgba8(colour[0], colour[1], colour[2], colour[3]);
        paint.anti_alias = true;
        paint
    }

    /// Fill `place`, in pixels, with `colour`.
    pub(crate) fn fill(canvas: &mut PixmapMut<'_>, place: Rect, colour: [u8; 4]) {
        if let Some(rect) = tiny_skia::Rect::from_xywh(
            place.x as f32,
            place.y as f32,
            place.w as f32,
            place.h as f32,
        ) {
            canvas.fill_rect(rect, &solid(colour), Transform::identity(), None);
        }
    }

    /// A rectangle with its corners rounded `radius`.
    #[cfg(feature = "panel")]
    pub(crate) fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
        let r = radius.min(w / 2.0).min(h / 2.0);
        let (right, bottom) = (x + w, y + h);
        let mut path = tiny_skia::PathBuilder::new();
        path.move_to(x + r, y);
        path.line_to(right - r, y);
        path.quad_to(right, y, right, y + r);
        path.line_to(right, bottom - r);
        path.quad_to(right, bottom, right - r, bottom);
        path.line_to(x + r, bottom);
        path.quad_to(x, bottom, x, bottom - r);
        path.line_to(x, y + r);
        path.quad_to(x, y, x + r, y);
        path.close();
        path.finish()
    }
}

/// Turn the `areas` of `canvas`, a picture `width` pixels wide, from what
/// tiny-skia leaves, premultiplied RGBA, into a `wl_shm` buffer's ARGB8888,
/// in place. Both are premultiplied; they differ in order. tiny-skia keeps
/// R, G, B, A in memory, and ARGB8888 is a little-endian word, so in memory
/// it is B, G, R, A. Clear pixels read the same either way, so a picture
/// clear but for a few areas needs only those turned.
pub(crate) fn argb(canvas: &mut [u8], width: u32, areas: &[(u32, u32, u32, u32)]) {
    let width = width as usize;
    let rows = canvas.len() / 4 / width.max(1);
    for &(x, y, w, h) in areas {
        let (x, y) = (x as usize, y as usize);
        let right = (x + w as usize).min(width);
        for row in y.min(rows)..(y + h as usize).min(rows) {
            let line = &mut canvas[(row * width + x.min(right)) * 4..(row * width + right) * 4];
            for pixel in line.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::{Color, Pixmap};

    use super::*;

    #[test]
    fn pixels_are_premultiplied_in_argb_order() {
        let mut picture = Pixmap::new(2, 1).expect("a picture");
        picture.fill(Color::from_rgba8(0xff, 0x80, 0x00, 0x80));
        let canvas = picture.data_mut();
        argb(canvas, 2, &[(0, 0, 2, 1)]);
        assert_eq!(
            canvas[..4],
            [0x00, 0x40, 0x80, 0x80],
            "blue, green, red, alpha, each colour scaled by alpha"
        );
        assert_eq!(canvas[..4], canvas[4..], "and every pixel the same");
    }

    #[test]
    fn only_the_areas_asked_for_are_turned() {
        let mut picture = Pixmap::new(4, 3).expect("a picture");
        picture.fill(Color::from_rgba8(0xff, 0x00, 0x00, 0xff));
        let canvas = picture.data_mut();
        argb(canvas, 4, &[(1, 1, 2, 1), (3, 2, 9, 9)]);
        let pixel = |x: usize, y: usize| canvas[(y * 4 + x) * 4..][..4].to_vec();
        assert_eq!(pixel(0, 0), [0xff, 0, 0, 0xff], "outside, as it was");
        assert_eq!(pixel(1, 1), [0, 0, 0xff, 0xff]);
        assert_eq!(pixel(2, 1), [0, 0, 0xff, 0xff]);
        assert_eq!(pixel(3, 1), [0xff, 0, 0, 0xff]);
        assert_eq!(
            pixel(3, 2),
            [0, 0, 0xff, 0xff],
            "an area past the edge is cut to it"
        );
    }
}
