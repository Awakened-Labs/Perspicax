//! What the mixer keeps, on disk: `mixer.json` in perspicax's state
//! folder, written as the compositor writes where windows last closed
//! (`remembered.rs` in perspicax-compositor). Only the person can read it,
//! in a folder that is theirs alone, and it is written whole beside itself
//! and renamed over, so that a shell stopped mid-write leaves the last one
//! whole. It is written at each change, which is always a person's press
//! in the mixer, never a burst.
//!
//! A file missing is nothing kept yet. One that cannot be read, or that
//! another perspicax wrote another way, is warned of, and the mixer starts
//! with nothing kept, written over at the next change.

use std::{
    ffi::OsString,
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use crate::model::kept::{Kept, Unread};

/// What is kept, and where.
pub(super) struct Keeper {
    /// The file: `None` keeps nothing, with no state folder to keep it in.
    path: Option<PathBuf>,
    kept: Kept,
}

impl Keeper {
    /// What the file at `path` keeps.
    pub(super) fn load(path: Option<PathBuf>) -> Self {
        let kept = path.as_deref().map(read).unwrap_or_default();
        Self { path, kept }
    }

    pub(super) fn kept(&self) -> &Kept {
        &self.kept
    }

    /// Change what is kept with `change`, and write it if that changed
    /// anything. Whether it did.
    pub(super) fn change(&mut self, change: impl FnOnce(&mut Kept) -> bool) -> bool {
        if !change(&mut self.kept) {
            return false;
        }
        if let Some(path) = &self.path
            && let Err(error) = save(path, &self.kept.written())
        {
            tracing::warn!(
                path = %path.display(),
                %error,
                "what the mixer hides and names could not be kept"
            );
        }
        true
    }
}

/// What the file at `path` keeps: nothing if there is none yet, or with a
/// warning if it cannot be read or is not one this perspicax wrote.
fn read(path: &Path) -> Kept {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Kept::default(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "could not read what the mixer hides and names; starting afresh"
            );
            return Kept::default();
        }
    };
    Kept::read(&text).unwrap_or_else(|unread| {
        match unread {
            Unread::Version(version) => tracing::warn!(
                path = %path.display(),
                version,
                "what the mixer hides and names is in a form this perspicax does not know; \
                 starting afresh"
            ),
            Unread::Garbled(error) => tracing::warn!(
                path = %path.display(),
                %error,
                "what the mixer hides and names could not be made sense of; starting afresh"
            ),
        }
        Kept::default()
    })
}

/// Write `text` to `path`: whole beside it, then renamed over it.
fn save(path: &Path, text: &str) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)?;
    }
    let mut beside = OsString::from(path);
    beside.push(".new");
    let beside = PathBuf::from(beside);
    // One left by a shell stopped mid-write, made who knows how. One that
    // will not go stops the write below, which will not write over it.
    fs::remove_file(&beside).ok();
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// A folder of the test's own, gone with it.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("perspicax-kept-{name}-{}", std::process::id()));
            fs::remove_dir_all(&path).ok();
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("there").permissions().mode() & 0o777
    }

    #[test]
    fn it_is_written_for_the_person_alone_beside_then_renamed_into_place() {
        let folder = Folder::new("written");
        let path = folder.0.join("perspicax/mixer.json");
        // A write a shell stopped in the middle of.
        fs::create_dir_all(path.parent().expect("a folder")).expect("made");
        fs::write(folder.0.join("perspicax/mixer.json.new"), "{").expect("written");
        let mut keeper = Keeper::load(Some(path.clone()));
        assert_eq!(keeper.kept(), &Kept::default(), "nothing kept yet");
        assert!(keeper.change(|kept| kept.hide("two", true)));
        assert_eq!(mode(&path), 0o600);
        assert!(
            !folder.0.join("perspicax/mixer.json.new").exists(),
            "renamed into place"
        );
        assert!(!keeper.change(|kept| kept.hide("two", true)), "no change");
        let again = Keeper::load(Some(path.clone()));
        assert!(again.kept().hides("two", None), "read back");

        let fresh = Folder::new("fresh");
        let path = fresh.0.join("perspicax/mixer.json");
        Keeper::load(Some(path.clone())).change(|kept| kept.rename("one", "Desk"));
        assert_eq!(
            mode(&fresh.0.join("perspicax")),
            0o700,
            "a folder made theirs"
        );
    }

    #[test]
    fn a_corrupt_file_or_another_versions_starts_afresh_and_is_written_over() {
        let folder = Folder::new("corrupt");
        fs::create_dir_all(&folder.0).expect("made");
        let path = folder.0.join("mixer.json");
        for text in [
            "{\"version\": 1, \"hidden\": [",
            r#"{"version": 2, "hidden": ["two"]}"#,
        ] {
            fs::write(&path, text).expect("written");
            let mut keeper = Keeper::load(Some(path.clone()));
            assert_eq!(keeper.kept(), &Kept::default(), "{text}");
            keeper.change(|kept| kept.hide("one", true));
            let read = Keeper::load(Some(path.clone()));
            assert!(read.kept().hides("one", None), "written over");
            assert!(!read.kept().hides("two", None));
        }
    }

    #[test]
    fn with_nowhere_to_keep_it_is_kept_for_the_session_alone() {
        let mut keeper = Keeper::load(None);
        assert!(keeper.change(|kept| kept.hide("two", true)));
        assert!(keeper.kept().hides("two", None));
    }
}
