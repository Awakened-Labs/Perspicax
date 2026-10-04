//! Icons as images: found in the theme, read from disk once, and kept for
//! every menu and panel after.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use tiny_skia::{FilterQuality, Pixmap, PixmapMut, PixmapPaint, Transform};

use crate::{
    layout::Rect,
    model::{fs::Disk, icons::Icons, image},
};

/// The icon theme, and the images already read from it.
#[derive(Default)]
pub(crate) struct Images {
    /// The theme's name, as the config gives it.
    theme: Option<String>,
    icons: Icons,
    /// By name and size in pixels; `None` for one that could not be found
    /// or read, so it is not looked for again.
    read: HashMap<(String, u32), Option<Pixmap>>,
}

impl Images {
    /// The icon theme named `theme`, hicolor if none, looked for in the data
    /// folders `data` and in `home`.
    pub(crate) fn new(theme: Option<&str>, data: &[PathBuf], home: Option<&Path>) -> Self {
        Self {
            theme: theme.map(str::to_owned),
            icons: Icons::new(&Disk, theme, data, home),
            read: HashMap::new(),
        }
    }

    /// The theme's name, as the config gives it.
    pub(crate) fn theme(&self) -> Option<&str> {
        self.theme.as_deref()
    }

    /// The image of icon `name`, for drawing `size` logical pixels square
    /// at `scale`.
    pub(crate) fn get(&mut self, name: &str, size: u32, scale: u32) -> Option<&Pixmap> {
        let icons = &self.icons;
        self.read
            .entry((name.to_owned(), size * scale))
            .or_insert_with(|| {
                let path = icons.find(&Disk, name, size, scale)?;
                image::load(&path)
                    .map_err(|error| {
                        tracing::debug!("the icon {} could not be read: {error}", path.display());
                    })
                    .ok()
            })
            .as_ref()
    }
}

/// Draw `image` on `canvas`, scaled to fill `place`, in pixels.
pub(crate) fn draw(canvas: &mut PixmapMut<'_>, image: &Pixmap, place: Rect) {
    let (sx, sy) = (
        place.w as f32 / image.width() as f32,
        place.h as f32 / image.height() as f32,
    );
    canvas.draw_pixmap(
        0,
        0,
        image.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        },
        Transform::from_row(sx, 0.0, 0.0, sy, place.x as f32, place.y as f32),
        None,
    );
}
