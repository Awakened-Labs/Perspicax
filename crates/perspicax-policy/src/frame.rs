//! The frame this compositor draws around a window: a titlebar and a border,
//! as rectangles.
//!
//! A frame sits *outside* the client's window geometry. The client keeps the
//! size it is asked for; the frame adds to it. So every place that turns an
//! area into a window's rect (opening, maximizing, snapping, rescuing) asks for
//! the client rect that leaves room for the frame, which is [`inset`], and
//! every place that wants what a person sees asks for the frame's outside,
//! which is [`outset`].
//!
//! Which windows get a frame is the compositor's question: a client that draws
//! its own, or a fullscreen one, has [`Insets::NONE`].

use crate::Rect;

/// An RGB colour, as `config.toml` writes it: `"#rrggbb"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Colour {
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `"#rrggbb"`, or `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        Some(Self::rgb(byte(0)?, byte(2)?, byte(4)?))
    }
}

/// How windows are decorated: the `[decorations]` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decorations {
    /// Offer to draw the frame. A client that asks to draw its own still
    /// does; `false` tells every client to draw its own.
    pub server: bool,
    /// The titlebar's height, in logical pixels.
    pub title: i32,
    /// The border's width around the other three sides.
    pub border: i32,
    /// The frame of the window with the keyboard.
    pub focused: Colour,
    /// Every other frame.
    pub unfocused: Colour,
}

impl Default for Decorations {
    fn default() -> Self {
        Self {
            server: true,
            title: 24,
            border: 2,
            focused: Colour::rgb(0x3d, 0xae, 0xe9),
            unfocused: Colour::rgb(0x47, 0x50, 0x57),
        }
    }
}

/// What state a framed window is in, as far as its frame cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Normal,
    /// Maximized: the titlebar stays, so the window can still be dragged out
    /// and closed, and the border goes, because there is nothing to resize.
    Maximized,
    /// Fullscreen: no frame at all.
    Fullscreen,
}

/// How far a frame reaches past each side of the client's window geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Insets {
    pub top: i32,
    pub left: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Insets {
    /// No frame.
    pub const NONE: Self = Self {
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
    };

    /// The frame a window decorated by `decorations` has in `look`.
    #[must_use]
    pub fn of(decorations: &Decorations, look: Look) -> Self {
        let border = decorations.border.max(0);
        let title = decorations.title.max(0);
        match look {
            Look::Normal => Self {
                top: title + border,
                left: border,
                right: border,
                bottom: border,
            },
            Look::Maximized => Self {
                top: title,
                ..Self::NONE
            },
            Look::Fullscreen => Self::NONE,
        }
    }

    #[must_use]
    pub fn is_none(self) -> bool {
        self == Self::NONE
    }
}

/// The frame's outside: what a person sees of a window with this client rect.
#[must_use]
pub fn outset(client: Rect, insets: Insets) -> Rect {
    Rect::new(
        client.x - insets.left,
        client.y - insets.top,
        client.w + insets.left + insets.right,
        client.h + insets.top + insets.bottom,
    )
}

/// The client rect whose frame fills `area` exactly. Never smaller than 1x1:
/// a client asked to be zero-sized decides its own size instead.
#[must_use]
pub fn inset(area: Rect, insets: Insets) -> Rect {
    Rect::new(
        area.x + insets.left,
        area.y + insets.top,
        (area.w - insets.left - insets.right).max(1),
        (area.h - insets.top - insets.bottom).max(1),
    )
}

/// The rectangles the frame of a window with this client rect is drawn as:
/// the titlebar (with the border above it) across the whole width, then the
/// left, right and bottom borders. Empty strips are left out, so a frame with
/// [`Insets::NONE`] is no rectangles at all.
///
/// These are exactly the frame's pixels and nothing else. The resize margins
/// a pointer can grab beyond them are not drawn, so they are not here.
#[must_use]
pub fn frame_rects(client: Rect, insets: Insets) -> Vec<Rect> {
    let outer = outset(client, insets);
    [
        Rect::new(outer.x, outer.y, outer.w, insets.top),
        Rect::new(outer.x, client.y, insets.left, client.h),
        Rect::new(client.x + client.w, client.y, insets.right, client.h),
        Rect::new(outer.x, client.y + client.h, outer.w, insets.bottom),
    ]
    .into_iter()
    .filter(|strip| strip.w > 0 && strip.h > 0)
    .collect()
}

/// Where a window placed with its client at `at` has to go so that the top
/// and left of its frame are inside `area`: a titlebar above the top of the
/// screen cannot be grabbed to bring it back.
///
/// Only ever moves it right and down. A window bigger than the area keeps its
/// top-left corner in it and hangs off the other two sides, as it would
/// without a frame.
#[must_use]
pub fn fit(at: (i32, i32), insets: Insets, area: Rect) -> (i32, i32) {
    (
        at.0.max(area.x + insets.left),
        at.1.max(area.y + insets.top),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed() -> Insets {
        Insets::of(&Decorations::default(), Look::Normal)
    }

    #[test]
    fn a_normal_window_has_a_titlebar_and_a_border_all_round() {
        let insets = framed();
        assert_eq!(
            insets,
            Insets {
                top: 26,
                left: 2,
                right: 2,
                bottom: 2
            }
        );
    }

    #[test]
    fn maximized_keeps_only_the_titlebar_and_fullscreen_keeps_nothing() {
        let decorations = Decorations::default();
        assert_eq!(
            Insets::of(&decorations, Look::Maximized),
            Insets {
                top: 24,
                ..Insets::NONE
            }
        );
        assert!(Insets::of(&decorations, Look::Fullscreen).is_none());
    }

    #[test]
    fn inset_and_outset_undo_each_other() {
        let area = Rect::new(0, 32, 1920, 1048);
        let client = inset(area, framed());
        assert_eq!(client, Rect::new(2, 58, 1916, 1020));
        assert_eq!(outset(client, framed()), area);
    }

    #[test]
    fn an_area_too_small_for_the_frame_still_asks_for_a_window() {
        let client = inset(Rect::new(0, 0, 3, 10), framed());
        assert_eq!((client.w, client.h), (1, 1));
    }

    #[test]
    fn the_frame_is_four_strips_that_tile_the_outside_without_the_client() {
        let client = Rect::new(100, 100, 400, 300);
        let strips = frame_rects(client, framed());
        assert_eq!(
            strips,
            vec![
                Rect::new(98, 74, 404, 26),
                Rect::new(98, 100, 2, 300),
                Rect::new(500, 100, 2, 300),
                Rect::new(98, 400, 404, 2),
            ]
        );
        let area: i32 = strips.iter().map(|strip| strip.w * strip.h).sum();
        let outer = outset(client, framed());
        assert_eq!(area, outer.w * outer.h - client.w * client.h);
    }

    #[test]
    fn no_frame_is_no_rectangles() {
        assert!(frame_rects(Rect::new(0, 0, 10, 10), Insets::NONE).is_empty());
        let titled = Insets::of(&Decorations::default(), Look::Maximized);
        assert_eq!(frame_rects(Rect::new(0, 24, 10, 10), titled).len(), 1);
    }

    #[test]
    fn a_window_placed_at_the_corner_moves_so_its_titlebar_is_on_screen() {
        let area = Rect::new(0, 32, 1920, 1048);
        assert_eq!(fit((0, 32), framed(), area), (2, 58));
        assert_eq!(fit((300, 400), framed(), area), (300, 400), "already in");
        assert_eq!(fit((0, 32), Insets::NONE, area), (0, 32));
    }

    #[test]
    fn colours_are_read_as_config_writes_them() {
        assert_eq!(
            Colour::parse("#3daee9"),
            Some(Colour::rgb(0x3d, 0xae, 0xe9))
        );
        assert_eq!(
            Colour::parse("#3DAEE9"),
            Some(Colour::rgb(0x3d, 0xae, 0xe9))
        );
        assert_eq!(Colour::parse("3daee9"), None);
        assert_eq!(Colour::parse("#3daee"), None);
        assert_eq!(Colour::parse("#3daeeg"), None);
        assert_eq!(Colour::parse("#ééé"), None);
    }
}
