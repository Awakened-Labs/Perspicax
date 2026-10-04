//! perspicax-shell: the desktop a person logs in to.
//!
//! A wallpaper on every monitor, a menu of the installed applications on a
//! right-click, and a panel with a start menu and a clock, to be joined by a
//! taskbar, a tray and desktop icons: each a cargo feature, and each a key
//! in the `[shell]` table of perspicax's own config file. perspicax starts
//! it, but it is an ordinary Wayland client: its surfaces are layer-shell
//! surfaces, its taskbar speaks wlr-foreign-toplevel-management, and the one
//! thing it needs from perspicax alone, being told that the person asked for
//! a menu, comes over `perspicax-shell-v1`. A compositor without that
//! channel still gets a desktop, with no menu on a key.
//!
//! The shell is drawn in software: tiny-skia paints into memory, and the
//! picture is copied into a `wl_shm` buffer. A desktop changes rarely, and
//! software draws it on any machine, under any compositor, in CI.
//!
//! Everything it draws is also described on the accessibility bus, through
//! AccessKit, one tree per surface, so an agent reads the shell as it reads
//! any application.
//!
//! [`run`] is the whole shell. The binary calls it with the person's session;
//! the compositor's live tests call it on a thread, with a connection to a
//! compositor of their own.

#[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
mod a11y;
#[cfg(feature = "menus")]
mod launch;
#[cfg(any(feature = "menus", feature = "panel"))]
mod layout;
#[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
mod model;
#[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
mod paint;
#[cfg(feature = "menus")]
mod update;
mod wl;

use std::path::{Path, PathBuf};

use perspicax_config::{Profile, Shell, ShellBuilt};
use wayland_client::Connection;

/// What this shell was built with, which decides what its config may ask
/// for.
pub const BUILT: ShellBuilt = ShellBuilt {
    wallpaper: cfg!(feature = "wallpaper"),
    panel: cfg!(feature = "panel"),
    menus: cfg!(feature = "menus"),
    tray: cfg!(feature = "tray"),
    icons: cfg!(feature = "icons"),
};

/// How to run.
#[derive(Debug, Default)]
pub struct Options {
    /// The compositor to draw on. `None` is the one `WAYLAND_DISPLAY` names,
    /// as for any Wayland client.
    pub connection: Option<Connection>,
    /// The config file. `None` is perspicax's own,
    /// `$XDG_CONFIG_HOME/perspicax/config.toml`.
    pub config: Option<PathBuf>,
}

/// Why the shell stopped.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The config file could not be used.
    #[error("config: {0}")]
    Config(#[from] perspicax_config::Error),
    /// No compositor to connect to.
    #[error("could not connect to a Wayland compositor: {0}")]
    Connect(#[from] wayland_client::ConnectError),
    /// The compositor lacks a protocol the shell cannot do without.
    #[error("the compositor does not offer {0}, which the shell draws with")]
    Missing(&'static str),
    /// The compositor ended the connection over a request it refused.
    #[error("the compositor refused a request: {0}")]
    Protocol(wayland_client::backend::protocol::ProtocolError),
    /// Talking to the compositor failed for another reason.
    #[error("the connection to the compositor failed: {0}")]
    Wayland(String),
}

impl Error {
    /// The exit status for this error. 78 is `EX_CONFIG` from sysexits.h:
    /// a config the shell cannot use, which perspicax answers by waiting
    /// for the file to change rather than starting the shell again at once.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Config(_) => 78,
            _ => 1,
        }
    }
}

/// Run the shell until the compositor goes away.
///
/// # Errors
///
/// [`Error::Config`] for a config file the shell cannot use, which it reads
/// before connecting to anything; otherwise whatever stopped it short of the
/// compositor ending the session.
pub fn run(options: Options) -> Result<(), Error> {
    let path = options.config.or_else(perspicax_config::default_path);
    let shell = read(path.as_deref())?;
    let connection = match options.connection {
        Some(connection) => connection,
        None => Connection::connect_to_env()?,
    };
    wl::run(connection, shell, path)
}

/// The `[shell]` table of the config file at `path`, as this build reads it,
/// or the classic profile's with no file to read.
fn read(path: Option<&Path>) -> Result<Shell, perspicax_config::Error> {
    match path {
        Some(path) => perspicax_config::load_shell(path, BUILT),
        None => Ok(Shell::profile(Profile::Classic, BUILT)),
    }
}
