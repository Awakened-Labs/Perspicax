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

use crate::{Edges, Rect, Rgba};

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

    /// `"#rrggbb"`, in lower case: the inverse of [`parse`](Self::parse).
    #[must_use]
    pub fn hex(self) -> String {
        let Self { r, g, b } = self;
        format!("#{r:02x}{g:02x}{b:02x}")
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
    /// Its title and buttons.
    pub focused_ink: Colour,
    /// Every other frame.
    pub unfocused: Colour,
    /// Their titles and buttons.
    pub unfocused_ink: Colour,
    /// How a window an agent may act on is marked out.
    pub agent: AgentLook,
    /// Which buttons the titlebar carries, at which end.
    pub buttons: Buttons,
    /// What the close button is lit in under the pointer.
    pub close_hover: Colour,
    /// How round the titlebar's top corners are, in logical pixels: 0 for
    /// square.
    pub radius: i32,
    /// The soft shadow cast under a window with a frame of ours.
    pub shadow: Shadow,
    /// How long its moves take.
    pub animation: Animation,
}

/// How long a frame's moves take: the `[decorations.animation]` table, each
/// in milliseconds, 0 for none, at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Animation {
    /// Rolling a window up to its titlebar, or down from it, the whole way.
    pub shade_ms: u32,
    /// A titlebar button's light coming up under the pointer, or going out.
    pub hover_ms: u32,
}

impl Default for Decorations {
    fn default() -> Self {
        Self {
            server: true,
            title: 24,
            border: 2,
            focused: Colour::rgb(0x2d, 0x6f, 0xa3),
            focused_ink: Colour::rgb(0xff, 0xff, 0xff),
            unfocused: Colour::rgb(0x47, 0x50, 0x57),
            unfocused_ink: Colour::rgb(0xff, 0xff, 0xff),
            agent: AgentLook::default(),
            buttons: Buttons::default(),
            close_hover: Colour::rgb(0xda, 0x44, 0x53),
            radius: 0,
            shadow: Shadow::default(),
            animation: Animation::default(),
        }
    }
}

/// A soft shadow under a window with a frame of ours: the
/// `[decorations.shadow]` table. It is for the eye alone: it covers nothing
/// an agent might act on, and takes no clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shadow {
    /// How far past the frame it reaches, in logical pixels: 0 for none.
    pub size: i32,
    /// How far it falls from under the window, right and down.
    pub offset: (i32, i32),
    /// How dark it is where it is darkest, as a percentage.
    pub opacity: u8,
    pub colour: Colour,
}

impl Default for Shadow {
    /// None.
    fn default() -> Self {
        Self {
            size: 0,
            offset: (0, 0),
            opacity: 35,
            colour: Colour::rgb(0, 0, 0),
        }
    }
}

/// How a piece of a shadow is drawn from the picture of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stretch {
    /// As it is: a corner, where the shadow rounds.
    None,
    /// One column of it stretched across: the top or the bottom, which
    /// fades the same way all along.
    Across,
    /// One row of it stretched down: a side.
    Down,
}

/// A piece of a shadow: where it goes, relative to the client's geometry,
/// and how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub at: Rect,
    pub stretch: Stretch,
}

/// The pieces the shadow under a frame whose outside is `outer` is drawn
/// as, its top corners rounded `radius` round: four corners, two edges
/// across and two down, ringing the frame, and nothing in the middle, which
/// the frame covers whatever the offset, so that a window seen through
/// shows no shadow under itself. None for a shadow of no size.
///
/// Every edge fades the same way all along it, so the corners and one line
/// across each edge are all that is ever drawn, whatever size the window is.
#[must_use]
pub fn shadow_pieces(outer: Rect, radius: i32, shadow: &Shadow) -> Vec<Piece> {
    let reach = shadow.size.max(0);
    if reach == 0 || outer.w <= 0 || outer.h <= 0 {
        return Vec::new();
    }
    let (dx, dy) = shadow.offset;
    // In from the edge of what casts it, far enough that the middle is under
    // the frame wherever it falls, and past the rounding of its corners.
    let inset = radius.max(dx.abs()).max(dy.abs()).max(0);
    let cast = Rect::new(outer.x + dx, outer.y + dy, outer.w, outer.h);
    let corner = reach + inset;
    let (left, top) = (cast.x - reach, cast.y - reach);
    let (right, bottom) = (cast.x + cast.w + reach, cast.y + cast.h + reach);
    let across = right - left - 2 * corner;
    let down = bottom - top - 2 * corner;
    let piece = |x, y, w, h, stretch| Piece {
        at: Rect::new(x, y, w, h),
        stretch,
    };
    [
        piece(left, top, corner, corner, Stretch::None),
        piece(right - corner, top, corner, corner, Stretch::None),
        piece(left, bottom - corner, corner, corner, Stretch::None),
        piece(
            right - corner,
            bottom - corner,
            corner,
            corner,
            Stretch::None,
        ),
        piece(left + corner, top, across, corner, Stretch::Across),
        piece(
            left + corner,
            bottom - corner,
            across,
            corner,
            Stretch::Across,
        ),
        piece(left, top + corner, corner, down, Stretch::Down),
        piece(right - corner, top + corner, corner, down, Stretch::Down),
    ]
    .into_iter()
    .filter(|piece| piece.at.w > 0 && piece.at.h > 0)
    .collect()
}

/// How dark the shadow under a frame whose outside is `outer` is at the
/// point `(x, y)`, as a share of its darkest: as a box blurred is, half as
/// dark at the edge of what casts it, darker under it, and fading to
/// nothing `shadow.size` beyond it. What casts it is the frame, offset, its
/// top corners rounded `radius` round as the frame's are, and its bottom
/// corners square.
#[must_use]
pub fn shadow_at(x: f64, y: f64, outer: Rect, radius: i32, shadow: &Shadow) -> f64 {
    let reach = f64::from(shadow.size.max(0));
    if reach <= 0.0 {
        return 0.0;
    }
    let (dx, dy) = shadow.offset;
    let (left, top) = (f64::from(outer.x + dx), f64::from(outer.y + dy));
    let (right, bottom) = (left + f64::from(outer.w), top + f64::from(outer.h));
    let radius = f64::from(radius.max(0));
    // How far outside the cast it is, less than nothing inside it: round a
    // top corner, from the corner's centre; elsewhere, from the nearer
    // edges.
    let outside = if y < top + radius && (x < left + radius || x > right - radius) {
        let centre = if x < left + radius {
            left + radius
        } else {
            right - radius
        };
        (x - centre).hypot(y - (top + radius)) - radius
    } else {
        let (across, down) = ((left - x).max(x - right), (top - y).max(y - bottom));
        if across <= 0.0 && down <= 0.0 {
            across.max(down)
        } else {
            across.max(0.0).hypot(down.max(0.0))
        }
    };
    1.0 - crate::transition::smoothstep((outside + reach) / (2.0 * reach))
}

/// How a window an agent may act on is marked out, where a person has to
/// agree to that: the `[decorations.agent]` table. Never where consent is
/// everyone's, which would mark every window there is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentLook {
    /// Its titlebar with the keyboard, where this compositor draws one.
    pub focused: Colour,
    /// Its titlebar without it.
    pub unfocused: Colour,
    /// How wide a line is drawn round it, just outside its frame, or its
    /// edge for a window that draws its own: 0 for none. Every such window
    /// has one, for most draw their own frame.
    pub outline: i32,
    pub outline_colour: Colour,
    /// The line drawn round a window for a moment when the person asks to
    /// grant it and it is refused: an X11 window, one nobody can say which
    /// process drew, or a program no agent is ever given.
    pub refused_colour: Colour,
}

impl Default for AgentLook {
    fn default() -> Self {
        Self {
            focused: Colour::rgb(0x5a, 0x2d, 0x82),
            unfocused: Colour::rgb(0x3b, 0x24, 0x54),
            outline: 3,
            outline_colour: Colour::rgb(0xb3, 0x7f, 0xeb),
            refused_colour: Colour::rgb(0xe5, 0x48, 0x4d),
        }
    }
}

/// What a frame is drawn in: its bar, and the title and buttons on it. A
/// window an agent may act on (`marked`) wears the agent's colours, written
/// in whichever of black or white reads on them.
#[must_use]
pub fn frame_colours(decorations: &Decorations, in_use: bool, marked: bool) -> (Colour, Colour) {
    let agent = &decorations.agent;
    match (marked, in_use) {
        (true, true) => (agent.focused, agent.focused.ink()),
        (true, false) => (agent.unfocused, agent.unfocused.ink()),
        (false, true) => (decorations.focused, decorations.focused_ink),
        (false, false) => (decorations.unfocused, decorations.unfocused_ink),
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

/// The client rect a window rolled up to its titlebar is framed as: where
/// its client is, as wide, and of no height. Its frame is then the titlebar,
/// with the border above it, and the bottom border just under the bar
/// ([`frame_rects`]); the side borders, as tall as the client, are gone.
/// Its client keeps its own size, and is simply not drawn.
#[must_use]
pub fn rolled_up(client: Rect) -> Rect {
    Rect::new(client.x, client.y, client.w, 0)
}

/// How round the top corners of a frame with these insets, `width` wide,
/// are drawn in `look`: [`Decorations::radius`] for a normal window, and
/// square for a maximized one, whose bar meets the screen's own corners,
/// and for one with no titlebar of ours. Never rounder than the titlebar is
/// tall, so the side borders below it stay straight, nor than half the
/// frame is wide.
#[must_use]
pub fn corner_radius(decorations: &Decorations, look: Look, insets: Insets, width: i32) -> i32 {
    if look != Look::Normal {
        return 0;
    }
    decorations.radius.min(insets.top).min(width / 2).max(0)
}

/// The pieces a frame with this client rect is drawn as, its top corners
/// rounded `radius` round: the solid rectangles, and the two squares, top
/// left and top right, each drawn as a quarter disc, the bar's colour
/// inside its arc and nothing outside it. Together they tile
/// [`frame_rects`] exactly, and a radius of 0 is [`frame_rects`] and no
/// corners.
///
/// The facts still have [`frame_rects`]: a pixel outside an arc is said to
/// be frame, though nothing is drawn there, which errs the way that refuses
/// rather than the way that lets a click through.
#[must_use]
pub fn rounded_rects(client: Rect, insets: Insets, radius: i32) -> (Vec<Rect>, Vec<Rect>) {
    let strips = frame_rects(client, insets);
    let radius = radius.clamp(0, insets.top);
    if radius == 0 || strips.is_empty() {
        return (strips, Vec::new());
    }
    let top = strips[0];
    let corners = vec![
        Rect::new(top.x, top.y, radius, radius),
        Rect::new(top.x + top.w - radius, top.y, radius, radius),
    ];
    let solids = [
        Rect::new(top.x + radius, top.y, top.w - 2 * radius, radius),
        Rect::new(top.x, top.y + radius, top.w, top.h - radius),
    ]
    .into_iter()
    .chain(strips.into_iter().skip(1))
    .filter(|strip| strip.w > 0 && strip.h > 0)
    .collect();
    (solids, corners)
}

/// The pieces the line `width` wide round a frame whose corners are
/// rounded `radius` round is drawn as: the solid rectangles, and the two
/// squares, `radius + width` a side, each drawn as a quarter ring from
/// `radius` to `radius + width` round, following the frame's arc. They tile
/// [`outline_rects`] and the frame's own two corner squares: the ring's
/// inner edge reaches into the clear pixels outside the frame's arc, and
/// draws only there. A radius of 0 is [`outline_rects`] and no corners.
#[must_use]
pub fn rounded_outline(around: Rect, width: i32, radius: i32) -> (Vec<Rect>, Vec<Rect>) {
    let strips = outline_rects(around, width);
    let radius = radius.max(0);
    if radius == 0 || strips.is_empty() {
        return (strips, Vec::new());
    }
    let width = width.max(0);
    let side = radius + width;
    let (left, top) = (around.x - width, around.y - width);
    let right = around.x + around.w + width;
    let corners = vec![
        Rect::new(left, top, side, side),
        Rect::new(right - side, top, side, side),
    ];
    let solids = [
        Rect::new(left + side, top, around.w - 2 * radius, width),
        Rect::new(left, around.y + radius, width, around.h - radius),
        Rect::new(
            around.x + around.w,
            around.y + radius,
            width,
            around.h - radius,
        ),
        Rect::new(left, around.y + around.h, around.w + 2 * width, width),
    ]
    .into_iter()
    .filter(|strip| strip.w > 0 && strip.h > 0)
    .collect();
    (solids, corners)
}

/// The rectangles a line `width` wide round `around` is drawn as, just
/// outside it: [`frame_rects`] of a frame as wide on every side. No
/// rectangles for a width of 0.
#[must_use]
pub fn outline_rects(around: Rect, width: i32) -> Vec<Rect> {
    let width = width.max(0);
    let even = Insets {
        top: width,
        left: width,
        right: width,
        bottom: width,
    };
    frame_rects(around, even)
}

/// A button on the titlebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameButton {
    /// Roll the window up to its titlebar, or back.
    Shade,
    /// Keep the window on every workspace, or take it back to one.
    Sticky,
    /// Keep the window in front of the others, or let it go back among
    /// them.
    OnTop,
    Minimize,
    Maximize,
    Close,
}

impl FrameButton {
    /// Every button, in the order the issue that let a person choose them
    /// listed them, and the one #64 added after.
    pub const ALL: [Self; 6] = [
        Self::Shade,
        Self::Sticky,
        Self::Minimize,
        Self::Maximize,
        Self::Close,
        Self::OnTop,
    ];

    /// Its name as `[decorations] left` and `right` write it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Shade => "shade",
            Self::Sticky => "sticky",
            Self::OnTop => "on-top",
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Close => "close",
        }
    }
}

/// What a titlebar button shows: the button, and for one that toggles,
/// which way it is. Each is named for what a press of the button then
/// does, as the `[decorations.icons]` keys are: maximize, or restore what
/// is maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Close,
    Minimize,
    Maximize,
    /// Maximize, while maximized.
    Restore,
    Shade,
    /// Shade, while rolled up.
    Unshade,
    Stick,
    /// Sticky, while on every workspace.
    Unstick,
    OnTop,
    /// On top, while kept in front of the others.
    OffTop,
}

impl Face {
    /// Every face, a button's and then its toggled one's.
    pub const ALL: [Self; 10] = [
        Self::Close,
        Self::Minimize,
        Self::Maximize,
        Self::Restore,
        Self::Shade,
        Self::Unshade,
        Self::Stick,
        Self::Unstick,
        Self::OnTop,
        Self::OffTop,
    ];

    /// Its name as `[decorations.icons]` writes it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Restore => "restore",
            Self::Shade => "shade",
            Self::Unshade => "unshade",
            Self::Stick => "stick",
            Self::Unstick => "unstick",
            Self::OnTop => "on-top",
            Self::OffTop => "off-top",
        }
    }

    /// The face `button` shows, `on` being whether its toggle is: the
    /// window maximized, rolled up, on every workspace, or kept on top.
    /// Close and minimize toggle nothing, and show one face either way.
    #[must_use]
    pub const fn of(button: FrameButton, on: bool) -> Self {
        match (button, on) {
            (FrameButton::Close, _) => Self::Close,
            (FrameButton::Minimize, _) => Self::Minimize,
            (FrameButton::Maximize, false) => Self::Maximize,
            (FrameButton::Maximize, true) => Self::Restore,
            (FrameButton::Shade, false) => Self::Shade,
            (FrameButton::Shade, true) => Self::Unshade,
            (FrameButton::Sticky, false) => Self::Stick,
            (FrameButton::Sticky, true) => Self::Unstick,
            (FrameButton::OnTop, false) => Self::OnTop,
            (FrameButton::OnTop, true) => Self::OffTop,
        }
    }
}

/// Which buttons a titlebar carries, and at which end: the
/// `[decorations] left` and `right` lists, each in the order it is drawn
/// from the bar's left, with the title or the tabs between the two. A
/// button is at one end at most, and either end may have none.
///
/// Held in arrays rather than lists, so that [`Decorations`] stays `Copy`:
/// there are only so many buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buttons {
    left: [Option<FrameButton>; FrameButton::ALL.len()],
    right: [Option<FrameButton>; FrameButton::ALL.len()],
}

impl Buttons {
    /// `left` at the bar's left end and `right` at its right, or the first
    /// button listed a second time, at either end.
    pub fn new(left: &[FrameButton], right: &[FrameButton]) -> Result<Self, FrameButton> {
        let listed: Vec<FrameButton> = left.iter().chain(right).copied().collect();
        for (n, button) in listed.iter().enumerate() {
            if listed[..n].contains(button) {
                return Err(*button);
            }
        }
        // Listed once each, so there are no more of them than there are
        // buttons, and every one has a place.
        let mut buttons = Self {
            left: [None; FrameButton::ALL.len()],
            right: [None; FrameButton::ALL.len()],
        };
        for (place, button) in buttons.left.iter_mut().zip(left) {
            *place = Some(*button);
        }
        for (place, button) in buttons.right.iter_mut().zip(right) {
            *place = Some(*button);
        }
        Ok(buttons)
    }

    /// The buttons at the bar's left end, from the left.
    pub fn left(&self) -> impl Iterator<Item = FrameButton> + '_ {
        self.left.iter().flatten().copied()
    }

    /// The buttons at its right end, from the left: close, by default, last
    /// and in the corner.
    pub fn right(&self) -> impl Iterator<Item = FrameButton> + '_ {
        self.right.iter().flatten().copied()
    }

    /// Whether the bar carries `button`, at either end.
    #[must_use]
    pub fn contains(&self, button: FrameButton) -> bool {
        self.left()
            .chain(self.right())
            .any(|carried| carried == button)
    }
}

impl Default for Buttons {
    /// Shade, minimize, maximize and close, all at the right, close in the
    /// corner, and none at the left.
    fn default() -> Self {
        use FrameButton::{Close, Maximize, Minimize, Shade};
        Self::new(&[], &[Shade, Minimize, Maximize, Close]).expect("four buttons, once each")
    }
}

/// How a button on a titlebar is lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    /// The pointer is over it.
    Hover,
    /// It was pressed, and the pointer is over it still: letting go here
    /// does its job.
    Pressed,
}

/// Which of a window's titlebar buttons is lit, and how. `held` is the one
/// the left button went down on, if it went down on one of this window's,
/// and `over` is the one of this window's the pointer is over now.
///
/// A button held is lit only while the pointer is over it, because only
/// there does letting go do anything; dragged off it, nothing is lit, not
/// even the button the pointer is over then, which the press is not for.
#[must_use]
pub fn light(held: Option<FrameButton>, over: Option<FrameButton>) -> Option<(FrameButton, Light)> {
    match (held, over) {
        (Some(held), Some(over)) if held == over => Some((held, Light::Pressed)),
        (Some(_), _) => None,
        (None, over) => over.map(|over| (over, Light::Hover)),
    }
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

/// What a titlebar button lit as `light` is lit in, on a bar whose title and
/// buttons are written in `ink`: that ink, faint under the pointer and less
/// faint pressed, so it reads on whatever colour the bar is, an agent's
/// included; or for close, whose press cannot be taken back, the close
/// colour, darker pressed.
#[must_use]
pub fn wash(decorations: &Decorations, button: FrameButton, light: Light, ink: Colour) -> Rgba {
    const HOVER: u8 = 0x33;
    const PRESSED: u8 = 0x59;
    let lit = |colour: Colour, a: u8| Rgba::new(colour.r, colour.g, colour.b, a);
    match (button, light) {
        (FrameButton::Close, Light::Hover) => lit(decorations.close_hover, 0xff),
        (FrameButton::Close, Light::Pressed) => {
            lit(decorations.close_hover.mix(Colour::rgb(0, 0, 0), 200), 0xff)
        }
        (_, Light::Hover) => lit(ink, HOVER),
        (_, Light::Pressed) => lit(ink, PRESSED),
    }
}

/// The titlebar's own strip, without the border above it: directly above the
/// client and as wide as it. `None` for a frame with no titlebar.
#[must_use]
pub fn titlebar(client: Rect, insets: Insets, decorations: &Decorations) -> Option<Rect> {
    let height = decorations.title.min(insets.top);
    (height > 0 && client.w > 0).then(|| Rect::new(client.x, client.y - height, client.w, height))
}

/// The titlebar's buttons, square, at the two ends [`Decorations::buttons`]
/// puts them, with the title or the tabs between: by default shade,
/// minimize, maximize and close at the right, close in the corner.
///
/// As many as fit. Close is the last to go, so a narrow window can still be
/// closed. The others go from the inside out, the nearest the title first,
/// and of two as near, the one on the left first: by default shade goes
/// first, then minimize, then maximize. They come in the order they are
/// kept in, close first.
///
/// `shades` is false for a strip of tabs over a frame its client draws,
/// which has no titlebar of ours to roll up to, and no shade button.
#[must_use]
pub fn buttons(
    client: Rect,
    insets: Insets,
    decorations: &Decorations,
    shades: bool,
) -> Vec<(FrameButton, Rect)> {
    titlebar(client, insets, decorations).map_or_else(Vec::new, |bar| {
        buttons_in(bar, &decorations.buttons, shades)
    })
}

/// The buttons on a titlebar that is `bar`, as [`buttons`] places them.
#[must_use]
pub fn buttons_in(bar: Rect, buttons: &Buttons, shades: bool) -> Vec<(FrameButton, Rect)> {
    lay_out(bar, buttons, shades).0
}

/// The tabs on a titlebar that is `bar`, for a group of `count`: the room
/// between the buttons at its two ends, shared out evenly, left to right.
/// One tab, the whole room, for a window in no group. `shades` as for
/// [`buttons`].
#[must_use]
pub fn tab_rects(bar: Rect, count: usize, buttons: &Buttons, shades: bool) -> Vec<Rect> {
    let (_, (start, end)) = lay_out(bar, buttons, shades);
    let room = (end - start).max(0);
    let count = i32::try_from(count.max(1)).unwrap_or(1);
    (0..count)
        .map(|n| {
            let (from, to) = (room * n / count, room * (n + 1) / count);
            Rect::new(start + from, bar.y, to - from, bar.h)
        })
        .collect()
}

/// The buttons kept on `bar`, placed, in the order they are kept in, and
/// where the room between the two ends starts and ends.
fn lay_out(bar: Rect, buttons: &Buttons, shades: bool) -> (Vec<(FrameButton, Rect)>, (i32, i32)) {
    let side = bar.h;
    let fit = usize::try_from(bar.w / side.max(1)).unwrap_or(0);
    let shown = |button: &FrameButton| shades || *button != FrameButton::Shade;
    let left: Vec<FrameButton> = buttons.left().filter(shown).collect();
    let right: Vec<FrameButton> = buttons.right().filter(shown).collect();

    // How far each is from the title, counted in the buttons between them:
    // the last on the left and the first on the right are beside it.
    let mut kept: Vec<(bool, usize, FrameButton)> = left
        .iter()
        .enumerate()
        .map(|(n, button)| (true, left.len() - 1 - n, *button))
        .chain(
            right
                .iter()
                .enumerate()
                .map(|(n, button)| (false, n, *button)),
        )
        .collect();
    kept.sort_by_key(|&(on_left, from_title, button)| {
        (
            button != FrameButton::Close,
            std::cmp::Reverse(from_title),
            on_left,
        )
    });
    kept.truncate(fit);

    let is_kept = |button: &FrameButton| kept.iter().any(|(_, _, held)| held == button);
    let width = |count: usize| i32::try_from(count).unwrap_or(0) * side;
    let at_left: Vec<FrameButton> = left.into_iter().filter(is_kept).collect();
    let at_right: Vec<FrameButton> = right.into_iter().filter(is_kept).collect();
    let (start, end) = (
        bar.x + width(at_left.len()),
        bar.x + bar.w - width(at_right.len()),
    );
    let place = |button: FrameButton| {
        let (from, n) = match at_left.iter().position(|left| *left == button) {
            Some(n) => (bar.x, n),
            None => (
                end,
                at_right
                    .iter()
                    .position(|right| *right == button)
                    .unwrap_or(0),
            ),
        };
        (button, Rect::new(from + width(n), bar.y, side, side))
    };
    (
        kept.iter().map(|&(_, _, button)| place(button)).collect(),
        (start, end),
    )
}

/// What part of the frame of a window with this client rect `point` is on,
/// or `None` if it is on the client or off the frame altogether.
///
/// `resizable` is false for a maximized window: it keeps its titlebar, and
/// has no edge to drag. It is false too for a window rolled up to its
/// titlebar, which is asked about as its [`rolled_up`] client: then any of
/// its frame is its titlebar, to move it by, or a button. `tabs` is how many
/// tabs its group has, one for a window in no group, which has a title and
/// no tabs, and `shades` is as for [`buttons`]. Otherwise every edge can be
/// grabbed from the border and up to [`GRIP`] pixels outside it, and a
/// corner from [`CORNER`] pixels along either side of it, so the titlebar's
/// two ends resize diagonally.
#[must_use]
pub fn part_at(
    point: (f64, f64),
    client: Rect,
    insets: Insets,
    decorations: &Decorations,
    resizable: bool,
    tabs: usize,
    shades: bool,
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
    if let Some((button, _)) = buttons(client, insets, decorations, shades)
        .into_iter()
        .find(|(_, rect)| on(*rect))
    {
        return Some(Part::Button(button));
    }
    let tab = titlebar(client, insets, decorations)
        .filter(|_| tabs > 1)
        .and_then(|bar| {
            tab_rects(bar, tabs, &decorations.buttons, shades)
                .into_iter()
                .position(on)
        });
    Some(tab.map_or(Part::Title, Part::Tab))
}

/// A press of a button: when, in milliseconds, and where.
pub type Press = (u32, (f64, f64));

/// The double-click time, in milliseconds, of a config that does not say:
/// GTK's and Qt's.
pub const DOUBLE_CLICK_MS: u32 = 400;

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
///
/// What every window keeps, whenever the room it has changes. A window
/// opening with a person at the seat is first put wholly on the area, once,
/// by `settle`.
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

    /// Rolled up, the frame is the bar, with the border above it and the
    /// border under it, and nothing beside a client that is not drawn.
    /// Maximized, it has no border, and is the bar alone.
    #[test]
    fn a_window_rolled_up_is_framed_by_its_titlebar_and_the_border_round_it() {
        let client = Rect::new(100, 100, 400, 300);
        assert_eq!(
            frame_rects(rolled_up(client), framed()),
            vec![Rect::new(98, 74, 404, 26), Rect::new(98, 100, 404, 2)]
        );
        let decorations = Decorations::default();
        let maximized = Insets::of(&decorations, Look::Maximized);
        assert_eq!(
            frame_rects(rolled_up(Rect::new(0, 24, 1920, 1056)), maximized),
            vec![Rect::new(0, 0, 1920, 24)]
        );
    }

    /// Nothing is under the bar to press on, and no edge to drag: every
    /// pixel of the frame moves the window, but its buttons.
    #[test]
    fn a_rolled_up_window_has_no_edges_and_nothing_under_its_titlebar() {
        let decorations = Decorations::default();
        let client = rolled_up(Rect::new(100, 100, 400, 300));
        let at = |x: f64, y: f64| part_at((x, y), client, framed(), &decorations, false, 1, true);
        assert_eq!(
            at(300.0, 101.0),
            Some(Part::Title),
            "the border under the bar"
        );
        assert_eq!(at(300.0, 103.0), None, "where the client was");
        assert_eq!(at(300.0, 250.0), None);
        assert_eq!(at(99.0, 80.0), Some(Part::Title), "not an edge");
        assert_eq!(at(99.0, 101.0), Some(Part::Title), "nor a corner");
        assert_eq!(at(95.0, 80.0), None, "and no grip outside it");
        assert_eq!(at(488.0, 88.0), Some(Part::Button(FrameButton::Close)));
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
            buttons(client, framed(), &decorations, true),
            vec![
                (FrameButton::Close, Rect::new(476, 76, 24, 24)),
                (FrameButton::Maximize, Rect::new(452, 76, 24, 24)),
                (FrameButton::Minimize, Rect::new(428, 76, 24, 24)),
                (FrameButton::Shade, Rect::new(404, 76, 24, 24)),
            ]
        );
        let narrow = Rect::new(0, 100, 50, 50);
        assert_eq!(
            buttons(narrow, framed(), &decorations, true)
                .iter()
                .map(|(button, _)| *button)
                .collect::<Vec<_>>(),
            vec![FrameButton::Close, FrameButton::Maximize],
            "a narrow window keeps close first"
        );
    }

    /// Leftmost of the four, and the first to go as the bar narrows: a
    /// window too narrow for all of them can still be closed, maximized and
    /// minimized before it can be rolled up.
    #[test]
    fn the_shade_button_is_leftmost_and_the_first_to_go_on_a_narrow_bar() {
        let on = |width| {
            buttons_in(Rect::new(0, 0, width, 24), &Buttons::default(), true)
                .into_iter()
                .map(|(button, _)| button)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            on(96),
            vec![
                FrameButton::Close,
                FrameButton::Maximize,
                FrameButton::Minimize,
                FrameButton::Shade,
            ]
        );
        assert_eq!(
            buttons_in(Rect::new(0, 0, 96, 24), &Buttons::default(), true)[3],
            (FrameButton::Shade, Rect::new(0, 0, 24, 24)),
            "at the bar's left end"
        );
        assert_eq!(
            on(72),
            vec![
                FrameButton::Close,
                FrameButton::Maximize,
                FrameButton::Minimize
            ]
        );
        assert_eq!(on(24), vec![FrameButton::Close]);
    }

    /// A strip of tabs over a window that draws its own frame has no
    /// titlebar of ours to roll up to: the other three keep their places,
    /// the tabs take the room, and where a shade button would be is the
    /// strip.
    #[test]
    fn a_frame_its_client_draws_has_no_shade_button() {
        let bar = Rect::new(100, 76, 400, 24);
        let shown = buttons_in(bar, &Buttons::default(), false);
        assert_eq!(shown, buttons_in(bar, &Buttons::default(), true)[..3]);
        assert_eq!(
            tab_rects(bar, 1, &Buttons::default(), false),
            vec![Rect::new(100, 76, 328, 24)]
        );
        let decorations = Decorations::default();
        let client = Rect::new(100, 100, 400, 300);
        let at = |x: f64| part_at((x, 88.0), client, framed(), &decorations, true, 2, false);
        assert_eq!(at(416.0), Some(Part::Tab(1)));
        assert_eq!(at(440.0), Some(Part::Button(FrameButton::Minimize)));
    }

    #[test]
    fn a_point_on_the_frame_says_which_part() {
        let decorations = Decorations::default();
        let client = Rect::new(100, 100, 400, 300);
        let at = |x: f64, y: f64| part_at((x, y), client, framed(), &decorations, true, 1, true);
        assert_eq!(at(200.0, 90.0), Some(Part::Title));
        assert_eq!(at(488.0, 88.0), Some(Part::Button(FrameButton::Close)));
        assert_eq!(at(440.0, 88.0), Some(Part::Button(FrameButton::Minimize)));
        assert_eq!(at(416.0, 88.0), Some(Part::Button(FrameButton::Shade)));
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
        assert_eq!(
            tab_rects(bar, 1, &Buttons::default(), true),
            vec![Rect::new(100, 76, 304, 24)]
        );
        assert_eq!(
            tab_rects(bar, 3, &Buttons::default(), true),
            vec![
                Rect::new(100, 76, 101, 24),
                Rect::new(201, 76, 101, 24),
                Rect::new(302, 76, 102, 24),
            ],
            "evenly, with the odd pixel at the end"
        );
        let decorations = Decorations::default();
        let client = Rect::new(100, 100, 400, 300);
        let at =
            |x: f64, tabs| part_at((x, 88.0), client, framed(), &decorations, true, tabs, true);
        assert_eq!(at(150.0, 3), Some(Part::Tab(0)));
        assert_eq!(at(400.0, 3), Some(Part::Tab(2)));
        assert_eq!(
            at(400.0, 1),
            Some(Part::Title),
            "one window: a title, not a tab"
        );
        assert_eq!(at(488.0, 3), Some(Part::Button(FrameButton::Close)));
    }

    /// Fluxbox's own bar, minimal's until #64: sticky at the left, the other
    /// four at the right.
    fn fluxbox() -> Buttons {
        use FrameButton::*;
        Buttons::new(&[Sticky], &[Shade, Minimize, Maximize, Close]).unwrap()
    }

    #[test]
    fn buttons_at_both_ends_leave_the_tabs_the_room_between() {
        let bar = Rect::new(100, 76, 400, 24);
        let placed = buttons_in(bar, &fluxbox(), true);
        let at = |wanted| {
            placed
                .iter()
                .find(|(button, _)| *button == wanted)
                .map(|(_, rect)| *rect)
        };
        assert_eq!(at(FrameButton::Sticky), Some(Rect::new(100, 76, 24, 24)));
        assert_eq!(at(FrameButton::Shade), Some(Rect::new(404, 76, 24, 24)));
        assert_eq!(at(FrameButton::Close), Some(Rect::new(476, 76, 24, 24)));
        assert_eq!(
            tab_rects(bar, 1, &fluxbox(), true),
            vec![Rect::new(124, 76, 280, 24)],
            "after sticky, before shade"
        );
        assert_eq!(
            tab_rects(bar, 2, &fluxbox(), true),
            vec![Rect::new(124, 76, 140, 24), Rect::new(264, 76, 140, 24)]
        );
        let bare = Buttons::new(&[], &[]).unwrap();
        assert!(buttons_in(bar, &bare, true).is_empty(), "no buttons at all");
        assert_eq!(
            tab_rects(bar, 1, &bare, true),
            vec![bar],
            "and the whole bar"
        );
    }

    /// Close is the last to go; the rest go from beside the title outwards,
    /// and of two as near the title, the one on the left goes first.
    #[test]
    fn a_narrow_bar_keeps_close_then_drops_from_the_title_outwards() {
        use FrameButton::*;
        let on = |width| {
            let mut kept: Vec<_> = buttons_in(Rect::new(0, 0, width, 24), &fluxbox(), true)
                .into_iter()
                .map(|(button, _)| button)
                .collect();
            kept.sort_by_key(|button| FrameButton::ALL.iter().position(|b| b == button));
            kept
        };
        assert_eq!(on(120), vec![Shade, Sticky, Minimize, Maximize, Close]);
        assert_eq!(
            on(96),
            vec![Shade, Minimize, Maximize, Close],
            "sticky and shade are as near the title, and the left one goes"
        );
        assert_eq!(on(72), vec![Minimize, Maximize, Close]);
        assert_eq!(on(48), vec![Maximize, Close]);
        assert_eq!(on(24), vec![Close]);
        assert!(on(23).is_empty());
        assert_eq!(
            buttons_in(Rect::new(0, 0, 48, 24), &fluxbox(), true),
            vec![
                (Close, Rect::new(24, 0, 24, 24)),
                (Maximize, Rect::new(0, 0, 24, 24))
            ],
            "what is kept closes up to the end it is at"
        );
    }

    /// minimal's bar puts on top beside sticky, and so beside the title: of
    /// the buttons as near it, the one on the left, it goes first.
    #[test]
    fn an_on_top_button_is_let_go_first_on_a_narrow_minimal_bar() {
        use FrameButton::*;
        let minimal = Buttons::new(&[Sticky, OnTop], &[Shade, Minimize, Maximize, Close]).unwrap();
        let on = |width| {
            buttons_in(Rect::new(100, 76, width, 24), &minimal, true)
                .into_iter()
                .collect::<Vec<_>>()
        };
        let all = on(400);
        assert_eq!(all.len(), 6);
        assert!(
            all.contains(&(OnTop, Rect::new(124, 76, 24, 24))),
            "beside sticky, at the title's left: {all:?}"
        );
        let fewer: Vec<_> = on(120).into_iter().map(|(button, _)| button).collect();
        assert_eq!(fewer.len(), 5);
        assert!(!fewer.contains(&OnTop), "{fewer:?}");
    }

    /// Close is kept first wherever it is: at the far left, as on a Mac, or
    /// beside the title.
    #[test]
    fn close_is_kept_wherever_it_is_listed() {
        use FrameButton::*;
        let mac = Buttons::new(&[Close, Minimize, Maximize], &[]).unwrap();
        assert_eq!(
            buttons_in(Rect::new(0, 0, 24, 24), &mac, true),
            vec![(Close, Rect::new(0, 0, 24, 24))]
        );
        let beside = Buttons::new(&[], &[Close, Maximize]).unwrap();
        assert_eq!(
            buttons_in(Rect::new(0, 0, 24, 24), &beside, true),
            vec![(Close, Rect::new(0, 0, 24, 24))],
            "and closes up to its end"
        );
        assert_eq!(
            buttons_in(Rect::new(0, 0, 400, 24), &mac, true),
            vec![
                (Close, Rect::new(0, 0, 24, 24)),
                (Minimize, Rect::new(24, 0, 24, 24)),
                (Maximize, Rect::new(48, 0, 24, 24)),
            ],
            "kept close first, then the farthest from the title"
        );
    }

    #[test]
    fn a_point_on_a_left_hand_button_says_which() {
        let decorations = Decorations {
            buttons: fluxbox(),
            ..Decorations::default()
        };
        let client = Rect::new(100, 100, 400, 300);
        let at =
            |x: f64, tabs| part_at((x, 88.0), client, framed(), &decorations, true, tabs, true);
        assert_eq!(at(110.0, 1), Some(Part::Button(FrameButton::Sticky)));
        assert_eq!(at(130.0, 1), Some(Part::Title));
        assert_eq!(
            at(130.0, 2),
            Some(Part::Tab(0)),
            "the first tab is after it"
        );
        assert_eq!(at(488.0, 1), Some(Part::Button(FrameButton::Close)));
        assert_eq!(
            at(99.0, 1),
            Some(Part::Edge(Edges {
                top: true,
                left: true,
                ..Edges::default()
            })),
            "the bar's end still resizes"
        );
    }

    /// The layout nobody chose is the one there always was: every button at
    /// the right, where the tests above, written before there was a choice,
    /// still find them.
    #[test]
    fn the_default_layout_is_todays_to_the_pixel() {
        let today = Buttons::new(
            &[],
            &[
                FrameButton::Shade,
                FrameButton::Minimize,
                FrameButton::Maximize,
                FrameButton::Close,
            ],
        )
        .unwrap();
        assert_eq!(Buttons::default(), today);
        assert_eq!(Decorations::default().buttons, today);
        assert_eq!(Buttons::default().left().count(), 0);
        assert!(!Buttons::default().contains(FrameButton::Sticky));
        assert!(!Buttons::default().contains(FrameButton::OnTop));
    }

    #[test]
    fn a_button_listed_twice_is_named() {
        use FrameButton::*;
        assert_eq!(Buttons::new(&[Close], &[Shade, Close]), Err(Close));
        assert_eq!(Buttons::new(&[Sticky, Sticky], &[]), Err(Sticky));
        assert_eq!(
            Buttons::new(&[Shade, Sticky, Minimize], &[Maximize, Close, Shade]),
            Err(Shade),
            "six names for five buttons"
        );
        assert!(
            Buttons::new(&FrameButton::ALL, &[]).is_ok(),
            "all six at one end"
        );
    }

    #[test]
    fn light_is_pressed_only_while_held_over_its_own_button() {
        use FrameButton::*;
        assert_eq!(light(None, Some(Close)), Some((Close, Light::Hover)));
        assert_eq!(light(None, None), None);
        assert_eq!(
            light(Some(Close), Some(Close)),
            Some((Close, Light::Pressed))
        );
        assert_eq!(light(Some(Close), None), None, "dragged off it");
        assert_eq!(
            light(Some(Close), Some(Maximize)),
            None,
            "nor the one it was dragged to"
        );
    }

    /// Under the pointer a button is lit faintly in its bar's ink, which
    /// reads on any bar, and pressed less faintly; close goes red, darker
    /// pressed.
    #[test]
    fn a_button_is_lit_in_its_bars_ink_and_close_in_red() {
        let decorations = Decorations::default();
        let black = Colour::rgb(0, 0, 0);
        let white = Colour::rgb(0xff, 0xff, 0xff);
        let hover = wash(&decorations, FrameButton::Maximize, Light::Hover, white);
        let pressed = wash(&decorations, FrameButton::Maximize, Light::Pressed, white);
        assert_eq!((hover.r, hover.g, hover.b), (0xff, 0xff, 0xff));
        assert!(
            0 < hover.a && hover.a < pressed.a && pressed.a < 0x80,
            "{hover:?} {pressed:?}"
        );
        let on_light = wash(&decorations, FrameButton::Shade, Light::Hover, black);
        assert_eq!(
            (on_light.r, on_light.g, on_light.b),
            (0, 0, 0),
            "a light bar's ink"
        );
        assert_eq!(
            wash(&decorations, FrameButton::Close, Light::Hover, white),
            Rgba::new(0xda, 0x44, 0x53, 0xff)
        );
        let red = wash(&decorations, FrameButton::Close, Light::Pressed, white);
        assert!(
            red.a == 0xff && red.r < 0xda && red.r > 0x80,
            "darker: {red:?}"
        );
    }

    /// A toggle's second face is what a press then does; close and
    /// minimize have one face whatever the window is.
    #[test]
    fn a_toggle_shows_what_a_press_of_it_does() {
        use FrameButton::*;
        assert_eq!(Face::of(Maximize, false), Face::Maximize);
        assert_eq!(Face::of(Maximize, true), Face::Restore);
        assert_eq!(Face::of(Shade, true), Face::Unshade);
        assert_eq!(Face::of(Sticky, true), Face::Unstick);
        assert_eq!(Face::of(Sticky, false), Face::Stick);
        assert_eq!(Face::of(OnTop, false), Face::OnTop);
        assert_eq!(Face::of(OnTop, true), Face::OffTop);
        assert_eq!(Face::of(Close, true), Face::of(Close, false));
        assert_eq!(Face::of(Minimize, true), Face::Minimize);
        let keys: Vec<_> = Face::ALL.iter().map(|face| face.key()).collect();
        assert_eq!(
            keys,
            [
                "close", "minimize", "maximize", "restore", "shade", "unshade", "stick", "unstick",
                "on-top", "off-top"
            ]
        );
    }

    /// Every pixel of a list of rectangles, once each, or `None` if any is
    /// covered twice.
    fn tiled(pieces: &[Rect]) -> Option<std::collections::BTreeSet<(i32, i32)>> {
        let mut covered = std::collections::BTreeSet::new();
        for piece in pieces {
            for y in piece.y..piece.y + piece.h {
                for x in piece.x..piece.x + piece.w {
                    if !covered.insert((x, y)) {
                        return None;
                    }
                }
            }
        }
        Some(covered)
    }

    /// Rounded or not, a frame covers the same pixels: the solids and the
    /// corner squares tile the square frame, none twice. A radius of 0 is
    /// the square frame, and no corners.
    #[test]
    fn rounded_pieces_tile_the_square_frame_exactly() {
        let client = Rect::new(100, 100, 400, 300);
        let square = tiled(&frame_rects(client, framed())).unwrap();
        for radius in [1, 5, 16, 26] {
            let (solids, corners) = rounded_rects(client, framed(), radius);
            assert_eq!(corners.len(), 2);
            assert_eq!(corners[0], Rect::new(98, 74, radius, radius), "top left");
            let all: Vec<Rect> = solids.iter().chain(&corners).copied().collect();
            assert_eq!(tiled(&all).as_ref(), Some(&square), "radius {radius}");
        }
        let rolled = rolled_up(client);
        let (solids, corners) = rounded_rects(rolled, framed(), 5);
        let all: Vec<Rect> = solids.iter().chain(&corners).copied().collect();
        assert_eq!(
            tiled(&all),
            tiled(&frame_rects(rolled, framed())),
            "rolled up too"
        );
        assert_eq!(
            rounded_rects(client, framed(), 0),
            (frame_rects(client, framed()), vec![])
        );

        let around = outset(client, framed());
        let ring = tiled(&outline_rects(around, 3)).unwrap();
        let (solids, corners) = rounded_outline(around, 3, 5);
        assert_eq!(
            corners[1],
            Rect::new(502 + 3 - 8, 71, 8, 8),
            "top right, 5 + 3 a side"
        );
        let all: Vec<Rect> = solids.iter().chain(&corners).copied().collect();
        let (_, frame_corners) = rounded_rects(client, framed(), 5);
        let mut reached = ring;
        reached.extend(tiled(&frame_corners).unwrap());
        assert_eq!(
            tiled(&all).as_ref(),
            Some(&reached),
            "the agent's line, round the frame's arcs"
        );
        assert_eq!(
            rounded_outline(around, 3, 0),
            (outline_rects(around, 3), vec![])
        );
    }

    /// A maximized window's bar meets the screen's corners, and a window
    /// that draws its own frame has no bar of ours: both are square.
    #[test]
    fn a_maximized_or_fullscreen_frame_is_square() {
        let decorations = Decorations {
            radius: 6,
            ..Decorations::default()
        };
        let normal = Insets::of(&decorations, Look::Normal);
        assert_eq!(corner_radius(&decorations, Look::Normal, normal, 400), 6);
        let maximized = Insets::of(&decorations, Look::Maximized);
        assert_eq!(
            corner_radius(&decorations, Look::Maximized, maximized, 400),
            0
        );
        assert_eq!(
            corner_radius(&decorations, Look::Fullscreen, Insets::NONE, 400),
            0
        );
        assert_eq!(
            corner_radius(&decorations, Look::Normal, Insets::NONE, 400),
            0
        );
    }

    #[test]
    fn a_radius_never_reaches_past_the_titlebar() {
        let decorations = Decorations {
            radius: 40,
            ..Decorations::default()
        };
        let insets = Insets::of(&decorations, Look::Normal);
        assert_eq!(
            corner_radius(&decorations, Look::Normal, insets, 400),
            26,
            "the bar's height"
        );
        assert_eq!(
            corner_radius(&decorations, Look::Normal, insets, 30),
            15,
            "half the width"
        );
    }

    fn classic_shadow() -> Shadow {
        Shadow {
            size: 14,
            offset: (0, 3),
            ..Shadow::default()
        }
    }

    /// The shadow rings the frame: its pieces cover what is within its
    /// reach of the frame, offset, and never the middle, which is under the
    /// frame wherever it falls.
    #[test]
    fn the_shadow_rings_the_frame_and_covers_none_of_its_middle() {
        let outer = Rect::new(98, 74, 404, 328);
        let shadow = classic_shadow();
        let pieces = shadow_pieces(outer, 5, &shadow);
        assert_eq!(pieces.len(), 8);
        let rects: Vec<Rect> = pieces.iter().map(|piece| piece.at).collect();
        let covered = tiled(&rects).expect("no pixel twice");
        // Fallen 3 down, reaching 14 past it, its middle 5 in from its edge
        // (the corners' rounding, more than its fall), and that middle
        // inside the frame.
        let reached = Rect::new(98 - 14, 74 + 3 - 14, 404 + 28, 328 + 28);
        let middle = Rect::new(98 + 5, 77 + 5, 404 - 10, 328 - 10);
        assert!(
            middle.x >= outer.x
                && middle.y >= outer.y
                && middle.x + middle.w <= outer.x + outer.w
                && middle.y + middle.h <= outer.y + outer.h,
            "under the frame"
        );
        for y in reached.y..reached.y + reached.h {
            for x in reached.x..reached.x + reached.w {
                let in_middle = x >= middle.x
                    && x < middle.x + middle.w
                    && y >= middle.y
                    && y < middle.y + middle.h;
                assert_eq!(covered.contains(&(x, y)), !in_middle, "({x}, {y})");
            }
        }
        assert!(
            pieces
                .iter()
                .filter(|piece| piece.stretch == Stretch::None)
                .all(|piece| piece.at.w == 14 + 5 && piece.at.h == 14 + 5),
            "corners the same whatever the window's size"
        );
        assert!(
            shadow_pieces(outer, 5, &Shadow::default()).is_empty(),
            "no size, no shadow"
        );
    }

    /// Dark under what casts it, half at its edge, fading to nothing at its
    /// reach; round at the top corners, where the frame is, and the same
    /// all along an edge.
    #[test]
    fn the_shadow_fades_to_nothing_at_its_reach() {
        let outer = Rect::new(0, 0, 200, 100);
        let shadow = classic_shadow();
        let at = |x: f64, y: f64| shadow_at(x, y, outer, 6, &shadow);
        assert!((at(100.0, 50.0) - 1.0).abs() < 1e-9, "under the window");
        assert!(
            (at(100.0, 103.0) - 0.5).abs() < 1e-9,
            "half at its foot, fallen 3 below, as a blurred box's edge is"
        );
        assert!(at(100.0, 100.0) > 0.5, "darker under the window's own foot");
        assert!(at(100.0, 103.0 + 14.0).abs() < 1e-9, "nothing at its reach");
        assert!(at(100.0, 110.0) > at(100.0, 114.0), "fading as it goes");
        assert!(
            (at(30.0, 110.0) - at(170.0, 110.0)).abs() < 1e-9,
            "the same all along"
        );
        assert!(
            at(-7.0, 50.0) > 0.0 && at(-7.0, 50.0) < 1.0,
            "beside it too"
        );
        assert!(
            at(-3.0, 3.0 - 3.0) < at(-3.0, 50.0),
            "rounder, and so lighter, at a top corner than down a side"
        );
    }

    /// The frame's facts are its strips alone: nothing about a shadow
    /// reaches them, so it covers nothing for an agent.
    #[test]
    fn frame_facts_ignore_the_shadow() {
        let client = Rect::new(100, 100, 400, 300);
        let flat = Decorations::default();
        let shadowed = Decorations {
            shadow: classic_shadow(),
            ..Decorations::default()
        };
        let insets = |decorations: &Decorations| Insets::of(decorations, Look::Normal);
        assert_eq!(insets(&flat), insets(&shadowed));
        assert_eq!(
            frame_rects(client, insets(&shadowed)),
            frame_rects(client, insets(&flat))
        );
    }

    #[test]
    fn a_maximized_titlebar_has_no_edges() {
        let decorations = Decorations::default();
        let maximized = Insets::of(&decorations, Look::Maximized);
        let client = Rect::new(0, 24, 1920, 1056);
        let at = |x: f64, y: f64| part_at((x, y), client, maximized, &decorations, false, 1, true);
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

    #[test]
    fn an_outline_rings_what_it_goes_round_and_covers_none_of_it() {
        let around = Rect::new(10, 20, 100, 50);
        let strips = outline_rects(around, 3);
        assert_eq!(strips.len(), 4);
        assert_eq!(
            strips[0],
            Rect::new(7, 17, 106, 3),
            "the top, over both corners"
        );
        let area: i32 = strips.iter().map(|strip| strip.w * strip.h).sum();
        assert_eq!(area, 106 * 56 - 100 * 50, "every pixel of the ring, once");
        for strip in &strips {
            assert!(!crate::intersects(*strip, around), "{strip:?}");
        }
        assert!(outline_rects(around, 0).is_empty(), "0 is none");
    }

    #[test]
    fn a_marked_frame_wears_the_agents_colours_with_ink_that_reads() {
        let decorations = Decorations {
            agent: AgentLook {
                unfocused: Colour::rgb(0xee, 0xee, 0xee),
                ..AgentLook::default()
            },
            ..Decorations::default()
        };
        let agent = decorations.agent;
        assert_eq!(
            frame_colours(&decorations, true, true),
            (agent.focused, Colour::rgb(0xff, 0xff, 0xff))
        );
        assert_eq!(
            frame_colours(&decorations, false, true),
            (agent.unfocused, Colour::rgb(0, 0, 0)),
            "a light bar is written on in black"
        );
        assert_eq!(
            frame_colours(&decorations, true, false),
            (decorations.focused, decorations.focused_ink)
        );
        assert_eq!(
            frame_colours(&decorations, false, false),
            (decorations.unfocused, decorations.unfocused_ink)
        );
    }
}
