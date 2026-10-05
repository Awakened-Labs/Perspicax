//! A wallpaper's image: where on a monitor it goes, and how big to keep it.
//!
//! An image is read once and kept: a monitor plugged in later, or a
//! workspace switched to, is painted from what is already in memory rather
//! than from the disk. One filling or fitting the monitors is kept no
//! bigger than the largest of them needs, since a photograph can hold many
//! times a monitor's pixels; one centred or tiled is drawn at its own size,
//! and kept at it.

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

/// How big to keep an image of `image` pixels drawn filling, or fitting,
/// monitors of `areas` pixels: its own shape, at the smallest size that
/// still covers each, with the side that decides it the monitor's own, so
/// that the largest is painted from it pixel for pixel. Its own size if
/// none needs less, or no monitor is known.
pub(crate) fn kept(image: (u32, u32), areas: &[(u32, u32)]) -> (u32, u32) {
    let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
    let across = |&(aw, _): &(u32, u32)| aw as f32 / iw;
    let down = |&(_, ah): &(u32, u32)| ah as f32 / ih;
    let Some(largest) = areas
        .iter()
        .max_by(|a, b| across(a).max(down(a)).total_cmp(&across(b).max(down(b))))
    else {
        return image;
    };
    let scale = across(largest).max(down(largest));
    if scale >= 1.0 {
        return image;
    }
    let (aw, ah) = *largest;
    if across(largest) >= down(largest) {
        (aw, ((ih * scale).ceil() as u32).max(ah))
    } else {
        (((iw * scale).ceil() as u32).max(aw), ah)
    }
}

/// How big to keep an image of `image` pixels for a wallpaper in `mode` on
/// monitors of `areas` pixels: no bigger than they need when it is scaled
/// to them, and at its own size when it is not.
pub(crate) fn kept_for(image: (u32, u32), mode: WallpaperMode, areas: &[(u32, u32)]) -> (u32, u32) {
    match mode {
        WallpaperMode::Fill | WallpaperMode::Fit => kept(image, areas),
        WallpaperMode::Center | WallpaperMode::Tile => image,
    }
}

/// Whether an image of `image` pixels kept at `size` draws a wallpaper in
/// `mode` on monitors of `areas` pixels as well as the whole image would,
/// or has to be read again.
pub(crate) fn serves(
    size: (u32, u32),
    image: (u32, u32),
    mode: WallpaperMode,
    areas: &[(u32, u32)],
) -> bool {
    areas.iter().all(|&area| {
        let needed = kept_for(image, mode, &[area]);
        size.0 >= needed.0 && size.1 >= needed.1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_is_kept_only_as_big_as_the_largest_monitor_needs() {
        let photo = (6000, 4000);
        assert_eq!(
            kept(photo, &[(1920, 1080), (2560, 1440)]),
            (2560, 1707),
            "as wide as the wider monitor, and tall enough to cover it"
        );
        assert_eq!(
            kept(photo, &[(1080, 1920)]),
            (2880, 1920),
            "a monitor on its side decides by its height"
        );
        assert_eq!(
            kept((1280, 720), &[(2560, 1440)]),
            (1280, 720),
            "never grown"
        );
        assert_eq!(kept(photo, &[]), photo, "nor shrunk for no monitor");
    }

    #[test]
    fn an_image_is_read_again_only_when_it_was_kept_too_small() {
        use WallpaperMode::{Fill, Fit, Tile};

        let photo = (6000, 4000);
        let laptop = [(1920, 1080)];
        let kept = kept_for(photo, Fill, &laptop);
        assert_eq!(kept, (1920, 1280));
        assert!(
            serves(kept, photo, Fill, &laptop),
            "painted again from memory"
        );
        assert!(
            serves(kept, photo, Fit, &laptop),
            "fitting needs no more than filling"
        );
        assert!(
            !serves(kept, photo, Fill, &[(1920, 1080), (3840, 2160)]),
            "a bigger monitor plugged in needs it read again"
        );
        assert!(
            !serves(kept, photo, Tile, &laptop),
            "and so does the same image tiled on another workspace"
        );
        assert_eq!(
            kept_for(photo, Tile, &laptop),
            photo,
            "a tile at its own size"
        );
        assert!(
            serves(photo, photo, Fill, &[(7680, 4320)]),
            "the whole image is all there is"
        );
    }

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
