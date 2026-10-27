//! Owning the session, rather than hoping somebody else did.
//!
//! A toolkit only joins the accessibility bus if three separate things are
//! true, and on a desktop all three are arranged by a session manager nobody
//! thinks about. In a container none of them are, and every one of them fails
//! *silently* -- the application starts, draws its window, and simply never
//! appears on the bus.
//!
//! This compositor spawns the applications it hosts, so it is the session
//! manager for them, and arranging this is its job rather than a preamble in
//! whatever script happens to run it:
//!
//! 1. **The registry has to be running before the clients start.** GTK asks
//!    once and gives up; Qt activates it on demand. Start both toolkits at once
//!    against a cold bus and the Qt one arrives, the GTK one does not, and the
//!    symptom looks exactly like an ingest that cannot read GTK. Running for
//!    this session's accessibility bus, too, as that bus says: one serving
//!    another bus serves none of these clients ([`Registry::ensure`]).
//! 2. **`org.a11y.Status.IsEnabled` has to be true.** Qt's bridge gates on it
//!    and GTK's ignores it, so a false flag loses one toolkit and keeps the
//!    other. A desktop sets it from dconf; a fresh `HOME` has no dconf state,
//!    so it defaults to false exactly where it is hardest to notice.
//! 3. **The toolkits have to be told.** `GTK_A11Y`, `QT_ACCESSIBILITY`, and
//!    `QT_LINUX_ACCESSIBILITY_ALWAYS_ON`, which bypasses the flag above.
//!
//! All three were learned the expensive way while measuring M1 and are written
//! down here so that they are a property of the program rather than of a
//! runbook.
//!
//! `--session`, which is what a display manager starts, needs two things more.
//! A session bus, which a display manager that is not systemd's does not give
//! it ([`ensure_bus`]), and whose accessibility bus it stops on the way out
//! ([`stop_accessibility_bus`]). And a log of its own ([`log_path`],
//! [`open_log`]), since its stderr goes wherever the display manager keeps
//! such things.
//!
//! Under `--mcp`, the bus is asked which process serves it ([`bus_daemon`]),
//! since a daemon given the agent's wire would pass it on to every service it
//! starts.
//!
//! And a session may have a bus of its own, which it starts itself
//! ([`own_bus`], [`Bus`]): asked for with `--own-bus`, or under `--mcp` when
//! there is no bus at all. A bus somebody else starts for it holds whatever it
//! was given -- `dbus-run-session` keeps a copy of everything it passes on, the
//! agent's wire included, for the whole run -- where one this process starts
//! is given nothing of it (issue #161). The process runs itself again first,
//! for the bus's address to be in its own environment, the one place every
//! reader of the session bus looks ([`run_again_on_a_bus_of_its_own`]). The
//! bus is kept in a runtime folder of its own, so the accessibility bus it
//! starts cannot take the person's, and it goes when this process goes,
//! however that happens ([`become_bus_daemon`]).

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, BufRead as _},
    os::{
        fd::AsRawFd as _,
        unix::{
            ffi::OsStrExt as _,
            fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
            net::UnixStream,
            process::CommandExt as _,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use perspicax_config::AgentSocket;
use rustix::process::{Pid, Signal, kill_process};
use zbus::{Connection, fdo::DBusProxy, names::WellKnownName};

/// Environment every child of this compositor gets, so that a toolkit which
/// would otherwise decide nobody is listening joins the bus anyway.
///
/// `QT_LINUX_ACCESSIBILITY_ALWAYS_ON` is the load-bearing one: it bypasses the
/// `IsEnabled` check entirely, which matters because that flag lives in dconf
/// and is invisible to anyone debugging with `env`.
#[must_use]
pub fn accessibility_env() -> Vec<(String, String)> {
    [
        ("GTK_A11Y", "atspi"),
        ("QT_ACCESSIBILITY", "1"),
        ("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

/// What this desktop calls itself in `XDG_CURRENT_DESKTOP`: the name an
/// application's `OnlyShowIn`, a portal's `UseIn` and xdg-desktop-portal's
/// choice of `perspicax-portals.conf` are all matched against.
pub const DESKTOP: &str = "perspicax";

/// What a person's session says about itself, to its own programs and to the
/// session bus alike (issue #27).
///
/// `XDG_SESSION_TYPE` is always `wayland`: a session started from a text
/// console inherits `tty` from logind, which stops being true the moment this
/// compositor is running. The two desktop names keep the value the session
/// was started with -- a display manager sets them from the session entry's
/// `DesktopNames`, and a person may have added a second name to borrow another
/// desktop's `OnlyShowIn` entries -- and say [`DESKTOP`] when there is none.
///
/// `inherited` reads the environment; a function rather than the process's
/// own, so the rule can be checked without changing the test's environment.
#[must_use]
pub fn desktop_env(inherited: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let named = |key: &str| {
        inherited(key)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DESKTOP.to_owned())
    };
    vec![
        (
            "XDG_CURRENT_DESKTOP".to_owned(),
            named("XDG_CURRENT_DESKTOP"),
        ),
        (
            "XDG_SESSION_DESKTOP".to_owned(),
            named("XDG_SESSION_DESKTOP"),
        ),
        ("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned()),
    ]
}

/// Where a program inside the session finds the agent interface: the socket
/// `--mcp-socket` serves, which `perspicax attach` connects to (issue #30).
pub const AGENT_SOCKET: &str = "PERSPICAX_MCP_SOCKET";

/// The agent socket's name in `$XDG_RUNTIME_DIR`, where `[agent] socket =
/// true` serves it when `--mcp-socket` names no other place.
pub const SOCKET_NAME: &str = "perspicax-mcp";

/// Where a person's session serves the agent interface, if anywhere: where
/// `--mcp-socket` says, which `flag` is, unless the config's `[agent] socket`
/// says otherwise. The file goes over the flag, because a session a display
/// manager starts is given no flags, and a person who wrote `false` should not
/// have it undone by a script that passes one.
///
/// `var` reads the environment, as [`has_bus`]'s does.
///
/// # Errors
///
/// `true` with no `--mcp-socket` and no runtime folder to serve it in.
pub fn agent_socket(
    flag: Option<&Path>,
    said: &AgentSocket,
    var: impl Fn(&str) -> Option<String>,
) -> Result<Option<PathBuf>> {
    Ok(match said {
        AgentSocket::Unsaid => flag.map(Path::to_owned),
        AgentSocket::Off => None,
        AgentSocket::At(path) => Some(path.clone()),
        AgentSocket::On => match flag {
            Some(flag) => Some(flag.to_owned()),
            None => {
                // The XDG spec's rule: a relative folder is no folder at all.
                let runtime = var("XDG_RUNTIME_DIR")
                    .map(PathBuf::from)
                    .filter(|dir| dir.is_absolute())
                    .context(
                        "[agent] socket = true serves the agent in XDG_RUNTIME_DIR, which is \
                         unset or not a whole path; set it, or write the socket's whole path",
                    )?;
                Some(runtime.join(SOCKET_NAME))
            }
        },
    })
}

/// What every program the session starts is told about the agent interface:
/// where its socket is, or, by an empty value, that this session serves none.
///
/// Empty rather than left out, because what is left out is inherited. A
/// session started from inside another -- a headless run from a terminal on a
/// seat -- would pass the outer session's socket on to its own programs, and an
/// agent among them would drive a desktop other than the one it was started
/// in. The bus is told the same way, as it is told an empty `DISPLAY`.
///
/// # Errors
///
/// A path that is not UTF-8, which the launch environment cannot carry.
pub fn agent_env(socket: Option<&Path>) -> Result<Vec<(String, String)>> {
    let value = match socket {
        Some(path) => path
            .to_str()
            .with_context(|| format!("the agent socket's path is not UTF-8: {}", path.display()))?
            .to_owned(),
        None => String::new(),
    };
    Ok(vec![(AGENT_SOCKET.to_owned(), value)])
}

/// What every program the session starts is told about its config: the file
/// it read, which `perspicax lock` reads too, so a key or swayidle locks in
/// the session's own theme. Empty, as the agent's socket is, when it read
/// none, and for a path that is not UTF-8, which the launch environment
/// cannot carry: `perspicax lock` then reads the default.
#[must_use]
pub fn config_env(config: Option<&Path>) -> Vec<(String, String)> {
    let value = config.and_then(Path::to_str).unwrap_or_default();
    vec![(crate::lock::CONFIG.to_owned(), value.to_owned())]
}

/// Set in the environment of the process `dbus-run-session` starts in this
/// one's place, so that it cannot start another if it still finds no bus.
pub const BUS_STARTED: &str = "PERSPICAX_BUS_STARTED";

/// Whether this session has a bus: one named in `DBUS_SESSION_BUS_ADDRESS`,
/// or one answering at `$XDG_RUNTIME_DIR/bus`, where a systemd user manager
/// keeps the user's.
///
/// The socket is connected to rather than looked at. A bus that has gone
/// leaves its socket behind, and a session that took it for a bus would have
/// none.
///
/// `var` reads the environment; a function rather than the process's own, so
/// the rule can be checked without changing the test's environment.
#[must_use]
pub fn has_bus(var: impl Fn(&str) -> Option<String>) -> bool {
    let var = |name: &str| var(name).filter(|value| !value.is_empty());
    var("DBUS_SESSION_BUS_ADDRESS").is_some()
        || var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .is_some_and(|dir| UnixStream::connect(dir.join("bus")).is_ok())
}

/// Give a session a bus of its own if it was started without one, by
/// replacing this process with `dbus-run-session` running it again, with
/// [`BUS_STARTED`] set.
///
/// Called before anything is logged or started, because when it works nothing
/// after it runs: the process that carries on is the one `dbus-run-session`
/// starts. The bus ends with that process, and takes every service it
/// activated with it, but for the one [`stop_accessibility_bus`] stops.
///
/// # Errors
///
/// Why the session goes on without a bus: `dbus-run-session` could not be run,
/// or it was, and this process, which it started, still has no bus.
pub fn ensure_bus() -> Result<()> {
    if has_bus(|name| env::var(name).ok()) {
        return Ok(());
    }
    if env::var_os(BUS_STARTED).is_some() {
        bail!("dbus-run-session started this session, and it still has no session bus");
    }
    let this = env::current_exe()
        .context("no session bus, and no path to this program to run it under one")?;
    let error = Command::new("dbus-run-session")
        .arg("--")
        .arg(this)
        .args(env::args_os().skip(1))
        .env(BUS_STARTED, "1")
        .exec();
    Err(error).context("no session bus, and dbus-run-session could not be run to start one")
}

/// Set, to this process's own pid, in the environment of the process
/// [`run_again_on_a_bus_of_its_own`] runs in this one's place: the bus its
/// `DBUS_SESSION_BUS_ADDRESS` names is this process's to start.
///
/// A pid rather than a flag, because every program the session starts
/// inherits it, and each of them -- a perspicax among them -- has a pid of its
/// own, so none takes the bus for one it has to start.
pub const OWN_BUS: &str = "PERSPICAX_OWN_BUS";

/// Set beside [`OWN_BUS`] to the bus the session was started on, so that its
/// daemon can still be asked whether it holds the agent's wire:
/// `dbus-run-session -- perspicax --mcp --own-bus` hands that daemon the wire
/// as surely as without the flag. Empty when there was none, rather than left
/// out, since what is left out is inherited.
pub const OUTER_BUS: &str = "PERSPICAX_OUTER_BUS";

/// Whether a session is to have a bus of its own, and the folder it is kept
/// in ([`own_bus`]).
#[derive(Debug, PartialEq, Eq)]
pub enum OwnBus {
    /// Not asked for: the session's bus, if it has one, is the one it was
    /// given.
    Unasked,
    /// Asked for: this process is to run itself again with the bus's address
    /// in its environment ([`run_again_on_a_bus_of_its_own`]).
    Asked(PathBuf),
    /// This process was run again for it, and starts it.
    Due(PathBuf),
}

/// The longest path of a socket dbus-daemon will listen on: its own limit, a
/// few bytes inside the kernel's, so that the same address serves on every
/// system it is built for.
const SOCKET_PATH_MAX: usize = 99;

/// Whether a session is to have a bus of its own (issue #161): asked for with
/// `--own-bus`, which `asked` is, or under `--mcp`, which `mcp` is, by a session
/// with no bus at all ([`has_bus`]). `pid` is this process's, and `var` reads
/// the environment, as [`has_bus`]'s does.
///
/// A bus somebody else starts for perspicax holds whatever it was given.
/// `dbus-run-session` keeps its own copy of every descriptor it passes on, the
/// agent's wire among them, for as long as the run lasts, and any process
/// running as the same user can reopen that copy through its `/proc`. A bus
/// perspicax starts is given nothing of the wire, and nothing else holds it.
///
/// It is kept in a folder of its own in `$XDG_RUNTIME_DIR`, which is the
/// person's alone, named for this process. Under `--mcp`, a session with
/// nowhere to keep one goes on without a bus, as it always has: only the flag
/// is refused.
///
/// # Errors
///
/// `--own-bus` with no runtime folder, or one whose path leaves no room for
/// the bus's sockets.
pub fn own_bus(
    asked: bool,
    mcp: bool,
    var: impl Fn(&str) -> Option<String>,
    pid: u32,
) -> Result<OwnBus> {
    let due = var(OWN_BUS).is_some_and(|owner| owner == pid.to_string());
    let wanted = asked || mcp && !has_bus(&var);
    if !(due || wanted) {
        return Ok(OwnBus::Unasked);
    }
    // The XDG spec's rule: a relative folder is no folder at all.
    let folder = var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(format!("perspicax-bus-{pid}")));
    // Room for the longest socket the folder holds, the accessibility bus's,
    // which `at-spi-bus-launcher` serves in the runtime folder it is given; and
    // a name an address carries as it is, since zbus reads an escaped one as
    // it is written, and would look for a socket nobody serves.
    let fits = |folder: &Path| {
        folder.join("at-spi/bus").as_os_str().len() <= SOCKET_PATH_MAX
            && folder.as_os_str().as_bytes().iter().all(|&byte| {
                matches!(byte, b'-' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'/' | b'.')
            })
    };
    Ok(match folder {
        Some(folder) if fits(&folder) && due => OwnBus::Due(folder),
        Some(folder) if fits(&folder) => OwnBus::Asked(folder),
        _ if !asked && !due => OwnBus::Unasked,
        Some(folder) => bail!(
            "a bus of its own is kept in {}, too long a path for its sockets, or one a bus \
             address would have to escape; set XDG_RUNTIME_DIR to a short, plain one",
            folder.display()
        ),
        None => bail!(
            "--own-bus keeps its bus in XDG_RUNTIME_DIR, which is unset or not a whole path; \
             set it"
        ),
    })
}

/// The address of the bus kept in `folder`, one [`own_bus`] chose: its socket,
/// `bus`, in a name that needs no escaping.
fn bus_address(folder: &Path) -> String {
    format!("unix:path={}", folder.join("bus").display())
}

/// Variables a process is started with in place of the ones it would inherit:
/// each set to its value, or removed where there is none.
type Changes = Vec<(&'static str, Option<OsString>)>;

/// Start `command` with `changes` made to what it inherits.
fn changed(command: &mut Command, changes: Changes) -> &mut Command {
    for (name, value) in changes {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command
}

/// What this process is run again with, for a bus of its own in `folder`
/// ([`run_again_on_a_bus_of_its_own`]): the bus's address, and [`OWN_BUS`] and
/// [`OUTER_BUS`] to say whose it is and which it replaces. `var` reads the
/// environment, as [`has_bus`]'s does.
fn own_bus_env(folder: &Path, pid: u32, var: impl Fn(&str) -> Option<String>) -> Changes {
    vec![
        ("DBUS_SESSION_BUS_ADDRESS", Some(bus_address(folder).into())),
        (OWN_BUS, Some(pid.to_string().into())),
        (
            OUTER_BUS,
            Some(var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default().into()),
        ),
        // What `dbus-run-session` takes away from the program it starts, each
        // naming something of a bus other than its own.
        ("DBUS_SESSION_BUS_PID", None),
        ("DBUS_SESSION_BUS_WINDOWID", None),
        ("DBUS_STARTER_ADDRESS", None),
        ("DBUS_STARTER_BUS_TYPE", None),
        // And the accessibility bus, which a toolkit takes from here before it
        // asks the session bus for it: the person's, not this session's.
        ("AT_SPI_BUS_ADDRESS", None),
    ]
}

/// Run this process again for a bus of its own in `folder`, with that bus's
/// address in its environment, and [`OWN_BUS`] and [`OUTER_BUS`] beside it.
/// The bus itself is started by the process that carries on, which finds it
/// [`OwnBus::Due`].
///
/// Run again rather than started here, because the environment is the one
/// place every reader of the session bus looks -- zbus, the accessibility
/// libraries, and every program the session starts -- and a process may not
/// change its own while anything could be reading it. A bus address passed by
/// hand instead would have to reach each of them, and one that was missed
/// would quietly use the person's bus.
///
/// Called before anything is logged or started, as [`ensure_bus`] is, and for
/// the same reason. Nothing changes but the environment: the same pid, the same
/// parent, and the same descriptors, the agent's wire among them, still on fds
/// 0 and 1 for the process that carries on to take.
///
/// Returns only when it could not be done, saying why.
pub fn run_again_on_a_bus_of_its_own(folder: &Path) -> anyhow::Error {
    let this = match env::current_exe() {
        Ok(this) => this,
        Err(error) => {
            return anyhow::Error::from(error)
                .context("no path to this program to run it again on a bus of its own");
        }
    };
    let error = changed(
        Command::new(this).args(env::args_os().skip(1)),
        own_bus_env(folder, std::process::id(), |name| env::var(name).ok()),
    )
    .exec();
    anyhow::Error::from(error).context("could not run this program again on a bus of its own")
}

/// What the daemon of a bus of the session's own is started with in place of
/// what it would inherit, and so what every service it starts on request
/// inherits too: `folder` for its runtime folder, and no display.
///
/// The accessibility bus is why. `at-spi-bus-launcher` serves it at
/// `$XDG_RUNTIME_DIR/at-spi/bus`, and a `dbus-daemon` binding a socket first
/// removes any socket already at its path. Given the person's runtime folder,
/// the launcher this bus starts would take the socket of the person's own
/// accessibility bus from under it, and delete it on the way out; given a
/// `DISPLAY`, it would write its address on that display's root window, over
/// theirs. Nor should anything this bus starts find the person's desktop to
/// draw on, which their `WAYLAND_DISPLAY` would be. A seat tells the bus its
/// own displays once they are up ([`crate::bus`]).
fn daemon_env(folder: &Path) -> Changes {
    vec![
        ("XDG_RUNTIME_DIR", Some(folder.into())),
        ("DISPLAY", None),
        ("WAYLAND_DISPLAY", None),
    ]
}

/// How long a bus of the session's own has to stop before it is killed. It
/// takes milliseconds.
const OWN_BUS_DEADLINE: Duration = Duration::from_secs(1);

/// A bus of the session's own ([`own_bus`]): a `dbus-daemon` this process
/// started, listening in its folder, and stopped when this is dropped or this
/// process ends, however it ends.
#[derive(Debug)]
pub struct Bus {
    daemon: Child,
    folder: PathBuf,
}

impl Bus {
    /// Start the session's own bus, in `folder`, and wait until it is
    /// listening.
    ///
    /// It is started as `perspicax bus-daemon` ([`become_bus_daemon`]), where
    /// `perspicax` is this program, or in a test the one built beside it. That
    /// makes it a daemon the kernel stops when this process ends: perspicax
    /// handles no signal, and an agent's client may well send it one. What the
    /// kernel watches is the thread that started it, so this is called on the
    /// main thread, which lasts as long as the process does.
    ///
    /// The daemon reads nothing and prints to this process's stderr, as every
    /// program the session starts does, and so does every service it starts
    /// on request: none of them is given the agent's wire, nor anything else
    /// of this process's.
    ///
    /// # Errors
    ///
    /// When the folder cannot be made, or is not this user's alone, or when
    /// the daemon cannot be started, or ends before it is listening.
    pub fn start(folder: &Path, perspicax: &Path) -> Result<Self> {
        make_own_folder(folder)?;
        let (said, saying) = io::pipe().context("no pipe for the bus to say its address on")?;
        let mut daemon = {
            let mut command = Command::new(perspicax);
            command
                .arg("bus-daemon")
                .arg(std::process::id().to_string())
                .arg(bus_address(folder));
            changed(&mut command, daemon_env(folder))
                .stdin(Stdio::null())
                .stdout(saying)
                .stderr(Stdio::inherit())
                .spawn()
            // The command goes here, and this process's copy of the pipe's far
            // end with it: the read below ends when the daemon's copy does.
        }
        .with_context(|| format!("could not start {} bus-daemon", perspicax.display()))?;

        // dbus-daemon says its address once it is listening, ending the line.
        // Anything less is a daemon that ended first, or never began.
        let mut line = String::new();
        let read = io::BufReader::new(said).read_line(&mut line);
        if read.is_err() || !line.ends_with('\n') {
            let ended = daemon
                .wait()
                .map_or_else(|error| error.to_string(), |status| status.to_string());
            bail!("the bus perspicax started for itself ended ({ended}) before it was listening");
        }
        tracing::info!(pid = daemon.id(), folder = %folder.display(), "started a bus of its own");
        Ok(Self {
            daemon,
            folder: folder.to_owned(),
        })
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        // Asked first, as the accessibility bus is: a daemon that stops of
        // its own accord removes its socket on the way out.
        let _ = kill_process(Pid::from_child(&self.daemon), Signal::TERM);
        let deadline = Instant::now() + OWN_BUS_DEADLINE;
        while matches!(self.daemon.try_wait(), Ok(None)) {
            if Instant::now() >= deadline {
                tracing::warn!(
                    pid = self.daemon.id(),
                    "the bus of its own was asked to stop, and is still running: killed"
                );
                let _ = self.daemon.kill();
                let _ = self.daemon.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // What is left: the socket of a daemon that was killed, and dconf's
        // note of the settings the bus's services read, each by name; then
        // the folders those services made and emptied on their way out --
        // the accessibility bus's, gvfs's, the daemon's own for services a
        // program installs. Nothing else: one still holding something stays,
        // to say so.
        for file in ["bus", "dconf/user"] {
            let _ = fs::remove_file(self.folder.join(file));
        }
        remove_empty_folders(&self.folder, OWN_BUS_FOLDER_DEPTH);
        if let Err(error) = fs::remove_dir(&self.folder) {
            tracing::debug!(%error, folder = %self.folder.display(), "the bus's folder stays");
        }
    }
}

/// How deep in a bus's folder [`remove_empty_folders`] looks: deeper than any
/// service nests what it makes, and shallow enough that a folder made to be
/// deep costs nothing.
const OWN_BUS_FOLDER_DEPTH: usize = 8;

/// Remove every empty folder in `folder`, `depth` deep, the deepest first, so
/// that one holding only empty folders goes too. `folder` itself stays.
///
/// Only folders, and only empty ones, as `rmdir` removes them: a file, or a
/// link, keeps the folder it is in. Never through a link, nor into another
/// filesystem -- a folder a service mounted there, as the document portal and
/// gvfs mount theirs in the runtime folder they are given, is never looked
/// into, let alone emptied.
fn remove_empty_folders(folder: &Path, depth: usize) {
    let Some(deeper) = depth.checked_sub(1) else {
        return;
    };
    let Ok(here) = fs::symlink_metadata(folder) else {
        return;
    };
    for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if fs::symlink_metadata(&path)
            .is_ok_and(|found| found.is_dir() && found.dev() == here.dev())
        {
            remove_empty_folders(&path, deeper);
            let _ = fs::remove_dir(&path);
        }
    }
}

/// Make `folder`, the person's alone, or take one this user made so, as a
/// session killed with the same pid would have left it.
fn make_own_folder(folder: &Path) -> Result<()> {
    let error = match DirBuilder::new().mode(0o700).create(folder) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if error.kind() == io::ErrorKind::AlreadyExists
        && fs::symlink_metadata(folder).is_ok_and(|found| {
            found.is_dir()
                && found.uid() == rustix::process::getuid().as_raw()
                && found.permissions().mode() & 0o777 == 0o700
        })
    {
        return Ok(());
    }
    Err(error).with_context(|| {
        format!(
            "could not make {} for a bus of its own, the person's alone",
            folder.display()
        )
    })
}

/// Become the daemon of a bus of a session's own, listening on `address`, for
/// `parent`, the session that started this process ([`Bus::start`]): what
/// `perspicax bus-daemon` does.
///
/// A step on the way rather than a program of its own. It asks the kernel to
/// stop this process when `parent` ends, which only a process can ask for
/// itself, and then becomes `dbus-daemon`, which keeps what was asked. One
/// started for a session that has already gone ends at once.
///
/// The daemon says its address once it is listening on what is stdout here,
/// moved to a descriptor of its own that it closes once it has said it. So
/// what is left on stdout is stderr, for every service the bus starts to
/// inherit, as the session's own programs print there.
///
/// Returns only when it could not be done, saying why.
pub fn become_bus_daemon(parent: u32, address: &str) -> anyhow::Error {
    let became = || -> Result<io::Error> {
        end_with(parent)?;
        let saying = rustix::io::fcntl_dupfd_cloexec(io::stdout(), 3)
            .context("could not keep stdout for the bus's address")?;
        rustix::io::fcntl_setfd(&saying, rustix::io::FdFlags::empty())
            .context("could not pass the bus's address on")?;
        rustix::stdio::dup2_stdout(io::stderr()).context("could not print to stderr")?;
        Ok(Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile"])
            .arg(format!("--address={address}"))
            .arg(format!("--print-address={}", saying.as_raw_fd()))
            .exec())
    };
    match became() {
        Ok(error) => anyhow::Error::from(error).context("could not run dbus-daemon"),
        Err(error) => error,
    }
}

/// Become `program`, run with `args`, for `parent`, the session that started
/// this process: what `perspicax ends-with` does. It is how a session starts
/// what would otherwise outlive it, as the [`Registry`] would (issue #249).
///
/// The same step on the way as [`become_bus_daemon`], with no address to
/// pass on: it asks the kernel to stop this process when `parent` ends, and
/// then becomes `program`, which keeps what was asked. One started for a
/// session that has already gone ends at once.
///
/// Returns only when it could not be done, saying why.
pub fn become_ending_with(parent: u32, program: &OsStr, args: &[OsString]) -> anyhow::Error {
    if let Err(error) = end_with(parent) {
        return error;
    }
    let error = Command::new(program).args(args).exec();
    anyhow::Error::from(error).context(format!("could not run {}", program.display()))
}

/// Ask the kernel to stop this process when `parent`, the session that
/// started it, ends, however it ends: the step [`become_bus_daemon`] and
/// [`become_ending_with`] take before they become what they start. Only a
/// process can ask it for itself, and what it asks outlives the exec that
/// follows.
///
/// What the kernel watches is the thread that started this process, not the
/// whole of `parent`: so the session starts it on a thread that lasts as long
/// as it is wanted.
///
/// # Errors
///
/// When it cannot be asked, or `parent` has already gone.
fn end_with(parent: u32) -> Result<()> {
    rustix::process::set_parent_process_death_signal(Some(Signal::TERM))
        .context("could not ask to be stopped with the session")?;
    // Asked too late, if the session has already gone: nothing would stop
    // this then.
    if u32::try_from(Pid::as_raw(rustix::process::getppid())).ok() != Some(parent) {
        bail!("the session that started this (pid {parent}) has gone");
    }
    Ok(())
}

/// The accessibility bus's name on the session bus.
const ACCESSIBILITY_BUS: &str = "org.a11y.Bus";

/// How long the accessibility bus has to stop before the session goes anyway.
/// It takes milliseconds, and a logout held up by one that will not stop would
/// be worse than a daemon left behind.
const ACCESSIBILITY_BUS_DEADLINE: Duration = Duration::from_secs(1);

/// Stop the accessibility bus that the session's own bus started, and say
/// which process served it; `None` when nothing did.
///
/// Everything a bus from [`ensure_bus`] activates ends with it, all but this.
/// `at-spi-bus-launcher` runs a `dbus-daemon` of its own and stops it only on
/// its way out. SDDM, ending the session, hangs up its process group as a
/// terminal would: the launcher dies of the hangup before it has stopped its
/// daemon, and `dbus-daemon` takes a hangup as an order to reread its
/// configuration. So it lived on, one more after every login (H3). Asked with
/// SIGTERM, the launcher stops its daemon and goes.
///
/// Called while `connection`'s bus is still there to say who serves the name,
/// and only on a bus this session started: on one shared with other sessions,
/// the accessibility bus is theirs too.
///
/// # Errors
///
/// When the bus will not say who serves it, the signal cannot be sent, or the
/// name is still served after [`ACCESSIBILITY_BUS_DEADLINE`].
pub async fn stop_accessibility_bus(connection: &Connection) -> Result<Option<u32>> {
    let bus = DBusProxy::new(connection)
        .await
        .context("no word from the session bus")?;
    let name = WellKnownName::from_static_str_unchecked(ACCESSIBILITY_BUS);
    // Asked about by name, which does not start it: a session nobody read has
    // no accessibility bus, and is not given one on its way out.
    let launcher = match bus
        .get_connection_unix_process_id(name.clone().into())
        .await
    {
        Ok(pid) => pid,
        Err(zbus::fdo::Error::NameHasNoOwner(_)) => return Ok(None),
        Err(error) => {
            return Err(error)
                .context("the session bus would not say who serves the accessibility bus");
        }
    };
    let pid = i32::try_from(launcher)
        .ok()
        .and_then(Pid::from_raw)
        .with_context(|| {
            format!("the accessibility bus is served by pid {launcher}, which is no process")
        })?;
    kill_process(pid, Signal::TERM)
        .with_context(|| format!("could not ask the accessibility bus (pid {launcher}) to stop"))?;

    // Waited for, because the hangup is coming: a launcher still stopping its
    // daemon when it arrives is the leak all over again.
    let deadline = Instant::now() + ACCESSIBILITY_BUS_DEADLINE;
    while bus
        .name_has_owner(name.clone().into())
        .await
        .unwrap_or(false)
    {
        if Instant::now() >= deadline {
            bail!("the accessibility bus (pid {launcher}) was asked to stop, and is still running");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(Some(launcher))
}

/// The bus daemon's own name on the bus it serves.
const BUS: &str = "org.freedesktop.DBus";

/// The process serving `connection`'s bus, as the bus itself says: the daemon
/// every service it starts on request is a child of, and inherits its
/// descriptors from (issue #52).
///
/// # Errors
///
/// When the bus will not say.
pub async fn bus_daemon(connection: &Connection) -> Result<u32> {
    DBusProxy::new(connection)
        .await
        .context("no word from the session bus")?
        .get_connection_unix_process_id(WellKnownName::from_static_str_unchecked(BUS).into())
        .await
        .context("the session bus would not say which process serves it")
}

/// Where a session keeps its log: `$XDG_STATE_HOME/perspicax/perspicax.log`,
/// with `~/.local/state` for a state folder that is not set. `None` with
/// neither variable set.
///
/// `var` reads the environment, as [`has_bus`]'s does.
#[must_use]
pub fn log_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    Some(perspicax_config::state_folder(var)?.join("perspicax.log"))
}

/// Where a seat keeps where each program's last window closed, for its next
/// one to open there: `windows.json`, beside the log. State, not config:
/// the person never writes it, as `ennius gui` keeps its window's apart
/// from its `config.toml`.
#[must_use]
pub fn windows_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    Some(perspicax_config::state_folder(var)?.join("windows.json"))
}

/// Open a fresh log at `path`, keeping the last one beside it as
/// `perspicax.log.old`, where the session that went wrong is looked for.
///
/// Only the person can read either, and a folder made for them is theirs alone:
/// a log names the applications run and the windows they opened.
///
/// # Errors
///
/// When the folder cannot be made, or the log not opened, or the last one not
/// kept: a log it would have overwritten is the one that mattered.
pub fn open_log(path: &Path) -> io::Result<File> {
    if let Some(folder) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)?;
    }
    let mut old = OsString::from(path);
    old.push(".old");
    if let Err(error) = fs::rename(path, &old)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// A registry this process started and is responsible for stopping: when
/// this is dropped, or when this process ends, however it ends.
pub struct Registry(Option<Child>);

impl Registry {
    /// Make sure an accessibility registry serves this session's
    /// accessibility bus, starting one if none does, as [`Registry::start`]
    /// does, with `perspicax`.
    ///
    /// Asked of the bus ([`a_registry_serves_this_session`]), not of which
    /// programs are running: one serving the person's desktop, when this
    /// session's bus is one of its own, or one a killed session left, serves
    /// some other bus, and GTK, asking this one once, would give up.
    ///
    /// Returns a handle that stops it again only if we were the ones to start
    /// it: on a real desktop the session's own registry is already up and
    /// serving every other application on the machine, and taking it down on
    /// the way out would be this program breaking somebody else's screen
    /// reader.
    ///
    /// # Errors
    ///
    /// Only if none serves the bus *and* none can be started.
    pub fn ensure(perspicax: &Path) -> Result<Self> {
        if a_registry_serves_this_session() {
            tracing::debug!("an accessibility registry serves this session already");
            return Ok(Self(None));
        }
        Self::start(perspicax).context("no accessibility registry serves this session")
    }

    /// Start an accessibility registry for this session's accessibility bus,
    /// told nothing that would lead it to another ([`registry_env`]).
    ///
    /// It is started as `perspicax ends-with` ([`become_ending_with`]), where
    /// `perspicax` is this program, or in a test the one built beside it, so
    /// that the kernel stops it when this process ends. perspicax handles no
    /// signal, and a registry nobody stops runs on for good, with both its
    /// buses gone (issue #249). What the kernel watches is the thread that
    /// started it, so this is called on one that lasts as long as the
    /// registry is wanted: a session's main thread.
    ///
    /// # Errors
    ///
    /// When none can be started.
    fn start(perspicax: &Path) -> Result<Self> {
        let registry = find_registry(Path::new("/")).with_context(|| {
            format!(
                "there is no {REGISTRY} to start in {}",
                REGISTRY_FOLDERS.join(", ")
            )
        })?;
        let mut command = Command::new(perspicax);
        command
            .arg("ends-with")
            .arg(std::process::id().to_string())
            .arg(&registry);
        // Nothing to read and nowhere to print, as for every program this
        // process starts: under `--mcp` its stdin is an agent's wire (#43).
        let mut child = changed(&mut command, registry_env())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| {
                format!(
                    "could not start {} ends-with {}",
                    perspicax.display(),
                    registry.display()
                )
            })?;
        tracing::info!(pid = child.id(), "started an accessibility registry");

        // Give it a moment to claim its bus name. A client that asks before it
        // has is the exact race this whole type exists to avoid, and polling
        // for the name is not better: the name appears before the registry is
        // ready to answer about children.
        std::thread::sleep(Duration::from_millis(500));
        // Said, since it prints nowhere. A registry that finds its name taken
        // ends at once and well, for one is serving already.
        if let Ok(Some(status)) = child.try_wait()
            && !status.success()
        {
            tracing::warn!(%status, "the accessibility registry ended as it started");
        }
        Ok(Self(Some(child)))
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The registry's program, which is never on `PATH`.
pub const REGISTRY: &str = "at-spi2-registryd";

/// Where distributions put [`REGISTRY`], in the order looked: Fedora's and
/// Gentoo's, Arch's, and older Debian's and Ubuntu's, which some put under a
/// multiarch triplet as well. A `*` is each folder there.
pub const REGISTRY_FOLDERS: [&str; 4] = [
    "/usr/libexec",
    "/usr/lib",
    "/usr/lib/at-spi2-core",
    "/usr/lib/*/at-spi2-core",
];

/// The first [`REGISTRY`] that may be run in [`REGISTRY_FOLDERS`] under
/// `root`, which is `/` but in a test. Triplets are taken in name order, so the
/// choice does not depend on the order a folder happens to list them in.
#[must_use]
pub fn find_registry(root: &Path) -> Option<PathBuf> {
    REGISTRY_FOLDERS
        .iter()
        .map(|folder| folder.trim_start_matches('/'))
        .flat_map(|folder| match folder.split_once("/*/") {
            Some((parent, child)) => {
                let mut triplets: Vec<_> = fs::read_dir(root.join(parent))
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path().join(child))
                    .collect();
                triplets.sort();
                triplets
            }
            None => vec![root.join(folder)],
        })
        .map(|folder| folder.join(REGISTRY))
        .find(|path| {
            path.metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// What the registry is started with in place of what it would inherit: no
/// accessibility bus named, and no X display, so that it finds its bus as
/// every read finds it, by asking the session bus ([`registry_serves`]).
///
/// A registry looks for its bus in `AT_SPI_BUS_ADDRESS` first, and then, given
/// an X display and no Wayland one, in the address that display's root window
/// carries, before it asks the session bus (at-spi2-core 2.56,
/// `atspi_get_a11y_bus`). Started from a terminal on an X desktop, it would
/// join the desktop's accessibility bus, find a registry there already and
/// end, leaving this session's with none.
fn registry_env() -> Changes {
    vec![("AT_SPI_BUS_ADDRESS", None), ("DISPLAY", None)]
}

/// The registry's name on the accessibility bus it serves.
const REGISTRY_NAME: &str = "org.a11y.atspi.Registry";

/// Whether a registry serves the accessibility bus `session` names: the one
/// every read is made on ([`perspicax_atspi::bus_address`]).
///
/// Asked of the buses rather than of the programs running (issue #249). A
/// registry serves one accessibility bus, and a program named for one may
/// serve another: the person's desktop's, or a killed session's, whose
/// registry runs on with its buses gone.
///
/// Asked by name, which starts nothing. An accessibility bus nobody has
/// started has no registry on it, and a registry the bus could start on
/// request has not been started: GTK asks for it once, and gives up.
///
/// # Errors
///
/// When the session bus, or the accessibility bus it names, will not say.
pub async fn registry_serves(session: &Connection) -> Result<bool> {
    let bus = DBusProxy::new(session)
        .await
        .context("no word from the session bus")?;
    let accessibility = WellKnownName::from_static_str_unchecked(ACCESSIBILITY_BUS);
    if !bus
        .name_has_owner(accessibility.into())
        .await
        .context("the session bus would not say whether it serves the accessibility bus")?
    {
        return Ok(false);
    }
    let address = perspicax_atspi::bus_address(session)
        .await
        .context("the session bus would not say where its accessibility bus is")?;
    let accessibility = zbus::connection::Builder::address(address.as_str())
        .with_context(|| format!("the accessibility bus's address is no address: {address}"))?
        .build()
        .await
        .with_context(|| format!("could not reach the accessibility bus at {address}"))?;
    DBusProxy::new(&accessibility)
        .await
        .context("no word from the accessibility bus")?
        .name_has_owner(WellKnownName::from_static_str_unchecked(REGISTRY_NAME).into())
        .await
        .context("the accessibility bus would not say whether a registry serves it")
}

/// Whether a registry serves this session's accessibility bus
/// ([`registry_serves`]), asking the session bus the environment names, as
/// every program the session starts finds it. Briefly borrows a runtime to
/// ask.
///
/// No, when it cannot be asked: with no bus to ask, there is none for a
/// registry to serve, and one started then ends at once of its own accord.
#[must_use]
pub fn a_registry_serves_this_session() -> bool {
    let asked = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("no runtime to ask the session bus with")
        .and_then(|runtime| {
            runtime.block_on(async {
                let session = Connection::session()
                    .await
                    .context("no session bus to ask")?;
                registry_serves(&session).await
            })
        });
    asked.unwrap_or_else(|error| {
        tracing::debug!("{error:#}: so no accessibility registry serves this session");
        false
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    /// A fresh directory for one test, emptied when it ends. Its name is short,
    /// because a socket's path has to fit in 108 bytes.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("perspicax-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("a scratch directory");
            Self(dir)
        }

        /// A file at `path` in it, with `mode`.
        fn file(&self, path: &str, mode: u32) -> PathBuf {
            let file = self.0.join(path);
            fs::create_dir_all(file.parent().expect("a folder")).expect("its folder");
            fs::write(&file, "").expect("a file");
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).expect("its mode");
            file
        }

        /// The environment, as `has_bus` and `log_path` read it: `vars`, with
        /// `{}` in a value standing for this directory.
        fn env(&self, vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
            let vars: Vec<_> = vars
                .iter()
                .map(|&(name, value)| {
                    (
                        name.to_owned(),
                        value.replace("{}", &self.0.to_string_lossy()),
                    )
                })
                .collect();
            move |name| {
                vars.iter()
                    .find(|(var, _)| var == name)
                    .map(|(_, value)| value.clone())
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("it exists").permissions().mode() & 0o777
    }

    #[test]
    fn a_bus_address_is_a_bus_and_an_empty_one_is_not() {
        let scratch = Scratch::new("address");
        assert!(has_bus(
            scratch.env(&[("DBUS_SESSION_BUS_ADDRESS", "unix:path=/x")])
        ));
        assert!(!has_bus(scratch.env(&[("DBUS_SESSION_BUS_ADDRESS", "")])));
        assert!(!has_bus(scratch.env(&[])));
    }

    #[test]
    fn a_bus_is_one_answering_in_the_runtime_folder_and_not_the_socket_it_left() {
        let scratch = Scratch::new("bus");
        let runtime = [("XDG_RUNTIME_DIR", "{}")];
        assert!(!has_bus(scratch.env(&runtime)), "no socket at all");

        let listening = UnixListener::bind(scratch.0.join("bus")).expect("a socket");
        assert!(has_bus(scratch.env(&runtime)));
        // The same folder named relatively is no folder.
        assert!(!has_bus(scratch.env(&[("XDG_RUNTIME_DIR", "run")])));

        drop(listening);
        assert!(scratch.0.join("bus").exists(), "the socket stays behind");
        // Soon, not at once: another test's fork may hold it a moment (see
        // `crate::forked`).
        assert!(crate::forked::soon(|| !has_bus(scratch.env(&runtime))));
    }

    /// This process's pid, as [`own_bus`] is given it, in the tests below.
    const PID: u32 = 4242;

    #[test]
    fn a_bus_of_its_own_is_one_asked_for_or_one_an_agent_has_none_without() {
        let scratch = Scratch::new("own-bus");
        let folder = scratch.0.join("perspicax-bus-4242");
        let own = |asked, mcp, vars: &[(&str, &str)]| {
            own_bus(asked, mcp, scratch.env(vars), PID).expect("a decision")
        };
        let runtime = ("XDG_RUNTIME_DIR", "{}");
        let a_bus = ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/x");

        assert_eq!(own(false, false, &[runtime]), OwnBus::Unasked);
        assert_eq!(own(true, false, &[runtime]), OwnBus::Asked(folder.clone()));
        assert_eq!(
            own(true, true, &[runtime, a_bus]),
            OwnBus::Asked(folder.clone()),
            "asked for, though there is a bus"
        );
        assert_eq!(own(false, true, &[runtime]), OwnBus::Asked(folder.clone()));
        assert_eq!(
            own(false, true, &[runtime, a_bus]),
            OwnBus::Unasked,
            "an agent's session with a bus keeps it"
        );
        assert_eq!(
            own(false, false, &[runtime, a_bus]),
            OwnBus::Unasked,
            "nor does a session with no agent get one unasked"
        );
    }

    #[test]
    fn the_bus_is_due_in_the_process_run_again_for_it_and_no_other() {
        let scratch = Scratch::new("due-bus");
        let folder = scratch.0.join("perspicax-bus-4242");
        let own = |asked, mcp, owner: &str| {
            own_bus(
                asked,
                mcp,
                scratch.env(&[("XDG_RUNTIME_DIR", "{}"), (OWN_BUS, owner)]),
                PID,
            )
            .expect("a decision")
        };
        for (asked, mcp) in [(true, false), (false, true), (false, false)] {
            assert_eq!(
                own(asked, mcp, "4242"),
                OwnBus::Due(folder.clone()),
                "--own-bus {asked}, --mcp {mcp}"
            );
        }
        // A program the session started inherits the marker, and is not the
        // process it names.
        assert_eq!(own(false, false, "17"), OwnBus::Unasked);
        assert_eq!(own(true, false, "17"), OwnBus::Asked(folder));
    }

    #[test]
    fn with_nowhere_to_keep_one_the_flag_is_refused_and_an_agent_goes_on_without() {
        // No folder, one named relatively, and one too long for the
        // accessibility bus's socket inside it.
        let long = format!("/{}", "r".repeat(69));
        for dir in [None, Some("run/user/1000"), Some(long.as_str())] {
            let error = own_bus(true, false, runtime(dir), PID).expect_err("nowhere");
            assert!(error.to_string().contains("XDG_RUNTIME_DIR"), "{error}");
            assert_eq!(
                own_bus(false, true, runtime(dir), PID).expect("no bus, as before"),
                OwnBus::Unasked,
                "XDG_RUNTIME_DIR = {dir:?}"
            );
        }
        // The longest that leaves room: `at-spi/bus` makes it 99 bytes.
        let longest = format!("/{}", "r".repeat(68));
        assert!(matches!(
            own_bus(true, false, runtime(Some(&longest)), PID),
            Ok(OwnBus::Asked(_))
        ));
    }

    #[test]
    fn a_folder_an_address_would_have_to_escape_is_nowhere_to_keep_one() {
        assert_eq!(
            bus_address(Path::new("/run/user/1000/perspicax-bus-42")),
            "unix:path=/run/user/1000/perspicax-bus-42/bus"
        );
        // Each a path zbus would read as written, escapes and all.
        for dir in [
            "/tmp/a folder",
            "/tmp/a,b",
            "/tmp/a=b",
            "/tmp/a%b",
            "/tmp/a;b",
            "/tmp/\u{e9}",
        ] {
            let error = own_bus(true, false, runtime(Some(dir)), PID).expect_err("unescaped");
            assert!(error.to_string().contains("XDG_RUNTIME_DIR"), "{error}");
        }
        assert!(matches!(
            own_bus(true, false, runtime(Some("/run/user/1000/a-b_c.d")), PID),
            Ok(OwnBus::Asked(_))
        ));
    }

    fn changes<'a>(changes: &'a Changes, name: &str) -> Option<Option<&'a str>> {
        changes
            .iter()
            .find(|(changed, _)| *changed == name)
            .map(|(_, value)| value.as_deref().and_then(std::ffi::OsStr::to_str))
    }

    #[test]
    fn run_again_it_is_told_its_bus_whose_it_is_and_the_one_it_replaced() {
        let folder = Path::new("/run/user/1000/perspicax-bus-42");
        let outer = "unix:path=/tmp/dbus-outer,guid=0123";
        let again = own_bus_env(folder, 42, |name| {
            (name == "DBUS_SESSION_BUS_ADDRESS").then(|| outer.to_owned())
        });
        assert_eq!(
            changes(&again, "DBUS_SESSION_BUS_ADDRESS"),
            Some(Some("unix:path=/run/user/1000/perspicax-bus-42/bus"))
        );
        assert_eq!(changes(&again, OWN_BUS), Some(Some("42")));
        assert_eq!(changes(&again, OUTER_BUS), Some(Some(outer)));
        for gone in [
            "DBUS_SESSION_BUS_PID",
            "DBUS_SESSION_BUS_WINDOWID",
            "DBUS_STARTER_ADDRESS",
            "DBUS_STARTER_BUS_TYPE",
            "AT_SPI_BUS_ADDRESS",
        ] {
            assert_eq!(changes(&again, gone), Some(None), "{gone}");
        }

        let unwrapped = own_bus_env(folder, 42, |_| None);
        assert_eq!(
            changes(&unwrapped, OUTER_BUS),
            Some(Some("")),
            "said, and empty, so an outer session's is not inherited"
        );
    }

    #[test]
    fn the_registry_is_told_nothing_that_would_lead_it_to_another_bus() {
        let started = registry_env();
        for gone in ["AT_SPI_BUS_ADDRESS", "DISPLAY"] {
            assert_eq!(changes(&started, gone), Some(None), "{gone}");
        }
    }

    #[test]
    fn a_bus_is_kept_in_a_folder_the_person_alone_may_enter() {
        let scratch = Scratch::new("bus-folder");
        let made = scratch.0.join("made");
        make_own_folder(&made).expect("a new folder");
        assert_eq!(mode(&made), 0o700);
        make_own_folder(&made).expect("one of ours, left by a session killed");

        let open = scratch.0.join("open");
        fs::create_dir(&open).expect("a folder");
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).expect("its mode");
        assert!(make_own_folder(&open).is_err(), "one others may enter");

        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&made, &link).expect("a link");
        assert!(make_own_folder(&link).is_err(), "a link, to anywhere");
    }

    #[test]
    fn what_the_buss_services_emptied_goes_and_what_they_left_stays() {
        let scratch = Scratch::new("bus-leftovers");
        let folder = scratch.0.join("perspicax-bus-1");
        for made in ["at-spi", "dbus-1/services", "gvfsd", "kept"] {
            fs::create_dir_all(folder.join(made)).expect("a folder");
        }
        fs::write(folder.join("kept/note"), "").expect("a file");
        let elsewhere = scratch.0.join("elsewhere/empty");
        fs::create_dir_all(&elsewhere).expect("a folder outside");
        std::os::unix::fs::symlink(&elsewhere, folder.join("link")).expect("a link");

        remove_empty_folders(&folder, OWN_BUS_FOLDER_DEPTH);
        for gone in ["at-spi", "dbus-1", "gvfsd"] {
            assert!(!folder.join(gone).exists(), "{gone} is still there");
        }
        assert!(folder.join("kept/note").exists(), "a file a service left");
        assert!(
            fs::symlink_metadata(folder.join("link")).is_ok(),
            "a link, which is no folder"
        );
        assert!(elsewhere.exists(), "the folder the link leads to");

        let deep = scratch.0.join("deep");
        fs::create_dir_all(deep.join("1/2/3")).expect("folders");
        remove_empty_folders(&deep, 2);
        assert!(deep.join("1/2/3").exists(), "deeper than asked");
    }

    #[test]
    fn the_log_is_in_the_state_folder_or_else_the_home_one() {
        let scratch = Scratch::new("log-path");
        let log = |vars: &[(&str, &str)]| log_path(scratch.env(vars));
        assert_eq!(
            log(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/someone")]),
            Some(PathBuf::from("/state/perspicax/perspicax.log"))
        );
        for state in ["", "state"] {
            assert_eq!(
                log(&[("XDG_STATE_HOME", state), ("HOME", "/home/someone")]),
                Some(PathBuf::from(
                    "/home/someone/.local/state/perspicax/perspicax.log"
                )),
                "XDG_STATE_HOME = {state:?}"
            );
        }
        assert_eq!(log(&[]), None);
    }

    #[test]
    fn where_windows_closed_is_kept_beside_the_log() {
        let scratch = Scratch::new("windows-path");
        let windows = |vars: &[(&str, &str)]| windows_path(scratch.env(vars));
        assert_eq!(
            windows(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/someone")]),
            Some(PathBuf::from("/state/perspicax/windows.json"))
        );
        assert_eq!(
            windows(&[("XDG_STATE_HOME", "state"), ("HOME", "/home/someone")]),
            Some(PathBuf::from(
                "/home/someone/.local/state/perspicax/windows.json"
            ))
        );
        assert_eq!(windows(&[]), None);
    }

    #[test]
    fn a_new_log_keeps_the_last_one_and_only_the_last_one() {
        let scratch = Scratch::new("log");
        let path = scratch.0.join("state/perspicax/perspicax.log");
        let old = scratch.0.join("state/perspicax/perspicax.log.old");
        let run = |said: &str| {
            let mut log = open_log(&path).expect("a log");
            io::Write::write_all(&mut log, said.as_bytes()).expect("written");
        };

        run("first");
        assert_eq!(mode(&scratch.0.join("state/perspicax")), 0o700);
        assert_eq!(mode(&path), 0o600);
        assert!(!old.exists());

        run("second");
        run("third");
        assert_eq!(fs::read_to_string(&path).expect("the log"), "third");
        assert_eq!(fs::read_to_string(&old).expect("the last"), "second");
        assert_eq!(mode(&old), 0o600);
    }

    #[test]
    fn the_registry_is_found_where_a_distribution_put_it() {
        let scratch = Scratch::new("registry");
        let root = &scratch.0;
        assert_eq!(find_registry(root), None);

        scratch.file(
            "usr/lib/x86_64-linux-gnu/at-spi2-core/at-spi2-registryd",
            0o755,
        );
        let multiarch = scratch.file(
            "usr/lib/aarch64-linux-gnu/at-spi2-core/at-spi2-registryd",
            0o755,
        );
        assert_eq!(
            find_registry(root),
            Some(multiarch),
            "triplets in name order"
        );

        let debian = scratch.file("usr/lib/at-spi2-core/at-spi2-registryd", 0o755);
        assert_eq!(find_registry(root), Some(debian));

        let arch = scratch.file("usr/lib/at-spi2-registryd", 0o755);
        assert_eq!(find_registry(root), Some(arch.clone()));

        scratch.file("usr/libexec/at-spi2-registryd", 0o644);
        assert_eq!(
            find_registry(root),
            Some(arch),
            "a file that may not be run is passed over"
        );

        let fedora = scratch.file("usr/libexec/at-spi2-registryd", 0o755);
        assert_eq!(find_registry(root), Some(fedora));
    }

    fn value<'a>(env: &'a [(String, String)], key: &str) -> &'a str {
        env.iter()
            .find(|(name, _)| name == key)
            .map_or("", |(_, value)| value)
    }

    #[test]
    fn a_session_from_a_text_console_calls_itself_perspicax_and_wayland() {
        let env = desktop_env(|key| (key == "XDG_SESSION_TYPE").then(|| "tty".to_owned()));
        assert_eq!(value(&env, "XDG_CURRENT_DESKTOP"), "perspicax");
        assert_eq!(value(&env, "XDG_SESSION_DESKTOP"), "perspicax");
        assert_eq!(value(&env, "XDG_SESSION_TYPE"), "wayland");
    }

    #[test]
    fn a_desktop_name_the_session_was_given_is_kept_and_an_empty_one_is_not() {
        let env = desktop_env(|key| match key {
            "XDG_CURRENT_DESKTOP" => Some("perspicax:GNOME".to_owned()),
            "XDG_SESSION_DESKTOP" => Some(String::new()),
            _ => None,
        });
        assert_eq!(value(&env, "XDG_CURRENT_DESKTOP"), "perspicax:GNOME");
        assert_eq!(value(&env, "XDG_SESSION_DESKTOP"), "perspicax");
    }

    #[test]
    fn programs_are_told_where_the_agent_socket_is_or_that_there_is_none() {
        let served = agent_env(Some(Path::new("/run/user/1000/perspicax-mcp"))).expect("UTF-8");
        assert_eq!(
            served,
            [(
                AGENT_SOCKET.to_owned(),
                "/run/user/1000/perspicax-mcp".to_owned()
            )]
        );
        assert_eq!(
            agent_env(None).expect("nothing to encode"),
            [(AGENT_SOCKET.to_owned(), String::new())],
            "said, and empty, so an outer session's is not inherited"
        );

        let unreadable = {
            use std::os::unix::ffi::OsStrExt as _;
            PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff"))
        };
        assert!(agent_env(Some(&unreadable)).is_err());
    }

    #[test]
    fn programs_are_told_the_config_the_session_read() {
        let name = crate::lock::CONFIG.to_owned();
        assert_eq!(
            config_env(Some(Path::new("/home/me/.config/perspicax/config.toml"))),
            [(
                name.clone(),
                "/home/me/.config/perspicax/config.toml".to_owned()
            )]
        );
        assert_eq!(
            config_env(None),
            [(name, String::new())],
            "said, and empty, so an outer session's is not inherited"
        );
    }

    /// The runtime folder as a person's session has it, or as one started
    /// from somewhere without one has it.
    fn runtime(dir: Option<&str>) -> impl Fn(&str) -> Option<String> {
        let dir = dir.map(str::to_owned);
        move |name| dir.clone().filter(|_| name == "XDG_RUNTIME_DIR")
    }

    const FLAG: &str = "/tmp/agent";

    #[test]
    fn the_command_line_decides_when_the_config_is_silent() {
        let served = |flag: Option<&str>| {
            agent_socket(
                flag.map(Path::new),
                &AgentSocket::Unsaid,
                runtime(Some("/run/user/1000")),
            )
            .expect("no folder needed")
        };
        assert_eq!(served(Some(FLAG)), Some(PathBuf::from(FLAG)));
        assert_eq!(served(None), None, "none asked for, none served");
    }

    #[test]
    fn socket_false_closes_it_though_the_command_line_names_one() {
        for flag in [Some(Path::new(FLAG)), None] {
            assert_eq!(
                agent_socket(flag, &AgentSocket::Off, runtime(Some("/run/user/1000")))
                    .expect("nothing to find"),
                None,
                "--mcp-socket {flag:?}"
            );
        }
    }

    #[test]
    fn socket_true_serves_where_the_command_line_says_or_in_the_runtime_dir() {
        let on = |flag: Option<&str>, dir: Option<&str>| {
            agent_socket(flag.map(Path::new), &AgentSocket::On, runtime(dir))
        };
        assert_eq!(
            on(None, Some("/run/user/1000")).expect("a folder"),
            Some(PathBuf::from("/run/user/1000/perspicax-mcp"))
        );
        assert_eq!(
            on(Some(FLAG), Some("/run/user/1000")).expect("a place"),
            Some(PathBuf::from(FLAG)),
            "the flag says where, and the file only that there is one"
        );
        assert_eq!(
            on(Some(FLAG), None).expect("no folder needed"),
            Some(PathBuf::from(FLAG))
        );
        for dir in [None, Some("run/user/1000")] {
            let error = on(None, dir).expect_err("nowhere to serve it");
            assert!(error.to_string().contains("XDG_RUNTIME_DIR"), "{error}");
        }
    }

    #[test]
    fn a_socket_path_in_the_config_goes_over_the_command_lines() {
        let at = AgentSocket::At(PathBuf::from("/run/user/1000/agent"));
        for flag in [Some(Path::new(FLAG)), None] {
            assert_eq!(
                agent_socket(flag, &at, runtime(None)).expect("no folder needed"),
                Some(PathBuf::from("/run/user/1000/agent")),
                "--mcp-socket {flag:?}"
            );
        }
    }
}
