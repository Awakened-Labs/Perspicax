//! The disk, as the menus and the taskbar read it.
//!
//! Through a trait, so that a test of which applications a menu lists, or
//! which icon it finds, says in the test itself which files it relies on,
//! rather than in a folder of fixtures somewhere else.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// What the shell asks of a filesystem.
pub(crate) trait Fs {
    /// A text file's contents, or `None` for one that is missing, cannot be
    /// read, or is not UTF-8.
    fn read(&self, path: &Path) -> Option<String>;
    /// What a directory holds, by name, each with whether it is a directory
    /// itself. Nothing for a directory that is missing or cannot be read.
    fn list(&self, dir: &Path) -> Vec<(OsString, bool)>;
    /// Whether a file is there.
    fn is_file(&self, path: &Path) -> bool;
    /// Whether a file is there and may be run.
    fn is_executable(&self, path: &Path) -> bool;
}

/// The real one.
pub(crate) struct Disk;

impl Fs for Disk {
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn list(&self, dir: &Path) -> Vec<(OsString, bool)> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| {
                // Followed, so a symlinked folder of entries is read too.
                let is_dir = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
                (entry.file_name(), is_dir)
            })
            .collect()
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_executable(&self, path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
}

/// Where a program named `name` would be found and run from: itself if it
/// is a path, otherwise the first folder of `path` that holds it.
pub(crate) fn which(fs: &impl Fs, name: &str, path: &[PathBuf]) -> Option<PathBuf> {
    if name.contains('/') {
        let named = PathBuf::from(name);
        return fs.is_executable(&named).then_some(named);
    }
    path.iter()
        .map(|dir| dir.join(name))
        .find(|candidate| fs.is_executable(candidate))
}

/// Files held in memory, for tests to describe a disk with.
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    #[derive(Default)]
    pub(crate) struct Files {
        text: BTreeMap<PathBuf, String>,
        executable: BTreeSet<PathBuf>,
    }

    impl Files {
        /// A file at `path` holding `text`.
        pub(crate) fn with(mut self, path: &str, text: &str) -> Self {
            self.text.insert(path.into(), text.to_owned());
            self
        }

        /// A program at `path`.
        pub(crate) fn program(mut self, path: &str) -> Self {
            self.executable.insert(path.into());
            self.with(path, "")
        }
    }

    impl Fs for Files {
        fn read(&self, path: &Path) -> Option<String> {
            self.text.get(path).cloned()
        }

        fn list(&self, dir: &Path) -> Vec<(OsString, bool)> {
            let mut found = BTreeMap::new();
            for path in self.text.keys() {
                let Ok(rest) = path.strip_prefix(dir) else {
                    continue;
                };
                let mut parts = rest.components();
                if let Some(first) = parts.next() {
                    let is_dir = parts.next().is_some();
                    *found.entry(first.as_os_str().to_owned()).or_insert(false) |= is_dir;
                }
            }
            found.into_iter().collect()
        }

        fn is_file(&self, path: &Path) -> bool {
            self.text.contains_key(path)
        }

        fn is_executable(&self, path: &Path) -> bool {
            self.executable.contains(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fake::Files, *};

    #[test]
    fn a_program_is_found_on_the_path_in_order() {
        let files = Files::default()
            .program("/usr/local/bin/foot")
            .program("/usr/bin/foot")
            .with("/usr/bin/readme", "");
        let path = [PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")];
        assert_eq!(
            which(&files, "foot", &path),
            Some(PathBuf::from("/usr/local/bin/foot"))
        );
        assert_eq!(which(&files, "readme", &path), None, "not executable");
        assert_eq!(
            which(&files, "/usr/bin/foot", &[]),
            Some(PathBuf::from("/usr/bin/foot")),
            "a path is itself"
        );
    }
}
