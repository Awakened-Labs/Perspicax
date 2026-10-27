//! `perspicax doctor`: what this machine lacks for a session, and whether an
//! agent inside one could reach it.
//!
//! A session without what it needs either will not start, and says why only
//! as it fails -- Xwayland missing on a build that starts it -- or starts and
//! says so only in its log: a desktop left bare for want of perspicax-shell,
//! a Lock the menus hide for want of swaylock, nothing on screen readable for
//! want of an accessibility registry. So each of those is a row here, asked of
//! the machine as it is, before a session has to find out.
//!
//! And whether an agent could reach the desktop: whether a session serves it
//! a socket, and whether Claude Code and ennius are told how to reach it,
//! each in the words of its own `status`, as one row. Those two rows warn at
//! most. No session dies of them, and their own `status` says the rest.
//!
//! Nothing here starts anything, or takes anything: the config is read, the
//! shell asked whether it takes it, and a session's agent socket connected to
//! and let go, before its `initialize`, which is what takes the conversation.

use std::{
    ffi::OsString,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use perspicax_config::{AgentSocket, Built, Config, Shell, ShellBuilt};

use crate::{
    check::{self, Said},
    claude_code, ennius, lock,
    report::{self, Row, Status},
    session,
};

/// What of the machine the rows are asked of, besides its files: gathered by
/// [`Machine::here`], and written out by hand in a test.
pub struct Machine<'a> {
    /// The environment.
    pub var: &'a dyn Fn(&str) -> Option<String>,
    /// What this perspicax was built with.
    pub built: Built,
    /// The perspicax-shell a session would start, found as it would find it.
    pub shell: Option<PathBuf>,
    /// The accessibility registry a session would start, found where
    /// [`session::find_registry`] looks.
    pub registry: Option<PathBuf>,
    /// Whether one serves this session's accessibility bus already, as that
    /// bus says ([`session::a_registry_serves_this_session`]).
    pub registry_serving: bool,
    /// Whether `/tmp/.X11-unix`, where Xwayland's socket goes, is there.
    pub x11_sockets: bool,
}

impl Machine<'_> {
    /// This machine, through `var`.
    #[must_use]
    pub fn here(var: &dyn Fn(&str) -> Option<String>, built: Built) -> Machine<'_> {
        Machine {
            var,
            built,
            shell: check::shell(),
            registry: session::find_registry(Path::new("/")),
            registry_serving: session::a_registry_serves_this_session(),
            x11_sockets: Path::new(X11_SOCKETS).is_dir(),
        }
    }

    fn path(&self) -> Option<OsString> {
        (self.var)("PATH").map(OsString::from)
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        perspicax_config::which(program, self.path().as_deref())
    }
}

/// Where Xwayland's sockets go, which smithay does not make.
const X11_SOCKETS: &str = "/tmp/.X11-unix";

/// Ask this machine, with the config at `named` or else the session's, print
/// what it says, and return the exit status: 1 if anything failed.
#[must_use]
pub fn run(named: Option<&Path>, built: Built) -> i32 {
    let var = |name: &str| std::env::var(name).ok();
    report::print(&rows(&Machine::here(&var, built), named))
}

/// What `machine` says, a row a thing a session needs.
#[must_use]
pub fn rows(machine: &Machine<'_>, named: Option<&Path>) -> Vec<Row> {
    let built = machine.built;
    let path = named
        .map(Path::to_owned)
        .or_else(|| lock::config_path(machine.var));
    // Read once, for every row that needs it, as the seat and the shell would.
    let config = path
        .as_deref()
        .map(|path| perspicax_config::load(path, built));
    let shell = path
        .as_deref()
        .map(|path| perspicax_config::load_shell(path, ShellBuilt::FULL));
    let config = config.and_then(Result::ok);
    let shell = shell.and_then(Result::ok);
    let seat = |row: Row| {
        if built.seat {
            row
        } else {
            Row::new(
                row.name,
                Status::Skipped,
                "headless only: built without `seat`",
            )
        }
    };
    vec![
        build(built),
        runtime_dir(machine),
        session_bus(machine),
        dbus(machine),
        accessibility(machine),
        seat(shell_row(machine)),
        xwayland(machine, config.as_ref()),
        // Headless reads no config: what CI and an agent run is the same on
        // every machine.
        seat(config_row(machine, path.as_deref(), named.is_some())),
        seat(leave(machine, shell.as_ref())),
        seat(agent_socket(machine, config.as_ref())),
        claude_code_row(machine),
        ennius_row(machine),
    ]
}

/// What this perspicax was built with.
fn build(built: Built) -> Row {
    let features = [
        ("seat", built.seat),
        ("xwayland", built.xwayland),
        ("capture", built.capture),
        ("svg", built.svg),
    ];
    let with: Vec<&str> = features
        .iter()
        .filter(|(_, built)| *built)
        .map(|(name, _)| *name)
        .collect();
    let detail = match (built.seat, with.is_empty()) {
        (true, _) => with.join(", "),
        (false, true) => {
            "headless only: built with none of seat, xwayland, capture, svg".to_owned()
        }
        (false, false) => format!("{}; headless only: built without `seat`", with.join(", ")),
    };
    Row::new("build", Status::Ok, detail)
}

/// `XDG_RUNTIME_DIR`, where the Wayland socket goes.
fn runtime_dir(machine: &Machine<'_>) -> Row {
    const ROW: &str = "runtime-dir";
    let fix = "over SSH, `export XDG_RUNTIME_DIR=/run/user/$(id -u)`";
    match (machine.var)("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        None => Row::new(
            ROW,
            Status::Fail,
            format!("XDG_RUNTIME_DIR is not set, so no Wayland socket can be bound: {fix}"),
        ),
        Some(dir) if !Path::new(&dir).is_absolute() => Row::new(
            ROW,
            Status::Fail,
            format!("XDG_RUNTIME_DIR is {dir}, which is not a whole path: {fix}"),
        ),
        Some(dir) if !Path::new(&dir).is_dir() => Row::new(
            ROW,
            Status::Fail,
            format!("XDG_RUNTIME_DIR is {dir}, which is not a folder: {fix}"),
        ),
        Some(dir) => Row::new(ROW, Status::Ok, dir),
    }
}

/// A session bus, which a seat tells where its displays are.
fn session_bus(machine: &Machine<'_>) -> Row {
    const ROW: &str = "session-bus";
    if session::has_bus(machine.var) {
        let how = match (machine.var)("DBUS_SESSION_BUS_ADDRESS").filter(|bus| !bus.is_empty()) {
            Some(address) => address,
            None => "$XDG_RUNTIME_DIR/bus answers".to_owned(),
        };
        return Row::new(ROW, Status::Ok, how);
    }
    Row::new(
        ROW,
        Status::Warn,
        "no session bus: a seat runs without one, its tray and portal with it, and `--mcp` \
         starts one of its own; over SSH, `export \
         DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus`",
    )
}

/// The programs a bus of perspicax's own is started with.
fn dbus(machine: &Machine<'_>) -> Row {
    const ROW: &str = "dbus";
    let daemon = machine.which("dbus-daemon");
    let run_session = machine
        .built
        .seat
        .then(|| machine.which("dbus-run-session"));
    let mut missing = Vec::new();
    if daemon.is_none() {
        missing.push(
            "no dbus-daemon on PATH, so `--own-bus`, and `--mcp` with no session bus, cannot \
             start a bus",
        );
    }
    if run_session.as_ref().is_some_and(Option::is_none) {
        missing.push("no dbus-run-session on PATH, so `--session` with no bus runs without one");
    }
    if !missing.is_empty() {
        return Row::new(ROW, Status::Warn, missing.join("; "));
    }
    let found: Vec<String> = [daemon, run_session.flatten()]
        .into_iter()
        .flatten()
        .map(|program| program.display().to_string())
        .collect();
    Row::new(ROW, Status::Ok, found.join(", "))
}

/// The accessibility registry, without which nothing on screen can be read.
fn accessibility(machine: &Machine<'_>) -> Row {
    const ROW: &str = "accessibility";
    let looked = session::REGISTRY_FOLDERS.join(", ");
    match (&machine.registry, machine.registry_serving) {
        (Some(registry), _) => Row::new(ROW, Status::Ok, registry.display().to_string()),
        (None, true) => Row::new(
            ROW,
            Status::Warn,
            format!(
                "no {} in {looked}, but one serves this session's accessibility bus, which a \
                 session on this bus uses; one on a bus of its own (`--own-bus`, or `--mcp` \
                 with no bus) starts its own, and cannot: install at-spi2-core",
                session::REGISTRY
            ),
        ),
        (None, false) => Row::new(
            ROW,
            Status::Fail,
            format!(
                "no {} in {looked}, and none serves this session's accessibility bus, so \
                 nothing on screen can be read: install at-spi2-core",
                session::REGISTRY
            ),
        ),
    }
}

/// The shell a seat starts, which draws the desktop.
fn shell_row(machine: &Machine<'_>) -> Row {
    const ROW: &str = "shell";
    match &machine.shell {
        Some(shell) => Row::new(ROW, Status::Ok, shell.display().to_string()),
        None => Row::new(
            ROW,
            Status::Warn,
            "no perspicax-shell beside perspicax or on PATH, so a seat's desktop stays bare: \
             install it beside perspicax",
        ),
    }
}

/// Xwayland, which a session that starts it cannot start without.
fn xwayland(machine: &Machine<'_>, config: Option<&Config>) -> Row {
    const ROW: &str = "xwayland";
    if !machine.built.xwayland {
        return Row::new(ROW, Status::Skipped, "built without `xwayland`");
    }
    // A seat starts it as its config says; headless, only `--xwayland` does.
    let asked = if machine.built.seat {
        match config {
            Some(config) if !config.xwayland => {
                return Row::new(ROW, Status::Skipped, "the config turns it off");
            }
            Some(_) => true,
            None => {
                return Row::new(
                    ROW,
                    Status::Skipped,
                    "the config is refused, so what it says of Xwayland is unknown",
                );
            }
        }
    } else {
        false
    };
    let (status, who) = if asked {
        (Status::Fail, "a session would not start")
    } else {
        (Status::Warn, "`--xwayland` would not start")
    };
    match (machine.which("Xwayland"), machine.x11_sockets) {
        (Some(program), true) => Row::new(ROW, Status::Ok, program.display().to_string()),
        (None, _) => Row::new(
            ROW,
            status,
            format!(
                "no Xwayland on PATH, so {who}: install it{}",
                if asked {
                    ", or set `xwayland = false`"
                } else {
                    ""
                }
            ),
        ),
        (Some(_), false) => Row::new(
            ROW,
            status,
            format!("{X11_SOCKETS} is not there for Xwayland's socket, so {who}"),
        ),
    }
}

/// The config, in the words of both programs that read it.
fn config_row(machine: &Machine<'_>, path: Option<&Path>, named: bool) -> Row {
    const ROW: &str = "config";
    let Some(path) = path else {
        return Row::new(
            ROW,
            Status::Skipped,
            format!(
                "none of {}, XDG_CONFIG_HOME or HOME is set, so there is no config to check",
                lock::CONFIG
            ),
        );
    };
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) if named => {
            return Row::new(
                ROW,
                Status::Fail,
                format!("{}: there is no such file", path.display()),
            );
        }
        Ok(false) => {
            return Row::new(
                ROW,
                Status::Ok,
                format!(
                    "{}: there is no config here, so a session runs the classic profile",
                    path.display()
                ),
            );
        }
        Err(error) => {
            return Row::new(
                ROW,
                Status::Fail,
                format!("could not check {}: {error}", path.display()),
            );
        }
    }
    match check::check(path, machine.built, machine.shell.as_deref()) {
        Ok(said) => {
            let refused: Vec<&str> = said
                .iter()
                .filter_map(|said| match said {
                    Said::Refuses(line) => Some(line.as_str()),
                    Said::Takes(_) | Said::Unasked(_) => None,
                })
                .collect();
            if !refused.is_empty() {
                return Row::new(ROW, Status::Fail, refused.join("; "));
            }
            let unasked = said.iter().find_map(|said| match said {
                Said::Unasked(line) => Some(line.as_str()),
                Said::Takes(_) | Said::Refuses(_) => None,
            });
            Row::new(
                ROW,
                Status::Ok,
                unasked.map_or_else(
                    || format!("{}: perspicax and perspicax-shell take it", path.display()),
                    |unasked| format!("perspicax takes it; {unasked}"),
                ),
            )
        }
        Err(error) => Row::new(
            ROW,
            Status::Fail,
            format!("could not check {}: {error:#}", path.display()),
        ),
    }
}

/// The ways to leave at the menus' foot, each of which shows only with its
/// program installed.
fn leave(machine: &Machine<'_>, shell: Option<&Shell>) -> Row {
    const ROW: &str = "leave";
    let Some(shell) = shell else {
        return Row::new(
            ROW,
            Status::Skipped,
            "the config's `[shell]` is refused, so what the menus run is unknown",
        );
    };
    let (mut found, mut hidden) = (Vec::new(), Vec::new());
    for &way in &shell.leave {
        // Log Out runs nothing: it asks the compositor.
        let Some(program) = shell.runs(way).and_then(|argv| argv.into_iter().next()) else {
            continue;
        };
        if machine.which(&program).is_some() {
            found.push(way.key());
        } else {
            hidden.push(format!("{} (no {program} on PATH)", way.key()));
        }
    }
    if !hidden.is_empty() {
        return Row::new(
            ROW,
            Status::Warn,
            format!("the menus hide {}", hidden.join(", ")),
        );
    }
    let detail = if found.is_empty() {
        "the menus offer no way to leave that runs a program".to_owned()
    } else {
        format!("each has its program: {}", found.join(", "))
    };
    Row::new(ROW, Status::Ok, detail)
}

/// Whether a session serves an agent inside it a socket, and inside one,
/// whether it answers.
fn agent_socket(machine: &Machine<'_>, config: Option<&Config>) -> Row {
    const ROW: &str = "agent-socket";
    let Some(config) = config else {
        return Row::new(
            ROW,
            Status::Skipped,
            "the config is refused, so what it says of `[agent] socket` is unknown",
        );
    };
    let fix = "set `[agent] socket = true`";
    let (mut status, mut detail) =
        match session::agent_socket(None, &config.agent.socket, machine.var) {
            Ok(Some(path)) => (
                Status::Ok,
                format!("a session serves the agent interface at {}", path.display()),
            ),
            Ok(None) if config.agent.socket == AgentSocket::Off => (
                Status::Warn,
                format!(
                    "`[agent] socket = false`, so an agent inside a session cannot attach: {fix}"
                ),
            ),
            Ok(None) => (
                Status::Warn,
                format!(
                    "a session a display manager starts serves no agent socket, so an agent inside \
                 it cannot attach: {fix}"
                ),
            ),
            Err(error) => (Status::Warn, format!("{error:#}")),
        };
    match (machine.var)(session::AGENT_SOCKET) {
        Some(socket) if socket.is_empty() => {
            detail.push_str("; this session serves none");
        }
        Some(socket) => {
            // Connected to and let go: the conversation is taken by
            // `initialize`, which is never sent.
            if UnixStream::connect(&socket).is_ok() {
                detail.push_str(&format!("; this session's, {socket}, answers"));
            } else {
                status = Status::Warn;
                detail.push_str(&format!("; this session's, {socket}, does not answer"));
            }
        }
        None => {}
    }
    Row::new(ROW, status, detail)
}

/// Whether Claude Code is told how to reach the desktop.
fn claude_code_row(machine: &Machine<'_>) -> Row {
    let rows = claude_code::status(machine.var);
    summed("claude-code", &rows, "perspicax claude-code status")
}

/// Whether ennius is told how to reach the desktop, and granted its tools.
fn ennius_row(machine: &Machine<'_>) -> Row {
    let rows = ennius::status(machine.var);
    summed("ennius", &rows, "perspicax ennius status")
}

/// `rows` as one row called `name`: the worst of them, at most a warning,
/// with the words of each that is not as it should be, or else of them all.
fn summed(name: &'static str, rows: &[Row], status: &str) -> Row {
    let rank = |status: Status| match status {
        Status::Skipped => 0,
        Status::Ok => 1,
        Status::Warn => 2,
        Status::Fail => 3,
    };
    let worst = rows
        .iter()
        .map(|row| row.status)
        .max_by_key(|&status| rank(status))
        .unwrap_or(Status::Skipped);
    let wrong: Vec<&str> = rows
        .iter()
        .filter(|row| matches!(row.status, Status::Warn | Status::Fail))
        .map(|row| row.detail.as_str())
        .collect();
    let detail = if wrong.is_empty() {
        rows.iter()
            .filter(|row| row.status != Status::Skipped || rows.len() == 1)
            .map(|row| row.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    } else {
        wrong.join("; ")
    };
    match worst {
        // No session dies of it, and its own status says the rest.
        Status::Fail => Row::new(name, Status::Warn, format!("{detail} (see `{status}`)")),
        worst => Row::new(name, worst, detail),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs};

    use perspicax_config::Profile;

    use super::*;

    const EVERYTHING: Built = Built {
        seat: true,
        xwayland: true,
        capture: true,
        svg: true,
    };

    const HEADLESS: Built = Built {
        seat: false,
        xwayland: false,
        capture: false,
        svg: false,
    };

    /// A folder for one test, with a `bin` for its `PATH`, emptied when it
    /// ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("perspicax-doctor-{name}-{}", std::process::id()));
            fs::create_dir_all(dir.join("bin")).expect("a scratch folder");
            Self(dir)
        }

        /// A program on its `PATH`: written as [`crate::forked`] says.
        fn program(&self, name: &str) -> PathBuf {
            let path = self.0.join("bin").join(name);
            crate::forked::program(&path, "#!/bin/sh\n");
            path
        }

        /// An environment: its home and `PATH` here, and `more`.
        fn env(&self, more: &[(&str, &str)]) -> HashMap<String, String> {
            let mut env: HashMap<String, String> = [
                ("HOME", self.0.display().to_string()),
                ("PATH", self.0.join("bin").display().to_string()),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
            env.extend(
                more.iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
            );
            env
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A machine with `env` for its environment, built as `built`, with
    /// everything else there.
    fn machine<'a>(var: &'a dyn Fn(&str) -> Option<String>, built: Built) -> Machine<'a> {
        Machine {
            var,
            built,
            shell: Some(PathBuf::from("/usr/bin/perspicax-shell")),
            registry: Some(PathBuf::from("/usr/libexec/at-spi2-registryd")),
            registry_serving: false,
            x11_sockets: true,
        }
    }

    fn var(env: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        |name| env.get(name).cloned()
    }

    #[test]
    fn every_row_is_asked_in_order_and_a_headless_build_skips_the_seats() {
        let scratch = Scratch::new("order");
        let env = scratch.env(&[]);
        let var = var(&env);
        // Named, since the default is found in this process's own
        // environment, which is the person's running the tests.
        let config = scratch.0.join("config.toml");
        fs::write(&config, "profile = \"minimal\"\n").unwrap();
        let rows = rows(&machine(&var, HEADLESS), Some(&config));
        let names: Vec<&str> = rows.iter().map(|row| row.name).collect();
        assert_eq!(
            names,
            [
                "build",
                "runtime-dir",
                "session-bus",
                "dbus",
                "accessibility",
                "shell",
                "xwayland",
                "config",
                "leave",
                "agent-socket",
                "claude-code",
                "ennius"
            ]
        );
        for name in [
            "shell",
            "xwayland",
            "config",
            "leave",
            "agent-socket",
            "claude-code",
            "ennius",
        ] {
            let row = rows.iter().find(|row| row.name == name).unwrap();
            assert_eq!(row.status, Status::Skipped, "{row:?}");
        }
        assert!(rows[0].detail.starts_with("headless only"), "{:?}", rows[0]);
    }

    #[test]
    fn a_runtime_folder_is_a_whole_path_to_a_folder() {
        let scratch = Scratch::new("runtime");
        let folder = scratch.0.display().to_string();
        for (said, status) in [
            (None, Status::Fail),
            (Some("run/user/1000"), Status::Fail),
            (Some("/nowhere/at/all"), Status::Fail),
            (Some(folder.as_str()), Status::Ok),
        ] {
            let env = scratch.env(
                &said
                    .map(|dir| ("XDG_RUNTIME_DIR", dir))
                    .into_iter()
                    .collect::<Vec<_>>(),
            );
            let var = var(&env);
            assert_eq!(
                runtime_dir(&machine(&var, EVERYTHING)).status,
                status,
                "{said:?}"
            );
        }
    }

    #[test]
    fn no_bus_and_no_bus_daemon_are_warnings() {
        let scratch = Scratch::new("bus");
        let env = scratch.env(&[]);
        let var = var(&env);
        let machine = machine(&var, EVERYTHING);
        assert_eq!(session_bus(&machine).status, Status::Warn);
        let row = dbus(&machine);
        assert_eq!(row.status, Status::Warn);
        assert!(
            row.detail.contains("no dbus-daemon") && row.detail.contains("no dbus-run-session"),
            "{row:?}"
        );
        scratch.program("dbus-daemon");
        scratch.program("dbus-run-session");
        assert_eq!(dbus(&machine).status, Status::Ok);
        let env = scratch.env(&[("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus")]);
        let var = self::var(&env);
        assert_eq!(
            session_bus(&self::machine(&var, EVERYTHING)).status,
            Status::Ok
        );
    }

    #[test]
    fn a_registry_serving_the_bus_but_not_installed_serves_a_session_on_it_only() {
        let env = HashMap::new();
        let var = var(&env);
        let mut machine = machine(&var, EVERYTHING);
        assert_eq!(accessibility(&machine).status, Status::Ok);
        machine.registry = None;
        machine.registry_serving = true;
        assert_eq!(accessibility(&machine).status, Status::Warn);
        machine.registry_serving = false;
        let row = accessibility(&machine);
        assert_eq!(row.status, Status::Fail);
        assert!(
            row.detail.contains("nothing on screen can be read"),
            "{row:?}"
        );
    }

    #[test]
    fn xwayland_is_needed_as_the_config_asks_for_it() {
        let scratch = Scratch::new("xwayland");
        let env = scratch.env(&[]);
        let var = var(&env);
        let mut machine = machine(&var, EVERYTHING);
        let mut config = Config::profile(Profile::Classic, EVERYTHING);
        let row = xwayland(&machine, Some(&config));
        assert_eq!(row.status, Status::Fail, "{row:?}");
        assert!(row.detail.contains("a session would not start"), "{row:?}");
        config.xwayland = false;
        assert_eq!(xwayland(&machine, Some(&config)).status, Status::Skipped);
        config.xwayland = true;
        scratch.program("Xwayland");
        assert_eq!(xwayland(&machine, Some(&config)).status, Status::Ok);
        machine.x11_sockets = false;
        assert_eq!(xwayland(&machine, Some(&config)).status, Status::Fail);
        machine.built = Built {
            seat: false,
            ..EVERYTHING
        };
        assert_eq!(
            xwayland(&machine, None).status,
            Status::Warn,
            "headless, only --xwayland asks for it"
        );
        machine.built = HEADLESS;
        assert_eq!(xwayland(&machine, None).status, Status::Skipped);
    }

    #[test]
    fn the_config_is_checked_in_both_programs_words() {
        let scratch = Scratch::new("config");
        let env = scratch.env(&[]);
        let var = var(&env);
        let mut machine = machine(&var, EVERYTHING);
        machine.shell = None;
        let path = scratch.0.join("config.toml");
        let row = config_row(&machine, Some(&path), false);
        assert_eq!(row.status, Status::Ok);
        assert!(row.detail.contains("classic profile"), "{row:?}");
        assert_eq!(
            config_row(&machine, Some(&path), true).status,
            Status::Fail,
            "named and not there"
        );
        fs::write(&path, "[keys]\n\"Ctrl+Alt+F2\" = \"close\"\n").unwrap();
        let row = config_row(&machine, Some(&path), false);
        assert_eq!(row.status, Status::Fail);
        assert!(row.detail.contains("perspicax refuses it"), "{row:?}");
        fs::write(&path, "profile = \"minimal\"\n").unwrap();
        let row = config_row(&machine, Some(&path), false);
        assert_eq!(row.status, Status::Ok);
        assert!(
            row.detail.starts_with("perspicax takes it; "),
            "the shell was not asked: {row:?}"
        );
    }

    #[test]
    fn a_way_to_leave_without_its_program_is_one_the_menus_hide() {
        let scratch = Scratch::new("leave");
        let env = scratch.env(&[]);
        let var = var(&env);
        let machine = machine(&var, EVERYTHING);
        let shell = Shell::profile(Profile::Classic, ShellBuilt::FULL);
        let row = leave(&machine, Some(&shell));
        assert_eq!(row.status, Status::Warn);
        assert!(
            row.detail
                .starts_with("the menus hide lock (no swaylock on PATH)"),
            "{row:?}"
        );
        scratch.program("swaylock");
        scratch.program("loginctl");
        let row = leave(&machine, Some(&shell));
        assert_eq!(row.status, Status::Ok, "{row:?}");
        assert_eq!(leave(&machine, None).status, Status::Skipped);
    }

    #[test]
    fn an_agent_can_attach_only_where_a_session_serves_a_socket() {
        let scratch = Scratch::new("socket");
        let runtime = scratch.0.display().to_string();
        let env = scratch.env(&[("XDG_RUNTIME_DIR", &runtime)]);
        let var = var(&env);
        let mut config = Config::profile(Profile::Classic, EVERYTHING);
        let row = agent_socket(&machine(&var, EVERYTHING), Some(&config));
        assert_eq!(row.status, Status::Warn, "{row:?}");
        assert!(
            row.detail.ends_with("set `[agent] socket = true`"),
            "{row:?}"
        );
        config.agent.socket = AgentSocket::On;
        let row = agent_socket(&machine(&var, EVERYTHING), Some(&config));
        assert_eq!(row.status, Status::Ok, "{row:?}");
        assert!(row.detail.ends_with("/perspicax-mcp"), "{row:?}");

        // Inside a session: its socket answers, or does not.
        let socket = scratch.0.join("perspicax-mcp");
        let listening = std::os::unix::net::UnixListener::bind(&socket).expect("a socket");
        let shown = socket.display().to_string();
        let env = scratch.env(&[
            ("XDG_RUNTIME_DIR", &runtime),
            (session::AGENT_SOCKET, &shown),
        ]);
        let inside = self::var(&env);
        let row = agent_socket(&machine(&inside, EVERYTHING), Some(&config));
        assert!(row.detail.ends_with("answers"), "{row:?}");
        drop(listening);
        // Soon, not at once: another test's fork may hold it a moment (see
        // `crate::forked`).
        let row = || agent_socket(&machine(&inside, EVERYTHING), Some(&config));
        assert!(
            crate::forked::soon(|| row().detail.ends_with("does not answer")),
            "{:?}",
            row()
        );
        assert_eq!(row().status, Status::Warn, "{:?}", row());
    }

    #[test]
    fn a_failure_of_claude_code_or_ennius_is_a_warning_here() {
        let rows = [
            Row::new("server", Status::Fail, "two ways in"),
            Row::new("grant", Status::Skipped, "no server"),
        ];
        let row = summed("ennius", &rows, "perspicax ennius status");
        assert_eq!(row.status, Status::Warn);
        assert_eq!(row.detail, "two ways in (see `perspicax ennius status`)");
        let rows = [
            Row::new("server", Status::Ok, "wired"),
            Row::new("grant", Status::Ok, "granted"),
        ];
        assert_eq!(summed("ennius", &rows, "").detail, "wired; granted");
        let rows = [Row::new("mcp-server", Status::Skipped, "not installed")];
        let row = summed("claude-code", &rows, "");
        assert_eq!(
            (row.status, row.detail.as_str()),
            (Status::Skipped, "not installed")
        );
    }
}
