//! The open menus, drawn over a clear surface the size of the monitor.
//!
//! Each menu is a panel in the theme's colours with a thin border,
//! square-cornered so that all of it is opaque and the compositor can say so
//! to an agent. A line is
//! an icon, a label cut short if it must be, and an arrow for a submenu.
//! The line the keyboard is on is drawn in the accent, and each line whose
//! submenu is open in a paler one. What a person typed sits on a
//! line of its own above the rest, behind a magnifying glass; the start
//! menu's line is there before anything is typed, saying what it is for.
//! In a tray icon's menu, an item its program greyed out is written faint,
//! and one that is on or off has a box, ticked when on, or a ring, with a
//! dot when on, where an icon would be. The shapes are drawn rather than
//! taken from a font, so they show with any font, or none.

use tiny_skia::{LineCap, PathBuilder, PixmapMut, Stroke, Transform};

use perspicax_config::{Palette, Role};

use super::{
    colour, fill,
    icons::{self, Images},
    scaled, solid,
    text::Text,
};
use crate::{
    layout::{
        Rect,
        menu::{ARROW, BORDER, GAP, ICON, INSET},
    },
    model::menu::Does,
    update::View,
};

/// What an empty search line says.
const HINT: &str = "Type to search";

/// A menu's colours, from the theme, as fills take them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Colours {
    pub(crate) background: [u8; 4],
    pub(crate) edge: [u8; 4],
    pub(crate) ink: [u8; 4],
    /// The line the keyboard is on, and the ink on it.
    pub(crate) highlight: [u8; 4],
    pub(crate) on_highlight: [u8; 4],
    /// A line whose submenu is open, while the keyboard is in the submenu.
    pub(crate) opened: [u8; 4],
    pub(crate) rule: [u8; 4],
    /// Behind what a person typed.
    pub(crate) typed: [u8; 4],
    /// The search line's hint, and a greyed-out item.
    pub(crate) hint: [u8; 4],
}

impl Colours {
    pub(crate) fn of(palette: &Palette) -> Self {
        Self {
            background: colour(palette, Role::Menu),
            edge: colour(palette, Role::MenuEdge),
            ink: colour(palette, Role::MenuInk),
            highlight: colour(palette, Role::Accent),
            on_highlight: colour(palette, Role::OnAccent),
            opened: colour(palette, Role::MenuOpened),
            rule: colour(palette, Role::MenuRule),
            typed: colour(palette, Role::MenuTyped),
            hint: colour(palette, Role::MenuHint),
        }
    }
}

/// Draw `view` on `canvas`, a surface's pixels at `scale` times its size.
pub(crate) fn paint(
    view: &View<'_>,
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    images: &mut Images,
    palette: &Palette,
) {
    canvas.fill(tiny_skia::Color::TRANSPARENT);
    let colours = Colours::of(palette);
    let px = |rect: Rect| scaled(rect, scale);
    let s = scale as f32;
    let size = text.size() * s;
    for menu in &view.menus {
        fill(canvas, px(menu.rect), colours.edge);
        fill(canvas, px(inset(menu.rect, BORDER)), colours.background);
        if let (Some(header), Some(query)) = (menu.header, view.query) {
            fill(canvas, px(header), colours.typed);
            magnifier(canvas, px(icon_box(header)), s, colours.ink);
            let (words, ink) = if query.is_empty() {
                (HINT, colours.hint)
            } else {
                (query, colours.ink)
            };
            text.write(canvas, words, px(label_box(header, false)), size, ink);
        }
        for line in &menu.lines {
            if let Does::Separator = line.item.does {
                let rule = Rect::new(
                    line.rect.x + INSET,
                    line.rect.y + line.rect.h / 2,
                    line.rect.w - 2 * INSET,
                    1,
                );
                fill(canvas, px(rule), colours.rule);
                continue;
            }
            let ink = if line.focused {
                fill(canvas, px(line.rect), colours.highlight);
                colours.on_highlight
            } else if !line.item.choosable() {
                colours.hint
            } else {
                if line.selected {
                    fill(canvas, px(line.rect), colours.opened);
                }
                colours.ink
            };
            #[cfg(feature = "tray")]
            if let Does::Tell(crate::model::menu::Choice {
                mark: Some(drawn), ..
            }) = line.item.does
            {
                mark(canvas, px(icon_box(line.rect)), s, drawn, ink);
            }
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
                size,
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

/// A box, ticked if it is on, or a ring, with a dot in it if it is on,
/// centred in `place`.
#[cfg(feature = "tray")]
fn mark(
    canvas: &mut PixmapMut<'_>,
    place: Rect,
    s: f32,
    drawn: crate::model::menu::Mark,
    ink: [u8; 4],
) {
    use crate::model::menu::Mark;

    let (cx, cy) = (
        place.x as f32 + place.w as f32 / 2.0,
        place.y as f32 + place.h as f32 / 2.0,
    );
    let half = 6.0 * s;
    let mut path = PathBuilder::new();
    let on = match drawn {
        Mark::Check(on) => {
            if let Some(square) =
                tiny_skia::Rect::from_xywh(cx - half, cy - half, 2.0 * half, 2.0 * half)
            {
                path.push_rect(square);
            }
            if on {
                path.move_to(cx - half / 2.0, cy);
                path.line_to(cx - half / 8.0, cy + half / 2.0);
                path.line_to(cx + half / 2.0, cy - half / 2.0);
            }
            false
        }
        Mark::Radio(on) => {
            path.push_circle(cx, cy, half);
            on
        }
    };
    stroke(canvas, path, s, ink);
    if on {
        let mut dot = PathBuilder::new();
        dot.push_circle(cx, cy, half / 2.0);
        if let Some(dot) = dot.finish() {
            canvas.fill_path(
                &dot,
                &solid(ink),
                tiny_skia::FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// A magnifying glass, centred in `place`.
fn magnifier(canvas: &mut PixmapMut<'_>, place: Rect, s: f32, ink: [u8; 4]) {
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
    stroke(canvas, path, s, ink);
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

    /// The default theme's menu colours: what menus were before there were
    /// themes.
    const BACKGROUND: [u8; 4] = [0xfc, 0xfc, 0xfc, 0xff];
    const EDGE: [u8; 4] = [0xa0, 0xa4, 0xa8, 0xff];
    const HIGHLIGHT: [u8; 4] = [0x3d, 0xae, 0xe9, 0xff];
    const OPENED: [u8; 4] = [0xc4, 0xe5, 0xf7, 0xff];

    fn painted(state: &State, scale: u32) -> Pixmap {
        painted_in(state, scale, &Palette::default())
    }

    fn painted_in(state: &State, scale: u32, palette: &Palette) -> Pixmap {
        let mut picture = Pixmap::new(400 * scale, 300 * scale).expect("a picture");
        let view = state.view().expect("open");
        paint(
            &view,
            &mut picture.as_mut(),
            scale,
            &mut Text::without_fonts(),
            &mut Images::default(),
            palette,
        );
        picture
    }

    #[test]
    fn the_default_theme_paints_menus_as_they_were_and_another_its_own() {
        let colours = Colours::of(&Palette::default());
        assert_eq!(
            [
                colours.background,
                colours.edge,
                colours.ink,
                colours.highlight,
                colours.on_highlight,
                colours.opened,
                colours.rule,
                colours.typed,
                colours.hint,
            ],
            [
                BACKGROUND,
                EDGE,
                [0x23, 0x26, 0x29, 0xff],
                HIGHLIGHT,
                [0xff, 0xff, 0xff, 0xff],
                OPENED,
                [0xdc, 0xde, 0xe0, 0xff],
                [0xef, 0xf0, 0xf1, 0xff],
                [0x7f, 0x8c, 0x8d, 0xff],
            ]
        );

        let state = state();
        let menu = state.view().unwrap().menus[0].rect;
        let dark = perspicax_config::Builtin::BreezeDark.palette();
        assert_eq!(
            pixel(&painted_in(&state, 1, &dark), menu.x + 2, menu.y + 2),
            colour(&dark, Role::Menu)
        );
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
