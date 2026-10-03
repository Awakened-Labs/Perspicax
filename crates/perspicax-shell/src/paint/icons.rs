//! Icons as images: found in the theme, read from disk once, and kept for
//! every menu after.

use std::collections::HashMap;

use tiny_skia::Pixmap;

use crate::model::{fs::Disk, icons::Icons, image};

/// The icon theme, and the images already read from it.
#[derive(Default)]
pub(crate) struct Images {
    icons: Icons,
    /// By name and size in pixels; `None` for one that could not be found
    /// or read, so it is not looked for again.
    read: HashMap<(String, u32), Option<Pixmap>>,
}

impl Images {
    pub(crate) fn new(icons: Icons) -> Self {
        Self {
            icons,
            read: HashMap::new(),
        }
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
