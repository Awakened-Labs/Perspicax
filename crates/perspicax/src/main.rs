//! `perspicax` -- the composition root.
//!
//! One binary, because the pieces have to share a process: the semantic index
//! is only worth anything joined to the compositor that can say what is
//! visible, and an agent interface that had to reach either of them over IPC
//! would be paying for a boundary that buys nothing.
//!
//! # Running it
//!
//! ```sh
//! perspicax --headless --spawn gtk4-widget-factory --run-for 10
//! ```
//!
//! Or, from a TTY on a build with `--features desktop`, as a real session:
//!
//! ```sh
//! perspicax --seat
//! ```
//!
//! A display manager's session entry runs `perspicax --session`: the same
//! session, under a session bus of its own when it was started without one,
//! and logging to `~/.local/state/perspicax/perspicax.log`.
//!
//! An agent running inside the session -- a Claude Code in a terminal on the
//! seat -- reaches the agent interface on a socket, which it may leave and
//! come back to while the session runs on:
//!
//! ```sh
//! perspicax --seat --mcp-socket "$XDG_RUNTIME_DIR/perspicax-mcp" --spawn foot
//! ```
//!
//! It needs `XDG_RUNTIME_DIR` set, which is where the Wayland socket goes. Over
//! SSH that is `export XDG_RUNTIME_DIR=/run/user/$(id -u)`, the same variable
//! `perspicax-probe` needs for the accessibility bus and for the same reason: a login
//! shell has a session, and an SSH command does not.
//!
//! Set `RUST_LOG=info` for a running account of clients connecting and windows
//! mapping; headless, that log is the only way to watch a compositor that
//! deliberately draws nothing.

use std::{
    fs::File,
    io,
    os::unix::net::UnixListener,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use perspicax::{bus, desk::Desk, keep::keep_current, observe, session};
use perspicax_compositor::{Backend, Config, Facts, Host, Requests, Stop, Virtual};
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt::writer::BoxMakeWriter};

#[derive(Parser)]
#[command(
    name = "perspicax",
    about = "An agent-native Wayland compositor: everything on screen, as a typed API.",
    version,
    group = clap::ArgGroup::new("backend")
        .required(true)
        .args(["headless", "seat", "session"])
)]
struct Cli {
    /// Run with no window system at all: a virtual output, no rendering, no
    /// GPU. What CI runs, and what an agent with no person present wants.
    ///
    /// One of `--headless`, `--seat` or `--session` is required rather than
    /// defaulted, so that no script written against one silently starts
    /// meaning another.
    #[arg(long)]
    headless: bool,

    /// Run as a real session on this machine's seat: the connected monitors,
    /// the keyboards and pointers, device access through libseat. Start it
    /// from a TTY. Needs a build with `--features seat` (or `desktop`).
    #[arg(long)]
    seat: bool,

    /// Run as a whole session, as a display manager starts one: `--seat`,
    /// under a session bus of its own when it was given none, and logging to
    /// `$XDG_STATE_HOME/perspicax/perspicax.log` (the last run's is kept as
    /// `perspicax.log.old`). What the session entry runs.
    #[arg(long)]
    session: bool,

    /// A virtual output's size, as `WIDTHxHEIGHT`. Repeat it for more than one
    /// monitor: they are named `HEADLESS-1`, `HEADLESS-2`, ... and placed left
    /// to right in the order given. Every global rectangle the index reports
    /// is in the space they make. Headless only: a seat's outputs are the size
    /// its monitors are.
    #[arg(
        long,
        default_value = "1920x1080",
        value_parser = parse_size,
        conflicts_with_all = ["seat", "session"]
    )]
    size: Vec<(i32, i32)>,

    /// The session's config file, instead of
    /// `$XDG_CONFIG_HOME/perspicax/config.toml`. Seat only: headless reads no
    /// config, so what CI and an agent run is the same on every machine.
    #[arg(long, value_name = "PATH", conflicts_with = "headless")]
    config: Option<PathBuf>,

    /// A command to run against this compositor, repeatable. Split on spaces,
    /// so quoting an argument containing one will not do what you want.
    #[arg(long = "spawn", value_name = "COMMAND")]
    spawn: Vec<String>,

    /// Exit after this many seconds instead of running until killed.
    #[arg(long, value_name = "SECONDS")]
    run_for: Option<f64>,

    /// Wait this many seconds for the spawned clients to settle, then read the
    /// accessibility bus, join it to what this compositor drew, judge every
    /// node, print the result, and stop.
    ///
    /// The wait is for the applications, not for us: a toolkit maps its window
    /// and then populates its accessibility tree, and reading too early finds a
    /// window with nothing in it. The stop afterwards is a request rather than
    /// a deadline, because reading a Qt tree takes seconds nobody can predict.
    #[arg(long, value_name = "SECONDS")]
    dump_tree: Option<f64>,

    /// Serve the agent interface: an MCP server on stdin and stdout, beside
    /// the compositor and in the same process.
    ///
    /// In the same process because it has to be. `Compositor` is not `Send` and
    /// the index lives beside it, so the server is a thread with its own
    /// runtime rather than a peer reached over IPC -- a boundary there would
    /// cost a serialisation of every node and buy nothing.
    ///
    /// **This takes over stdout**, which becomes the JSON-RPC wire. Logging
    /// never goes there, in any mode; see the note in `main`.
    ///
    /// Headless, the run ends when the client does: an agent-driven compositor
    /// with no agent left has nothing to host, and it exits non-zero if the
    /// agent interface failed rather than ended. On a seat the session is the
    /// person's, and it outlives the client however the client goes.
    ///
    /// Its one client is whatever started this process. An agent running
    /// inside the session wants `--mcp-socket`.
    #[arg(long)]
    mcp: bool,

    /// Serve the agent interface on a Unix socket at PATH instead of on stdin
    /// and stdout, for an agent that did not start this process: above all one
    /// running inside the session it drives, which cannot be what started it.
    ///
    /// Each connection is one MCP conversation, and the desktop holds one at a
    /// time: another agent's `initialize` is refused, naming the process that
    /// holds it. A connection ending, however it ends, ends that conversation
    /// and nothing else, so an agent may quit, restart and connect again with
    /// the session up throughout. Headless too, which runs until it is killed
    /// or `--run-for` ends it.
    ///
    /// The socket is the owner's alone, and removed on the way out. One a
    /// killed session left behind is replaced; one another session is serving
    /// is refused, naming who serves it. Every program the session starts is
    /// told where it is in `PERSPICAX_MCP_SOCKET`. `$XDG_RUNTIME_DIR` is the
    /// place for it.
    #[arg(long, value_name = "PATH", conflicts_with = "mcp")]
    mcp_socket: Option<PathBuf>,

    /// How long the agent interface (`--mcp` or `--mcp-socket`) lets an
    /// application draw before reading it.
    ///
    /// The same wait `--dump-tree` takes as its argument, and for the same
    /// reason: a toolkit maps its window and *then* populates its
    /// accessibility tree, so reading too early finds a window with nothing in
    /// it. Every application gets this long from when it first draws, whether
    /// it was spawned at the start or by the person an hour later. The server
    /// itself starts immediately and answers honestly in the meantime -- an
    /// empty desktop is a real answer, and a client that had to wait eight
    /// seconds for `initialize` would look like one talking to a hung process.
    #[arg(long, value_name = "SECONDS", default_value = "8")]
    settle: f64,
}

impl Cli {
    /// Whether this is a person's session on this machine's seat, which both
    /// `--seat` and `--session` are. They differ only in the bus and the log.
    fn on_seat(&self) -> bool {
        self.seat || self.session
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Before the log is opened and before anything is started, because when it
    // works nothing after it runs: this process becomes `dbus-run-session`,
    // which runs it again under a bus of its own.
    let bus = if cli.session {
        session::ensure_bus()
    } else {
        Ok(())
    };

    // NEVER STDOUT. Under `--mcp` stdout is the JSON-RPC wire, and a single log
    // line written to it corrupts a frame -- which arrives at the client as a
    // parse error with nothing in it pointing back here. So stderr whatever the
    // flags, rather than only under `--mcp`, because a log destination that
    // depends on a flag is one somebody adds a `println!` next to -- with one
    // exception, and that one a file: a display manager keeps a session's
    // stderr wherever it keeps such things, so `--session` has a log of its own.
    //
    // With no `RUST_LOG`, a person's session says its warnings: one of them
    // is the only word anywhere on why a keyring lookup hangs (issue #27). One
    // a display manager started says what it did as well, since its log is
    // read after the fact. Headless says only errors, as it always has.
    let quiet = if cli.session {
        LevelFilter::INFO
    } else if cli.seat {
        LevelFilter::WARN
    } else {
        LevelFilter::ERROR
    };
    let (writer, unopened) = match cli.session.then(open_log) {
        Some(Ok(log)) => (BoxMakeWriter::new(Mutex::new(log)), None),
        Some(Err(error)) => (BoxMakeWriter::new(io::stderr), Some(error)),
        None => (BoxMakeWriter::new(io::stderr), None),
    };
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(quiet.into())
                .from_env_lossy(),
        )
        .init();
    let own_bus = cli.session && bus.is_ok() && std::env::var_os(session::BUS_STARTED).is_some();
    match bus {
        Err(error) => tracing::warn!("{error:#}; the session goes on without a bus"),
        Ok(()) if own_bus => {
            tracing::info!("the session bus is this session's own, from dbus-run-session");
        }
        Ok(()) => {}
    }
    let to_file = cli.session && unopened.is_none();
    if let Some(error) = unopened {
        tracing::warn!("{error:#}: logging to stderr instead");
    }

    let ended = run(&cli);
    // Said on stderr too, as `main`'s error always is, but a session's is
    // looked for in its log, beside whatever led up to it.
    if to_file && let Err(error) = &ended {
        tracing::error!("{error:#}");
    }
    // While the bus is still up to say who serves it: `dbus-run-session` takes
    // the bus down only once this process has gone.
    if own_bus && let Err(error) = stop_accessibility_bus() {
        tracing::warn!("{error:#}");
    }
    ended
}

/// Open a session's log, or say why there is none.
fn open_log() -> Result<File> {
    let path = session::log_path(|name| std::env::var(name).ok())
        .context("a session logs under XDG_STATE_HOME or HOME, and neither is set")?;
    session::open_log(&path).with_context(|| format!("could not open {}", path.display()))
}

/// Everything once there is a log: the run, from its first child to its end.
fn run(cli: &Cli) -> Result<()> {
    let backend = if cli.on_seat() {
        Backend::Seat
    } else {
        Backend::Headless {
            outputs: cli
                .size
                .iter()
                .enumerate()
                .map(|(at, &size)| Virtual::numbered(at + 1, size))
                .collect(),
            // One workspace: an agent's desk is whatever it spawned.
            workspaces: Default::default(),
            access: Default::default(),
        }
    };
    // First, before the accessibility bus is touched: a backend this binary
    // cannot run is the one thing worth saying, and it should not arrive
    // after a registry has been started for nothing.
    backend.ensure_built()?;

    // Second, for the same reason: a socket another session is serving is a
    // reason not to start at all. The file is held to the end of this
    // function, which is what removes it on the way out, however `run` ends.
    let (socket, listener) = cli
        .mcp_socket
        .as_deref()
        .map(perspicax_mcp::Socket::bind)
        .transpose()?
        .unzip();
    let agent_env = session::agent_env(socket.as_ref().map(perspicax_mcp::Socket::path))?;

    // Before anything is spawned, and in this order. A registry that arrives
    // after its clients is a registry GTK has already given up on.
    //
    // Without one, an agent run headless has nothing to read, and stops. A
    // person's session is still a session, and says so.
    let _registry = match session::Registry::ensure() {
        Ok(registry) => Some(registry),
        Err(error) if cli.on_seat() => {
            tracing::warn!("{error:#}");
            None
        }
        Err(error) => return Err(error),
    };
    if let Err(error) = enable_accessibility() {
        tracing::warn!("{error:#}");
    }

    // A person's session says what it is, to its programs and to the session
    // bus alike. Headless, the agent's programs live in whatever desktop the
    // run was started from, and telling its bus anything would send that
    // desktop's portals and notifications to a compositor nobody can see.
    let desktop = if cli.on_seat() {
        session::desktop_env(|key| std::env::var(key).ok())
    } else {
        Vec::new()
    };

    let config = Config {
        backend,
        spawn: cli
            .spawn
            .iter()
            .map(|command| {
                command
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect(),
        env: [
            session::accessibility_env(),
            desktop.clone(),
            agent_env.clone(),
        ]
        .concat(),
        run_for: cli.run_for.map(Duration::from_secs_f64),
        config: cli
            .on_seat()
            .then(|| cli.config.clone().or_else(perspicax_config::default_path))
            .flatten(),
        socket: None,
        xwayland: false,
    };

    // Created here rather than inside the compositor, because the thread that
    // reads these facts is not the thread that publishes them.
    let facts = Facts::new();
    let stop = Stop::new();
    // Named rather than built at the call below, because `Host` needs the same
    // one: `Requests` hands its receiving end to exactly one loop, and a second
    // channel would be a `Host` nobody is listening to.
    let requests = Requests::new();

    if cli.on_seat() {
        bus::start(
            bus::Options::session([desktop, agent_env].concat()),
            facts.watch_session(),
        );
    }

    if let Some(after) = cli.dump_tree.map(Duration::from_secs_f64) {
        dump_when_ready(facts.clone(), stop.clone(), after);
    }

    // Headless, the agent is who the run is for. On a seat it is the person's
    // session, and no agent interface ending -- cleanly or not -- may take
    // their windows with it (issue #26). Over a socket, conversations come
    // and go and none of them ends anything: the interface itself ends only
    // when the socket can no longer be served.
    let agent_ends_session = cli.headless;
    let interface = if cli.mcp {
        Some(Interface::Stdio)
    } else {
        listener.map(Interface::Socket)
    };
    let agent = interface.map(|interface| {
        let desk = Arc::new(Desk::new(&facts, &Host::new(&facts, &requests)));
        let ended = serve(
            Arc::clone(&desk),
            interface,
            stop.clone(),
            agent_ends_session,
        );
        keep_current(
            desk,
            facts.clone(),
            stop.clone(),
            Duration::from_secs_f64(cli.settle),
        );
        ended
    });

    perspicax_compositor::run(&config, &facts, &requests, &stop)
        .context("the compositor stopped")?;

    // A run the agent interface ended by failing is not a clean end, and a
    // script or a supervisor watching the exit status should be able to tell.
    if agent_ends_session && let Some(Ok(Err(error))) = agent.map(|ended| ended.try_recv()) {
        return Err(error.context("the agent interface failed"));
    }
    Ok(())
}

/// Which way the agent interface is served.
enum Interface {
    /// On this process's stdin and stdout, to whatever started it: `--mcp`.
    Stdio,
    /// On a Unix socket, to whoever connects: `--mcp-socket`.
    Socket(UnixListener),
}

/// Serve the agent interface on its own thread, with its own runtime, and say
/// on the channel returned how it ended.
///
/// The same arrangement `dump_when_ready` uses and for the same reason: Wayland
/// state is not `Send` and the calloop loop must not be blocked, while an MCP
/// server spends its life waiting on a pipe.
///
/// `ends_session` is whether the interface ending ends the session too. Over
/// stdio it cannot come back: there is no second stdin for another client to
/// arrive on. Over a socket it ends only if the socket cannot be served, since
/// a client going away is not the interface ending.
fn serve(
    desk: Arc<Desk>,
    interface: Interface,
    stop: Stop,
    ends_session: bool,
) -> mpsc::Receiver<Result<()>> {
    let (tell, ended) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("no runtime for the agent interface")
            .and_then(|runtime| {
                Ok(runtime.block_on(async {
                    match interface {
                        Interface::Stdio => perspicax_mcp::serve(desk).await,
                        Interface::Socket(listener) => perspicax_mcp::serve_socket(desk, listener)
                            .await
                            .map(|never| match never {}),
                    }
                })?)
            });
        match &outcome {
            Ok(()) => tracing::info!("the agent's client went away"),
            Err(error) => tracing::error!("the agent interface stopped: {error:#}"),
        }

        // Told before the stop is asked for, so that `main`, which looks once
        // the compositor has stopped, finds it there. Nobody listening is
        // `main` already on its way out, which needs telling nothing.
        let _ = tell.send(outcome);
        if ends_session {
            // This process exists to be driven, and there is nobody left to
            // drive it.
            stop.request();
        } else {
            tracing::warn!("the session runs on without an agent interface");
        }
    });
    ended
}

/// Turn accessibility on for this session, briefly borrowing a runtime to do
/// it.
///
/// A warning rather than a failure when it does not work: a machine with no
/// `org.a11y.Bus` at all can still host windows, and a compositor that refused
/// to start because nothing would be readable would be refusing to do the half
/// of its job that still works.
fn enable_accessibility() -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("no runtime to enable accessibility with")?
        .block_on(perspicax_atspi::enable())
        .context("could not turn accessibility on for this session")
}

/// Stop the accessibility bus this session's own bus started, briefly
/// borrowing a runtime to do it, as [`enable_accessibility`] does.
fn stop_accessibility_bus() -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("no runtime to stop the accessibility bus with")?
        .block_on(async {
            let connection = zbus::Connection::session()
                .await
                .context("no session bus to stop the accessibility bus on")?;
            if let Some(pid) = session::stop_accessibility_bus(&connection).await? {
                tracing::info!(pid, "stopped the accessibility bus this session started");
            }
            Ok(())
        })
}

/// Read and report the desktop on its own thread, then ask the compositor to
/// stop.
///
/// A thread rather than a task, and a thread with its own runtime rather than
/// one shared with the compositor: Wayland state is not `Send` and the calloop
/// loop must not be blocked, while the accessibility read is D-Bus and spends
/// most of its time waiting. Neither side can host the other, which is exactly
/// what the published-facts boundary exists to allow.
fn dump_when_ready(facts: Facts, stop: Stop, after: Duration) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(%error, "no runtime for the accessibility read");
                stop.request();
                return;
            }
        };

        runtime.block_on(async {
            tokio::time::sleep(after).await;
            match observe::observe(&facts).await {
                Ok(reading) => observe::report(&reading, &facts.read()),
                Err(error) => tracing::error!("{error:#}"),
            }
        });

        // Asked for rather than assumed: the read above has no predictable
        // duration, and a compositor that exited on a guess would take the
        // applications being read down with it.
        stop.request();
    });
}

/// `1920x1080` into a pair. Rejected rather than clamped: a compositor asked
/// for a zero-sized output should say so, not quietly invent one.
fn parse_size(raw: &str) -> Result<(i32, i32)> {
    let (width, height) = raw
        .split_once(['x', 'X'])
        .with_context(|| format!("expected WIDTHxHEIGHT, got `{raw}`"))?;
    let width: i32 = width.trim().parse().context("width")?;
    let height: i32 = height.trim().parse().context("height")?;
    if width <= 0 || height <= 0 {
        bail!("an output must have a positive size, got {width}x{height}");
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory as _, error::ErrorKind};

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, ErrorKind> {
        Cli::try_parse_from(["perspicax"].iter().chain(args)).map_err(|error| error.kind())
    }

    #[test]
    fn the_arguments_agree_with_each_other() {
        Cli::command().debug_assert();
    }

    #[test]
    fn exactly_one_backend_is_named() {
        assert_eq!(parse(&[]).err(), Some(ErrorKind::MissingRequiredArgument));
        for two in [["--seat", "--session"], ["--headless", "--session"]] {
            assert_eq!(
                parse(&two).err(),
                Some(ErrorKind::ArgumentConflict),
                "{two:?}"
            );
        }
    }

    #[test]
    fn a_session_is_on_the_seat_and_ended_by_no_agent() {
        let session = parse(&["--session"]).expect("--session alone");
        assert!(session.on_seat());
        assert!(!session.headless);
        assert!(parse(&["--seat"]).expect("--seat alone").on_seat());
        assert!(!parse(&["--headless"]).expect("--headless alone").on_seat());
    }

    #[test]
    fn only_headless_takes_a_size_and_only_a_seat_a_config() {
        for seat in ["--seat", "--session"] {
            assert_eq!(
                parse(&[seat, "--size", "800x600"]).err(),
                Some(ErrorKind::ArgumentConflict),
                "{seat}"
            );
            assert!(
                parse(&[seat, "--config", "perspicax.toml"]).is_ok(),
                "{seat}"
            );
        }
        assert!(parse(&["--headless", "--size", "800x600"]).is_ok());
        assert_eq!(
            parse(&["--headless", "--config", "perspicax.toml"]).err(),
            Some(ErrorKind::ArgumentConflict)
        );
    }

    #[test]
    fn the_agent_interface_is_served_on_stdio_or_on_a_socket_and_not_both() {
        for backend in ["--headless", "--seat", "--session"] {
            let cli =
                parse(&[backend, "--mcp-socket", "/run/user/1000/perspicax-mcp"]).expect(backend);
            assert_eq!(
                cli.mcp_socket.as_deref(),
                Some(std::path::Path::new("/run/user/1000/perspicax-mcp"))
            );
            assert!(!cli.mcp, "{backend}");
        }
        assert_eq!(
            parse(&["--headless", "--mcp", "--mcp-socket", "perspicax-mcp"]).err(),
            Some(ErrorKind::ArgumentConflict)
        );
        assert!(
            parse(&["--headless", "--mcp-socket"]).is_err(),
            "a socket is somewhere"
        );
    }
}
