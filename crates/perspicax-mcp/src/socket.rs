//! The agent interface on a Unix socket, for an agent that did not start this
//! process.
//!
//! Over stdio the only possible client is whatever started the server, and it
//! gets one conversation. That rules out the agent a person most wants beside
//! them: one running inside the very session it would drive, which cannot start
//! that session, and which is restarted, reconnected and replaced as a matter
//! of course. Issue #30.
//!
//! A socket fixes both. Each connection is one conversation through the same
//! gate stdio goes through, and a connection ending in any way ends that
//! conversation and nothing else. The desktop takes [one conversation at a
//! time](crate::slot), because its deltas are one queue.
//!
//! # Who may connect
//!
//! The file is the owner's alone (0600), and a connection from a process
//! running as anybody else is closed unanswered. The second check is not belt
//! and braces: between `bind` and `chmod` the socket's mode is whatever the
//! umask made it, and the uid is what still holds in that window. Whoever
//! passes both runs as the same user as the programs the agent could drive, so
//! the socket gives nobody anything they could not already do to those
//! programs -- but any of them can drive whatever the agent may.

use std::{
    convert::Infallible,
    fs, io,
    os::unix::{
        fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::{Desktop, ServeError, slot::Slot};

/// How often, while nobody is connected, what has changed is let go of.
const LET_GO: Duration = Duration::from_secs(1);

/// How long a failed `accept` waits before the next. Running out of file
/// descriptors fails every accept until one is freed, and without a pause the
/// loop would spin on it.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// The socket's file, removed when this is dropped.
///
/// Only the file: the listener goes to whichever thread serves it, while this
/// stays with whoever decides when the session is over. A process that is
/// killed runs no destructors, and leaves the file for the next one to find
/// unanswered and replace.
#[derive(Debug)]
pub struct Socket {
    path: PathBuf,
    /// The file this bound, by device and inode, so that dropping removes it
    /// and never a file that has since replaced it.
    file: (u64, u64),
}

/// Why a socket could not be served.
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    /// Something is answering on the path already: very likely another
    /// perspicax, and not one to take it from.
    #[error(
        "{} is already being served{}, so this perspicax will not take it over",
        path.display(),
        pid.map(|pid| format!(" by pid {pid}")).unwrap_or_default()
    )]
    Live {
        path: PathBuf,
        /// The process answering, when the system can say.
        pid: Option<u32>,
    },
    /// The path is something that is not a socket, which is nobody's stale
    /// socket and not this process's to remove.
    #[error("{} exists and is not a socket, so it is left alone", path.display())]
    NotASocket { path: PathBuf },
    /// The filesystem said no.
    #[error("could not serve the agent interface at {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl Socket {
    /// Listen at `path`, made absolute, readable and writable by the owner only.
    ///
    /// A socket already there that nothing answers on is one a session left
    /// behind, and is replaced. One that answers is refused, naming who
    /// answered. Anything else at the path -- a file, a directory, a link, a
    /// socket this process may not even probe -- is left exactly as it is.
    ///
    /// The listener comes back non-blocking, ready for
    /// [`serve_socket`], and by value: a clone would share the non-blocking
    /// flag with it anyway, and only one thread ever accepts.
    ///
    /// Two processes binding one path in the same instant could both find it
    /// stale. The loser's file is replaced under it, and the device-and-inode
    /// check keeps the loser from removing the winner's when it goes.
    ///
    /// # Errors
    ///
    /// [`SocketError`], as above.
    pub fn bind(path: impl AsRef<Path>) -> Result<(Self, UnixListener), SocketError> {
        let asked = path.as_ref();
        let path = std::path::absolute(asked).map_err(|source| SocketError::Io {
            path: asked.to_owned(),
            source,
        })?;
        make_way(&path)?;

        let failed = |source| SocketError::Io {
            path: path.clone(),
            source,
        };
        let listener = UnixListener::bind(&path).map_err(failed)?;
        let bound = fs::symlink_metadata(&path).map_err(failed)?;
        // From here on, a failure removes what was bound.
        let socket = Self {
            file: (bound.dev(), bound.ino()),
            path,
        };
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))
            .map_err(|source| socket.failed(source))?;
        listener
            .set_nonblocking(true)
            .map_err(|source| socket.failed(source))?;
        Ok((socket, listener))
    }

    /// Where the socket is, absolute.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn failed(&self, source: io::Error) -> SocketError {
        SocketError::Io {
            path: self.path.clone(),
            source,
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        match fs::symlink_metadata(&self.path) {
            Ok(found) if (found.dev(), found.ino()) == self.file => {
                if let Err(error) = fs::remove_file(&self.path) {
                    tracing::warn!(%error, path = %self.path.display(), "could not remove the agent socket");
                }
            }
            Ok(_) => tracing::debug!(
                path = %self.path.display(),
                "the agent socket was replaced, and is left to whoever replaced it"
            ),
            Err(_) => {}
        }
    }
}

/// Serve the eight tools on `listener`, a conversation per connection and one
/// at a time, for as long as the process runs.
///
/// A connection ending, however it ends, ends its conversation and nothing
/// else; the next connection is served as the first was. While nobody holds a
/// conversation, what changes on the desktop is let go of rather than kept for
/// nobody, and when one opens its deltas begin with it.
///
/// Call it under a runtime with IO and time enabled: the listener is taken into
/// that runtime here.
///
/// # Errors
///
/// [`ServeError::Transport`] if the listener cannot be served at all. Nothing a
/// client does returns from this.
pub async fn serve_socket(
    desktop: Arc<dyn Desktop>,
    listener: UnixListener,
) -> Result<Infallible, ServeError> {
    let listener = tokio::net::UnixListener::from_std(listener)
        .map_err(|error| ServeError::Transport(error.to_string()))?;
    let slot = Slot::new(Arc::clone(&desktop));
    let owner = rustix::process::geteuid().as_raw();
    let mut idle = tokio::time::interval(LET_GO);
    idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => admit(stream, &desktop, &slot, owner),
                Err(error) => {
                    tracing::warn!(%error, "could not accept a connection to the agent interface");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
            _ = idle.tick() => slot.let_go_if_idle(),
        }
    }
}

/// Serve one connection's conversation, if it comes from the socket's owner.
fn admit(stream: tokio::net::UnixStream, desktop: &Arc<dyn Desktop>, slot: &Slot, owner: u32) {
    let peer = match stream.peer_cred() {
        Ok(peer) => peer,
        Err(error) => {
            tracing::warn!(%error, "closed a connection that would not say who made it");
            return;
        }
    };
    let pid = peer.pid().and_then(|pid| u32::try_from(pid).ok());
    // See the module documentation: required, not a second opinion.
    if peer.uid() != owner {
        tracing::warn!(
            uid = peer.uid(),
            ?pid,
            "closed a connection from another user"
        );
        return;
    }

    let conversation = crate::serve_conversation(Arc::clone(desktop), stream, slot.clone(), pid);
    tokio::spawn(async move {
        match conversation.await {
            Ok(()) => tracing::info!(?pid, "an agent's connection ended"),
            Err(error) => tracing::warn!(?pid, "an agent's conversation ended badly: {error}"),
        }
    });
}

/// Clear `path` for a new socket, or say why it cannot be.
fn make_way(path: &Path) -> Result<(), SocketError> {
    let failed = |source| SocketError::Io {
        path: path.to_owned(),
        source,
    };
    match fs::symlink_metadata(path) {
        Ok(found) if found.file_type().is_socket() => match UnixStream::connect(path) {
            Ok(answered) => Err(SocketError::Live {
                path: path.to_owned(),
                pid: peer_pid(&answered),
            }),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                tracing::info!(path = %path.display(), "replacing a socket nothing answers on");
                remove_if_present(path).map_err(failed)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failed(error)),
        },
        Ok(_) => Err(SocketError::NotASocket {
            path: path.to_owned(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failed(error)),
    }
}

/// The process on the other end of `stream`, when the system can say.
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        rustix::net::sockopt::socket_peercred(stream)
            .ok()
            .and_then(|peer| u32::try_from(peer.pid.as_raw_nonzero().get()).ok())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = stream;
        None
    }
}

/// Remove `path`, which somebody else may have removed first.
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
