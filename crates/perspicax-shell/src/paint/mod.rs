//! Pixels: tiny-skia paints straight into the buffer a surface shows, and
//! [`argb`] puts the buffer's bytes in the order Wayland reads them.

#[cfg(feature = "menus")]
pub(crate) mod icons;
#[cfg(feature = "menus")]
pub(crate) mod menu;
#[cfg(feature = "menus")]
pub(crate) mod text;
#[cfg(feature = "wallpaper")]
pub(crate) mod wallpaper;

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
