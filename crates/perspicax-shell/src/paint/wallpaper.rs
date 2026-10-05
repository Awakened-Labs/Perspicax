//! A wallpaper, painted for one monitor: its colour, and its image placed
//! over it the way [`WallpaperMode`] says.

use perspicax_config::{Wallpaper, WallpaperMode};
use tiny_skia::{
    Color, FilterQuality, Paint, Pattern, Pixmap, PixmapMut, PixmapPaint, Rect, SpreadMode,
    Transform,
};

use crate::model::wallpaper::place;

/// Paint `wallpaper` over the whole of `picture`, with `image` if it has
/// one that could be read.
pub(crate) fn paint(wallpaper: &Wallpaper, image: Option<&Pixmap>, picture: &mut PixmapMut<'_>) {
    let colour = wallpaper.colour;
    picture.fill(Color::from_rgba8(colour.r, colour.g, colour.b, 0xff));
    let Some(image) = image else {
        return;
    };
    let size = (picture.width(), picture.height());
    let placed = place((image.width(), image.height()), size, wallpaper.mode);
    let at = Transform::from_row(placed.scale, 0.0, 0.0, placed.scale, placed.x, placed.y);
    if wallpaper.mode == WallpaperMode::Tile {
        let tiles = Pattern::new(
            image.as_ref(),
            SpreadMode::Repeat,
            FilterQuality::Nearest,
            1.0,
            at,
        );
        let paint = Paint {
            shader: tiles,
            ..Paint::default()
        };
        if let Some(whole) = Rect::from_xywh(0.0, 0.0, size.0 as f32, size.1 as f32) {
            picture.fill_rect(whole, &paint, Transform::identity(), None);
        }
    } else {
        // Scaled smoothly; at its own size, copied pixel for pixel.
        let quality = if placed.scale == 1.0 {
            FilterQuality::Nearest
        } else {
            FilterQuality::Bicubic
        };
        let paint = PixmapPaint {
            quality,
            ..PixmapPaint::default()
        };
        picture.draw_pixmap(0, 0, image.as_ref(), &paint, at, None);
    }
}

#[cfg(test)]
mod tests {
    use perspicax_policy::Colour;

    use super::*;

    const SLATE: Colour = Colour::rgb(0x3c, 0x40, 0x48);

    fn wallpaper(mode: WallpaperMode) -> Wallpaper {
        Wallpaper {
            colour: SLATE,
            image: None,
            mode,
        }
    }

    /// The premultiplied RGBA of an opaque colour, as the picture holds it.
    fn opaque(colour: Colour) -> [u8; 4] {
        [colour.r, colour.g, colour.b, 0xff]
    }

    /// `wallpaper` painted on a fresh picture of `width` by `height`.
    fn painted(wallpaper: &Wallpaper, image: Option<&Pixmap>, width: u32, height: u32) -> Pixmap {
        let mut picture = Pixmap::new(width, height).expect("a picture");
        paint(wallpaper, image, &mut picture.as_mut());
        picture
    }

    fn pixel(picture: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * picture.width() + x) * 4) as usize;
        picture.data()[at..at + 4].try_into().unwrap()
    }

    /// An image of `width` by `height` in one opaque colour.
    fn image(width: u32, height: u32, colour: Colour) -> Pixmap {
        let mut image = Pixmap::new(width, height).unwrap();
        image.fill(Color::from_rgba8(colour.r, colour.g, colour.b, 0xff));
        image
    }

    #[test]
    fn a_colour_wallpaper_fills_the_buffer() {
        let picture = painted(&wallpaper(WallpaperMode::Fill), None, 8, 4);
        assert!(
            picture
                .data()
                .chunks_exact(4)
                .all(|pixel| pixel == opaque(SLATE)),
            "every pixel the colour"
        );
    }

    #[test]
    fn a_fitted_image_has_the_colour_above_and_below_it() {
        let red = Colour::rgb(0xff, 0, 0);
        let picture = painted(
            &wallpaper(WallpaperMode::Fit),
            Some(&image(8, 4, red)),
            8,
            8,
        );
        assert_eq!(pixel(&picture, 4, 0), opaque(SLATE), "a bar above");
        assert_eq!(
            pixel(&picture, 4, 4),
            opaque(red),
            "the image across the middle"
        );
        assert_eq!(pixel(&picture, 4, 7), opaque(SLATE), "a bar below");

        let filled = painted(
            &wallpaper(WallpaperMode::Fill),
            Some(&image(8, 4, red)),
            8,
            8,
        );
        assert_eq!(pixel(&filled, 0, 0), opaque(red), "filled to the corner");
    }

    #[test]
    fn a_tiled_image_repeats_from_the_corner() {
        let mut checker = Pixmap::new(2, 1).unwrap();
        checker.data_mut()[..4].copy_from_slice(&[0xff, 0, 0, 0xff]);
        checker.data_mut()[4..].copy_from_slice(&[0, 0, 0xff, 0xff]);
        let picture = painted(&wallpaper(WallpaperMode::Tile), Some(&checker), 5, 2);
        let red = [0xff, 0, 0, 0xff];
        let blue = [0, 0, 0xff, 0xff];
        assert_eq!(
            (0..5).map(|x| pixel(&picture, x, 1)).collect::<Vec<_>>(),
            [red, blue, red, blue, red]
        );
    }
}
