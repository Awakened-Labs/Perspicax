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
    /// The theme, with a program's own folder of icons looked in first, by
    /// that folder.
    #[cfg(feature = "tray")]
    folders: HashMap<PathBuf, Icons>,
}

impl Images {
    /// The icon theme named `theme`, hicolor if none, looked for in the data
    /// folders `data` and in `home`.
    pub(crate) fn new(theme: Option<&str>, data: &[PathBuf], home: Option<&Path>) -> Self {
        Self {
            theme: theme.map(str::to_owned),
            icons: Icons::new(&Disk, theme, data, home),
            read: HashMap::new(),
            #[cfg(feature = "tray")]
            folders: HashMap::new(),
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
                image::load_icon(&path, size * scale)
                    .map_err(|error| {
                        tracing::debug!("the icon {} could not be read: {error}", path.display());
                    })
                    .ok()
            })
            .as_ref()
    }

    /// The image of icon `name`, as [`Images::get`] finds it, but looked for
    /// first in `folder`, a program's own folder of icons, if it has one:
    /// among its loose images, and in its copy of each theme's folders.
    #[cfg(feature = "tray")]
    pub(crate) fn get_from(
        &mut self,
        folder: Option<&Path>,
        name: &str,
        size: u32,
        scale: u32,
    ) -> Option<&Pixmap> {
        let Some(folder) = folder else {
            return self.get(name, size, scale);
        };
        let icons = &self.icons;
        let found = self
            .folders
            .entry(folder.to_owned())
            .or_insert_with(|| icons.with_folder(&Disk, folder))
            .find(&Disk, name, size, scale);
        match found.as_deref().and_then(Path::to_str) {
            Some(path) => self.get(path, size, scale),
            None => self.get(name, size, scale),
        }
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

#[cfg(all(test, feature = "svg"))]
mod tests {
    use super::*;

    /// A square of the highlight's blue, sixteen units on a side.
    const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#3daee9"/></svg>"##;

    /// The premultiplied RGBA of the pixel at `x`, `y`.
    fn pixel(image: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width() + x) * 4) as usize;
        image.data()[at..at + 4].try_into().expect("four bytes")
    }

    #[test]
    fn an_svg_icon_renders_at_the_asked_size() {
        let data = std::env::temp_dir().join(format!("perspicax-shell-svg-{}", std::process::id()));
        let hicolor = data.join("icons/hicolor");
        std::fs::create_dir_all(hicolor.join("scalable/apps")).expect("a theme");
        std::fs::write(
            hicolor.join("index.theme"),
            "[Icon Theme]\nName=Hicolor\nDirectories=scalable/apps\n\
             [scalable/apps]\nSize=48\nType=Scalable\nMinSize=8\nMaxSize=512\n",
        )
        .expect("its index");
        std::fs::write(hicolor.join("scalable/apps/square.svg"), SQUARE).expect("an icon");
        let mut images = Images::new(None, std::slice::from_ref(&data), None);
        let drawn = [1, 2].map(|scale| {
            images.get("square", 48, scale).map(|image| {
                let middle = image.width() / 2;
                (image.width(), image.height(), pixel(image, middle, middle))
            })
        });
        std::fs::remove_dir_all(&data).ok();
        let blue = [0x3d, 0xae, 0xe9, 0xff];
        assert_eq!(
            drawn[0],
            Some((48, 48, blue)),
            "found in hicolor, and drawn"
        );
        assert_eq!(
            drawn[1],
            Some((96, 96, blue)),
            "twice as many pixels at scale 2"
        );
    }
}
