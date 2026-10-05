//! The desktop folder: what is in it, as icons on the wallpaper, and what a
//! click on one does.
//!
//! The folder is the one `user-dirs.dirs` names as the desktop, or else
//! `~/Desktop`. Everything in it but what is hidden, a name starting with a
//! dot, is an icon: a folder or a file, opened as the person's other
//! programs open it, by `xdg-open`; or a desktop entry, which shows a name
//! and an icon of its own. An application's entry starts its program, and a
//! link's opens its address.
//!
//! An application's entry is trusted only when it may be run, as Plasma
//! trusts one. Anyone's download can be saved to the desktop, and an entry
//! there could call itself a document and run anything at all; one that may
//! not be run is shown as the file it is, and opened as one, or not shown,
//! as the config says.
//!
//! Folders come first, then everything else, each by name, ignoring case.
//!
//! A press of the left button on an icon selects it, and a second press on
//! it soon after opens it; a press off them selects none. A press is timed
//! by the pointer's own clock, in milliseconds, which is what tells a
//! double-click from two clicks.

use std::path::{Path, PathBuf};

use perspicax_config::UntrustedLaunchers;

use super::{
    Button,
    apps::Run,
    desktop::{self, Locale},
    fs::Fs,
};

/// One thing in the desktop folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Icon {
    /// The file, which the icon is known by while it is there.
    pub(crate) path: PathBuf,
    /// What it is called: an entry's name, or the file's.
    pub(crate) name: String,
    /// Its picture: an icon's name in the theme, or a path to an image.
    pub(crate) image: String,
    /// The theme's icon drawn when it has no `image`: its kind's.
    pub(crate) fallback: &'static str,
    /// What opening it starts.
    pub(crate) opens: Run,
    pub(crate) is_folder: bool,
}

/// Where the desktop folder is: as `user-dirs.dirs` in `config_home` names
/// it, or else `Desktop` in `home`. `None` with no home to find it in, or
/// when the file turns the desktop folder off by naming the home folder
/// itself, as `user-dirs.dirs(5)` says to.
pub(crate) fn desktop_dir(
    fs: &impl Fs,
    config_home: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let written = config_home
        .and_then(|config| fs.read(&config.join("user-dirs.dirs")))
        .and_then(|text| {
            text.lines().find_map(|line| {
                let value = line.trim().strip_prefix("XDG_DESKTOP_DIR=")?;
                let value = value.trim();
                Some(
                    value
                        .strip_prefix('"')
                        .and_then(|value| value.strip_suffix('"'))
                        .unwrap_or(value)
                        .to_owned(),
                )
            })
        });
    let dir = match written {
        Some(written) => match written.strip_prefix("$HOME") {
            Some(rest) => home?.join(rest.trim_start_matches('/')),
            None if written.starts_with('/') => PathBuf::from(written),
            None => return None,
        },
        None => home?.join("Desktop"),
    };
    (Some(dir.as_path()) != home).then_some(dir)
}

/// The icons of what the folder `dir` holds, in the order shown, with an
/// application's entry that may not be run shown as `untrusted` says. An
/// entry's name is in `locale`'s language where it has one, and a link's
/// `$HOME` is `home`.
pub(crate) fn read(
    fs: &impl Fs,
    dir: &Path,
    untrusted: UntrustedLaunchers,
    locale: &Locale,
    home: Option<&Path>,
) -> Vec<Icon> {
    let mut icons: Vec<Icon> = fs
        .list(dir)
        .into_iter()
        .filter_map(|(name, is_dir)| {
            let name = name.to_str().filter(|name| !name.starts_with('.'))?;
            let path = dir.join(name);
            if is_dir {
                return Some(Icon {
                    opens: open(&path)?,
                    path,
                    name: name.to_owned(),
                    image: "folder".to_owned(),
                    fallback: "folder",
                    is_folder: true,
                });
            }
            let entry = name
                .ends_with(".desktop")
                .then(|| entry(fs, &path, locale, home))
                .flatten();
            match entry {
                Some(Entry::Shown(icon)) => Some(icon),
                Some(Entry::Untrusted) if untrusted == UntrustedLaunchers::Hidden => None,
                _ => file(path, name),
            }
        })
        .collect();
    icons.sort_by_cached_key(|icon| (!icon.is_folder, icon.name.to_lowercase(), icon.path.clone()));
    icons
}

/// A desktop entry on the desktop, as far as it is more than a file.
enum Entry {
    /// An application's that may be run, or a link's: its own name and icon.
    Shown(Icon),
    /// An application's that may not be run.
    Untrusted,
}

/// The desktop entry at `path`, as an icon of its own or as untrusted.
/// `None` for anything else, which is shown as a file.
fn entry(fs: &impl Fs, path: &Path, locale: &Locale, home: Option<&Path>) -> Option<Entry> {
    let entry = desktop::parse(&fs.read(path)?, locale).ok()?;
    let (opens, fallback) = if entry.application {
        if !fs.is_executable(path) {
            tracing::debug!("{} may not be run, so is not trusted", path.display());
            return Some(Entry::Untrusted);
        }
        let run = Run {
            argv: entry.exec?,
            terminal: entry.terminal,
            dir: entry.path.map(PathBuf::from),
        };
        (run, "application-x-executable")
    } else {
        let url = entry.url?;
        let url = match home.and_then(Path::to_str) {
            Some(home) => url.replace("$HOME", home),
            None => url,
        };
        // A file's address as its path, which every opener takes, and
        // drawn as a folder, or as the file it names, when its theme has
        // no icon of its own name.
        let local = url
            .strip_prefix("file://")
            .or_else(|| url.strip_prefix("file:"));
        let fallback = match local {
            Some(place) if fs.is_file(Path::new(place)) => "text-x-generic",
            Some(_) => "folder",
            None => "text-html",
        };
        (open(Path::new(local.unwrap_or(&url)))?, fallback)
    };
    Some(Entry::Shown(Icon {
        path: path.to_owned(),
        name: entry.name,
        image: entry.icon.unwrap_or_else(|| fallback.to_owned()),
        fallback,
        opens,
        is_folder: false,
    }))
}

/// A file named `name` at `path` as an icon, its picture by the kind its
/// extension says it is.
fn file(path: PathBuf, name: &str) -> Option<Icon> {
    const KINDS: [(&str, &[&str]); 8] = [
        ("application-pdf", &["pdf"]),
        ("x-office-document", &["odt", "doc", "docx", "rtf"]),
        ("x-office-spreadsheet", &["ods", "xls", "xlsx", "csv"]),
        (
            "image-x-generic",
            &[
                "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "tiff",
            ],
        ),
        (
            "audio-x-generic",
            &["mp3", "ogg", "oga", "flac", "wav", "opus", "m4a"],
        ),
        (
            "video-x-generic",
            &["mp4", "mkv", "webm", "avi", "mov", "ogv"],
        ),
        (
            "package-x-generic",
            &["zip", "tar", "gz", "tgz", "xz", "bz2", "zst", "7z", "rar"],
        ),
        ("text-html", &["html", "htm", "url"]),
    ];
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    let image = KINDS
        .iter()
        .find(|(_, extensions)| {
            extension
                .as_deref()
                .is_some_and(|it| extensions.contains(&it))
        })
        .map_or("text-x-generic", |&(image, _)| image);
    Some(Icon {
        opens: open(&path)?,
        path,
        name: name.to_owned(),
        image: image.to_owned(),
        fallback: "text-x-generic",
        is_folder: false,
    })
}

/// Opening `place`, a path or an address, as the person's programs open it.
/// `None` for a path that is not UTF-8, which cannot be passed as one.
fn open(place: &Path) -> Option<Run> {
    Some(Run {
        argv: vec!["xdg-open".to_owned(), place.to_str()?.to_owned()],
        terminal: false,
        dir: None,
    })
}

/// The icons shown, which is selected, and what was last pressed.
#[derive(Debug)]
pub(crate) struct Folder {
    icons: Vec<Icon>,
    selected: Option<PathBuf>,
    /// The icon last pressed with the left button, and when.
    last: Option<(PathBuf, u32)>,
    /// How soon a second press on an icon must follow the first to open it,
    /// in milliseconds.
    pub(crate) double_click_ms: u32,
}

/// What a press did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pressed {
    /// Nothing that shows.
    Nothing,
    /// Another icon, or none, is selected now.
    Selected,
    /// Start this.
    Open(Run),
}

impl Folder {
    /// No icons yet, opened by a double-click within `double_click_ms`.
    pub(crate) fn new(double_click_ms: u32) -> Self {
        Self {
            icons: Vec::new(),
            selected: None,
            last: None,
            double_click_ms,
        }
    }

    /// Show `icons` from now on, the one selected still selected if it is
    /// among them.
    pub(crate) fn show(&mut self, icons: Vec<Icon>) {
        let kept = |path: &PathBuf| icons.iter().any(|icon| icon.path == *path);
        if !self.selected.as_ref().is_some_and(kept) {
            self.selected = None;
        }
        if !self.last.as_ref().is_some_and(|(path, _)| kept(path)) {
            self.last = None;
        }
        self.icons = icons;
    }

    pub(crate) fn icons(&self) -> &[Icon] {
        &self.icons
    }

    pub(crate) fn is_selected(&self, icon: &Icon) -> bool {
        self.selected.as_ref() == Some(&icon.path)
    }

    /// `button` went down at `time` on icon `on`, an index into
    /// [`Folder::icons`], or on none.
    pub(crate) fn press(&mut self, on: Option<usize>, button: Button, time: u32) -> Pressed {
        if button != Button::Left {
            return Pressed::Nothing;
        }
        let Some(icon) = on.and_then(|at| self.icons.get(at)) else {
            self.last = None;
            return self.select(None);
        };
        let again = self.last.as_ref().is_some_and(|(path, then)| {
            *path == icon.path && time.wrapping_sub(*then) <= self.double_click_ms
        });
        if again {
            // A third press starts another pair, rather than opening twice.
            self.last = None;
            return Pressed::Open(icon.opens.clone());
        }
        self.last = Some((icon.path.clone(), time));
        self.select(on)
    }

    /// Select icon `on`, or none.
    pub(crate) fn select(&mut self, on: Option<usize>) -> Pressed {
        let selected = on
            .and_then(|at| self.icons.get(at))
            .map(|icon| icon.path.clone());
        if selected == self.selected {
            return Pressed::Nothing;
        }
        self.selected = selected;
        Pressed::Selected
    }

    /// What opening icon `at` starts.
    pub(crate) fn open(&self, at: usize) -> Option<Run> {
        self.icons.get(at).map(|icon| icon.opens.clone())
    }
}

#[cfg(test)]
mod tests {
    use perspicax_config::DOUBLE_CLICK_MS;

    use super::*;
    use crate::model::fs::fake::Files;

    const HOME: &str = "/home/ada";

    fn found(files: &Files) -> Option<PathBuf> {
        desktop_dir(
            files,
            Some(Path::new("/home/ada/.config")),
            Some(Path::new(HOME)),
        )
    }

    fn dirs(written: &str) -> Files {
        Files::default().with("/home/ada/.config/user-dirs.dirs", written)
    }

    #[test]
    fn the_desktop_dir_comes_from_user_dirs() {
        let translated = dirs(
            "# written by xdg-user-dirs-update\n\
             XDG_DOCUMENTS_DIR=\"$HOME/Dokumente\"\n\
             XDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"\n",
        );
        assert_eq!(found(&translated), Some("/home/ada/Schreibtisch".into()));
        assert_eq!(
            found(&dirs("XDG_DESKTOP_DIR=\"/srv/desk\"\n")),
            Some("/srv/desk".into()),
            "or a path of its own"
        );
        assert_eq!(
            found(&dirs("XDG_DOCUMENTS_DIR=\"$HOME/Documents\"\n")),
            Some("/home/ada/Desktop".into()),
            "~/Desktop when it does not say"
        );
        assert_eq!(
            found(&Files::default()),
            Some("/home/ada/Desktop".into()),
            "and with no file at all"
        );
        assert_eq!(
            found(&dirs("XDG_DESKTOP_DIR=\"$HOME/\"\n")),
            None,
            "the home folder itself turns it off"
        );
        assert_eq!(desktop_dir(&Files::default(), None, None), None, "no home");
    }

    /// The disk `files` describes, read as Ada's desktop folder.
    fn icons(files: &Files) -> Vec<Icon> {
        icons_trusting(files, UntrustedLaunchers::AsFiles)
    }

    /// The same, with an untrusted application's entry shown as `untrusted`
    /// says.
    fn icons_trusting(files: &Files, untrusted: UntrustedLaunchers) -> Vec<Icon> {
        read(
            files,
            Path::new("/home/ada/Desktop"),
            untrusted,
            &Locale::default(),
            Some(Path::new(HOME)),
        )
    }

    fn entry(kind: &str, more: &str) -> String {
        format!(
            "[Desktop Entry]\nType={kind}\nName=Field notes\nIcon=accessories-text-editor\n{more}"
        )
    }

    #[test]
    fn folders_come_first_then_everything_by_name() {
        let files = Files::default()
            .with("/home/ada/Desktop/zebra.txt", "")
            .with("/home/ada/Desktop/Apple.pdf", "")
            .with("/home/ada/Desktop/.hidden", "")
            .with("/home/ada/Desktop/Projects/plan.txt", "")
            .with("/home/ada/Desktop/archive/old.txt", "");
        let shown: Vec<_> = icons(&files)
            .into_iter()
            .map(|icon| (icon.name, icon.image))
            .collect();
        assert_eq!(
            shown,
            [
                ("archive".to_owned(), "folder".to_owned()),
                ("Projects".to_owned(), "folder".to_owned()),
                ("Apple.pdf".to_owned(), "application-pdf".to_owned()),
                ("zebra.txt".to_owned(), "text-x-generic".to_owned()),
            ],
            "ignoring case, and nothing hidden"
        );
    }

    #[test]
    fn an_entry_that_may_be_run_starts_its_program_and_one_that_may_not_is_a_file() {
        let trusted = Files::default()
            .program("/home/ada/Desktop/notes.desktop")
            .with(
                "/home/ada/Desktop/notes.desktop",
                &entry("Application", "Exec=gedit %U\n"),
            );
        let icon = &icons(&trusted)[0];
        assert_eq!(
            (icon.name.as_str(), icon.image.as_str()),
            ("Field notes", "accessories-text-editor")
        );
        assert_eq!(icon.opens.argv, ["gedit"]);

        let downloaded = Files::default().with(
            "/home/ada/Desktop/notes.desktop",
            &entry("Application", "Exec=gedit %U\n"),
        );
        let icon = &icons(&downloaded)[0];
        assert_eq!(icon.name, "notes.desktop", "shown as the file it is");
        assert_eq!(
            icon.opens.argv,
            ["xdg-open", "/home/ada/Desktop/notes.desktop"],
            "and opened as one"
        );
    }

    #[test]
    fn an_entry_that_may_not_be_run_can_be_hidden() {
        let files = Files::default()
            .program("/home/ada/Desktop/notes.desktop")
            .with(
                "/home/ada/Desktop/notes.desktop",
                &entry("Application", "Exec=gedit %U\n"),
            )
            .with(
                "/home/ada/Desktop/invoice.desktop",
                &entry("Application", "Exec=invoice-viewer\n"),
            )
            .with("/home/ada/Desktop/broken.desktop", "not an entry at all")
            .with("/home/ada/Desktop/plan.txt", "");
        let names = |untrusted| {
            icons_trusting(&files, untrusted)
                .into_iter()
                .map(|icon| icon.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(UntrustedLaunchers::Hidden),
            ["broken.desktop", "Field notes", "plan.txt"],
            "the one that may not be run is gone; the rest, and a file that is no entry, stay"
        );
        assert_eq!(
            names(UntrustedLaunchers::AsFiles),
            [
                "broken.desktop",
                "Field notes",
                "invoice.desktop",
                "plan.txt"
            ]
        );
    }

    #[test]
    fn a_link_opens_its_address() {
        let files = Files::default()
            .with(
                "/home/ada/Desktop/home.desktop",
                "[Desktop Entry]\nType=Link\nName=Home\nIcon=user-home\nURL=file:$HOME\n",
            )
            .with(
                "/home/ada/Desktop/plan.desktop",
                "[Desktop Entry]\nType=Link\nName=Plan\nURL=file:///srv/plan.txt\n",
            )
            .with("/srv/plan.txt", "")
            .with(
                "/home/ada/Desktop/docs.desktop",
                "[Desktop Entry]\nType=Link\nName=Docs\nURL=https://example.org/docs\n",
            );
        let opened: Vec<_> = icons(&files)
            .into_iter()
            .map(|icon| {
                let at = icon.opens.argv[1].clone();
                (icon.name, icon.image, icon.fallback, at)
            })
            .collect();
        let link = |name: &str, image: &str, fallback, at: &str| {
            (name.to_owned(), image.to_owned(), fallback, at.to_owned())
        };
        assert_eq!(
            opened,
            [
                link("Docs", "text-html", "text-html", "https://example.org/docs"),
                link("Home", "user-home", "folder", HOME),
                link("Plan", "text-x-generic", "text-x-generic", "/srv/plan.txt"),
            ],
            "a folder's link drawn as a folder, and a file's as a file, wanting their own"
        );
    }

    fn folder() -> Folder {
        let files = Files::default()
            .with("/home/ada/Desktop/a.txt", "")
            .with("/home/ada/Desktop/b.txt", "");
        let mut folder = Folder::new(DOUBLE_CLICK_MS);
        folder.show(icons(&files));
        folder
    }

    fn selected(folder: &Folder) -> Vec<&str> {
        folder
            .icons()
            .iter()
            .filter(|icon| folder.is_selected(icon))
            .map(|icon| icon.name.as_str())
            .collect()
    }

    #[test]
    fn a_double_click_opens_and_a_single_click_selects() {
        let mut folder = folder();
        assert_eq!(folder.press(Some(0), Button::Left, 1000), Pressed::Selected);
        assert_eq!(selected(&folder), ["a.txt"]);
        assert_eq!(
            folder.press(Some(0), Button::Left, 1000 + DOUBLE_CLICK_MS),
            Pressed::Open(Run {
                argv: vec!["xdg-open".to_owned(), "/home/ada/Desktop/a.txt".to_owned()],
                terminal: false,
                dir: None,
            }),
            "a second press soon after opens it"
        );
        assert_eq!(
            folder.press(Some(0), Button::Left, 1100 + DOUBLE_CLICK_MS),
            Pressed::Nothing,
            "a third starts another pair, still selected"
        );

        assert_eq!(folder.press(Some(1), Button::Left, 5000), Pressed::Selected);
        assert_eq!(
            folder.press(Some(1), Button::Left, 5001 + DOUBLE_CLICK_MS),
            Pressed::Nothing,
            "too slow to be a double-click"
        );
        assert_eq!(folder.press(Some(0), Button::Left, 6000), Pressed::Selected);
        assert_eq!(
            folder.press(Some(1), Button::Left, 6100),
            Pressed::Selected,
            "two icons in quick succession are two clicks"
        );
        assert_eq!(selected(&folder), ["b.txt"]);

        assert_eq!(
            folder.press(Some(0), Button::Right, 7000),
            Pressed::Nothing,
            "the right button is the root menu's"
        );
        assert_eq!(folder.press(None, Button::Left, 8000), Pressed::Selected);
        assert!(
            selected(&folder).is_empty(),
            "a press off them selects none"
        );
        assert_eq!(folder.press(None, Button::Left, 8100), Pressed::Nothing);
    }

    #[test]
    fn the_double_click_time_is_the_configs() {
        let mut slow = folder();
        slow.double_click_ms = 800;
        slow.press(Some(0), Button::Left, 1000);
        assert!(
            matches!(slow.press(Some(0), Button::Left, 1700), Pressed::Open(_)),
            "a slower hand's pair"
        );
        let mut quick = folder();
        quick.double_click_ms = 200;
        quick.press(Some(0), Button::Left, 1000);
        assert_eq!(
            quick.press(Some(0), Button::Left, 1300),
            Pressed::Nothing,
            "two clicks to a quicker one"
        );
    }

    #[test]
    fn a_double_click_is_timed_across_the_clocks_wrap() {
        let mut folder = folder();
        folder.press(Some(0), Button::Left, u32::MAX - 10);
        assert!(matches!(
            folder.press(Some(0), Button::Left, 100),
            Pressed::Open(_)
        ));
    }

    #[test]
    fn the_selection_outlives_a_read_that_still_holds_it() {
        let mut folder = folder();
        folder.press(Some(1), Button::Left, 0);
        let files = Files::default()
            .with("/home/ada/Desktop/0.txt", "")
            .with("/home/ada/Desktop/b.txt", "");
        folder.show(icons(&files));
        assert_eq!(selected(&folder), ["b.txt"], "known by its file");
        folder.show(Vec::new());
        folder.show(icons(&files));
        assert!(selected(&folder).is_empty(), "and gone with it");
    }
}
