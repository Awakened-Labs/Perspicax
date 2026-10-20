//! Where each program's last window closed, kept between sessions (#89).
//!
//! State, not config: the person never writes it, and a session takes up
//! where the last one left off. The binary names the file
//! ([`crate::Config::places`]), `$XDG_STATE_HOME/perspicax/windows.json`
//! beside the session's log. Headless keeps none.
//!
//! It is the person's history, which programs they ran and where, so it
//! names a program by its app id alone, never by a window's title, which
//! carries document names, URLs and chat subjects. Only the person may read
//! it (0600, in a folder that is theirs alone), and it is written whole
//! beside itself and renamed over, so that a session ending mid-write leaves
//! the last one whole. None of it is in the facts, or anything an agent is
//! told.
//!
//! Written at most once a second, however many windows go: a change is
//! owed, and written when the second has passed, or as the session ends.

use std::{
    ffi::OsString,
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

use perspicax_policy::{FAR, Places, Spot};
use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::state::Compositor;

/// What the file says first, so that one a later perspicax writes some
/// other way is not guessed at: it is left unread, and written over at the
/// next change.
const VERSION: u32 = 1;

/// How long a change waits to be written, so that windows closing one
/// after another are written once.
const OWED_FOR: Duration = Duration::from_secs(1);

/// The file.
#[derive(Debug, Serialize, Deserialize)]
struct Kept {
    version: u32,
    /// The most recently closed first.
    windows: Vec<Window>,
}

/// One program's place: its app id, its monitor's connector, and its
/// window's corner from that monitor's own, in logical pixels.
#[derive(Debug, Serialize, Deserialize)]
struct Window {
    app: String,
    output: String,
    x: i32,
    y: i32,
}

/// The places kept at `path`. None if there is no file yet; none, with a
/// warning, if it cannot be read or is not one this perspicax wrote. Then it
/// is written over at the next change. A place past the world's edge, which
/// this perspicax never keeps, is a file changed by hand, and forgotten: a
/// window opened there would carry its corner past an `i32` (#154).
pub(crate) fn load(path: &Path) -> Places {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Places::default(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "could not read where programs' windows last closed; starting afresh"
            );
            return Places::default();
        }
    };
    match serde_json::from_str::<Kept>(&text) {
        Ok(kept) if kept.version == VERSION => {
            let far = |at: i32| !(-FAR..=FAR).contains(&at);
            Places::from_newest_first(
                kept.windows
                    .into_iter()
                    .filter(|window| !far(window.x) && !far(window.y))
                    .map(|window| {
                        let spot = Spot {
                            output: window.output,
                            at: (window.x, window.y),
                        };
                        (window.app, spot)
                    }),
            )
        }
        Ok(kept) => {
            tracing::warn!(
                path = %path.display(),
                version = kept.version,
                "where programs' windows last closed is in a form this perspicax does not \
                 know; starting afresh"
            );
            Places::default()
        }
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "where programs' windows last closed could not be made sense of; starting afresh"
            );
            Places::default()
        }
    }
}

/// Keep `places` at `path`: written whole beside it, then renamed over it.
fn save(path: &Path, places: &Places) -> io::Result<()> {
    let kept = Kept {
        version: VERSION,
        windows: places
            .newest_first()
            .map(|(app, spot)| Window {
                app: app.to_owned(),
                output: spot.output.clone(),
                x: spot.at.0,
                y: spot.at.1,
            })
            .collect(),
    };
    let mut text = serde_json::to_string_pretty(&kept).map_err(io::Error::other)?;
    text.push('\n');
    if let Some(folder) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)?;
    }
    let beside = beside(path);
    // One left by a session that ended mid-write, made who knows how.
    gone(&beside)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&beside)?;
    file.write_all(text.as_bytes())?;
    // On the disk before it takes the last one's name: a rename can
    // otherwise reach the disk first, and a crash leave an empty file.
    file.sync_all()?;
    fs::rename(&beside, path)
}

/// No file at `path`, nor one half-written beside it.
fn delete(path: &Path) -> io::Result<()> {
    gone(&beside(path))?;
    gone(path)
}

/// Where `path` is written before it is renamed into place.
fn beside(path: &Path) -> PathBuf {
    let mut beside = OsString::from(path);
    beside.push(".new");
    PathBuf::from(beside)
}

/// `path` removed, if there was one.
fn gone(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

impl Compositor {
    /// Keep the places at `path` from now on, starting from what is there,
    /// less what is no longer to be remembered.
    pub(crate) fn keep_places_at(&mut self, path: PathBuf) {
        self.places = load(&path);
        self.places_kept = Some(path);
        self.forget_unremembered();
    }

    /// The places changed: written within a second, if they are kept.
    pub(crate) fn owe_places(&mut self) {
        if self.places_kept.is_none() || self.places_owed {
            return;
        }
        self.places_owed = true;
        let armed =
            self.loop_handle
                .insert_source(Timer::from_duration(OWED_FOR), |_, (), state| {
                    state.save_places();
                    TimeoutAction::Drop
                });
        if let Err(error) = armed {
            tracing::warn!(%error, "where programs' windows last closed is written now");
            self.save_places();
        }
    }

    /// Write the places now, if a change to them is owed.
    pub(crate) fn save_places(&mut self) {
        if !std::mem::take(&mut self.places_owed) {
            return;
        }
        let Some(path) = &self.places_kept else {
            return;
        };
        if let Err(error) = save(path, &self.places) {
            tracing::warn!(
                path = %path.display(),
                %error,
                "could not keep where programs' windows last closed"
            );
        }
    }

    /// Nothing is remembered any more: what was, goes, file and all, and
    /// no write owed puts it back.
    pub(crate) fn delete_places(&mut self) {
        self.places_owed = false;
        let Some(path) = &self.places_kept else {
            return;
        };
        match delete(path) {
            Ok(()) => tracing::info!(path = %path.display(), "nothing is remembered; forgotten"),
            Err(error) => tracing::warn!(
                path = %path.display(),
                %error,
                "nothing is remembered, but what was could not be forgotten"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{env, os::unix::fs::PermissionsExt as _, process};

    use super::*;

    /// A folder of a test's own, gone when it is.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                env::temp_dir().join(format!("perspicax-remembered-{name}-{}", process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a scratch folder");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("state/perspicax/windows.json")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("there").permissions().mode() & 0o777
    }

    fn on(output: &str, x: i32, y: i32) -> Spot {
        Spot {
            output: output.to_owned(),
            at: (x, y),
        }
    }

    fn places(each: &[(&str, Spot)]) -> Places {
        let mut places = Places::default();
        for (app, spot) in each.iter().rev() {
            places.remember(app, spot.clone());
        }
        places
    }

    #[test]
    fn places_kept_are_read_back_as_they_were() {
        let scratch = Scratch::new("round-trip");
        let kept = places(&[
            ("com.mitchellh.ghostty", on("DP-1", 1280, 40)),
            ("firefox", on("HDMI-A-1", -20, 0)),
        ]);
        save(&scratch.file(), &kept).expect("kept");
        assert_eq!(load(&scratch.file()), kept);
    }

    #[test]
    fn a_place_kept_past_the_world_is_forgotten_when_read() {
        let scratch = Scratch::new("far");
        let kept = places(&[
            ("foot", on("DP-1", i32::MAX, 0)),
            ("firefox", on("DP-1", 40, -65536)),
            ("ghostty", on("DP-1", 0, -65537)),
        ]);
        save(&scratch.file(), &kept).expect("kept");
        assert_eq!(
            load(&scratch.file()),
            places(&[("firefox", on("DP-1", 40, -65536))])
        );
    }

    #[test]
    fn only_the_person_may_read_them_and_nothing_is_left_beside() {
        let scratch = Scratch::new("modes");
        let file = scratch.file();
        save(&file, &places(&[("foot", on("DP-1", 0, 0))])).expect("kept");
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().expect("a folder")), 0o700);
        assert!(!beside(&file).exists());

        // One half-written by a session that ended, open to anyone.
        fs::write(beside(&file), "{").expect("left behind");
        fs::set_permissions(beside(&file), fs::Permissions::from_mode(0o644))
            .expect("open to anyone");
        save(&file, &places(&[("ghostty", on("DP-1", 5, 5))])).expect("kept again");
        assert_eq!(mode(&file), 0o600);
        assert!(!beside(&file).exists());
        assert_eq!(
            load(&file).recall("ghostty"),
            Some(&on("DP-1", 5, 5)),
            "replaced, not added to"
        );
        assert_eq!(load(&file).recall("foot"), None);
    }

    #[test]
    fn they_name_programs_and_monitors_and_nothing_else() {
        let scratch = Scratch::new("text");
        save(
            &scratch.file(),
            &places(&[("org.gnome.Nautilus", on("DP-1", 3, 4))]),
        )
        .expect("kept");
        let text = fs::read_to_string(scratch.file()).expect("written");
        let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(
            value,
            serde_json::json!({
                "version": 1,
                "windows": [{ "app": "org.gnome.Nautilus", "output": "DP-1", "x": 3, "y": 4 }],
            })
        );
    }

    #[test]
    fn a_file_missing_unreadable_or_from_another_version_is_nothing_remembered() {
        let scratch = Scratch::new("unread");
        let file = scratch.file();
        assert_eq!(load(&file), Places::default(), "none yet");
        fs::create_dir_all(file.parent().expect("a folder")).expect("its folder");
        for text in [
            "",
            "{",
            "[]",
            r#"{"version": 2, "windows": [{"app": "foot", "output": "DP-1", "x": 0, "y": 0}]}"#,
            r#"{"version": 1, "windows": [{"app": "foot"}]}"#,
        ] {
            fs::write(&file, text).expect("written");
            assert_eq!(load(&file), Places::default(), "{text}");
        }
    }

    #[test]
    fn a_file_with_too_many_is_read_to_the_most_recent_and_the_first_of_each() {
        let scratch = Scratch::new("too-many");
        let windows: Vec<serde_json::Value> = (0..300)
            .map(|nth| serde_json::json!({ "app": format!("app{nth}"), "output": "DP-1", "x": nth, "y": 0 }))
            .chain([serde_json::json!({ "app": "app0", "output": "DP-2", "x": 9, "y": 9 })])
            .collect();
        fs::create_dir_all(scratch.file().parent().expect("a folder")).expect("its folder");
        fs::write(
            scratch.file(),
            serde_json::json!({ "version": 1, "windows": windows }).to_string(),
        )
        .expect("written");
        let read = load(&scratch.file());
        assert_eq!(read.newest_first().count(), perspicax_policy::REMEMBERED);
        assert_eq!(read.recall("app0"), Some(&on("DP-1", 0, 0)));
        assert_eq!(read.recall("app250"), None);
    }

    #[test]
    fn forgetting_takes_the_file_and_anything_half_written() {
        let scratch = Scratch::new("delete");
        let file = scratch.file();
        save(&file, &places(&[("foot", on("DP-1", 0, 0))])).expect("kept");
        fs::write(beside(&file), "{").expect("left behind");
        delete(&file).expect("forgotten");
        assert!(!file.exists() && !beside(&file).exists());
        delete(&file).expect("forgetting nothing is no error");
    }
}
