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
//!    symptom looks exactly like an ingest that cannot read GTK.
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

use std::{
    env,
    ffi::OsString,
    fs::{self, DirBuilder, File, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
        net::UnixStream,
        process::CommandExt as _,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
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

/// Where a session keeps its log: `$XDG_STATE_HOME/perspicax/perspicax.log`,
/// with `~/.local/state` for a state folder that is not set. `None` with
/// neither variable set.
///
/// `var` reads the environment, as [`has_bus`]'s does.
#[must_use]
pub fn log_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    // The XDG spec's rule: a relative folder is no folder at all.
    let folder = |name: &str| var(name).map(PathBuf::from).filter(|dir| dir.is_absolute());
    let state = folder("XDG_STATE_HOME")
        .or_else(|| folder("HOME").map(|home| home.join(".local/state")))?;
    Some(state.join("perspicax").join("perspicax.log"))
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

/// A registry this process started and is responsible for stopping.
pub struct Registry(Option<Child>);

impl Registry {
    /// Make sure an accessibility registry is running, starting one if not.
    ///
    /// Returns a handle that stops it again only if we were the ones to start
    /// it: on a real desktop the session's own registry is already up and
    /// serving every other application on the machine, and taking it down on
    /// the way out would be this program breaking somebody else's screen
    /// reader.
    ///
    /// # Errors
    ///
    /// Only if no registry is running *and* none can be started.
    pub fn ensure() -> Result<Self> {
        if registry_is_running() {
            tracing::debug!("an accessibility registry is already running");
            return Ok(Self(None));
        }

        let registry = find_registry(Path::new("/")).with_context(|| {
            format!(
                "no accessibility registry is running, and there is no {REGISTRY} to start in {}",
                REGISTRY_FOLDERS.join(", ")
            )
        })?;
        let child = Command::new(&registry)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| {
                format!(
                    "no accessibility registry is running, and {} could not be started",
                    registry.display()
                )
            })?;
        tracing::info!(pid = child.id(), "started an accessibility registry");

        // Give it a moment to claim its bus name. A client that asks before it
        // has is the exact race this whole type exists to avoid, and polling
        // for the name is not better: the name appears before the registry is
        // ready to answer about children.
        std::thread::sleep(Duration::from_millis(500));
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
const REGISTRY: &str = "at-spi2-registryd";

/// Where distributions put [`REGISTRY`], in the order looked: Fedora's and
/// Gentoo's, Arch's, and older Debian's and Ubuntu's, which some put under a
/// multiarch triplet as well. A `*` is each folder there.
const REGISTRY_FOLDERS: [&str; 4] = [
    "/usr/libexec",
    "/usr/lib",
    "/usr/lib/at-spi2-core",
    "/usr/lib/*/at-spi2-core",
];

/// The first [`REGISTRY`] that may be run in [`REGISTRY_FOLDERS`] under
/// `root`, which is `/` but in a test. Triplets are taken in name order, so the
/// choice does not depend on the order a folder happens to list them in.
fn find_registry(root: &Path) -> Option<PathBuf> {
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

/// Whether something is already serving the accessibility registry.
///
/// Read out of `/proc` rather than by shelling out to `pgrep`, for two
/// reasons. A minimal container image need not have `procps` at all, and the
/// absence would be indistinguishable from "no registry is running" -- so this
/// compositor would start a second one on a desktop that already had one.
/// And `pgrep` without `-f` matches `/proc/<pid>/comm`, which the kernel
/// truncates to fifteen characters: `at-spi2-registryd` is seventeen, so the
/// obvious spelling silently matches nothing.
fn registry_is_running() -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        fs::read(entry.path().join("cmdline")).is_ok_and(|cmdline| {
            // Arguments are NUL-separated; only the executable is of interest,
            // so that a command line merely *mentioning* the registry -- a
            // shell running this very check, for instance -- is not mistaken
            // for the registry itself.
            cmdline
                .split(|byte| *byte == 0)
                .next()
                .is_some_and(|argv0| argv0.ends_with(REGISTRY.as_bytes()))
        })
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
        assert!(!has_bus(scratch.env(&runtime)));
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
}
