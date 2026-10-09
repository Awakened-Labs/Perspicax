//! A pie, drawn: each slot's icon round the ring, on nothing, so what is
//! under the pie shows through as it did under PieDock.

use tiny_skia::PixmapMut;

use super::icons::{self, Images};
use crate::{
    layout::{Rect, pie::ZOOM},
    pie::View,
};

/// Draw `view` on `canvas`, a surface's pixels at `scale` times its size.
pub(crate) fn paint(view: &View<'_>, canvas: &mut PixmapMut<'_>, scale: u32, images: &mut Images) {
    canvas.fill(tiny_skia::Color::TRANSPARENT);
    let ring = &view.ring;
    // Every icon is read once, at the size it is drawn at its biggest, and
    // drawn smaller from that.
    let biggest = (ring.icon * (1.0 + ZOOM)).ceil() as u32;
    let s = f64::from(scale);
    for (index, slot) in view.slots.iter().enumerate() {
        let ((x, y), side) = ring.place(index, view.spin, None);
        let place = Rect::new(
            ((x - side / 2.0) * s).round() as i32,
            ((y - side / 2.0) * s).round() as i32,
            (side * s).round() as i32,
            (side * s).round() as i32,
        );
        let found = slot
            .icons
            .iter()
            .find(|name| images.get(name, biggest, scale).is_some());
        if let Some(image) = found.and_then(|name| images.get(name, biggest, scale)) {
            icons::draw(canvas, image, place);
        }
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::*;
    use crate::{
        layout::pie::Ring,
        model::pie::{Does, Slot},
    };

    #[test]
    fn a_pie_of_icons_that_cannot_be_found_is_clear() {
        let slots = vec![Slot {
            label: "nothing".to_owned(),
            icons: vec!["/nowhere/at/all.png".to_owned()],
            does: Does::Open(Vec::new()),
        }];
        let view = View {
            output: "eDP-1",
            label: "launchers",
            ring: Ring::new(1, 128, (64, 64), Rect::new(0, 0, 128, 128)),
            slots: &slots,
            spin: 0,
            pointer: None,
            picked: None,
        };
        let mut picture = Pixmap::new(128, 128).expect("a picture");
        picture.fill(tiny_skia::Color::WHITE);
        paint(&view, &mut picture.as_mut(), 1, &mut Images::default());
        assert!(picture.data().iter().all(|&byte| byte == 0));
    }
}
