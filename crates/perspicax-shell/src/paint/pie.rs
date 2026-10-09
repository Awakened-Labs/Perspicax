//! A pie, drawn: each slot's icon round the ring, grown as the pointer
//! points near it, and the name of the one pointed at in the middle on a
//! cartouche, all on nothing, so what is under the pie shows through as it
//! did under PieDock. Under an icon with windows open, a dot for each, up
//! to three; under a submenu with anything running in it, one.
//!
//! Only the pie's square is drawn in: the rest of the surface is left as
//! it was, clear, so each picture costs the square and no more.

use perspicax_config::{Palette, Role};
use tiny_skia::{FillRule, PathBuilder, PixmapMut, Transform};

use super::{
    colour,
    icons::{self, Images},
    rounded, solid,
    text::Text,
};
use crate::{
    layout::{Measure, Rect, pie::ZOOM},
    model::pie::Does,
    pie::View,
};

/// The most dots under an icon, however many windows it has.
const MOST_DOTS: usize = 3;
/// How big a dot is, as a share of its icon.
const DOT: f32 = 0.035;
/// How opaque the title's cartouche is, of 255: PieDock's.
const CARTOUCHE: u8 = 192;
/// The room either side of the title on its cartouche, and above and below.
const PADDING: f32 = 8.0;
const ROUNDING: f32 = 6.0;

/// Draw `view` on `canvas`, a surface's pixels at `scale` times its size,
/// inside the pie's square, which is clear.
pub(crate) fn paint(
    view: &View<'_>,
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    images: &mut Images,
    palette: &Palette,
) {
    let ring = &view.ring;
    // Every icon is read once, at the size it is drawn at its biggest, and
    // drawn smaller from that.
    let biggest = (ring.icon * (1.0 + ZOOM)).ceil() as u32;
    let s = f64::from(scale);
    let mut order: Vec<_> = (0..view.slots.len())
        .map(|index| (index, ring.place(index, view.spin, view.pointer)))
        .collect();
    // The biggest last, so the icon pointed at is over its neighbours.
    order.sort_by(|(_, (_, a)), (_, (_, b))| a.total_cmp(b));
    for (index, ((x, y), side)) in order {
        let place = Rect::new(
            ((x - side / 2.0) * s).round() as i32,
            ((y - side / 2.0) * s).round() as i32,
            (side * s).round() as i32,
            (side * s).round() as i32,
        );
        let icons = &view.slots[index].icons;
        let found = icons
            .iter()
            .find(|name| images.get(name, biggest, scale).is_some());
        if let Some(image) = found.and_then(|name| images.get(name, biggest, scale)) {
            icons::draw(canvas, image, place);
        }
        let slot = &view.slots[index];
        let dots = match slot.does {
            _ if slot.windows.is_empty() => 0,
            Does::Open(_) => 1,
            Does::Launch(_) | Does::Switch(_) => slot.windows.len().min(MOST_DOTS),
        };
        running(
            canvas,
            palette,
            ((x * s) as f32, ((y + side / 2.0) * s) as f32),
            side * s,
            dots,
        );
    }
    if let Some(picked) = view.picked.and_then(|index| view.slots.get(index)) {
        title(view, canvas, scale as f32, text, palette, &picked.label);
    }
}

/// `dots` dots in a row along the inside of an icon's lower edge, whose
/// middle is `edge`, for an icon `side` pixels across: over the icon, as
/// PieDock drew its mark, so that nothing is drawn outside the pie's
/// square, however big the icon has grown.
fn running(
    canvas: &mut PixmapMut<'_>,
    palette: &Palette,
    edge: (f32, f32),
    side: f64,
    dots: usize,
) {
    if dots == 0 {
        return;
    }
    let radius = (side as f32 * DOT).max(1.5);
    let step = radius * 3.0;
    let first = edge.0 - step * (dots as f32 - 1.0) / 2.0;
    let mut path = PathBuilder::new();
    for dot in 0..dots {
        path.push_circle(first + step * dot as f32, edge.1 - 2.0 * radius, radius);
    }
    if let Some(path) = path.finish() {
        canvas.fill_path(
            &path,
            &solid(colour(palette, Role::Accent)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// The name of the slot pointed at, in the middle of the ring, on a
/// cartouche no wider than the room inside the icons.
fn title(
    view: &View<'_>,
    canvas: &mut PixmapMut<'_>,
    s: f32,
    text: &mut Text,
    palette: &Palette,
    label: &str,
) {
    let ring = &view.ring;
    let room = (2.0 * (ring.radius - ring.icon / 2.0)).max(0.0) as f32;
    let width = (text.width(label) + 2.0 * PADDING).min(room);
    let height = text.size() + 2.0 * PADDING;
    let (x, y) = (
        ring.centre.0 as f32 - width / 2.0,
        ring.centre.1 as f32 - height / 2.0,
    );
    let [r, g, b, _] = colour(palette, Role::Panel);
    if let Some(path) = rounded(x * s, y * s, width * s, height * s, ROUNDING * s) {
        canvas.fill_path(
            &path,
            &solid([r, g, b, CARTOUCHE]),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let place = Rect::new(
        ((x + PADDING) * s).round() as i32,
        (y * s).round() as i32,
        ((width - 2.0 * PADDING) * s).round() as i32,
        (height * s).round() as i32,
    );
    let ink = colour(palette, Role::PanelInk);
    text.write(canvas, label, place, text.size() * s, ink);
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::{layout::pie::Ring, model::pie::Slot};

    /// A red square icon, written as a PNG where the test can find it,
    /// named for `test` so that tests running at once keep apart.
    fn red_icon(test: &str) -> String {
        let path = std::env::temp_dir().join(format!(
            "perspicax-shell-pie-{test}-{}.png",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).expect("a file");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 16, 16);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header");
        writer
            .write_image_data(&[0xff, 0x00, 0x00, 0xff].repeat(16 * 16))
            .expect("written");
        path.display().to_string()
    }

    fn slots(icon: &str) -> Vec<Slot> {
        ["up", "right", "down", "left"]
            .map(|label| Slot {
                label: label.to_owned(),
                icons: vec![icon.to_owned()],
                does: Does::Open(Vec::new()),
                windows: Vec::new(),
            })
            .to_vec()
    }

    fn view<'a>(slots: &'a [Slot], pointer: Option<(f64, f64)>, picked: Option<usize>) -> View<'a> {
        View {
            output: "eDP-1",
            label: "launchers",
            ring: Ring::new(slots.len(), 256, (128, 128), Rect::new(0, 0, 256, 256)),
            slots,
            spin: 0,
            pointer,
            picked,
        }
    }

    fn drawn(view: &View<'_>) -> Pixmap {
        let mut picture = Pixmap::new(256, 256).expect("a picture");
        paint(
            view,
            &mut picture.as_mut(),
            1,
            &mut Text::without_fonts(),
            &mut Images::default(),
            &Palette::default(),
        );
        picture
    }

    fn red(picture: &Pixmap) -> usize {
        picture
            .data()
            .chunks_exact(4)
            .filter(|pixel| pixel[0] > 0xf0 && pixel[1] == 0 && pixel[3] == 0xff)
            .count()
    }

    fn pixel(picture: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * picture.width() + x) * 4) as usize;
        picture.data()[at..at + 4].try_into().expect("four bytes")
    }

    #[test]
    fn each_icon_is_drawn_at_its_place_and_nothing_where_none_is_found() {
        let icon = red_icon("placed");
        let slots = slots(&icon);
        let at_rest = drawn(&view(&slots, None, None));
        let ring = view(&slots, None, None).ring;
        let ((x, y), _) = ring.place(0, 0, None);
        assert_eq!(pixel(&at_rest, x as u32, y as u32), [0xff, 0, 0, 0xff]);
        assert_eq!(
            pixel(&at_rest, 128, 128),
            [0, 0, 0, 0],
            "the middle is clear"
        );
        let missing = vec![Slot {
            label: "nothing".to_owned(),
            icons: vec!["/nowhere/at/all.png".to_owned()],
            does: Does::Open(Vec::new()),
            windows: Vec::new(),
        }];
        assert!(
            drawn(&view(&missing, None, None))
                .data()
                .iter()
                .all(|&byte| byte == 0)
        );
        std::fs::remove_file(icon).ok();
    }

    #[test]
    fn pointing_grows_the_icon_pointed_at_and_puts_its_name_on_a_cartouche() {
        let icon = red_icon("grown");
        let slots = slots(&icon);
        let at_rest = drawn(&view(&slots, None, None));
        let pointed = drawn(&view(&slots, Some((128.0, 0.0)), Some(0)));
        assert!(
            red(&pointed) > red(&at_rest),
            "the one pointed at is bigger"
        );
        let [r, g, b, _] = colour(&Palette::default(), Role::Panel);
        let middle = pixel(&pointed, 128, 128);
        assert_eq!(
            middle[3], CARTOUCHE,
            "a cartouche, see-through, in the middle"
        );
        let blend = |c: u8| (u32::from(c) * u32::from(CARTOUCHE) / 255) as u8;
        assert!(
            middle[..3]
                .iter()
                .zip([r, g, b])
                .all(|(&got, want)| got.abs_diff(blend(want)) <= 1),
            "in the panel's colour: {middle:?}"
        );
        std::fs::remove_file(icon).ok();
    }

    #[test]
    fn a_slot_with_windows_open_has_a_dot_under_it_for_each_up_to_three() {
        let icon = red_icon("dotted");
        let mut slots = slots(&icon);
        slots[2].windows = vec![1, 2, 3, 4, 5];
        let picture = drawn(&view(&slots, None, None));
        let ring = view(&slots, None, None).ring;
        let accent = colour(&Palette::default(), Role::Accent);
        let dotted = |item: usize| {
            let ((x, y), side) = ring.place(item, 0, None);
            let radius = (side as f32 * DOT).max(1.5);
            let row = (y + side / 2.0) as f32 - 2.0 * radius;
            (0..picture.width())
                .filter(|&column| {
                    let pixel = pixel(&picture, column, row.round() as u32);
                    pixel == accent && (f64::from(column) - x).abs() < side / 2.0
                })
                .count()
        };
        assert!(dotted(2) > 0, "the slot with windows");
        assert_eq!(dotted(0), 0, "and none under the others");
        std::fs::remove_file(icon).ok();
    }
}
