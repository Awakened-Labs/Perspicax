//! The desktop folder's icons, over the wallpaper: each one's picture, and
//! its name under it in the theme's label ink over a shadow -- white over
//! dark, unless the theme says otherwise, which reads on any wallpaper,
//! light or dark. A selected icon sits in a see-through wash of the accent.

use tiny_skia::PixmapMut;

use perspicax_config::{Palette, Role};

use super::{
    colour, fill,
    icons::{self, Images},
    scaled,
    text::Text,
};
use crate::{
    layout::{
        Rect,
        folder::{ICON, Spot},
    },
    model::folder::Folder,
};

/// Draw the icons of `folder` at `spots`, one for each icon as far as they
/// go, on `canvas`, a surface's pixels at `scale` times its size.
pub(crate) fn paint(
    folder: &Folder,
    spots: &[Spot],
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    text: &mut Text,
    images: &mut Images,
    palette: &Palette,
) {
    // Behind a selected icon, a see-through wash; a name, and the shadow a
    // pixel below and to the right of it.
    let (selected, ink, shadow) = (
        colour(palette, Role::Selected),
        colour(palette, Role::LabelInk),
        colour(palette, Role::LabelShadow),
    );
    let px = |rect: Rect| scaled(rect, scale);
    let size = text.size() * scale as f32;
    for (icon, spot) in folder.icons().iter().zip(spots) {
        if folder.is_selected(icon) {
            fill(canvas, px(spot.place), selected);
        }
        let name = [icon.image.as_str(), icon.fallback]
            .into_iter()
            .find(|name| images.get(name, ICON as u32, scale).is_some());
        if let Some(image) = name.and_then(|name| images.get(name, ICON as u32, scale)) {
            icons::draw(canvas, image, px(spot.picture));
        }
        let label = spot.label;
        let below = Rect::new(label.x + 1, label.y + 1, label.w, label.h);
        text.write(canvas, &icon.name, px(below), size, shadow);
        text.write(canvas, &icon.name, px(label), size, ink);
    }
}
