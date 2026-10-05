//! A panel: a dark bar along the edge of a monitor, with a rule where it
//! meets the desktop. The start button is a grid of four squares, drawn in
//! the highlight's shade while its menu is open. Each task is a face a
//! little lighter than the bar, holding its application's icon and its
//! window's title: the window with the keyboard in the highlight's shade,
//! with a line of the highlight along the screen's edge, and a minimized one
//! with no face and its title faint. The pager is a grid of small screens,
//! each with its workspace's name, the one showing lit. Each of the tray's
//! icons sits in the middle of its slot, drawn from the icon its program
//! names if it is found, otherwise from the picture its program sent
//! nearest the size. The clock is the time, in the panel's ink. Every pixel
//! of it is opaque.

use perspicax_config::{Edge, Item};
use tiny_skia::PixmapMut;

use super::{
    fill,
    icons::{self, Images},
    scaled,
    text::Text,
};
use crate::layout::{
    Measure, Rect, TEXT,
    panel::{CLOCK_PAD, Cell, Placed, Task},
};

/// The panel's colours, as premultiplied RGBA. Dark, after Breeze's.
pub(crate) const BAR: [u8; 4] = [0x23, 0x26, 0x29, 0xff];
pub(crate) const INK: [u8; 4] = [0xfc, 0xfc, 0xfc, 0xff];
/// The rule along the edge it shares with the desktop, and around each of
/// the pager's cells.
pub(crate) const RULE: [u8; 4] = [0x3b, 0x40, 0x45, 0xff];
/// Behind the start button while the start menu is open, the task of the
/// window with the keyboard, and the workspace showing: the highlight,
/// faint over the bar.
pub(crate) const OPEN: [u8; 4] = [0x2b, 0x4f, 0x63, 0xff];
/// The highlight itself, as the menus have it.
pub(crate) const HIGHLIGHT: [u8; 4] = [0x3d, 0xae, 0xe9, 0xff];
/// A task's face, and a workspace not showing.
pub(crate) const FACE: [u8; 4] = [0x31, 0x36, 0x3b, 0xff];
/// A minimized window's title.
pub(crate) const FAINT: [u8; 4] = [0x9a, 0xa0, 0xa6, 0xff];

/// One square of the start button's grid, and the room between them.
const SQUARE: i32 = 7;
const BETWEEN: i32 = 3;

/// A task's face is this far inside its place, above and below, and this
/// far either side, which leaves a gap between one task and the next.
const MARGIN: (i32, i32) = (1, 3);
/// The line along the edge of the active task.
const LINE: i32 = 2;
/// A task's icon, square, and the room around it and the title.
const ICON: i32 = 22;
const INSET: i32 = 6;
/// The icon a window is drawn with when its application has none, and a
/// status icon whose program gives none that can be drawn.
const GENERIC: &str = "application-x-executable";
/// The least room above and below a status icon.
#[cfg(feature = "tray")]
const TRAY_MARGIN: i32 = 2;

/// What a panel shows.
pub(crate) struct Shown<'a> {
    /// The panel's size, in its own logical pixels.
    pub(crate) size: (i32, i32),
    pub(crate) edge: Edge,
    pub(crate) placed: &'a Placed,
    pub(crate) time: &'a str,
    /// The start menu is open from this panel's button.
    pub(crate) open: bool,
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
) {
    let px = |rect: Rect| scaled(rect, scale);
    let (width, height) = shown.size;
    fill(canvas, px(Rect::new(0, 0, width, height)), BAR);
    let rule = match shown.edge {
        Edge::Bottom => Rect::new(0, 0, width, 1),
        Edge::Top => Rect::new(0, height - 1, width, 1),
    };
    for &(item, place) in &shown.placed.items {
        match item {
            Item::Start => {
                if shown.open {
                    fill(canvas, px(place), OPEN);
                }
                grid(canvas, px(place), scale as i32);
            }
            Item::Clock => {
                let words = Rect::new(
                    place.x + CLOCK_PAD,
                    place.y,
                    place.w - 2 * CLOCK_PAD,
                    place.h,
                );
                text.write(canvas, shown.time, px(words), TEXT * scale as f32, INK);
            }
            Item::Taskbar | Item::Pager | Item::Tray => {}
        }
    }
    let mut pen = Pen {
        canvas,
        scale,
        text,
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
    fill(pen.canvas, px(rule), RULE);
}

/// What tasks and cells are drawn on, and written with.
struct Pen<'c, 'p, 't> {
    canvas: &'c mut PixmapMut<'p>,
    scale: u32,
    text: &'t mut Text,
}

impl Pen<'_, '_, '_> {
    fn px(&self, rect: Rect) -> Rect {
        scaled(rect, self.scale)
    }

    /// A task, at `place`, on a panel along `edge`.
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
        if task.active {
            fill(self.canvas, self.px(face), OPEN);
            let line = match edge {
                Edge::Bottom => Rect::new(face.x, face.bottom() - LINE, face.w, LINE),
                Edge::Top => Rect::new(face.x, face.y, face.w, LINE),
            };
            fill(self.canvas, self.px(line), HIGHLIGHT);
        } else if !task.minimized {
            fill(self.canvas, self.px(face), FACE);
        }
        let mut left = face.x + INSET;
        if face.w >= ICON + 2 * INSET {
            let size = ICON as u32;
            let name = task
                .icon
                .as_deref()
                .filter(|name| images.get(name, size, self.scale).is_some())
                .unwrap_or(GENERIC);
            if let Some(image) = images.get(name, size, self.scale) {
                let at = Rect::new(left, face.y + (face.h - ICON) / 2, ICON, ICON);
                icons::draw(self.canvas, image, self.px(at));
            }
            left += ICON + INSET;
        }
        let words = Rect::new(left, face.y, face.right() - INSET - left, face.h);
        let ink = if task.minimized { FAINT } else { INK };
        let size = TEXT * self.scale as f32;
        self.text
            .write(self.canvas, &task.title, self.px(words), size, ink);
    }

    /// A status icon, in the middle of its slot at `place`.
    #[cfg(feature = "tray")]
    fn status(&mut self, item: &crate::model::tray::Item, place: Rect, images: &mut Images) {
        let side = ICON.min(place.h - 2 * TRAY_MARGIN).max(1);
        let at = self.px(Rect::new(
            place.x + (place.w - side) / 2,
            place.y + (place.h - side) / 2,
            side,
            side,
        ));
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
        let (edge, face) = if cell.active {
            (HIGHLIGHT, OPEN)
        } else {
            (RULE, FACE)
        };
        fill(self.canvas, self.px(place), edge);
        let inside = Rect::new(place.x + 1, place.y + 1, place.w - 2, place.h - 2);
        fill(self.canvas, self.px(inside), face);
        if place.h < TEXT as i32 + 4 {
            return;
        }
        let wide = (self.text.width(&cell.name).ceil() as i32 + 2).min(inside.w);
        let words = Rect::new(inside.x + (inside.w - wide) / 2, inside.y, wide, inside.h);
        let size = TEXT * self.scale as f32;
        self.text
            .write(self.canvas, &cell.name, self.px(words), size, INK);
    }
}

/// Four squares, two by two, centred in `place`, `s` pixels to a logical
/// one.
fn grid(canvas: &mut PixmapMut<'_>, place: Rect, s: i32) {
    let (square, between) = (SQUARE * s, BETWEEN * s);
    let side = 2 * square + between;
    let (left, top) = (
        place.x + (place.w - side) / 2,
        place.y + (place.h - side) / 2,
    );
    for (column, row) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let at = Rect::new(
            left + column * (square + between),
            top + row * (square + between),
            square,
            square,
        );
        fill(canvas, at, INK);
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::layout::{
        Monospace,
        panel::{Holding, lay_out},
    };

    /// Two windows, the second with the keyboard unless it is `minimized`,
    /// and two workspaces, the first showing.
    fn laid(minimized: bool) -> Placed {
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
                tasks: vec![task(0, "Editor", false), task(1, "Mail", true)],
                cells: vec![cell(10, 0), cell(11, 1)],
                #[cfg(feature = "tray")]
                tray: Vec::new(),
            },
            (600, 40),
            &mut Monospace(8.0),
        )
    }

    fn painted(placed: &Placed, open: bool, scale: u32) -> Pixmap {
        let mut picture = Pixmap::new(600 * scale, 40 * scale).expect("a picture");
        paint(
            &Shown {
                size: (600, 40),
                edge: Edge::Bottom,
                placed,
                time: "14:05",
                open,
                #[cfg(feature = "tray")]
                tray: &[],
            },
            &mut picture.as_mut(),
            scale,
            &mut Text::without_fonts(),
            &mut Images::default(),
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
    fn the_start_button_is_a_grid_lit_while_its_menu_is_open() {
        let placed = laid(false);
        let button = placed.item(Item::Start).unwrap();
        let closed = painted(&placed, false, 1);
        let middle = (button.x + button.w / 2, button.y + button.h / 2);
        // The middle is between the squares; a square's middle is off it.
        let square = (
            middle.0 - BETWEEN / 2 - SQUARE / 2 - 1,
            middle.1 - BETWEEN / 2 - SQUARE / 2 - 1,
        );
        assert_eq!(pixel(&closed, middle.0, middle.1), BAR);
        assert_eq!(pixel(&closed, square.0, square.1), INK);
        assert_eq!(pixel(&closed, button.x + 2, button.y + 2), BAR);

        let open = painted(&placed, true, 1);
        assert_eq!(pixel(&open, button.x + 2, button.y + 2), OPEN);
        assert_eq!(pixel(&open, square.0, square.1), INK);
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
