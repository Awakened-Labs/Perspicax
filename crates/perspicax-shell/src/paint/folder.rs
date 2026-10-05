//! The desktop folder's icons, over the wallpaper: each one's picture, and
//! its name under it in white over a dark shadow, which reads on any
//! wallpaper, light or dark. A selected icon sits in a see-through wash of
//! the highlight.

use tiny_skia::PixmapMut;

use super::{
    fill,
    icons::{self, Images},
    scaled,
    text::Text,
};
use crate::{
    layout::{
        Rect, TEXT,
        folder::{ICON, Spot},
    },
    model::folder::Folder,
};

/// Behind a selected icon: the highlight, as the menus have it, a little
/// over a third opaque. Straight alpha, as a fill takes it.
const SELECTED: [u8; 4] = [0x3d, 0xae, 0xe9, 0x66];
/// A name, and the shadow a pixel below and to the right of it.
const INK: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
const SHADOW: [u8; 4] = [0x00, 0x00, 0x00, 0xc0];

/// Draw the icons of `folder` at `spots`, one for each icon as far as they
/// go, on `canvas`, a surface's pixels at `scale` times its size.
pub(crate) fn paint(
    folder: &Folder,
    spots: &[Spot],
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    images: &mut Images,
) {
    let px = |rect: Rect| scaled(rect, scale);
    let size = TEXT * scale as f32;
    for (icon, spot) in folder.icons().iter().zip(spots) {
        if folder.is_selected(icon) {
            fill(canvas, px(spot.place), SELECTED);
        }
        let name = [icon.image.as_str(), icon.fallback]
            .into_iter()
            .find(|name| images.get(name, ICON as u32, scale).is_some());
        if let Some(image) = name.and_then(|name| images.get(name, ICON as u32, scale)) {
            icons::draw(canvas, image, px(spot.picture));
        }
        let label = spot.label;
        let below = Rect::new(label.x + 1, label.y + 1, label.w, label.h);
        text.write(canvas, &icon.name, px(below), size, SHADOW);
        text.write(canvas, &icon.name, px(label), size, INK);
    }
}
