//! The installed applications, read from their folders when first wanted,
//! and again whenever one of the folders has changed since: the menus list
//! them, and the taskbar finds each window's icon among them. Read once for
//! both.

use std::time::SystemTime;

use crate::model::{
    apps::{self, App, Places},
    fs::Disk,
};

/// The applications, and where they were read from.
pub(super) struct Installed {
    places: Places,
    apps: Vec<App>,
    /// When each folder of applications last changed, as of the last read;
    /// `None` before the first.
    read_at: Option<Vec<Option<SystemTime>>>,
    /// How many times they have been read.
    reads: u64,
}

impl Installed {
    pub(super) fn new(places: Places) -> Self {
        Self {
            places,
            apps: Vec::new(),
            read_at: None,
            reads: 0,
        }
    }

    /// Where applications, icons and programs are looked for.
    pub(super) fn places(&self) -> &Places {
        &self.places
    }

    /// Read the applications again if a folder of them changed since they
    /// were last read. The number of the read now current, which is new
    /// only when they were read again.
    pub(super) fn refresh(&mut self) -> u64 {
        let now = stamp(&self.places);
        if self.read_at.as_ref() != Some(&now) {
            self.apps = apps::scan(&Disk, &self.places);
            self.read_at = Some(now);
            self.reads += 1;
            tracing::debug!(applications = self.apps.len(), "the applications were read");
        }
        self.reads
    }

    /// The applications, as of the last [`Installed::refresh`].
    pub(super) fn apps(&self) -> &[App] {
        &self.apps
    }
}

/// When each folder of applications last changed: what was read from them
/// is current while this stays the same.
fn stamp(places: &Places) -> Vec<Option<SystemTime>> {
    places
        .data
        .iter()
        .map(|dir| {
            std::fs::metadata(dir.join("applications"))
                .and_then(|meta| meta.modified())
                .ok()
        })
        .collect()
}
