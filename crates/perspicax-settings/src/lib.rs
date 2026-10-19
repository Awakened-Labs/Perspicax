//! The settings app's writer: `config.toml` edited in place, so that the app
//! and a text editor can be used turn and turn about on the same file.
//!
//! Four rules shape it.
//!
//! **The file stays the person's.** A write changes the lines it must and no
//! other: every comment, blank, alignment and order survives it. A key the
//! file already has is changed where it stands, and keeps the comment beside
//! it. A key it does not have fills the file's own commented-out example
//! where there is one (`# border = 2`), and a table its commented-out header
//! (`# [splash]`), each where it stands. `[keys]` entries are found by the
//! chord they parse as, not by their text, so capturing Logo+a changes an
//! existing `"Super+a"` line and keeps its spelling.
//!
//! **Only what differs is written.** A key that is not written, set to the
//! value already in force, is not added: the file goes on following its
//! profile there. A key that is written stays written, even at the profile's
//! value, until it is [reset](Edit::Reset).
//!
//! **Only a file the session takes is written.** A candidate is read here
//! first, for its shape and its key names, with every feature on; then it is
//! put beside the file and the [`Judge`] is asked, which for the app is the
//! installed perspicax ([`Installed`]), the same two programs a session
//! starts, each as it was built. A refusal writes nothing, says why, and
//! offers each [`Fix`] that would be taken.
//!
//! **A write is a rename.** The candidate is written to
//! `.config.toml.perspicax-settings-<pid>` beside the file, then renamed over
//! it, so the session's watch, which listens for `config.toml` by name, only
//! ever hears a whole file arrive. If another program wrote the file since it
//! was read, the edits are made again on top of what it wrote.

mod edit;
mod file;
mod fix;
mod judge;
mod layout;

use std::{
    io,
    path::{Path, PathBuf},
};

use perspicax_config::{Spelled, Trigger};

pub use crate::judge::{Installed, Judge};

/// How many times a write is made again over a file another program keeps
/// rewriting, before it gives up.
const ATTEMPTS: usize = 3;

/// Where a value lives: its table's path, then its own name. Never split on
/// `.`, because a chord can hold one (`"Logo+."`), and so can an
/// application's id (`org.gnome.Nautilus`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub table: Vec<String>,
    pub leaf: String,
}

impl Key {
    /// `leaf` in the table at `table`; `&[]` for one at the top of the file.
    #[must_use]
    pub fn new(table: &[&str], leaf: &str) -> Self {
        Self {
            table: table.iter().map(|seg| (*seg).to_owned()).collect(),
            leaf: leaf.to_owned(),
        }
    }

    fn path(&self) -> Vec<&str> {
        let mut path: Vec<&str> = self.table.iter().map(String::as_str).collect();
        path.push(&self.leaf);
        path
    }
}

/// A value a [`Set`](Edit::Set) writes.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Strings(Vec<String>),
}

/// One change to the file.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Write `value` at `key`.
    Set { key: Key, value: Value },
    /// Back to the profile's value: the key's lines go, and a table left
    /// empty goes with them. A key that names a table resets all of it.
    Reset { key: Key },
    /// Bind `trigger` to `action` in `[keys]`; `None` writes `"none"`, which
    /// unbinds a chord the profile binds.
    Bind {
        trigger: Trigger,
        action: Option<Spelled>,
    },
    /// Back to the profile's binding for `trigger`: its line goes, however
    /// it was spelled.
    ResetBinding { trigger: Trigger },
}

/// What [`Document::apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The edits were written.
    Written,
    /// The file already said this, so nothing was written.
    AlreadySo,
    /// Another program had written the file since it was read, and the edits
    /// were made again on top of what it wrote.
    Replayed,
}

/// Why nothing was written, and what would be taken instead.
#[derive(Debug, thiserror::Error)]
#[error("{reason}")]
pub struct Refused {
    pub reason: Reason,
    /// The ways out: each one's edits were checked before it was offered.
    /// Empty where there are none to offer.
    pub fixes: Vec<Fix>,
}

impl From<Reason> for Refused {
    fn from(reason: Reason) -> Self {
        Self {
            reason,
            fixes: Vec::new(),
        }
    }
}

/// Why a write was refused.
#[derive(Debug, thiserror::Error)]
pub enum Reason {
    /// The file on disk is not TOML. Nothing is written into it, because
    /// nothing could be without rewriting what the person wrote: it is
    /// theirs to mend, in an editor.
    #[error("the file is not TOML, so nothing was written into it: {0}")]
    NotToml(String),
    /// An edit has no place in the file: a key under a value, or a chord
    /// this config cannot spell yet.
    #[error("{0}")]
    Unwritable(String),
    /// perspicax would refuse the file the edits make, in its own words.
    #[error("{0}")]
    Config(perspicax_config::Error),
    /// The judge refused it, in its own words.
    #[error("{0}")]
    Judged(String),
    /// Another program kept rewriting the file while it was being written.
    #[error("the file kept changing while it was being written, so nothing was")]
    Busy,
    /// The file, or the folder it is in, could not be read or written.
    #[error("{}: {source}", .path.display())]
    Io { path: PathBuf, source: io::Error },
}

/// A way out of a refusal.
#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    /// What it does, as a person is offered it.
    pub says: String,
    /// The edits to apply instead: the refused ones, and this fix's.
    pub edits: Vec<Edit>,
}

/// A config file, as it was last read or written.
#[derive(Debug)]
pub struct Document {
    path: PathBuf,
    text: String,
}

impl Document {
    /// Read the file at `path`. A file that is not there reads as empty, as
    /// a session reads it: the classic profile, with nothing on top.
    ///
    /// # Errors
    ///
    /// A file that is there and cannot be read.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let text = file::read(&path)?;
        Ok(Self { path, text })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's text as it was last read or written, LF. After an
    /// [`apply`](Self::apply), whatever it returned, that is the file as it
    /// is now: one another program rewrote is read again first.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Make `edits` in order and write the file, if it says something new
    /// and both this crate and `judge` take it.
    ///
    /// The file is read again first, and once more after the judge, so a
    /// write never loses what another program wrote meanwhile: the edits are
    /// made again on top of it. A link is written through, beside its
    /// target, so it stays a link.
    ///
    /// # Errors
    ///
    /// [`Refused`], with the reason and any fixes, and the file as it was.
    pub fn apply(&mut self, edits: &[Edit], judge: &impl Judge) -> Result<Applied, Refused> {
        let target = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let mut replayed = false;
        for _ in 0..ATTEMPTS {
            let now = read(&target)?;
            if now != self.text {
                self.text = now;
                replayed = true;
            }
            let candidate = edit::edited(&self.text, edits)?;
            if candidate == self.text {
                return Ok(Applied::AlreadySo);
            }
            if let Err(error) = perspicax_config::parse(&candidate, edit::EVERYTHING) {
                return Err(Refused {
                    fixes: fix::fixes(&self.text, edits, &error),
                    reason: Reason::Config(error),
                });
            }
            let temp = file::Temp::write(&target, &candidate).map_err(io_at(&target))?;
            judge.judge(temp.path()).map_err(Reason::Judged)?;
            // Written while the judge read ours: theirs is the newer, so the
            // edits are made again on top of it.
            if read(&target)? != self.text {
                continue;
            }
            temp.replace(&target).map_err(io_at(&target))?;
            self.text = candidate;
            return Ok(if replayed {
                Applied::Replayed
            } else {
                Applied::Written
            });
        }
        Err(Reason::Busy.into())
    }
}

fn read(path: &Path) -> Result<String, Reason> {
    file::read(path).map_err(io_at(path))
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> Reason {
    let path = path.to_owned();
    move |source| Reason::Io { path, source }
}
