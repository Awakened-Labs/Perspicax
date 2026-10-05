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

use crate::{Edges, Rect};

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

    /// Black or white, whichever reads better written on this colour: the
    /// title's ink, so a person who picks a light titlebar does not also have
    /// to pick dark text for it.
    #[must_use]
    pub fn ink(self) -> Self {
        // Rec. 709 luma: green counts most, blue least, as the eye weighs them.
        let luma = 2126 * u32::from(self.r) + 7152 * u32::from(self.g) + 722 * u32::from(self.b);
        if luma > 128 * 10_000 {
            Self::rgb(0, 0, 0)
        } else {
            Self::rgb(0xff, 0xff, 0xff)
        }
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
            focused: Colour::rgb(0x2d, 0x6f, 0xa3),
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

/// A button on the titlebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameButton {
    Minimize,
    Maximize,
    Close,
}

/// What part of a frame a point is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The titlebar, where a press moves the window.
    Title,
    /// One of a tab group's tabs on the titlebar, counting from the left.
    Tab(usize),
    Button(FrameButton),
    /// An edge or corner, where a press resizes.
    Edge(Edges),
}

/// How far outside the drawn frame the pointer can still grab an edge: a
/// two-pixel border is too thin a target to resize by.
pub const GRIP: i32 = 6;

/// How far along an edge from a corner still counts as the corner.
const CORNER: i32 = 16;

/// The titlebar's own strip, without the border above it: directly above the
/// client and as wide as it. `None` for a frame with no titlebar.
#[must_use]
pub fn titlebar(client: Rect, insets: Insets, decorations: &Decorations) -> Option<Rect> {
    let height = decorations.title.min(insets.top);
    (height > 0 && client.w > 0).then(|| Rect::new(client.x, client.y - height, client.w, height))
}

/// The titlebar's buttons, square, at its right-hand end: minimize, maximize,
/// close, with close in the corner. As many as fit, close first, so a narrow
/// window can still be closed.
#[must_use]
pub fn buttons(
    client: Rect,
    insets: Insets,
    decorations: &Decorations,
) -> Vec<(FrameButton, Rect)> {
    titlebar(client, insets, decorations).map_or_else(Vec::new, buttons_in)
}

/// The buttons on a titlebar that is `bar`, as [`buttons`] places them.
#[must_use]
pub fn buttons_in(bar: Rect) -> Vec<(FrameButton, Rect)> {
    let side = bar.h;
    let fit = usize::try_from(bar.w / side.max(1)).unwrap_or(0);
    [
        FrameButton::Close,
        FrameButton::Maximize,
        FrameButton::Minimize,
    ]
    .into_iter()
    .take(fit)
    .enumerate()
    .map(|(n, button)| {
        let n = i32::try_from(n).unwrap_or(0) + 1;
        (
            button,
            Rect::new(bar.x + bar.w - n * side, bar.y, side, side),
        )
    })
    .collect()
}

/// The tabs on a titlebar that is `bar`, for a group of `count`: the room
/// the buttons leave, shared out evenly, left to right. One tab, the whole
/// room, for a window in no group.
#[must_use]
pub fn tab_rects(bar: Rect, count: usize) -> Vec<Rect> {
    let taken: i32 = buttons_in(bar).iter().map(|(_, button)| button.w).sum();
    let room = (bar.w - taken).max(0);
    let count = i32::try_from(count.max(1)).unwrap_or(1);
    (0..count)
        .map(|n| {
            let (from, to) = (room * n / count, room * (n + 1) / count);
            Rect::new(bar.x + from, bar.y, to - from, bar.h)
        })
        .collect()
}

/// What part of the frame of a window with this client rect `point` is on,
/// or `None` if it is on the client or off the frame altogether.
///
/// `resizable` is false for a maximized window: it keeps its titlebar, and
/// has no edge to drag. `tabs` is how many tabs its group has, one for a
/// window in no group, which has a title and no tabs. Otherwise every edge can be grabbed from the border
/// and up to [`GRIP`] pixels outside it, and a corner from [`CORNER`] pixels
/// along either side of it, so the titlebar's two ends resize diagonally.
#[must_use]
pub fn part_at(
    point: (f64, f64),
    client: Rect,
    insets: Insets,
    decorations: &Decorations,
    resizable: bool,
    tabs: usize,
) -> Option<Part> {
    if insets.is_none() {
        return None;
    }
    let outer = outset(client, insets);
    let grip = if resizable { GRIP } else { 0 };
    let (x, y) = point;
    let within = |rect: Rect, margin: i32| {
        x >= f64::from(rect.x - margin)
            && x < f64::from(rect.x + rect.w + margin)
            && y >= f64::from(rect.y - margin)
            && y < f64::from(rect.y + rect.h + margin)
    };
    if !within(outer, grip) || within(client, 0) {
        return None;
    }
    if resizable {
        let top_border = insets.top - decorations.title.min(insets.top);
        let mut edges = Edges {
            left: x < f64::from(client.x),
            right: x >= f64::from(client.x + client.w),
            top: y < f64::from(outer.y + top_border),
            bottom: y >= f64::from(client.y + client.h),
        };
        let near = |value: f64, from: i32| (value - f64::from(from)).abs() < f64::from(CORNER);
        if edges.top || edges.bottom {
            edges.left |= near(x, outer.x);
            edges.right |= near(x, outer.x + outer.w);
        }
        if edges.left || edges.right {
            edges.top |= near(y, outer.y);
            edges.bottom |= near(y, outer.y + outer.h);
        }
        if edges != Edges::default() {
            return Some(Part::Edge(edges));
        }
    }
    let on = |rect: Rect| within(rect, 0);
    if let Some((button, _)) = buttons(client, insets, decorations)
        .into_iter()
        .find(|(_, rect)| on(*rect))
    {
        return Some(Part::Button(button));
    }
    let tab = titlebar(client, insets, decorations)
        .filter(|_| tabs > 1)
        .and_then(|bar| tab_rects(bar, tabs).into_iter().position(on));
    Some(tab.map_or(Part::Title, Part::Tab))
}

/// A press of a button: when, in milliseconds, and where.
pub type Press = (u32, (f64, f64));

/// Whether a press at `second` makes a double-click of the press at
/// `first`: soon enough after, within `within_ms`, and near enough, that a
/// hand meant the two as one gesture.
#[must_use]
pub fn is_double(first: Press, second: Press, within_ms: u32) -> bool {
    const WITHIN_PIXELS: f64 = 6.0;
    let ((then, (x0, y0)), (now, (x1, y1))) = (first, second);
    now.wrapping_sub(then) <= within_ms && (x1 - x0).hypot(y1 - y0) <= WITHIN_PIXELS
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
    fn the_titlebar_is_above_the_client_not_on_the_border() {
        let decorations = Decorations::default();
        let client = Rect::new(100, 100, 400, 300);
        assert_eq!(
            titlebar(client, framed(), &decorations),
            Some(Rect::new(100, 76, 400, 24))
        );
        let maximized = Insets::of(&decorations, Look::Maximized);
        assert_eq!(
            titlebar(Rect::new(0, 24, 1920, 1056), maximized, &decorations),
            Some(Rect::new(0, 0, 1920, 24))
        );
        assert_eq!(titlebar(client, Insets::NONE, &decorations), None);
    }

    #[test]
    fn the_buttons_sit_at_the_right_with_close_in_the_corner() {
        let decorations = Decorations::default();
        let client = Rect::new(100, 100, 400, 300);
        assert_eq!(
            buttons(client, framed(), &decorations),
            vec![
                (FrameButton::Close, Rect::new(476, 76, 24, 24)),
                (FrameButton::Maximize, Rect::new(452, 76, 24, 24)),
                (FrameButton::Minimize, Rect::new(428, 76, 24, 24)),
            ]
        );
        let narrow = Rect::new(0, 100, 50, 50);
        assert_eq!(
            buttons(narrow, framed(), &decorations)
                .iter()
                .map(|(button, _)| *button)
                .collect::<Vec<_>>(),
            vec![FrameButton::Close, FrameButton::Maximize],
            "a narrow window keeps close first"
        );
    }

    #[test]
    fn a_point_on_the_frame_says_which_part() {
        let decorations = Decorations::default();
        let client = Rect::new(100, 100, 400, 300);
        let at = |x: f64, y: f64| part_at((x, y), client, framed(), &decorations, true, 1);
        assert_eq!(at(200.0, 90.0), Some(Part::Title));
        assert_eq!(at(488.0, 88.0), Some(Part::Button(FrameButton::Close)));
        assert_eq!(at(440.0, 88.0), Some(Part::Button(FrameButton::Minimize)));
        assert_eq!(at(200.0, 200.0), None, "the client is the client's");
        assert_eq!(at(200.0, 500.0), None, "and far off is nothing");
        let edge = |left, right, top, bottom| {
            Some(Part::Edge(Edges {
                top,
                bottom,
                left,
                right,
            }))
        };
        assert_eq!(
            at(97.0, 200.0),
            edge(true, false, false, false),
            "the border"
        );
        assert_eq!(
            at(93.0, 200.0),
            edge(true, false, false, false),
            "just outside it"
        );
        assert_eq!(at(300.0, 405.0), edge(false, false, false, true));
        assert_eq!(
            at(300.0, 72.0),
            edge(false, false, true, false),
            "above the title"
        );
        assert_eq!(
            at(99.0, 80.0),
            edge(true, false, true, false),
            "a titlebar's end"
        );
        assert_eq!(
            at(506.0, 406.0),
            edge(false, true, false, true),
            "outside the corner"
        );
    }

    #[test]
    fn tabs_share_the_room_the_buttons_leave() {
        let bar = Rect::new(100, 76, 400, 24);
        assert_eq!(tab_rects(bar, 1), vec![Rect::new(100, 76, 328, 24)]);
        assert_eq!(
            tab_rects(bar, 3),
            vec![
                Rect::new(100, 76, 109, 24),
                Rect::new(209, 76, 109, 24),
                Rect::new(318, 76, 110, 24),
            ],
            "evenly, with the odd pixel at the end"
        );
        let decorations = Decorations::default();
        let client = Rect::new(100, 100, 400, 300);
        let at = |x: f64, tabs| part_at((x, 88.0), client, framed(), &decorations, true, tabs);
        assert_eq!(at(150.0, 3), Some(Part::Tab(0)));
        assert_eq!(at(400.0, 3), Some(Part::Tab(2)));
        assert_eq!(
            at(400.0, 1),
            Some(Part::Title),
            "one window: a title, not a tab"
        );
        assert_eq!(at(488.0, 3), Some(Part::Button(FrameButton::Close)));
    }

    #[test]
    fn a_maximized_titlebar_has_no_edges() {
        let decorations = Decorations::default();
        let maximized = Insets::of(&decorations, Look::Maximized);
        let client = Rect::new(0, 24, 1920, 1056);
        let at = |x: f64, y: f64| part_at((x, y), client, maximized, &decorations, false, 1);
        assert_eq!(at(0.0, 0.0), Some(Part::Title));
        assert_eq!(at(1910.0, 10.0), Some(Part::Button(FrameButton::Close)));
        assert_eq!(at(10.0, 500.0), None);
    }

    #[test]
    fn two_presses_close_in_time_and_place_are_a_double_click() {
        assert!(is_double((1000, (10.0, 10.0)), (1300, (12.0, 11.0)), 400));
        assert!(
            !is_double((1000, (10.0, 10.0)), (1500, (10.0, 10.0)), 400),
            "too slow"
        );
        assert!(
            is_double((1000, (10.0, 10.0)), (1500, (10.0, 10.0)), 600),
            "for a hand that asked for longer"
        );
        assert!(
            !is_double((1000, (10.0, 10.0)), (1100, (40.0, 10.0)), 400),
            "too far"
        );
        assert!(
            is_double((u32::MAX - 100, (0.0, 0.0)), (100, (0.0, 0.0)), 400),
            "across the clock wrapping"
        );
    }

    #[test]
    fn the_title_is_inked_in_whichever_of_black_and_white_reads() {
        let white = Colour::rgb(0xff, 0xff, 0xff);
        let black = Colour::rgb(0, 0, 0);
        assert_eq!(Decorations::default().focused.ink(), white);
        assert_eq!(Decorations::default().unfocused.ink(), white);
        assert_eq!(Colour::rgb(0xee, 0xee, 0xee).ink(), black);
        assert_eq!(
            Colour::rgb(0xff, 0xff, 0x00).ink(),
            black,
            "yellow is light"
        );
        assert_eq!(Colour::rgb(0x00, 0x00, 0xff).ink(), white, "blue is dark");
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
