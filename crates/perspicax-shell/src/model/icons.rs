//! Icons: an icon's name found as an image file, as the Icon Theme
//! Specification has it.
//!
//! A theme is a folder of a given name in any of the icon folders,
//! `~/.icons` and each data folder's `icons/`, described by its
//! `index.theme`: which of its subfolders hold icons of which size. An icon
//! is looked for in the theme at the size asked for, then at the nearest
//! size it has; then in each theme it inherits from, then in hicolor, the
//! theme every application installs its icon into; and last among the loose
//! images in `pixmaps/`. An icon written as a path is itself.
//!
//! PNG images are found, and with the `svg` feature SVG ones too, in the
//! spec's order: a PNG before an SVG of the same icon in the same folder.
//!
//! A theme's folders are listed once, the first time an icon is looked for
//! in it, and an icon is then found by name rather than by asking the disk
//! for every file it might be: an application missing from a large theme
//! would otherwise cost a thousand look-ups before hicolor found it.

use std::{
    cell::OnceCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

use super::fs::Fs;

/// The kinds of image found, best first.
#[cfg(feature = "svg")]
const EXTENSIONS: &[&str] = &["png", "svg"];
#[cfg(not(feature = "svg"))]
const EXTENSIONS: &[&str] = &["png"];

/// The theme every application installs its icon into, and every theme
/// falls back to.
const HICOLOR: &str = "hicolor";

/// The themes to look in, read once, and where to look.
#[derive(Debug, Clone, Default)]
pub(crate) struct Icons {
    /// Where themes are kept, in order.
    bases: Vec<PathBuf>,
    /// Where loose images are kept, the last resort.
    pixmaps: Vec<PathBuf>,
    /// The theme, what it inherits, and hicolor, in the order looked in.
    chain: Vec<Theme>,
}

#[derive(Debug, Clone)]
struct Theme {
    name: String,
    dirs: Vec<Dir>,
    /// Each of `dirs`' images by icon name, the first icon folder's where
    /// several hold one: listed once, when first looked in.
    images: OnceCell<Vec<HashMap<String, PathBuf>>>,
}

/// One of a theme's subfolders of icons.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dir {
    path: String,
    size: u32,
    scale: u32,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fixed,
    Scalable { min: u32, max: u32 },
    Threshold(u32),
}

impl Icons {
    /// Read `theme`, hicolor if none, and the themes it inherits from,
    /// looking in `data`'s folders and in `home`'s `.icons`.
    pub(crate) fn new(
        fs: &impl Fs,
        theme: Option<&str>,
        data: &[PathBuf],
        home: Option<&Path>,
    ) -> Self {
        let bases: Vec<PathBuf> = home
            .map(|home| home.join(".icons"))
            .into_iter()
            .chain(data.iter().map(|dir| dir.join("icons")))
            .collect();
        let mut icons = Self {
            pixmaps: data.iter().map(|dir| dir.join("pixmaps")).collect(),
            bases,
            chain: Vec::new(),
        };
        icons.add(fs, theme.unwrap_or(HICOLOR));
        icons.add(fs, HICOLOR);
        icons
    }

    /// Read theme `name` into the chain, then what it inherits, unless it
    /// is there already.
    fn add(&mut self, fs: &impl Fs, name: &str) {
        if self.chain.iter().any(|theme| theme.name == name) {
            return;
        }
        let Some(index) = self
            .bases
            .iter()
            .find_map(|base| fs.read(&base.join(name).join("index.theme")))
        else {
            tracing::debug!("no icon theme {name}");
            return;
        };
        let groups = ini(&index);
        let about = groups.get("Icon Theme");
        let listed = |key: &str| -> Vec<String> {
            about
                .and_then(|keys| keys.get(key))
                .map(|value| {
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|it| !it.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut names = listed("Directories");
        names.extend(listed("ScaledDirectories"));
        names.dedup();
        let dirs = names
            .into_iter()
            .filter_map(|path| Dir::read(groups.get(path.as_str())?, path))
            .collect();
        self.chain.push(Theme {
            name: name.to_owned(),
            dirs,
            images: OnceCell::new(),
        });
        for parent in listed("Inherits") {
            self.add(fs, &parent);
        }
    }

    /// The image for icon `name` drawn `size` logical pixels square at
    /// `scale`, if there is one.
    pub(crate) fn find(&self, fs: &impl Fs, name: &str, size: u32, scale: u32) -> Option<PathBuf> {
        if name.starts_with('/') {
            let path = PathBuf::from(name);
            return fs.is_file(&path).then_some(path);
        }
        self.chain
            .iter()
            .find_map(|theme| self.in_theme(fs, theme, name, size, scale))
            .or_else(|| {
                self.pixmaps.iter().find_map(|dir| {
                    EXTENSIONS
                        .iter()
                        .map(|extension| dir.join(format!("{name}.{extension}")))
                        .find(|path| fs.is_file(path))
                })
            })
    }

    /// The image for `name` in `theme`: at the size asked for, or else the
    /// nearest size it has.
    fn in_theme(
        &self,
        fs: &impl Fs,
        theme: &Theme,
        name: &str,
        size: u32,
        scale: u32,
    ) -> Option<PathBuf> {
        let images = theme.images.get_or_init(|| self.list(fs, theme));
        let dirs = || theme.dirs.iter().zip(images);
        dirs()
            .filter(|(dir, _)| dir.matches(size, scale))
            .find_map(|(_, images)| images.get(name))
            .or_else(|| {
                dirs()
                    .filter_map(|(dir, images)| {
                        Some((dir.distance(size, scale), images.get(name)?))
                    })
                    .min_by_key(|(distance, _)| *distance)
                    .map(|(_, path)| path)
            })
            .cloned()
    }

    /// The images in each of `theme`'s folders, by icon name.
    fn list(&self, fs: &impl Fs, theme: &Theme) -> Vec<HashMap<String, PathBuf>> {
        theme
            .dirs
            .iter()
            .map(|dir| {
                let mut images = HashMap::new();
                // The kinds of image best first, and the icon folders in
                // order: the first found of a name is the one kept.
                for extension in EXTENSIONS {
                    for base in &self.bases {
                        let folder = base.join(&theme.name).join(&dir.path);
                        for (file, is_dir) in fs.list(&folder) {
                            let Some(stem) = file
                                .to_str()
                                .and_then(|file| file.strip_suffix(extension)?.strip_suffix('.'))
                                .filter(|_| !is_dir)
                            else {
                                continue;
                            };
                            images
                                .entry(stem.to_owned())
                                .or_insert_with(|| folder.join(&file));
                        }
                    }
                }
                images
            })
            .collect()
    }
}

impl Dir {
    /// A subfolder as its group in `index.theme` describes it. One with no
    /// `Size` is not a folder of icons.
    fn read(keys: &HashMap<String, String>, path: String) -> Option<Self> {
        let number = |key: &str| {
            keys.get(key)
                .and_then(|value| value.trim().parse::<u32>().ok())
        };
        let size = number("Size")?;
        let kind = match keys.get("Type").map(|kind| kind.trim()) {
            Some("Fixed") => Kind::Fixed,
            Some("Scalable") => Kind::Scalable {
                min: number("MinSize").unwrap_or(size),
                max: number("MaxSize").unwrap_or(size),
            },
            _ => Kind::Threshold(number("Threshold").unwrap_or(2)),
        };
        Some(Self {
            path,
            size,
            scale: number("Scale").unwrap_or(1).max(1),
            kind,
        })
    }

    /// Whether its icons are drawn for `size` at `scale`.
    fn matches(&self, size: u32, scale: u32) -> bool {
        self.scale == scale
            && match self.kind {
                Kind::Fixed => self.size == size,
                Kind::Scalable { min, max } => (min..=max).contains(&size),
                Kind::Threshold(threshold) => {
                    (self.size.saturating_sub(threshold)..=self.size + threshold).contains(&size)
                }
            }
    }

    /// How far its icons are, in pixels, from `size` at `scale`.
    fn distance(&self, size: u32, scale: u32) -> u32 {
        let wanted = size * scale;
        let (low, high) = match self.kind {
            Kind::Fixed => (self.size, self.size),
            Kind::Scalable { min, max } => (min, max),
            Kind::Threshold(threshold) => {
                (self.size.saturating_sub(threshold), self.size + threshold)
            }
        };
        let (low, high) = (low * self.scale, high * self.scale);
        if wanted < low {
            low - wanted
        } else {
            wanted.saturating_sub(high)
        }
    }
}

/// An INI file's groups, each a map of its keys.
fn ini(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            current = Some(name.to_owned());
            groups.entry(name.to_owned()).or_default();
        } else if let (Some(group), Some((key, value))) = (&current, line.split_once('=')) {
            groups
                .entry(group.clone())
                .or_default()
                .entry(key.trim().to_owned())
                .or_insert_with(|| value.trim().to_owned());
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fs::fake::Files;

    const ICONS: &str = "/usr/share/icons";

    /// A disk with Breeze, which inherits from nothing, and hicolor.
    fn files() -> Files {
        Files::default()
            .with(
                &format!("{ICONS}/breeze/index.theme"),
                "[Icon Theme]\nName=Breeze\nDirectories=apps/22,apps/48,places/scalable\n\
                 ScaledDirectories=apps/22@2x\n\
                 [apps/22]\nSize=22\nType=Fixed\n\
                 [apps/22@2x]\nSize=22\nScale=2\nType=Fixed\n\
                 [apps/48]\nSize=48\nType=Fixed\n\
                 [places/scalable]\nSize=64\nType=Scalable\nMinSize=8\nMaxSize=512\n",
            )
            .with(
                &format!("{ICONS}/breeze/apps/22/utilities-terminal.png"),
                "",
            )
            .with(
                &format!("{ICONS}/breeze/apps/22@2x/utilities-terminal.png"),
                "",
            )
            .with(
                &format!("{ICONS}/breeze/apps/48/system-file-manager.png"),
                "",
            )
            .with(
                &format!("{ICONS}/hicolor/index.theme"),
                "[Icon Theme]\nName=Hicolor\nDirectories=24x24/apps,48x48/apps\n\
                 [24x24/apps]\nSize=24\n\
                 [48x48/apps]\nSize=48\n",
            )
            .with(&format!("{ICONS}/hicolor/24x24/apps/firefox.png"), "")
            .with(&format!("{ICONS}/hicolor/48x48/apps/firefox.png"), "")
            .with("/usr/share/pixmaps/xterm.png", "")
    }

    fn found(icons: &Icons, name: &str, size: u32, scale: u32) -> Option<String> {
        icons
            .find(&files(), name, size, scale)
            .map(|path| path.display().to_string())
    }

    fn breeze() -> Icons {
        Icons::new(
            &files(),
            Some("breeze"),
            &[PathBuf::from("/usr/share")],
            None,
        )
    }

    #[test]
    fn an_icon_missing_from_the_theme_is_found_in_hicolor() {
        assert_eq!(
            found(&breeze(), "firefox", 24, 1).as_deref(),
            Some("/usr/share/icons/hicolor/24x24/apps/firefox.png")
        );
        assert_eq!(
            found(&breeze(), "utilities-terminal", 22, 1).as_deref(),
            Some("/usr/share/icons/breeze/apps/22/utilities-terminal.png"),
            "and one the theme has is the theme's"
        );
    }

    #[test]
    fn the_nearest_size_is_found_when_the_one_asked_for_is_missing() {
        assert_eq!(
            found(&breeze(), "system-file-manager", 22, 1).as_deref(),
            Some("/usr/share/icons/breeze/apps/48/system-file-manager.png")
        );
        assert_eq!(
            found(&breeze(), "utilities-terminal", 22, 2).as_deref(),
            Some("/usr/share/icons/breeze/apps/22@2x/utilities-terminal.png"),
            "drawn for the scale"
        );
        assert_eq!(
            found(&breeze(), "firefox", 40, 1).as_deref(),
            Some("/usr/share/icons/hicolor/48x48/apps/firefox.png"),
            "a threshold folder two pixels either side, and then the nearest"
        );
    }

    #[test]
    fn a_loose_image_or_a_path_is_the_last_resort() {
        assert_eq!(
            found(&breeze(), "xterm", 24, 1).as_deref(),
            Some("/usr/share/pixmaps/xterm.png")
        );
        assert_eq!(
            found(&breeze(), "/usr/share/pixmaps/xterm.png", 24, 1).as_deref(),
            Some("/usr/share/pixmaps/xterm.png")
        );
        assert_eq!(found(&breeze(), "nothing-like-it", 24, 1), None);
    }

    /// A disk that counts what it is asked.
    struct Counting {
        files: Files,
        asked: std::cell::Cell<usize>,
    }

    impl Fs for Counting {
        fn read(&self, path: &Path) -> Option<String> {
            self.files.read(path)
        }
        fn list(&self, dir: &Path) -> Vec<(std::ffi::OsString, bool)> {
            self.asked.set(self.asked.get() + 1);
            self.files.list(dir)
        }
        fn is_file(&self, path: &Path) -> bool {
            self.asked.set(self.asked.get() + 1);
            self.files.is_file(path)
        }
        fn is_executable(&self, path: &Path) -> bool {
            self.files.is_executable(path)
        }
    }

    #[test]
    fn a_theme_is_listed_once_however_many_icons_are_looked_for() {
        let disk = Counting {
            files: files(),
            asked: std::cell::Cell::new(0),
        };
        let icons = Icons::new(&disk, Some("breeze"), &[PathBuf::from("/usr/share")], None);
        assert!(icons.find(&disk, "firefox", 24, 1).is_some());
        let first = disk.asked.get();
        assert!(icons.find(&disk, "firefox", 48, 1).is_some());
        assert!(icons.find(&disk, "utilities-terminal", 22, 1).is_some());
        assert_eq!(disk.asked.get(), first, "found by name, without the disk");
    }

    #[test]
    fn no_theme_named_is_hicolor() {
        let icons = Icons::new(&files(), None, &[PathBuf::from("/usr/share")], None);
        assert_eq!(found(&icons, "utilities-terminal", 22, 1), None);
        assert!(found(&icons, "firefox", 24, 1).is_some());
    }
}
