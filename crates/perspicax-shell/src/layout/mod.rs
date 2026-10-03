//! Where things go on a surface, in its logical pixels: worked out from what
//! is shown and how wide its text is, with no drawing and no Wayland.
//!
//! Text is measured through [`Measure`], which the shell answers with its
//! fonts and a test with fixed widths, so no test needs a font installed.

pub(crate) mod menu;

/// A rectangle in a surface's logical pixels, from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Rect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
}

impl Rect {
    pub(crate) const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub(crate) const fn right(self) -> i32 {
        self.x + self.w
    }

    pub(crate) const fn bottom(self) -> i32 {
        self.y + self.h
    }

    /// Whether a point, as a pointer gives it, is inside.
    pub(crate) fn contains(self, (x, y): (f64, f64)) -> bool {
        f64::from(self.x) <= x
            && x < f64::from(self.right())
            && f64::from(self.y) <= y
            && y < f64::from(self.bottom())
    }
}

/// How wide a line of text is drawn, in logical pixels.
pub(crate) trait Measure {
    fn width(&mut self, text: &str) -> f32;
}

/// Every character the same width, for tests.
#[cfg(test)]
pub(crate) struct Monospace(pub(crate) f32);

#[cfg(test)]
impl Measure for Monospace {
    fn width(&mut self, text: &str) -> f32 {
        self.0 * text.chars().count() as f32
    }
}
