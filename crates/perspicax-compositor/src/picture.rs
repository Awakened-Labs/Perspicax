//! Pictures read from files: PNGs decoded to premultiplied ARGB8888, and
//! shrunk to the size they are shown at.
//!
//! The lock screen's logo and the titlebar's icons are read with these, on
//! threads of their own (see [`crate::backdrop`]): a picture is a person's
//! file, and a slow disk or a hung home folder must stall nothing. Neither
//! is ever enlarged, so a picture is only ever shrunk, which a box filter
//! does exactly.

use std::io::{BufRead, Seek};

/// The widest and the tallest picture of the person's that is read, in
/// pixels. A larger one is refused from its header, before its pixels are
/// read.
const LARGEST: u32 = 4096;

/// What the PNG decoder may allocate: room for the largest picture at sixteen
/// bits a channel, before they are cut to eight.
const DECODER_BYTES: usize = 128 << 20;

/// A decoded image: premultiplied ARGB8888, which in memory is blue, green,
/// red, then alpha, as smithay's [`Fourcc::Argb8888`] buffers are.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Picture {
    pub(crate) size: (u32, u32),
    pub(crate) pixels: Vec<u8>,
}

/// `pixels`, four bytes each and `from` in size, shrunk to `to`: each pixel
/// the mean of the ones it covers, weighed by how much of each it covers,
/// which is exact for any shrinking. Premultiplied pixels stay so. `None`
/// for a size that is not smaller or the same, or pixels not `from` in size.
pub(crate) fn shrink(pixels: &[u8], from: (u32, u32), to: (u32, u32)) -> Option<Vec<u8>> {
    let side = |length: u32| usize::try_from(length).ok();
    let (from_w, from_h, to_w, to_h) = (side(from.0)?, side(from.1)?, side(to.0)?, side(to.1)?);
    if to_w == 0 || to_h == 0 || to_w > from_w || to_h > from_h {
        return None;
    }
    if pixels.len() != from_w * from_h * 4 {
        return None;
    }
    let (columns, rows) = (spans(from_w, to_w), spans(from_h, to_h));
    let whole = u64::try_from(from_w * from_h).ok()?;
    let mut shrunk = Vec::with_capacity(to_w * to_h * 4);
    // One row of sums at a time: the rows each row of `to` covers, weighed,
    // and then each of its columns.
    let mut line = vec![0_u32; from_w * 4];
    for row in &rows {
        line.fill(0);
        for &(y, weight) in row {
            let source = &pixels[y * from_w * 4..(y + 1) * from_w * 4];
            for (sum, &value) in line.iter_mut().zip(source) {
                *sum += u32::from(value) * weight;
            }
        }
        for column in &columns {
            let mut sums = [0_u64; 4];
            for &(x, weight) in column {
                for (sum, &value) in sums.iter_mut().zip(&line[x * 4..x * 4 + 4]) {
                    *sum += u64::from(value) * u64::from(weight);
                }
            }
            shrunk.extend(sums.map(|sum| u8::try_from((sum + whole / 2) / whole).unwrap_or(0xff)));
        }
    }
    Some(shrunk)
}

/// For each of `to` cells along a side `from` cells long, the cells it
/// covers and how much of each: a `from` cell is `to` units long and a `to`
/// cell is `from`, so every overlap is a whole number of units and each `to`
/// cell's add up to `from`.
fn spans(from: usize, to: usize) -> Vec<Vec<(usize, u32)>> {
    (0..to)
        .map(|cell| {
            let (start, end) = (cell * from, (cell + 1) * from);
            (start / to..end.div_ceil(to))
                .map(|source| {
                    let overlap = end.min((source + 1) * to) - start.max(source * to);
                    (source, u32::try_from(overlap).unwrap_or(u32::MAX))
                })
                .collect()
        })
        .collect()
}

/// Why a picture could not be read.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Unread {
    #[error("{0}")]
    Open(#[from] std::io::Error),
    #[error("not a PNG it can read: {0}")]
    Png(#[from] png::DecodingError),
    #[error("{0} by {1} pixels, more than {LARGEST} a side")]
    Large(u32, u32),
}

/// A PNG as premultiplied ARGB8888. Its size is read first, from its header,
/// and one over [`LARGEST`] a side is refused before its pixels are read.
pub(crate) fn decode(source: impl BufRead + Seek) -> Result<Picture, Unread> {
    let mut decoder = png::Decoder::new_with_limits(
        source,
        png::Limits {
            bytes: DECODER_BYTES,
        },
    );
    let (width, height) = decoder.read_header_info()?.size();
    if width > LARGEST || height > LARGEST {
        return Err(Unread::Large(width, height));
    }
    // Palettes and low depths expanded and sixteen bits cut to eight, so
    // what comes out is grey or colour, with or without alpha, a byte each.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut read = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or(Unread::Large(width, height))?
    ];
    let frame = reader.next_frame(&mut read)?;
    read.truncate(frame.buffer_size());
    let pixels = read
        .chunks_exact(frame.color_type.samples())
        .flat_map(|pixel| {
            let (r, g, b, a) = match *pixel {
                [r, g, b, a] => (r, g, b, a),
                [r, g, b] => (r, g, b, 0xff),
                [grey, a] => (grey, grey, grey, a),
                [grey] => (grey, grey, grey, 0xff),
                _ => (0, 0, 0, 0),
            };
            let times = |channel: u8| {
                u8::try_from((u16::from(channel) * u16::from(a) + 127) / 255).unwrap_or(0xff)
            };
            [times(b), times(g), times(r), a]
        })
        .collect();
    Ok(Picture {
        size: (frame.width, frame.height),
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture of one row, each pixel a grey `value` as opaque as `alpha`,
    /// premultiplied.
    fn row(pixels: &[(u8, u8)]) -> Vec<u8> {
        pixels
            .iter()
            .flat_map(|&(value, alpha)| [value, value, value, alpha])
            .collect()
    }

    #[test]
    fn a_two_by_two_shrinks_to_the_mean_of_its_four() {
        let pixels = [
            [0, 10, 20, 255],
            [40, 50, 60, 255],
            [80, 90, 100, 255],
            [1, 2, 3, 255],
        ];
        let shrunk = shrink(pixels.as_flattened(), (2, 2), (1, 1)).expect("shrunk");
        assert_eq!(shrunk, [30, 38, 46, 255], "rounded to the nearest");
    }

    #[test]
    fn a_solid_picture_stays_solid() {
        let solid: Vec<u8> = [12, 34, 56, 255].repeat(7 * 5);
        let shrunk = shrink(&solid, (7, 5), (3, 2)).expect("shrunk");
        assert_eq!(shrunk, [12, 34, 56, 255].repeat(3 * 2));
        assert_eq!(
            shrink(&solid, (7, 5), (7, 5)),
            Some(solid),
            "the same size, as it was"
        );
    }

    #[test]
    fn a_pixel_split_between_two_is_shared_by_both() {
        let shrunk = shrink(&row(&[(0, 255), (255, 255), (0, 255)]), (3, 1), (2, 1));
        assert_eq!(
            shrunk,
            Some(row(&[(85, 255), (85, 255)])),
            "two thirds and a third"
        );
    }

    #[test]
    fn alpha_stays_premultiplied() {
        let shrunk = shrink(&row(&[(255, 255), (0, 0)]), (2, 1), (1, 1));
        assert_eq!(
            shrunk,
            Some(row(&[(128, 128)])),
            "white half covered is half white, not white half seen"
        );
    }

    #[test]
    fn shrink_refuses_to_enlarge() {
        let one = row(&[(255, 255)]);
        assert_eq!(shrink(&one, (1, 1), (2, 1)), None);
        assert_eq!(shrink(&one, (1, 1), (1, 2)), None);
        assert_eq!(shrink(&one, (1, 1), (0, 1)), None, "nor to nothing");
        assert_eq!(
            shrink(&one, (2, 1), (1, 1)),
            None,
            "nor pixels of another size"
        );
    }

    /// `pixels` of `colour`, written as a PNG `size` in pixels, at eight bits
    /// a channel.
    fn png(size: (u32, u32), colour: png::ColorType, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, size.0, size.1);
        encoder.set_color(colour);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header");
        writer.write_image_data(pixels).expect("written");
        writer.finish().expect("finished");
        bytes
    }

    #[test]
    fn a_picture_is_read_premultiplied_whatever_its_colour_type() {
        let rgba = png(
            (2, 1),
            png::ColorType::Rgba,
            &[0xff, 0, 0, 0xff, 0xff, 0xff, 0xff, 0x80],
        );
        let picture = decode(std::io::Cursor::new(rgba)).expect("decoded");
        assert_eq!(picture.size, (2, 1));
        assert_eq!(
            picture.pixels,
            [0, 0, 0xff, 0xff, 0x80, 0x80, 0x80, 0x80],
            "opaque red, blue first, then half-seen white"
        );
        let grey = png((1, 1), png::ColorType::Grayscale, &[0x40]);
        let picture = decode(std::io::Cursor::new(grey)).expect("decoded");
        assert_eq!(picture.pixels, [0x40, 0x40, 0x40, 0xff]);
    }

    #[test]
    fn a_picture_too_large_or_not_a_png_is_refused() {
        let wide = png(
            (LARGEST + 1, 1),
            png::ColorType::Grayscale,
            &vec![0; LARGEST as usize + 1],
        );
        let refused = decode(std::io::Cursor::new(wide));
        assert!(
            matches!(refused, Err(Unread::Large(4097, 1))),
            "{refused:?}"
        );
        let gif = decode(std::io::Cursor::new(b"GIF89a".to_vec()));
        assert!(matches!(gif, Err(Unread::Png(_))), "{gif:?}");
    }
}
