//! A panel: a bar in the theme's colours (dark, in the default one) along
//! the edge of a monitor, with a rule where it meets the desktop: along its
//! length, and across each end that stops short of the monitor's side. The
//! start button is the icon `[shell.start-menu]` names or, with none or one
//! that cannot be found, Perspicax's mark, in its own inks whatever the
//! theme; on the accent's shade while its menu is open. Each task is a face a little lighter than the
//! bar, holding its application's icon and, where there is room, its
//! window's title; a face too narrow for both holds its icon alone, in its
//! middle. An application with no icon, in a theme with no generic one
//! either, is drawn as a window. The window with the keyboard is in the
//! accent's shade, with a line of the accent along the screen's edge, and a
//! minimized one has no face and its title and drawn window faint.
//! The pager is a grid of small screens, each with its workspace's name,
//! the one showing lit. Each of the tray's icons sits in the middle of its
//! slot, drawn from the icon its program names if it is found, otherwise
//! from the picture its program sent nearest the size. The clock is the
//! time, in the panel's ink. Every pixel of the bar is opaque, and the rest
//! of its strip clear.

use perspicax_config::{Edge, Item};
use tiny_skia::{FillRule, LineCap, LineJoin, PathBuilder, PixmapMut, Stroke, Transform};

use perspicax_config::{Palette, Role};

use super::{
    colour, fill,
    icons::{self, Images},
    rounded, scaled, solid,
    text::Text,
};
use crate::layout::{
    Measure, Rect,
    panel::{CLOCK_PAD, Cell, Placed, TASK_LEAST, Task},
};

/// The panel's colours, from the theme, as fills take them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Colours {
    pub(crate) bar: [u8; 4],
    pub(crate) ink: [u8; 4],
    /// The rule along the edge it shares with the desktop, and around each
    /// of the pager's cells.
    pub(crate) rule: [u8; 4],
    /// Behind the start button while the start menu is open, the task of
    /// the window with the keyboard, and the workspace showing.
    pub(crate) open: [u8; 4],
    /// The accent, along the task in use and around the workspace showing.
    pub(crate) highlight: [u8; 4],
    /// A task's face, and a workspace not showing.
    pub(crate) face: [u8; 4],
    /// A minimized window's title.
    pub(crate) faint: [u8; 4],
}

impl Colours {
    pub(crate) fn of(palette: &Palette) -> Self {
        Self {
            bar: colour(palette, Role::Panel),
            ink: colour(palette, Role::PanelInk),
            rule: colour(palette, Role::PanelRule),
            open: colour(palette, Role::PanelOpen),
            highlight: colour(palette, Role::Accent),
            face: colour(palette, Role::PanelFace),
            faint: colour(palette, Role::PanelFaint),
        }
    }
}

/// Perspicax's mark, as its small master draws it, in the master's own
/// units: `assets/brand/perspicax-mark-small.svg` in the ennius repository,
/// the brand's file for anything under 48 pixels, the start button by name.
/// Its viewBox, where it starts and how wide it is; the run, one line round
/// three sides, stopping short of the top right corner by the same 76 both
/// ways, so that the opening is a line of sight to the middle; how heavy the
/// run is; and the pane it sees, where it starts, how wide it is and how
/// round its corners are, as round as the run's joins. Change these only as
/// the master changes.
const VIEW: (f32, f32) = (73.0, 366.0);
const RUN: [(f32, f32); 5] = [
    (332.0, 104.0),
    (104.0, 104.0),
    (104.0, 408.0),
    (408.0, 408.0),
    (408.0, 180.0),
];
const WEIGHT: f32 = 46.0;
const PANE: (f32, f32, f32) = (171.0, 170.0, 23.0);
/// The mark's inks, the brand's terracotta for the run and sky for the pane:
/// never the theme's, since the brand keeps them on dark grounds and light.
const TERRACOTTA: [u8; 4] = [0xc0, 0x6a, 0x3c, 0xff];
const SKY: [u8; 4] = [0x7b, 0xae, 0xcd, 0xff];

/// A task's face is this far inside its place, above and below, and this
/// far either side, which leaves a gap between one task and the next.
const MARGIN: (i32, i32) = (1, 3);
/// The line along the edge of the active task.
const LINE: i32 = 2;
/// A task's icon, square, and the room around it and the title.
const ICON: i32 = 22;
const INSET: i32 = 6;
// A task as narrow as its bar lets it be before growing, which is as wide
// as one showing its icon alone, has room for its icon and none for its
// title: so its icon sits in its middle.
const _: () = assert!(TASK_LEAST - 2 * MARGIN.0 == ICON + 2 * INSET);
/// The icon a window is drawn with when its application has none, and a
/// status icon whose program gives none that can be drawn.
const GENERIC: &str = "application-x-executable";
/// The least room around a task's or a status icon.
const ICON_MARGIN: i32 = 2;

/// What a panel shows.
pub(crate) struct Shown<'a> {
    /// The panel's size, in its own logical pixels: its whole strip, the
    /// bar `placed` holds within it.
    pub(crate) size: (i32, i32),
    pub(crate) edge: Edge,
    pub(crate) placed: &'a Placed,
    pub(crate) time: &'a str,
    /// The layout in use's label, if there is a choice of layouts.
    pub(crate) layout: Option<&'a str>,
    /// The start menu is open from this panel's button.
    pub(crate) open: bool,
    /// The start button's icon, as the icon theme finds it, drawn in place
    /// of Perspicax's mark if it is found.
    pub(crate) start: Option<&'a str>,
    /// The status icons, by key, to draw those `placed` holds.
    #[cfg(feature = "tray")]
    pub(crate) tray: &'a [(u64, crate::model::tray::Item)],
}

/// Draw `shown` on `canvas`, a surface's pixels at `scale` times its size.
pub(crate) fn paint(
    shown: &Shown<'_>,
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    images: &mut Images,
    palette: &Palette,
) {
    let colours = Colours::of(palette);
    let px = |rect: Rect| scaled(rect, scale);
    let width = shown.size.0;
    let bar = shown.placed.bar;
    canvas.fill(tiny_skia::Color::TRANSPARENT);
    fill(canvas, px(bar), colours.bar);
    let along = match shown.edge {
        Edge::Bottom => Rect::new(bar.x, bar.y, bar.w, 1),
        Edge::Top => Rect::new(bar.x, bar.bottom() - 1, bar.w, 1),
    };
    let ends = [
        (bar.x > 0).then_some(Rect::new(bar.x, bar.y, 1, bar.h)),
        (bar.right() < width).then_some(Rect::new(bar.right() - 1, bar.y, 1, bar.h)),
    ];
    for &(item, place) in &shown.placed.items {
        match item {
            Item::Start => {
                if shown.open {
                    fill(canvas, px(place), colours.open);
                }
                let side = fitted(place.h);
                let at = px(middle(place, side));
                match shown
                    .start
                    .and_then(|icon| images.get(icon, side as u32, scale))
                {
                    Some(image) => icons::draw(canvas, image, at),
                    None => mark(canvas, at),
                }
            }
            // The layout's label is set as the clock's time is.
            Item::Clock | Item::Layout => {
                let words = match item {
                    Item::Clock => shown.time,
                    _ => shown.layout.unwrap_or_default(),
                };
                let room = Rect::new(
                    place.x + CLOCK_PAD,
                    place.y,
                    place.w - 2 * CLOCK_PAD,
                    place.h,
                );
                let size = text.size() * scale as f32;
                text.write(canvas, words, px(room), size, colours.ink);
            }
            Item::Taskbar | Item::Pager | Item::Tray => {}
        }
    }
    let mut pen = Pen {
        canvas,
        scale,
        text,
        colours,
    };
    for (task, place) in &shown.placed.tasks {
        pen.task(task, *place, shown.edge, images);
    }
    for (cell, place) in &shown.placed.cells {
        pen.cell(cell, *place);
    }
    #[cfg(feature = "tray")]
    for (icon, place) in &shown.placed.tray {
        if let Some((_, item)) = shown.tray.iter().find(|(key, _)| *key == icon.key) {
            pen.status(item, *place, images);
        }
    }
    for rule in ends.into_iter().flatten().chain([along]) {
        fill(pen.canvas, px(rule), colours.rule);
    }
}

/// What tasks and cells are drawn on, and written with.
struct Pen<'c, 'p, 't> {
    canvas: &'c mut PixmapMut<'p>,
    scale: u32,
    text: &'t mut Text,
    colours: Colours,
}

impl Pen<'_, '_, '_> {
    fn px(&self, rect: Rect) -> Rect {
        scaled(rect, self.scale)
    }

    /// A task, at `place`, on a panel along `edge`: its icon, then its
    /// window's title in what room is left, or in a face too narrow for
    /// both, its icon alone in the middle.
    fn task(&mut self, task: &Task, place: Rect, edge: Edge, images: &mut Images) {
        let (side, end) = MARGIN;
        let face = Rect::new(
            place.x + side,
            place.y + end,
            place.w - 2 * side,
            place.h - 2 * end,
        );
        if face.w <= 0 || face.h <= 0 {
            return;
        }
        let colours = self.colours;
        if task.active {
            fill(self.canvas, self.px(face), colours.open);
            let line = match edge {
                Edge::Bottom => Rect::new(face.x, face.bottom() - LINE, face.w, LINE),
                Edge::Top => Rect::new(face.x, face.y, face.w, LINE),
            };
            fill(self.canvas, self.px(line), colours.highlight);
        } else if !task.minimized {
            fill(self.canvas, self.px(face), colours.face);
        }
        let ink = if task.minimized {
            colours.faint
        } else {
            colours.ink
        };
        // Found at the one size the panel's height asks, however wide the
        // task, so that each icon is read once.
        let icon = fitted(place.h);
        if face.w < ICON + 2 * INSET {
            let shown = icon.min(face.w - 2 * ICON_MARGIN);
            if shown > 0 {
                let at = Rect::new(
                    face.x + (face.w - shown) / 2,
                    face.y + (face.h - shown) / 2,
                    shown,
                    shown,
                );
                self.icon(task, at, icon, ink, images);
            }
            return;
        }
        let slot = face.x + INSET;
        let at = Rect::new(
            slot + (ICON - icon) / 2,
            face.y + (face.h - icon) / 2,
            icon,
            icon,
        );
        self.icon(task, at, icon, ink, images);
        let left = slot + ICON + INSET;
        let words = Rect::new(left, face.y, face.right() - INSET - left, face.h);
        if words.w > 0 {
            let size = self.text.size() * self.scale as f32;
            self.text
                .write(self.canvas, &task.title, self.px(words), size, ink);
        }
    }

    /// `task`'s icon at `at`, found `size` square: its application's, or the
    /// generic one, or with neither to draw, a window in `ink`.
    fn icon(&mut self, task: &Task, at: Rect, size: i32, ink: [u8; 4], images: &mut Images) {
        let (size, at) = (size as u32, self.px(at));
        let name = task
            .icon
            .as_deref()
            .filter(|name| images.get(name, size, self.scale).is_some())
            .unwrap_or(GENERIC);
        match images.get(name, size, self.scale) {
            Some(image) => icons::draw(self.canvas, image, at),
            None => window(self.canvas, at, self.scale as i32, ink),
        }
    }

    /// A status icon, in the middle of its slot at `place`.
    #[cfg(feature = "tray")]
    fn status(&mut self, item: &crate::model::tray::Item, place: Rect, images: &mut Images) {
        let side = fitted(place.h);
        let at = self.px(middle(place, side));
        let (size, scale) = (side as u32, self.scale);
        let icon = item.drawn();
        if let Some(image) = icon
            .name
            .as_deref()
            .and_then(|name| images.get_from(icon.folder.as_deref(), name, size, scale))
        {
            icons::draw(self.canvas, image, at);
        } else if let Some(picture) = crate::model::tray::chosen(&icon.pixmaps, size * scale) {
            icons::draw(self.canvas, picture, at);
        } else if let Some(image) = images.get(GENERIC, size, scale) {
            icons::draw(self.canvas, image, at);
        }
    }

    /// A workspace's cell, at `place`: a small screen with its name in the
    /// middle, where there is room for it.
    fn cell(&mut self, cell: &Cell, place: Rect) {
        let colours = self.colours;
        let (edge, face) = if cell.active {
            (colours.highlight, colours.open)
        } else {
            (colours.rule, colours.face)
        };
        fill(self.canvas, self.px(place), edge);
        let inside = Rect::new(place.x + 1, place.y + 1, place.w - 2, place.h - 2);
        fill(self.canvas, self.px(inside), face);
        if place.h < self.text.size() as i32 + 4 {
            return;
        }
        let wide = (self.text.width(&cell.name).ceil() as i32 + 2).min(inside.w);
        let words = Rect::new(inside.x + (inside.w - wide) / 2, inside.y, wide, inside.h);
        let size = self.text.size() * self.scale as f32;
        self.text
            .write(self.canvas, &cell.name, self.px(words), size, colours.ink);
    }
}

/// How big an icon is on a panel `tall` high: [`ICON`], or less on a panel
/// too short for it.
pub(crate) fn fitted(tall: i32) -> i32 {
    ICON.min(tall - 2 * ICON_MARGIN).max(1)
}

/// A window, centred in `place`, `s` pixels to a logical one: an outline
/// with its title bar across the top, as wide as most of `place`. Nothing,
/// where it would be too small to make out.
fn window(canvas: &mut PixmapMut<'_>, place: Rect, s: i32, ink: [u8; 4]) {
    let (wide, line) = (place.w * 9 / 11, s);
    let tall = wide * 7 / 9;
    if wide < 4 * line || tall < 4 * line {
        return;
    }
    let (left, top) = (
        place.x + (place.w - wide) / 2,
        place.y + (place.h - tall) / 2,
    );
    let title = (tall / 4).max(2 * line);
    for edge in [
        Rect::new(left, top, wide, title),
        Rect::new(left, top, line, tall),
        Rect::new(left + wide - line, top, line, tall),
        Rect::new(left, top + tall - line, wide, line),
    ] {
        fill(canvas, edge, ink);
    }
}

/// A square `side` on a side in the middle of `place`.
fn middle(place: Rect, side: i32) -> Rect {
    Rect::new(
        place.x + (place.w - side) / 2,
        place.y + (place.h - side) / 2,
        side,
        side,
    )
}

/// Perspicax's mark filling `place`, a square in pixels: the master's own
/// numbers, drawn through its viewBox, so that the run's weight and the
/// pane's corners scale with it.
fn mark(canvas: &mut PixmapMut<'_>, place: Rect) {
    let (from, side) = VIEW;
    let k = place.w as f32 / side;
    let through = Transform::from_row(
        k,
        0.0,
        0.0,
        k,
        place.x as f32 - from * k,
        place.y as f32 - from * k,
    );
    let [(x, y), rest @ ..] = RUN;
    let mut run = PathBuilder::new();
    run.move_to(x, y);
    for (x, y) in rest {
        run.line_to(x, y);
    }
    if let Some(run) = run.finish() {
        let stroke = Stroke {
            width: WEIGHT,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        canvas.stroke_path(&run, &solid(TERRACOTTA), &stroke, through, None);
    }
    let (at, wide, radius) = PANE;
    if let Some(pane) = rounded(at, at, wide, wide, radius) {
        canvas.fill_path(&pane, &solid(SKY), FillRule::Winding, through, None);
    }
}

#[cfg(test)]
mod tests {
    use perspicax_config::{Align, PanelWidth};
    use tiny_skia::Pixmap;

    use super::*;
    use crate::layout::{
        Monospace,
        panel::{Holding, lay_out},
    };

    /// The default theme's panel colours, which these pictures are drawn in:
    /// what the panel was before there were themes.
    const BAR: [u8; 4] = [0x23, 0x26, 0x29, 0xff];
    const INK: [u8; 4] = [0xfc, 0xfc, 0xfc, 0xff];
    const RULE: [u8; 4] = [0x3b, 0x40, 0x45, 0xff];
    const OPEN: [u8; 4] = [0x2b, 0x4f, 0x63, 0xff];
    const HIGHLIGHT: [u8; 4] = [0x3d, 0xae, 0xe9, 0xff];
    const FACE: [u8; 4] = [0x31, 0x36, 0x3b, 0xff];
    const FAINT: [u8; 4] = [0x9a, 0xa0, 0xa6, 0xff];

    #[test]
    fn the_default_theme_paints_the_panel_as_it_was() {
        let colours = Colours::of(&Palette::default());
        assert_eq!(
            [
                colours.bar,
                colours.ink,
                colours.rule,
                colours.open,
                colours.highlight,
                colours.face,
                colours.faint,
            ],
            [BAR, INK, RULE, OPEN, HIGHLIGHT, FACE, FAINT]
        );
    }

    /// Another theme's panel is drawn in its colours.
    #[test]
    fn a_theme_colours_the_bar() {
        let palette = perspicax_config::Builtin::BreezeLight.palette();
        let mut picture = Pixmap::new(600, 40).expect("a picture");
        paint(
            &Shown {
                size: (600, 40),
                edge: Edge::Bottom,
                placed: &laid(false),
                time: "14:05",
                layout: None,
                open: false,
                start: None,
                #[cfg(feature = "tray")]
                tray: &[],
            },
            &mut picture.as_mut(),
            1,
            &mut Text::without_fonts(),
            &mut Images::default(),
            &palette,
        );
        assert_eq!(pixel(&picture, 430, 20), colour(&palette, Role::Panel));
    }

    /// Two windows, the second with the keyboard unless it is `minimized`,
    /// and two workspaces, the first showing, on a bar the panel's width.
    fn laid(minimized: bool) -> Placed {
        laid_in(minimized, (PanelWidth::FULL, Align::Center))
    }

    /// As [`laid`], on a bar shaped as `shape`.
    fn laid_in(minimized: bool, shape: (PanelWidth, Align)) -> Placed {
        laid_as(minimized, shape, true)
    }

    /// As [`laid_in`], its tasks showing their titles or not.
    fn laid_as(minimized: bool, shape: (PanelWidth, Align), task_titles: bool) -> Placed {
        let task = |serial: u64, title: &str, second: bool| Task {
            serial,
            title: title.to_owned(),
            icon: None,
            active: second && !minimized,
            minimized: second && minimized,
        };
        let cell = |serial: u64, column: u32| Cell {
            serial,
            name: (column + 1).to_string(),
            column,
            row: 0,
            active: column == 0,
        };
        lay_out(
            &[Item::Start, Item::Taskbar, Item::Pager, Item::Clock],
            Holding {
                time: "14:05",
                layout: None,
                tasks: vec![task(0, "Editor", false), task(1, "Mail", true)],
                task_titles,
                cells: vec![cell(10, 0), cell(11, 1)],
                #[cfg(feature = "tray")]
                tray: Vec::new(),
            },
            (600, 40),
            shape,
            &mut Monospace(8.0),
        )
    }

    fn painted(placed: &Placed, open: bool, scale: u32) -> Pixmap {
        painted_with(placed, open, scale, None, &mut Images::default())
    }

    /// As [`painted`], the start button's icon `start` looked for in
    /// `images`.
    fn painted_with(
        placed: &Placed,
        open: bool,
        scale: u32,
        start: Option<&str>,
        images: &mut Images,
    ) -> Pixmap {
        let mut picture = Pixmap::new(600 * scale, 40 * scale).expect("a picture");
        paint(
            &Shown {
                size: (600, 40),
                edge: Edge::Bottom,
                placed,
                time: "14:05",
                layout: None,
                open,
                start,
                #[cfg(feature = "tray")]
                tray: &[],
            },
            &mut picture.as_mut(),
            scale,
            &mut Text::without_fonts(),
            images,
            &Palette::default(),
        );
        picture
    }

    fn pixel(picture: &Pixmap, x: i32, y: i32) -> [u8; 4] {
        let at = ((y * picture.width() as i32 + x) * 4) as usize;
        picture.data()[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn a_panel_is_opaque_with_a_rule_along_the_desktop() {
        for scale in [1, 2] {
            let picture = painted(&laid(false), false, scale);
            assert!(
                picture.data().chunks_exact(4).all(|pixel| pixel[3] == 0xff),
                "every pixel opaque, at {scale}x"
            );
            let s = scale as i32;
            assert_eq!(
                pixel(&picture, 500 * s, 0),
                RULE,
                "the top row, at {scale}x"
            );
            assert_eq!(pixel(&picture, 430 * s, 20 * s), BAR, "the pager's margin");
        }
    }

    #[test]
    fn a_narrow_bar_is_opaque_and_the_rest_of_its_strip_clear() {
        const CLEAR: [u8; 4] = [0, 0, 0, 0];
        for scale in [1, 2] {
            let s = scale as i32;
            let middle = laid_in(false, (PanelWidth::Pixels(400), Align::Center));
            assert_eq!(middle.bar, Rect::new(100, 0, 400, 40));
            let picture = painted(&middle, false, scale);
            assert_eq!(
                pixel(&picture, 50 * s, 20 * s),
                CLEAR,
                "beside it, at {scale}x"
            );
            assert_eq!(pixel(&picture, 550 * s, 20 * s), CLEAR);
            assert_eq!(
                pixel(&picture, 100 * s, 20 * s),
                RULE,
                "across its left end"
            );
            assert_eq!(pixel(&picture, 500 * s - 1, 20 * s), RULE, "and its right");
            assert_eq!(pixel(&picture, 300 * s, 0), RULE, "along it");
            assert_eq!(pixel(&picture, 330 * s, 20 * s), BAR, "the pager's margin");

            let left = laid_in(false, (PanelWidth::Pixels(400), Align::Left));
            let picture = painted(&left, false, scale);
            assert_eq!(
                pixel(&picture, 0, 20 * s),
                BAR,
                "no rule against the monitor's side, at {scale}x"
            );
            assert_eq!(pixel(&picture, 400 * s - 1, 20 * s), RULE);
            assert_eq!(pixel(&picture, 450 * s, 20 * s), CLEAR);
        }
    }

    /// Points of the mark, in its master's units: on the run, down its left
    /// side; in the pane; the top right corner, where the opening leaves
    /// nothing; and between the run and the pane.
    const ON_RUN: (f32, f32) = (104.0, 256.0);
    const IN_PANE: (f32, f32) = (256.0, 256.0);
    const SIGHTLINE: (f32, f32) = (408.0, 104.0);
    const GAP: (f32, f32) = (149.0, 256.0);

    /// The pixel at `point` of the mark, as it is drawn in `button` at
    /// `scale`.
    fn on_mark(button: Rect, scale: u32, point: (f32, f32)) -> (i32, i32) {
        let s = scale as i32;
        let at = scaled(middle(button, fitted(button.h)), scale);
        let (from, side) = VIEW;
        let k = at.w as f32 / side;
        let x = at.x as f32 + (point.0 - from) * k;
        let y = at.y as f32 + (point.1 - from) * k;
        assert!(at.w == fitted(button.h) * s, "drawn the icons' size");
        (x as i32, y as i32)
    }

    #[test]
    fn the_start_button_is_the_mark_lit_while_its_menu_is_open() {
        let placed = laid(false);
        let button = placed.item(Item::Start).unwrap();
        for scale in [1, 2] {
            let s = scale as i32;
            let at = |picture: &Pixmap, point| {
                let (x, y) = on_mark(button, scale, point);
                pixel(picture, x, y)
            };
            let closed = painted(&placed, false, scale);
            assert_eq!(at(&closed, ON_RUN), TERRACOTTA, "the run, at {scale}x");
            assert_eq!(at(&closed, IN_PANE), SKY, "the pane, at {scale}x");
            assert_eq!(at(&closed, SIGHTLINE), BAR, "the opening, at {scale}x");
            assert_eq!(at(&closed, GAP), BAR, "between them, at {scale}x");
            assert_eq!(pixel(&closed, (button.x + 2) * s, (button.y + 2) * s), BAR);

            let open = painted(&placed, true, scale);
            assert_eq!(
                pixel(&open, (button.x + 2) * s, (button.y + 2) * s),
                OPEN,
                "lit, at {scale}x"
            );
            assert_eq!(at(&open, SIGHTLINE), OPEN, "through the opening too");
            assert_eq!(at(&open, ON_RUN), TERRACOTTA, "the mark over it");
            assert_eq!(at(&open, IN_PANE), SKY);
        }
    }

    /// The mark is the brand's, so another theme draws it the same, on that
    /// theme's bar.
    #[test]
    fn the_mark_keeps_its_inks_whatever_the_theme() {
        let palette = perspicax_config::Builtin::BreezeLight.palette();
        let placed = laid(false);
        let button = placed.item(Item::Start).unwrap();
        let mut picture = Pixmap::new(600, 40).expect("a picture");
        paint(
            &Shown {
                size: (600, 40),
                edge: Edge::Bottom,
                placed: &placed,
                time: "14:05",
                layout: None,
                open: false,
                start: None,
                #[cfg(feature = "tray")]
                tray: &[],
            },
            &mut picture.as_mut(),
            1,
            &mut Text::without_fonts(),
            &mut Images::default(),
            &palette,
        );
        let at = |point| {
            let (x, y) = on_mark(button, 1, point);
            pixel(&picture, x, y)
        };
        assert_ne!(colour(&palette, Role::Panel), BAR, "a theme of its own");
        assert_eq!(at(ON_RUN), TERRACOTTA);
        assert_eq!(at(IN_PANE), SKY);
        assert_eq!(at(SIGHTLINE), colour(&palette, Role::Panel));
    }

    /// A PNG of one colour, `side` pixels square, written for `test`; its
    /// whole path.
    fn png(test: &str, side: u32, colour: [u8; 4]) -> String {
        let path = std::env::temp_dir().join(format!(
            "perspicax-shell-panel-{test}-{}.png",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).expect("a file");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), side, side);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header");
        writer
            .write_image_data(&colour.repeat((side * side) as usize))
            .expect("written");
        path.display().to_string()
    }

    #[test]
    fn a_start_icon_that_is_found_is_drawn_in_place_of_the_mark() {
        const BLUE: [u8; 4] = [0x33, 0x66, 0x99, 0xff];
        let icon = png("found", 44, BLUE);
        let placed = laid(false);
        let button = placed.item(Item::Start).unwrap();
        let mut images = Images::default();
        for scale in [1, 2] {
            let s = scale as i32;
            for open in [false, true] {
                let picture = painted_with(&placed, open, scale, Some(&icon), &mut images);
                for point in [ON_RUN, IN_PANE, GAP] {
                    let (x, y) = on_mark(button, scale, point);
                    assert_eq!(
                        pixel(&picture, x, y),
                        BLUE,
                        "the icon over all of it, at {scale}x, open {open}"
                    );
                }
                let face = if open { OPEN } else { BAR };
                assert_eq!(
                    pixel(&picture, (button.x + 2) * s, (button.y + 2) * s),
                    face,
                    "the button around it, at {scale}x"
                );
            }
        }
        std::fs::remove_file(&icon).ok();
    }

    #[test]
    fn a_start_icon_that_cannot_be_found_is_the_mark() {
        let placed = laid(false);
        let button = placed.item(Item::Start).unwrap();
        for start in ["no-such-icon", "/no/such/icon.png"] {
            let picture = painted_with(&placed, false, 1, Some(start), &mut Images::default());
            let at = |point| {
                let (x, y) = on_mark(button, 1, point);
                pixel(&picture, x, y)
            };
            assert_eq!(at(ON_RUN), TERRACOTTA, "{start}");
            assert_eq!(at(IN_PANE), SKY, "{start}");
        }
    }

    /// An icon file that could not be read when the panel was drawn is read
    /// once it is mended and the config saved, which forgets the failure.
    #[test]
    fn a_start_icon_that_failed_is_looked_for_again_once_forgotten() {
        const GREEN: [u8; 4] = [0x22, 0x88, 0x44, 0xff];
        let placed = laid(false);
        let button = placed.item(Item::Start).unwrap();
        let (x, y) = on_mark(button, 1, IN_PANE);
        let icon = std::env::temp_dir()
            .join(format!(
                "perspicax-shell-panel-mended-{}.png",
                std::process::id()
            ))
            .display()
            .to_string();
        let mut images = Images::default();
        let before = painted_with(&placed, false, 1, Some(&icon), &mut images);
        assert_eq!(pixel(&before, x, y), SKY, "not there yet: the mark");
        assert_eq!(png("mended", 22, GREEN), icon);
        let cached = painted_with(&placed, false, 1, Some(&icon), &mut images);
        assert_eq!(pixel(&cached, x, y), SKY, "the failure is remembered");
        assert!(images.forget_failed(&icon));
        assert!(!images.forget_failed(&icon), "once");
        let after = painted_with(&placed, false, 1, Some(&icon), &mut images);
        assert_eq!(pixel(&after, x, y), GREEN, "and looked for again");
        assert!(!images.forget_failed(&icon), "a found icon is kept");
        std::fs::remove_file(&icon).ok();
    }

    #[test]
    fn the_task_with_the_keyboard_is_lit_and_a_minimized_one_has_no_face() {
        let placed = laid(false);
        let picture = painted(&placed, false, 1);
        let (editor, mail) = (placed.tasks[0].1, placed.tasks[1].1);
        // Right of the icon's room, where no title is written with no fonts.
        let inside = |rect: Rect| (rect.right() - 4, rect.y + 10);
        let at = |rect: Rect| {
            let (x, y) = inside(rect);
            pixel(&picture, x, y)
        };
        assert_eq!(at(editor), FACE);
        assert_eq!(at(mail), OPEN);
        assert_eq!(
            pixel(&picture, mail.x + 10, mail.bottom() - MARGIN.1 - 1),
            HIGHLIGHT,
            "a line along the screen's edge"
        );
        assert_eq!(
            pixel(&picture, editor.right() - 1, 20),
            BAR,
            "a gap between tasks"
        );

        let placed = laid(true);
        let picture = painted(&placed, false, 1);
        let (x, y) = inside(placed.tasks[1].1);
        assert_eq!(pixel(&picture, x, y), BAR, "minimized: no face");
    }

    /// Where a task at `place`, on a panel 40 high, has the window drawn in
    /// place of an icon it has none of: across its title bar, inside its
    /// outline, and down its left side.
    fn drawn_window(place: Rect) -> [(i32, i32); 3] {
        // The icon's 22 pixels from 6 inside the face; the window 18 of
        // them wide and 14 high in their middle, its title bar 3.
        let (left, top) = (place.x + 1 + 6 + 2, 3 + 6 + 4);
        [(left + 9, top + 1), (left + 9, top + 9), (left, top + 7)]
    }

    #[test]
    fn a_window_with_no_icon_to_draw_is_drawn_as_a_window() {
        let placed = laid(false);
        let picture = painted(&placed, false, 1);
        let [title, inside, side] = drawn_window(placed.tasks[0].1);
        assert_eq!(pixel(&picture, title.0, title.1), INK, "its title bar");
        assert_eq!(pixel(&picture, inside.0, inside.1), FACE, "hollow");
        assert_eq!(pixel(&picture, side.0, side.1), INK, "its outline");

        let placed = laid(true);
        let picture = painted(&placed, false, 1);
        let [title, inside, _] = drawn_window(placed.tasks[1].1);
        assert_eq!(pixel(&picture, title.0, title.1), FAINT, "minimized: faint");
        assert_eq!(pixel(&picture, inside.0, inside.1), BAR, "and no face");
    }

    #[test]
    fn a_task_as_narrow_as_its_icon_shows_it_alone_in_the_middle() {
        let placed = laid_as(false, (PanelWidth::FULL, Align::Center), false);
        let (editor, mail) = (placed.tasks[0].1, placed.tasks[1].1);
        assert_eq!((editor.w, mail.x), (TASK_LEAST, editor.right()));
        for scale in [1, 2] {
            let s = scale as i32;
            let picture = painted(&placed, false, scale);
            let at = |(x, y): (i32, i32)| pixel(&picture, x * s, y * s);
            let [title, inside, side] = drawn_window(editor);
            assert_eq!(at(title), INK, "at {scale}x");
            assert_eq!(at(inside), FACE);
            assert_eq!(at(side), INK);
            assert_eq!(
                title.0,
                editor.x + editor.w / 2,
                "the window in the task's middle"
            );
            assert_eq!(at((editor.x + 3, 20)), FACE, "the face either side");
            assert_eq!(at((editor.right() - 3, 20)), FACE);
            let [title, ..] = drawn_window(mail);
            assert_eq!(at(title), INK, "the next straight after");
            assert_eq!(at((mail.right() + 10, 20)), BAR, "and the rest empty");
        }
    }

    /// `place`'s task, holding no icon, painted alone on a panel 600 by
    /// `tall`.
    fn one_task(place: Rect, tall: i32) -> Pixmap {
        let placed = Placed {
            bar: Rect::new(0, 0, 600, tall),
            tasks: vec![(
                Task {
                    serial: 0,
                    title: "Editor".to_owned(),
                    icon: None,
                    active: false,
                    minimized: false,
                },
                place,
            )],
            ..Placed::default()
        };
        let mut picture = Pixmap::new(600, tall as u32).expect("a picture");
        paint(
            &Shown {
                size: (600, tall),
                edge: Edge::Bottom,
                placed: &placed,
                time: "14:05",
                layout: None,
                open: false,
                start: None,
                #[cfg(feature = "tray")]
                tray: &[],
            },
            &mut picture.as_mut(),
            1,
            &mut Text::without_fonts(),
            &mut Images::default(),
            &Palette::default(),
        );
        picture
    }

    /// The least rect holding every pixel of ink within `place`, if any.
    fn inked(picture: &Pixmap, place: Rect) -> Option<Rect> {
        let mut found: Vec<(i32, i32)> = Vec::new();
        for y in place.y..place.bottom() {
            for x in place.x..place.right() {
                if pixel(picture, x, y) == INK {
                    found.push((x, y));
                }
            }
        }
        let (xs, ys) = (found.iter().map(|&(x, _)| x), found.iter().map(|&(_, y)| y));
        let (left, right) = (xs.clone().min()?, xs.max()?);
        let (top, bottom) = (ys.clone().min()?, ys.max()?);
        Some(Rect::new(left, top, right - left + 1, bottom - top + 1))
    }

    #[test]
    fn a_crowded_task_shows_a_smaller_icon_in_its_middle() {
        let place = Rect::new(40, 0, 20, 40);
        let window = inked(&one_task(place, 40), place).expect("a window drawn");
        assert!(
            window.w < 18,
            "smaller than in a task with room: {window:?}"
        );
        let (before, after) = (window.x - place.x, place.right() - window.right());
        assert!((before - after).abs() <= 1, "in the middle: {window:?}");
    }

    #[test]
    fn a_short_panels_task_icons_fit_it_as_its_tray_icons_do() {
        let place = Rect::new(40, 0, 200, 16);
        let window = inked(&one_task(place, 16), place).expect("a window drawn");
        assert_eq!(fitted(16), 12, "as a status icon is");
        assert!(
            window.y >= ICON_MARGIN && window.bottom() <= 16 - ICON_MARGIN,
            "inside the panel: {window:?}"
        );
    }

    #[test]
    fn a_window_too_small_to_make_out_is_not_drawn() {
        let place = Rect::new(40, 0, 8, 40);
        assert_eq!(inked(&one_task(place, 40), place), None);
    }

    #[test]
    fn the_workspace_showing_is_lit() {
        let placed = laid(false);
        let picture = painted(&placed, false, 1);
        let (first, second) = (placed.cells[0].1, placed.cells[1].1);
        assert_eq!(pixel(&picture, first.x, first.y), HIGHLIGHT, "its edge");
        assert_eq!(pixel(&picture, first.x + 3, first.y + 3), OPEN);
        assert_eq!(pixel(&picture, second.x, second.y), RULE);
        assert_eq!(pixel(&picture, second.x + 3, second.y + 3), FACE);
    }
}
