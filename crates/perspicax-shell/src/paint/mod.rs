//! Pixels: tiny-skia paints straight into the buffer a surface shows, and
//! [`argb`] puts the buffer's bytes in the order Wayland reads them.

pub(crate) mod wallpaper;

/// Turn `canvas` from what tiny-skia leaves, premultiplied RGBA, into a
/// `wl_shm` buffer's ARGB8888, in place. Both are premultiplied; they differ
/// in order. tiny-skia keeps R, G, B, A in memory, and ARGB8888 is a
/// little-endian word, so in memory it is B, G, R, A.
pub(crate) fn argb(canvas: &mut [u8]) {
    for pixel in canvas.chunks_exact_mut(4) {
        pixel.swap(0, 2);
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
        argb(canvas);
        assert_eq!(
            canvas[..4],
            [0x00, 0x40, 0x80, 0x80],
            "blue, green, red, alpha, each colour scaled by alpha"
        );
        assert_eq!(canvas[..4], canvas[4..], "and every pixel the same");
    }
}
