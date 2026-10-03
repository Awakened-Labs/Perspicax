//! A wallpaper's image: where on a monitor it goes.
//!
//! The image is read once, when the shell starts, and kept: a monitor plugged
//! in later is painted from what is already in memory rather than from the
//! disk.

use perspicax_config::WallpaperMode;

/// Where an image goes on a monitor: drawn `scale` times its size with its
/// top-left corner at `x`, `y`, in the monitor's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placement {
    pub(crate) scale: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// Where an image of `image` pixels goes on a monitor of `area` pixels, in
/// `mode`. Centred in every mode but [`WallpaperMode::Tile`], which starts
/// at the corner and repeats. An image at its own size is put on whole
/// pixels, so it is not blurred by half of one.
pub(crate) fn place(image: (u32, u32), area: (u32, u32), mode: WallpaperMode) -> Placement {
    let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
    let (aw, ah) = (area.0 as f32, area.1 as f32);
    let scale = match mode {
        WallpaperMode::Fill => (aw / iw).max(ah / ih),
        WallpaperMode::Fit => (aw / iw).min(ah / ih),
        WallpaperMode::Center | WallpaperMode::Tile => 1.0,
    };
    let (x, y) = match mode {
        WallpaperMode::Tile => (0.0, 0.0),
        WallpaperMode::Center => (((aw - iw) / 2.0).round(), ((ah - ih) / 2.0).round()),
        WallpaperMode::Fill | WallpaperMode::Fit => {
            ((aw - iw * scale) / 2.0, (ah - ih * scale) / 2.0)
        }
    };
    Placement { scale, x, y }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_crops_and_fit_letterboxes() {
        // A wide image on a square monitor.
        let fill = place((200, 100), (100, 100), WallpaperMode::Fill);
        assert_eq!(
            fill,
            Placement {
                scale: 1.0,
                x: -50.0,
                y: 0.0
            },
            "as tall as the monitor, its sides cut off"
        );
        let fit = place((200, 100), (100, 100), WallpaperMode::Fit);
        assert_eq!(
            fit,
            Placement {
                scale: 0.5,
                x: 0.0,
                y: 25.0
            },
            "as wide as the monitor, with bars above and below"
        );
    }

    #[test]
    fn center_and_tile_keep_the_images_own_size() {
        assert_eq!(
            place((3, 3), (10, 10), WallpaperMode::Center),
            Placement {
                scale: 1.0,
                x: 4.0,
                y: 4.0
            },
            "on a whole pixel, not at 3.5"
        );
        assert_eq!(
            place((3, 3), (10, 10), WallpaperMode::Tile),
            Placement {
                scale: 1.0,
                x: 0.0,
                y: 0.0
            }
        );
    }
}
