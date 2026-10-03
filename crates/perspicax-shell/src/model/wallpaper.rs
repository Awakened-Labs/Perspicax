//! A wallpaper's image: where its file is, what is in it, and where on a
//! monitor it goes.
//!
//! The image is read once, when the shell starts, and kept: a monitor plugged
//! in later is painted from what is already in memory rather than from the
//! disk. Only PNG and JPEG are read, told apart by their first bytes rather
//! than by the file's name.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

use perspicax_config::WallpaperMode;
use tiny_skia::{IntSize, Pixmap};

/// Why an image could not be read. The shell then shows the colour alone.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LoadError {
    #[error("{0}")]
    Read(#[from] std::io::Error),
    #[error("not a PNG or JPEG image")]
    Format,
    #[error("not a PNG image it can read: {0}")]
    Png(#[from] png::DecodingError),
    #[error("not a JPEG image it can read: {0}")]
    Jpeg(#[from] zune_jpeg::errors::DecodeErrors),
    #[error("an image of no size, or too big to hold")]
    Size,
}

/// Where the image named `written` in the config file is. `~/` is the home
/// folder, and a relative path is beside the config file, so a person can
/// keep a wallpaper next to the config that names it.
pub(crate) fn locate(written: &Path, config: Option<&Path>, home: Option<&Path>) -> PathBuf {
    if let (Ok(rest), Some(home)) = (written.strip_prefix("~"), home) {
        return home.join(rest);
    }
    match config.and_then(Path::parent) {
        Some(beside) if written.is_relative() => beside.join(written),
        _ => written.to_owned(),
    }
}

/// The largest image read, in bytes once decoded: an 8K picture with room to
/// spare. The PNG decoder's own default is a quarter of this.
const LARGEST: usize = 512 << 20;

/// Read the image at `path` into premultiplied RGBA pixels.
pub(crate) fn load(path: &Path) -> Result<Pixmap, LoadError> {
    let bytes = std::fs::read(path)?;
    let (width, height, mut rgba) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        read_png(&bytes)?
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        read_jpeg(&bytes)?
    } else {
        return Err(LoadError::Format);
    };
    premultiply(&mut rgba);
    let size = IntSize::from_wh(width, height).ok_or(LoadError::Size)?;
    Pixmap::from_vec(rgba, size).ok_or(LoadError::Size)
}

/// A PNG of any colour type and depth, as 8-bit straight RGBA.
fn read_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), LoadError> {
    let mut decoder =
        png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: LARGEST });
    // Palettes and low depths expanded and sixteen bits cut to eight, so
    // what comes out is one of four colour types, each a byte a channel.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or(LoadError::Size)?];
    let frame = reader.next_frame(&mut pixels)?;
    pixels.truncate(frame.buffer_size());
    let rgba = match frame.color_type {
        png::ColorType::Rgba => pixels,
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 0xff])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|ga| [ga[0], ga[0], ga[0], ga[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 0xff]).collect(),
        png::ColorType::Indexed => return Err(LoadError::Format),
    };
    Ok((frame.width, frame.height, rgba))
}

/// A JPEG, as 8-bit RGBA, opaque.
fn read_jpeg(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), LoadError> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), options);
    let rgba = decoder.decode()?;
    let (width, height) = decoder.dimensions().ok_or(LoadError::Size)?;
    let side = |pixels: usize| u32::try_from(pixels).map_err(|_| LoadError::Size);
    Ok((side(width)?, side(height)?, rgba))
}

/// Straight alpha to premultiplied, in place: what tiny-skia keeps, and what
/// a compositor blends.
fn premultiply(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
}

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

    #[test]
    fn an_image_path_is_found_from_home_or_beside_the_config() {
        let config = Path::new("/etc/xdg/perspicax/config.toml");
        let home = Path::new("/home/ada");
        assert_eq!(
            locate(Path::new("~/Pictures/a.png"), Some(config), Some(home)),
            Path::new("/home/ada/Pictures/a.png")
        );
        assert_eq!(
            locate(Path::new("a.png"), Some(config), Some(home)),
            Path::new("/etc/xdg/perspicax/a.png")
        );
        assert_eq!(
            locate(Path::new("/srv/a.png"), Some(config), Some(home)),
            Path::new("/srv/a.png")
        );
    }

    #[test]
    fn a_png_is_read_as_premultiplied_rgba() {
        let path = std::env::temp_dir().join(format!(
            "perspicax-shell-wallpaper-{}.png",
            std::process::id()
        ));
        {
            let file = std::fs::File::create(&path).expect("a file");
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("a header");
            writer
                .write_image_data(&[0xff, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x80])
                .expect("written");
        }
        let image = load(&path).expect("read back");
        std::fs::remove_file(&path).ok();
        assert_eq!((image.width(), image.height()), (2, 1));
        assert_eq!(
            image.data(),
            [0xff, 0x00, 0x00, 0xff, 0x80, 0x80, 0x80, 0x80],
            "opaque red, then half-transparent white, premultiplied"
        );
    }

    #[test]
    fn a_file_that_is_not_an_image_is_refused() {
        let path = std::env::temp_dir().join(format!(
            "perspicax-shell-not-an-image-{}.png",
            std::process::id()
        ));
        std::fs::write(&path, b"GIF89a").expect("written");
        let result = load(&path);
        std::fs::remove_file(&path).ok();
        assert!(matches!(result, Err(LoadError::Format)), "{result:?}");
    }
}
