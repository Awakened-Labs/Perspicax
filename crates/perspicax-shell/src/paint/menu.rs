//! The open menus, drawn over a clear surface the size of the monitor.
//!
//! Each menu is a light panel with a thin border, square-cornered so that
//! all of it is opaque and the compositor can say so to an agent. A line is
//! an icon, a label cut short if it must be, and an arrow for a submenu.
//! The line the keyboard is on is drawn in the highlight colour, and each
//! line whose submenu is open in a paler one. What a person typed sits on a
//! line of its own above the rest, behind a magnifying glass; the start
//! menu's line is there before anything is typed, saying what it is for.
//! The shapes are drawn rather than taken from a font, so they show with any
//! font, or none.

use tiny_skia::{LineCap, PathBuilder, PixmapMut, Stroke, Transform};

use super::{
    fill,
    icons::{self, Images},
    scaled, solid,
    text::Text,
};
use crate::{
    layout::{
        Rect, TEXT,
        menu::{ARROW, BORDER, GAP, ICON, INSET},
    },
    model::menu::Does,
    update::View,
};

/// A menu's colours, as premultiplied RGBA. Light, after Breeze.
pub(crate) const BACKGROUND: [u8; 4] = [0xfc, 0xfc, 0xfc, 0xff];
pub(crate) const EDGE: [u8; 4] = [0xa0, 0xa4, 0xa8, 0xff];
pub(crate) const INK: [u8; 4] = [0x23, 0x26, 0x29, 0xff];
/// The line the keyboard is on, and the ink on it.
pub(crate) const HIGHLIGHT: [u8; 4] = [0x3d, 0xae, 0xe9, 0xff];
pub(crate) const ON_HIGHLIGHT: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
/// A line whose submenu is open, while the keyboard is in the submenu.
pub(crate) const OPENED: [u8; 4] = [0xc4, 0xe5, 0xf7, 0xff];
pub(crate) const RULE: [u8; 4] = [0xdc, 0xde, 0xe0, 0xff];
/// Behind what a person typed.
pub(crate) const TYPED: [u8; 4] = [0xef, 0xf0, 0xf1, 0xff];
/// What an empty search line says, and its ink.
const HINT: &str = "Type to search";
const HINT_INK: [u8; 4] = [0x7f, 0x8c, 0x8d, 0xff];

/// Draw `view` on `canvas`, a surface's pixels at `scale` times its size.
pub(crate) fn paint(
    view: &View<'_>,
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    images: &mut Images,
) {
    canvas.fill(tiny_skia::Color::TRANSPARENT);
    let px = |rect: Rect| scaled(rect, scale);
    let s = scale as f32;
    for menu in &view.menus {
        fill(canvas, px(menu.rect), EDGE);
        fill(canvas, px(inset(menu.rect, BORDER)), BACKGROUND);
        if let (Some(header), Some(query)) = (menu.header, view.query) {
            fill(canvas, px(header), TYPED);
            magnifier(canvas, px(icon_box(header)), s);
            let (words, ink) = if query.is_empty() {
                (HINT, HINT_INK)
            } else {
                (query, INK)
            };
            text.write(canvas, words, px(label_box(header, false)), TEXT * s, ink);
        }
        for line in &menu.lines {
            if let Does::Separator = line.item.does {
                let rule = Rect::new(
                    line.rect.x + INSET,
                    line.rect.y + line.rect.h / 2,
                    line.rect.w - 2 * INSET,
                    1,
                );
                fill(canvas, px(rule), RULE);
                continue;
            }
            let ink = if line.focused {
                fill(canvas, px(line.rect), HIGHLIGHT);
                ON_HIGHLIGHT
            } else {
                if line.selected {
                    fill(canvas, px(line.rect), OPENED);
                }
                INK
            };
            if let Some(image) = line
                .item
                .icon
                .as_deref()
                .and_then(|name| images.get(name, ICON as u32, scale))
            {
                icons::draw(canvas, image, px(icon_box(line.rect)));
            }
            let submenu = line.item.submenu().is_some();
            text.write(
                canvas,
                &line.item.label,
                px(label_box(line.rect, submenu)),
                TEXT * s,
                ink,
            );
            if submenu {
                chevron(canvas, px(arrow_box(line.rect)), s, ink);
            }
        }
    }
}

/// `rect` in pixels at `scale`.
fn inset(rect: Rect, by: i32) -> Rect {
    Rect::new(rect.x + by, rect.y + by, rect.w - 2 * by, rect.h - 2 * by)
}

/// Where a line's icon goes: square, at its left, centred down it.
fn icon_box(line: Rect) -> Rect {
    Rect::new(line.x + INSET, line.y + (line.h - ICON) / 2, ICON, ICON)
}

/// Where a line's label goes: between the icon and the arrow.
fn label_box(line: Rect, arrow: bool) -> Rect {
    let x = line.x + INSET + ICON + GAP;
    let right = line.right() - INSET - if arrow { ARROW + GAP } else { 0 };
    Rect::new(x, line.y, right - x, line.h)
}

/// Where a submenu's arrow goes: at the line's right.
fn arrow_box(line: Rect) -> Rect {
    Rect::new(line.right() - INSET - ARROW, line.y, ARROW, line.h)
}

/// A `>`, centred in `place`, `s` pixels to a logical one.
fn chevron(canvas: &mut PixmapMut<'_>, place: Rect, s: f32, ink: [u8; 4]) {
    let (cx, cy) = (
        place.x as f32 + place.w as f32 / 2.0,
        place.y as f32 + place.h as f32 / 2.0,
    );
    let half = 4.0 * s;
    let mut path = PathBuilder::new();
    path.move_to(cx - half / 2.0, cy - half);
    path.line_to(cx + half / 2.0, cy);
    path.line_to(cx - half / 2.0, cy + half);
    stroke(canvas, path, s, ink);
}

/// A magnifying glass, centred in `place`.
fn magnifier(canvas: &mut PixmapMut<'_>, place: Rect, s: f32) {
    let (cx, cy) = (
        place.x as f32 + place.w as f32 / 2.0 - 1.5 * s,
        place.y as f32 + place.h as f32 / 2.0 - 1.5 * s,
    );
    let radius = 5.0 * s;
    let mut path = PathBuilder::new();
    path.push_circle(cx, cy, radius);
    let reach = radius * std::f32::consts::FRAC_1_SQRT_2;
    path.move_to(cx + reach, cy + reach);
    path.line_to(cx + reach + 4.0 * s, cy + reach + 4.0 * s);
    stroke(canvas, path, s, INK);
}

fn stroke(canvas: &mut PixmapMut<'_>, path: PathBuilder, s: f32, ink: [u8; 4]) {
    if let Some(path) = path.finish() {
        let stroke = Stroke {
            width: 1.5 * s,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        canvas.stroke_path(&path, &solid(ink), &stroke, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::{
        layout::Monospace,
        model::{
            Button,
            apps::{App, Run},
            menu::{Session, root, start},
        },
        update::{Event, Key, State},
    };

    fn state() -> State {
        let app = App {
            id: "xcalc".to_owned(),
            name: "Calculator".to_owned(),
            comment: None,
            icon: None,
            run: Run {
                argv: vec!["xcalc".to_owned()],
                terminal: false,
                dir: None,
            },
            categories: vec!["Utility".to_owned()],
            keywords: Vec::new(),
            wm_class: None,
        };
        let mut state = State::new(
            root(std::slice::from_ref(&app), None, &Session::default()),
            start(&[app], &Session::default()),
        );
        state.update(
            Event::DesktopPress {
                output: "DP-1".to_owned(),
                area: Rect::new(0, 0, 400, 300),
                at: (20.0, 10.0),
                button: Button::Right,
            },
            &mut Monospace(8.0),
        );
        state
    }

    fn painted(state: &State, scale: u32) -> Pixmap {
        let mut picture = Pixmap::new(400 * scale, 300 * scale).expect("a picture");
        let view = state.view().expect("open");
        paint(
            &view,
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
    fn a_menu_is_opaque_where_it_is_laid_out_and_clear_elsewhere() {
        let state = state();
        let menu = state.view().unwrap().menus[0].rect;
        for scale in [1, 2] {
            let picture = painted(&state, scale);
            let s = scale as i32;
            assert_eq!(pixel(&picture, 5 * s, 5 * s), [0; 4], "clear off the menu");
            assert_eq!(pixel(&picture, menu.x * s, menu.y * s), EDGE, "its border");
            assert_eq!(
                pixel(&picture, (menu.x + 2) * s, (menu.y + 2) * s),
                BACKGROUND,
                "its padding, at {scale}x"
            );
            assert_eq!(
                pixel(&picture, menu.right() * s, menu.y * s),
                [0; 4],
                "and nothing past its edge"
            );
        }
    }

    #[test]
    fn the_line_the_keyboard_is_on_is_highlighted() {
        let mut state = state();
        let line = state.view().unwrap().menus[0].lines[0].rect;
        let at = (line.x + 2, line.y + 2);
        assert_eq!(pixel(&painted(&state, 1), at.0, at.1), BACKGROUND);
        state.update(Event::Key(Key::Down), &mut Monospace(8.0));
        assert_eq!(pixel(&painted(&state, 1), at.0, at.1), HIGHLIGHT);
        state.update(Event::Key(Key::Right), &mut Monospace(8.0));
        assert_eq!(
            pixel(&painted(&state, 1), at.0, at.1),
            OPENED,
            "paler once the keyboard is in its submenu"
        );
    }
}
