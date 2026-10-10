//! Where the pointer is allowed to be.
//!
//! Monitors of different sizes side by side do not make a rectangle. A 1080p
//! screen next to a 1440p one leaves a strip below the smaller that belongs to
//! no output, and a pointer clamped to the bounding box can wander into it and
//! vanish. So the pointer is kept on the union of the outputs themselves: a
//! point off every output moves to the nearest point on the nearest one, which
//! is what makes it slide along the short monitor's bottom edge instead.

use smithay::utils::{Logical, Point, Rectangle};

/// `point`, moved onto the nearest output if it is on none. Unchanged with no
/// outputs at all: with nothing to confine to, any answer would be invented.
pub(crate) fn confine(
    point: Point<f64, Logical>,
    outputs: &[Rectangle<i32, Logical>],
) -> Point<f64, Logical> {
    outputs
        .iter()
        .map(|output| nearest_on(point, *output))
        .min_by(|a, b| distance(point, *a).total_cmp(&distance(point, *b)))
        .unwrap_or(point)
}

/// The nearest point to `point` inside `output`. The far edges are
/// exclusive: the last pixel of a 1920-wide output is 1919, and a pointer at
/// 1920 would be on the next output (or none).
pub(crate) fn nearest_on(
    point: Point<f64, Logical>,
    output: Rectangle<i32, Logical>,
) -> Point<f64, Logical> {
    let (x0, y0) = (f64::from(output.loc.x), f64::from(output.loc.y));
    let x1 = x0 + f64::from(output.size.w.max(1)) - 1.0;
    let y1 = y0 + f64::from(output.size.h.max(1)) - 1.0;
    (point.x.clamp(x0, x1), point.y.clamp(y0, y1)).into()
}

pub(crate) fn distance(a: Point<f64, Logical>, b: Point<f64, Logical>) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// A 1440p monitor, and a 1080p one to its right.
    fn pair() -> [Rectangle<i32, Logical>; 2] {
        [output(0, 0, 2560, 1440), output(2560, 0, 1920, 1080)]
    }

    #[test]
    fn a_point_on_an_output_is_left_alone() {
        let point = (100.5, 200.25).into();
        assert_eq!(confine(point, &pair()), point);
    }

    #[test]
    fn the_pointer_stops_at_the_last_pixel_of_the_last_output() {
        assert_eq!(
            confine((5000.0, 10.0).into(), &pair()),
            (4479.0, 10.0).into()
        );
    }

    #[test]
    fn below_the_shorter_monitor_the_pointer_slides_along_its_bottom_edge() {
        // Right of the 1440p screen's edge, below the 1080p one: the dead strip.
        assert_eq!(
            confine((3000.0, 1300.0).into(), &pair()),
            (3000.0, 1079.0).into()
        );
    }

    #[test]
    fn with_no_outputs_nothing_is_invented() {
        let point = (-5.0, 7.0).into();
        assert_eq!(confine(point, &[]), point);
    }
}
