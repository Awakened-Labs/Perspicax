//! The file on disk: read as LF, and replaced whole.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

/// The text of the file at `path`, LF; empty for a file that is not there.
///
/// toml_edit writes every file back LF, so it is read as LF too: the layout
/// then works on the lines it will write, and the app shows the text it
/// does. A CRLF file is therefore written back LF.
pub(crate) fn read(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text.replace("\r\n", "\n")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error),
    }
}

/// A candidate, written beside the file it may replace: in the same
/// directory, so the replacing is a rename, and with a name the session's
/// watch does not listen for. Removed when dropped, unless it replaced the
/// file.
pub(crate) struct Temp {
    path: PathBuf,
}

impl Temp {
    /// Write `text` beside `target`, with `target`'s mode, and to the disk.
    pub(crate) fn write(target: &Path, text: &str) -> io::Result<Self> {
        let dir = target.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        let name = target
            .file_name()
            .ok_or_else(|| io::Error::other("a config file needs a name"))?;
        let path = dir.join(format!(
            ".{}.perspicax-settings-{}",
            name.to_string_lossy(),
            std::process::id()
        ));
        let mut file = match create(&path) {
            // One a write of this pid left when it was killed: the name is
            // ours, so the file is too.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                fs::remove_file(&path)?;
                create(&path)?
            }
            made => made?,
        };
        let temp = Self { path };
        if let Ok(old) = fs::metadata(target) {
            file.set_permissions(old.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(temp)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Rename it over `target`. Dropped after, there is nothing left at its
    /// name to remove.
    pub(crate) fn replace(self, target: &Path) -> io::Result<()> {
        fs::rename(&self.path, target)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        // Gone already after a rename; nothing to say if it is not.
        let _ = fs::remove_file(&self.path);
    }
}

/// A new file at `path`, never one already there, nor through a link.
fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}
