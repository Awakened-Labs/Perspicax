//! Pixels the compositor draws for itself, in software: shapes rasterised
//! into premultiplied ARGB8888 for a `MemoryRenderBuffer`.
//!
//! What a frame wears beyond its solid strips is drawn here, from nothing
//! but numbers, so it is drawn the same on a seat's GPU and into a picture
//! rendered with pixman. Each pixel is inked by how much of it the shape
//! covers, so an edge that is not straight is smooth, at any scale.

use perspicax_policy::Rgba;

/// Lay `colour`, straight alpha, over one ARGB8888 pixel, premultiplied: the
/// renderer blends premultiplied alpha, and a straight-alpha edge would come
/// out dark. The bytes are little-endian, so blue comes first.
pub(crate) fn over(pixel: &mut [u8], [r, g, b, a]: [u8; 4]) {
    let alpha = u32::from(a);
    let keep = 255 - alpha;
    for (channel, value) in pixel.iter_mut().zip([b, g, r]) {
        let premultiplied = u32::from(value) * alpha / 255;
        *channel = (premultiplied + u32::from(*channel) * keep / 255) as u8;
    }
    pixel[3] = (alpha + u32::from(pixel[3]) * keep / 255) as u8;
}

/// How much of the pixel whose centre is `distance` from an edge, positive
/// outside the shape, the shape covers: all of it a half pixel inside, none
/// a half pixel out, and in between in proportion.
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

/// A disc three quarters as wide as a square `side` pixels a side, centred
/// in it, in `colour`: what lights a titlebar button. Smaller than the
/// button, so it never reaches a corner the frame rounds off.
pub(crate) fn disc(side: i32, colour: Rgba) -> Vec<u8> {
    let side = side.max(1);
    let mut pixels = vec![0u8; (side * side * 4) as usize];
    let middle = side as f32 / 2.0;
    let radius = side as f32 * 0.375;
    for row in 0..side {
        for column in 0..side {
            let (x, y) = (column as f32 + 0.5 - middle, row as f32 + 0.5 - middle);
            let cover = coverage(x.hypot(y) - radius);
            if cover > 0.0 {
                let at = ((row * side + column) * 4) as usize;
                let alpha = (cover * f32::from(colour.a)).round() as u8;
                over(
                    &mut pixels[at..at + 4],
                    [colour.r, colour.g, colour.b, alpha],
                );
            }
        }
    }
    pixels
}

/// A square `side` pixels a side holding a quarter of a ring round the
/// square's inner bottom corner: everything from `inner` to `side` pixels
/// from it, in `colour`. The top left corner's quarter, or the top right's
/// if `right`. An `inner` of 0 is a quarter disc, a rounded frame's own
/// corner; more is the agent's line going round it.
pub(crate) fn corner(side: i32, inner: f32, colour: Rgba, right: bool) -> Vec<u8> {
    let side = side.max(1);
    let outer = side as f32;
    let centre = if right { 0.0 } else { outer };
    let mut pixels = vec![0u8; (side * side * 4) as usize];
    for row in 0..side {
        for column in 0..side {
            let (x, y) = (column as f32 + 0.5 - centre, row as f32 + 0.5 - outer);
            let distance = x.hypot(y);
            let cover = (coverage(distance - outer) - coverage(distance - inner)).max(0.0);
            if cover > 0.0 {
                let at = ((row * side + column) * 4) as usize;
                let alpha = (cover * f32::from(colour.a)).round() as u8;
                over(
                    &mut pixels[at..at + 4],
                    [colour.r, colour.g, colour.b, alpha],
                );
            }
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_is_premultiplied_and_a_full_one_replaces() {
        let mut pixel = [0u8; 4];
        over(&mut pixel, [255, 255, 255, 128]);
        assert_eq!(pixel, [128, 128, 128, 128], "half white, premultiplied");
        over(&mut pixel, [255, 0, 0, 255]);
        assert_eq!(pixel, [0, 0, 255, 255], "opaque red over it, blue first");
    }

    /// A frame's top left corner: clear outside its arc, at the square's
    /// own top left, and solid inside it, toward the bar; the top right's is
    /// its mirror. A ring is clear inside its inner arc too.
    #[test]
    fn a_corner_is_clear_outside_its_arc_and_solid_inside() {
        let colour = Rgba::new(0x2d, 0x6f, 0xa3, 0xff);
        let side = 10;
        let alpha =
            |pixels: &[u8], row: i32, column: i32| pixels[((row * side + column) * 4 + 3) as usize];
        let left = corner(side, 0.0, colour, false);
        assert_eq!(alpha(&left, 0, 0), 0, "outside the arc");
        assert_eq!(alpha(&left, 9, 9), 0xff, "inside it, beside the bar");
        assert!(
            alpha(&left, 0, 9) >= 0xf8,
            "along the top, where the bar goes on: the arc meets the edge there"
        );
        assert!(alpha(&left, 9, 0) >= 0xf8, "and down the side");
        assert!(
            left.chunks(4).any(|pixel| (1..0xff).contains(&pixel[3])),
            "smooth on the arc"
        );
        let right = corner(side, 0.0, colour, true);
        for (row, column) in [(0, 0), (0, 9), (9, 0), (4, 6)] {
            assert_eq!(
                alpha(&right, row, column),
                alpha(&left, row, side - 1 - column),
                "mirrored"
            );
        }
        let ring = corner(side, 6.0, colour, false);
        assert_eq!(
            alpha(&ring, 9, 9),
            0,
            "inside the inner arc: the frame's own corner"
        );
        assert_eq!(alpha(&ring, 0, 0), 0, "outside the outer");
        assert_eq!(alpha(&ring, 9, 1), 0xff, "on the ring");
    }

    /// Solid in its middle, clear at the corners and edges of its square,
    /// and smooth at its rim.
    #[test]
    fn a_disc_fills_three_quarters_of_its_square_and_none_of_its_corners() {
        let colour = Rgba::new(0xff, 0xff, 0xff, 0x33);
        let side = 24;
        let pixels = disc(side, colour);
        let alpha = |row: i32, column: i32| pixels[((row * side + column) * 4 + 3) as usize];
        assert_eq!(alpha(12, 12), 0x33, "the colour's own alpha, in the middle");
        assert_eq!(alpha(0, 0), 0, "a corner");
        assert_eq!(alpha(12, 0), 0, "and the square's edge, outside the disc");
        assert_eq!(alpha(12, 2), 0, "3 px in, still outside it");
        assert_eq!(alpha(12, 4), 0x33, "4 px in, inside");
        assert!(
            (0..side).any(|column| (1..0x33).contains(&alpha(12, column))),
            "smooth at the rim"
        );
    }
}
