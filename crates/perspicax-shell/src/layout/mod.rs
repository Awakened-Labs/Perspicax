//! Where things go on a surface, in its logical pixels: worked out from what
//! is shown and how wide its text is, with no drawing and no Wayland.
//!
//! Text is measured through [`Measure`], which the shell answers with its
//! fonts and a test with fixed widths, so no test needs a font installed.

#[cfg(feature = "icons")]
pub(crate) mod folder;
#[cfg(feature = "menus")]
pub(crate) mod menu;
#[cfg(feature = "panel")]
pub(crate) mod panel;

/// The text's size, in menus and on panels, unless the theme says another.
pub(crate) const TEXT: f32 = 14.0;

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

/// What is left of `monitor` beside `strip`, the shell's panel along its top
/// or its bottom, if it has one: where menus may go.
#[cfg(feature = "menus")]
pub(crate) fn usable(monitor: Rect, strip: Option<Rect>) -> Rect {
    match strip {
        None => monitor,
        Some(strip) if strip.y <= monitor.y => Rect::new(
            monitor.x,
            strip.bottom(),
            monitor.w,
            (monitor.bottom() - strip.bottom()).max(0),
        ),
        Some(strip) => Rect::new(
            monitor.x,
            monitor.y,
            monitor.w,
            (strip.y - monitor.y).max(0),
        ),
    }
}

/// The first of `monitors`, each a connector name and where its top-left
/// corner is on the desk: the leftmost, the topmost of those level with it.
/// Where a panel goes with `outputs = "first"`, and where the desktop
/// folder's icons go.
#[cfg(any(feature = "panel", feature = "icons"))]
pub(crate) fn first<'a>(monitors: &[(&'a str, (i32, i32))]) -> Option<&'a str> {
    monitors
        .iter()
        .min_by_key(|&&(_, at)| at)
        .map(|&(name, _)| name)
}

/// How wide a line of text is drawn, in logical pixels.
pub(crate) trait Measure {
    fn width(&mut self, text: &str) -> f32;

    /// The text's size, in logical pixels: the theme's font size. Only the
    /// desktop's icons lay out by it; menu rows are tall enough for any.
    #[cfg(feature = "icons")]
    fn size(&self) -> f32 {
        TEXT
    }
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

#[cfg(all(test, feature = "menus"))]
mod tests {
    use super::*;

    #[test]
    fn menus_have_what_a_panel_leaves() {
        let monitor = Rect::new(0, 0, 1280, 800);
        assert_eq!(
            usable(monitor, Some(Rect::new(0, 760, 1280, 40))),
            Rect::new(0, 0, 1280, 760),
            "above a panel along the bottom"
        );
        assert_eq!(
            usable(monitor, Some(Rect::new(0, 0, 1280, 32))),
            Rect::new(0, 32, 1280, 768),
            "below one along the top"
        );
        assert_eq!(usable(monitor, None), monitor, "and all of it with none");
    }
}
