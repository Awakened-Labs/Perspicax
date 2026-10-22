//! perspicax-shell: the desktop a person logs in to.
//!
//! A wallpaper on every monitor, a menu of the installed applications on a
//! right-click, a panel with a start menu, a taskbar, a pager, other
//! programs' status icons, the sound and a clock, and the desktop folder's
//! icons: each a cargo feature, and each a key in the `[shell]` table of
//! perspicax's own config file. Icons drawn in SVG, as most icon themes
//! draw them, are a feature of their own, `svg`.
//!
//! perspicax starts it, but it is an ordinary Wayland client: its surfaces
//! are layer-shell surfaces, its taskbar speaks
//! wlr-foreign-toplevel-management, and what it needs from perspicax alone
//! comes over `perspicax-shell-v1`: being told that the person asked for a
//! menu, and what is made of each window, and asking for a window to be made
//! sticky, kept on top, rolled up or sent to a workspace. A compositor
//! without that channel still gets a desktop, with no menu on a key, and a
//! window's menu of only what the taskbar's protocol can ask.
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
#[cfg(feature = "audio")]
mod audio;
#[cfg(feature = "menus")]
mod launch;
#[cfg(any(feature = "menus", feature = "panel"))]
mod layout;
#[cfg(feature = "audio")]
mod mixer;
#[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
mod model;
#[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
mod paint;
#[cfg(feature = "pie")]
mod pie;
#[cfg(feature = "tray")]
mod tray;
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
    pie: cfg!(feature = "pie"),
    audio: cfg!(feature = "audio"),
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
    /// The D-Bus bus the tray finds other programs' status icons on, by
    /// its address. `None` is the session bus, the one
    /// `DBUS_SESSION_BUS_ADDRESS` names, as for any program. Unused by a
    /// shell built without `tray`; what agents read is on the session's
    /// accessibility bus whatever this says.
    pub session_bus: Option<String>,
    /// The sound server's socket, which the panel's mixer reads and
    /// changes. `None` is the one the environment names, as libpulse finds
    /// it: `PULSE_SERVER`, or `native` in the runtime folder,
    /// `$PULSE_RUNTIME_PATH` or `$XDG_RUNTIME_DIR/pulse`. Unused by a shell
    /// built without `audio`.
    pub sound_server: Option<PathBuf>,
    /// The file the mixer keeps what the person hid and renamed in. `None`
    /// is `mixer.json` in perspicax's state folder,
    /// `$XDG_STATE_HOME/perspicax`. Unused by a shell built without `audio`.
    pub state: Option<PathBuf>,
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
    /// The connection to the compositor broke. Either the compositor ended,
    /// or it cut the shell off and runs on; from here the two look alike.
    #[error("lost the connection to the compositor: {0}")]
    Lost(std::io::Error),
    /// Talking to the compositor failed for another reason.
    #[error("the connection to the compositor failed: {0}")]
    Wayland(String),
}

impl Error {
    /// The exit status for this error. 78 is `EX_CONFIG` from sysexits.h:
    /// a config the shell cannot use, which perspicax answers by waiting
    /// for the file to change rather than starting the shell again at once.
    /// 69 is `EX_UNAVAILABLE`: a lost connection, which perspicax, if it is
    /// still there to read the status, answers as it does a crash.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Config(_) => 78,
            Self::Lost(_) => 69,
            _ => 1,
        }
    }
}

/// Run the shell until its connection to the compositor ends.
///
/// # Errors
///
/// [`Error::Config`] for a config file the shell cannot use, which it reads
/// before connecting to anything. [`Error::Lost`] when the connection
/// breaks, whether the compositor ended or cut the shell off: the shell
/// cannot tell which, and leaves that to whoever reads its exit status.
/// Otherwise whatever else stopped it. Nothing asks the shell to stop yet,
/// and `Ok` is kept for a stop that was asked for.
pub fn run(options: Options) -> Result<(), Error> {
    let path = options.config.or_else(perspicax_config::default_path);
    let shell = read(path.as_deref())?;
    let connection = match options.connection {
        Some(connection) => connection,
        None => Connection::connect_to_env()?,
    };
    let state = options.state.or_else(|| {
        perspicax_config::state_folder(|name| std::env::var(name).ok())
            .map(|folder| folder.join("mixer.json"))
    });
    wl::run(
        connection,
        shell,
        path,
        options.session_bus,
        options.sound_server,
        state,
    )
}

/// Read the config file as [`run`] would, and go no further: no compositor,
/// nothing drawn. `perspicax config check` runs `perspicax-shell --check` so
/// that a file is judged by the shell that is installed, with the components
/// it was built with, in its own words.
///
/// # Errors
///
/// [`Error::Config`] for a config file this build cannot use.
pub fn check(config: Option<PathBuf>) -> Result<Shell, Error> {
    let path = config.or_else(perspicax_config::default_path);
    Ok(read(path.as_deref())?)
}

/// The `[shell]` table of the config file at `path`, as this build reads it,
/// or the classic profile's with no file to read.
fn read(path: Option<&Path>) -> Result<Shell, perspicax_config::Error> {
    match path {
        Some(path) => perspicax_config::load_shell(path, BUILT),
        None => Ok(Shell::profile(Profile::Classic, BUILT)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checking_reads_the_file_as_this_build_does() {
        let path =
            std::env::temp_dir().join(format!("perspicax-shell-check-{}.toml", std::process::id()));
        std::fs::write(&path, "[shell]\nroot-menu = true\n").expect("a config");
        let checked = check(Some(path.clone()));
        std::fs::remove_file(&path).ok();
        // A root menu is the `menus` component's: taken by a shell with it,
        // and refused by one without, naming the key.
        assert_eq!(checked.is_ok(), cfg!(feature = "menus"), "{checked:?}");
        if let Err(error) = checked {
            assert_eq!(error.exit_code(), 78);
            assert!(error.to_string().contains("`shell.root-menu`"), "{error}");
        }
    }
}
