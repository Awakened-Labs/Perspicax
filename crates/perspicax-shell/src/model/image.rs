//! Images read from disk: a wallpaper, an icon. PNG, JPEG for a wallpaper,
//! which is what most photographs are, and SVG for an icon, which is what
//! most icon themes draw in; told apart by their first bytes rather than by
//! the file's name where they can be, and kept as premultiplied RGBA, which
//! is what tiny-skia draws with and a compositor blends.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

use tiny_skia::{IntSize, Pixmap};

/// Why an image could not be read. The shell then goes without: a
/// wallpaper's colour alone, a menu item with no icon.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LoadError {
    #[error("{0}")]
    Read(#[from] std::io::Error),
    #[error("not an image of a kind it reads")]
    Format,
    #[error("not a PNG image it can read: {0}")]
    Png(#[from] png::DecodingError),
    #[cfg(feature = "wallpaper")]
    #[error("not a JPEG image it can read: {0}")]
    Jpeg(#[from] zune_jpeg::errors::DecodeErrors),
    #[cfg(feature = "svg")]
    #[error("not an SVG image it can read: {0}")]
    Svg(#[from] resvg::usvg::Error),
    #[error("an image of no size, or too big to hold")]
    Size,
}

/// Where the image named `written` in the config file is. `~/` is the home
/// folder, and a relative path is beside the config file, so a person can
/// keep a wallpaper next to the config that names it.
#[cfg_attr(
    not(any(feature = "wallpaper", feature = "menus", test)),
    expect(
        dead_code,
        reason = "a wallpaper or a menu file; a panel alone names no file"
    )
)]
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

/// Read the image at `path` into premultiplied RGBA pixels, at its own size.
#[cfg_attr(
    not(any(feature = "wallpaper", test)),
    expect(dead_code, reason = "a wallpaper's; icons are read by load_icon")
)]
pub(crate) fn load(path: &Path) -> Result<Pixmap, LoadError> {
    raster(&std::fs::read(path)?)
}

/// Read the icon at `path` into premultiplied RGBA pixels. One drawn in SVG
/// has no size of its own, and is drawn `side` pixels square, as big as it
/// goes with its shape kept, in the middle; any other is read at its own
/// size, to be scaled where it is drawn.
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) fn load_icon(path: &Path, side: u32) -> Result<Pixmap, LoadError> {
    let bytes = std::fs::read(path)?;
    if is_svg(path, &bytes) {
        return svg(&bytes, side);
    }
    raster(&bytes)
}

/// A PNG, or a JPEG with `wallpaper`, as premultiplied RGBA.
fn raster(bytes: &[u8]) -> Result<Pixmap, LoadError> {
    let (width, height, mut rgba) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        read_png(bytes)?
    } else if cfg!(feature = "wallpaper") && bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        read_jpeg(bytes)?
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
#[cfg(feature = "wallpaper")]
fn read_jpeg(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), LoadError> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), options);
    let rgba = decoder.decode()?;
    let (width, height) = decoder.dimensions().ok_or(LoadError::Size)?;
    let side = |pixels: usize| u32::try_from(pixels).map_err(|_| LoadError::Size);
    Ok((side(width)?, side(height)?, rgba))
}

/// Never called: a shell built without wallpapers reads no JPEG.
#[cfg(not(feature = "wallpaper"))]
fn read_jpeg(_: &[u8]) -> Result<(u32, u32, Vec<u8>), LoadError> {
    Err(LoadError::Format)
}

/// Whether the file at `path`, holding `bytes`, is an SVG image: named so,
/// or starting as XML does. An SVG may open with a byte-order mark, white
/// space or a comment before either.
#[cfg(any(feature = "menus", feature = "panel"))]
fn is_svg(path: &Path, bytes: &[u8]) -> bool {
    let named = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"));
    let text = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = &text[text.iter().take_while(|b| b.is_ascii_whitespace()).count()..];
    named
        || [&b"<svg"[..], b"<?xml", b"<!--"]
            .iter()
            .any(|start| text.starts_with(start))
}

/// An SVG drawn `side` pixels square. Nothing outside the file is read: an
/// image it names by path is left out, as an icon needs none, and a path
/// could name anything on the disk, `/dev/zero` among it.
#[cfg(all(feature = "svg", any(feature = "menus", feature = "panel")))]
fn svg(bytes: &[u8], side: u32) -> Result<Pixmap, LoadError> {
    use resvg::usvg::{ImageHrefResolver, Options, Tree};

    let options = Options {
        image_href_resolver: ImageHrefResolver {
            resolve_data: ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Options::default()
    };
    let tree = Tree::from_data(bytes, &options)?;
    let mut picture = Pixmap::new(side, side).ok_or(LoadError::Size)?;
    let (width, height) = (tree.size().width(), tree.size().height());
    let side = side as f32;
    let scale = (side / width).min(side / height);
    let at = tiny_skia::Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        (side - width * scale) / 2.0,
        (side - height * scale) / 2.0,
    );
    resvg::render(&tree, at, &mut picture.as_mut());
    Ok(picture)
}

/// Never drawn: a shell built without `svg` reads no SVG.
#[cfg(all(not(feature = "svg"), any(feature = "menus", feature = "panel")))]
fn svg(_: &[u8], _: u32) -> Result<Pixmap, LoadError> {
    Err(LoadError::Format)
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(all(feature = "svg", any(feature = "menus", feature = "panel")))]
    #[test]
    fn a_wide_svg_keeps_its_shape_in_the_middle_of_its_square() {
        let path =
            std::env::temp_dir().join(format!("perspicax-shell-wide-{}.svg", std::process::id()));
        std::fs::write(
            &path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 16"><rect width="32" height="16" fill="red"/></svg>"##,
        )
        .expect("written");
        let image = load_icon(&path, 48);
        std::fs::remove_file(&path).ok();
        let image = image.expect("drawn");
        let pixel = |y: u32| image.data()[((y * 48 + 24) * 4) as usize..][..4].to_vec();
        assert_eq!((image.width(), image.height()), (48, 48));
        assert_eq!(pixel(24), [0xff, 0, 0, 0xff], "as wide as the square");
        assert_eq!(pixel(4), [0, 0, 0, 0], "and clear above");
        assert_eq!(pixel(43), [0, 0, 0, 0], "and below");
    }

    #[cfg(any(feature = "menus", feature = "panel"))]
    #[test]
    fn an_svg_is_known_by_its_name_or_its_start() {
        let plain = Path::new("/icons/a");
        assert!(is_svg(Path::new("/icons/a.SVG"), b""));
        assert!(is_svg(
            plain,
            b"\xef\xbb\xbf  <?xml version=\"1.0\"?><svg/>"
        ));
        assert!(is_svg(
            plain,
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
        ));
        assert!(!is_svg(plain, b"\x89PNG\r\n\x1a\n"));
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
