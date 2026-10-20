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
//! and the agent, in that terminal, runs `perspicax attach` as its MCP server.
//! A session the display manager starts, which no flag reaches, serves the
//! same socket when its config says `[agent] socket = true`.
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
    os::{fd::AsFd as _, unix::net::UnixListener},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use perspicax::{
    attach, bus, check, desk::Desk, keep::keep_current, lock, observe, session, wire::Wire,
};
use perspicax_compositor::{Backend, Config, Facts, Host, Requests, Stop, Virtual};
use perspicax_config::{AgentSocket, Built};
use perspicax_policy::FAR;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt::writer::BoxMakeWriter};

#[derive(Parser)]
#[command(
    name = "perspicax",
    about = "An agent-native Wayland compositor: everything on screen, as a typed API.",
    version,
    group = clap::ArgGroup::new("backend")
        .required(true)
        .args(["headless", "seat", "session"]),
    subcommand_negates_reqs = true,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Something other than a session.
    #[command(subcommand)]
    command: Option<Command>,

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
    /// so quoting an argument containing one will not do what you want. It
    /// reads nothing, and what it prints goes to this process's stderr.
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
    /// **This takes over stdin and stdout**, which become the JSON-RPC wire,
    /// and nothing else reaches them. They are moved before anything starts,
    /// leaving `/dev/null` and stderr in their place, so neither perspicax's
    /// log nor anything a program it starts prints can land between the
    /// replies, and no such program can read the agent's requests. A client
    /// that pipes stderr has to read it. Not with `--session`.
    ///
    /// A wire something else was given first is refused before anything
    /// starts: a stderr that is the wire too (`2>&1`), or a session bus
    /// daemon holding it, as `dbus-run-session -- perspicax --mcp` leaves it,
    /// to hand on to every service the bus starts. Pass perspicax the wire on
    /// fds 3 and 4 inside `dbus-run-session` instead, as the README shows, or
    /// use `--mcp-socket`.
    ///
    /// Nor may another process running as you take the wire: perspicax keeps
    /// them all out of itself, so none can reopen the wire through `/proc`,
    /// take it with `pidfd_getfd`, or attach to perspicax with ptrace. So it
    /// leaves no core dump, and looking at its descriptors takes root.
    ///
    /// Headless, the run ends when the client does: an agent-driven compositor
    /// with no agent left has nothing to host, and it exits non-zero if the
    /// agent interface failed rather than ended. On a seat the session is the
    /// person's, and it outlives the client however the client goes.
    ///
    /// Its one client is whatever started this process. An agent running
    /// inside the session wants `--mcp-socket`.
    // Not `--session`: its `dbus-run-session` runs this process again before
    // the wire can be kept from the bus and every service it starts (#43).
    #[arg(long, conflicts_with = "session")]
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
    /// told where it is in `PERSPICAX_MCP_SOCKET`, and `perspicax attach`
    /// connects to it. `$XDG_RUNTIME_DIR` is the place for it.
    ///
    /// On a seat, the config's `[agent] socket` goes over this, read once at
    /// start: `false` serves none, a path serves there, and `true` serves
    /// here, or at `$XDG_RUNTIME_DIR/perspicax-mcp` when this is not given.
    /// Beside `--mcp`, which is already serving an agent, it serves none.
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

#[derive(Subcommand)]
enum Command {
    /// Join this process's stdin and stdout to a session's agent socket, for
    /// an MCP client inside the session to run as its server:
    /// `claude mcp add perspicax -- perspicax attach`.
    ///
    /// Ends when the session closes the connection. The client closing stdin
    /// is passed on as the end of what it will say, so its conversation ends
    /// and the next agent may connect.
    ///
    /// Like `--mcp`, it keeps every other process running as you out of
    /// itself, so none can reopen or take the wire through it, or attach to
    /// it with ptrace.
    Attach {
        /// The socket, instead of the session's own in `$PERSPICAX_MCP_SOCKET`.
        #[arg(value_name = "PATH")]
        socket: Option<PathBuf>,
    },

    /// Lock the screen as the menus' Lock does: `[shell] lock`, swaylock by
    /// default, in the theme, with ARGS after its own. What a key or swayidle
    /// runs: `["swayidle", "-w", "before-sleep", "perspicax lock -f"]`.
    ///
    /// The config is the session's (`$PERSPICAX_CONFIG`), or else
    /// `$XDG_CONFIG_HOME/perspicax/config.toml`. One that cannot be read
    /// still locks, in the default theme.
    Lock {
        /// Passed on to the lock program, after `[shell] lock`'s own: `-f`,
        /// or a swaylock flag that wins over the theme's.
        #[arg(
            value_name = "ARGS",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        args: Vec<String>,
    },

    /// The config file, without a session.
    Config {
        #[command(subcommand)]
        verb: ConfigVerb,
    },
}

#[derive(Subcommand)]
enum ConfigVerb {
    /// Say whether a session would take the config, in the words of the
    /// two programs that read it: this perspicax, and the perspicax-shell
    /// the session would start, each with the features it was built with.
    ///
    /// A save the session refuses is otherwise only a line in its log, and
    /// it carries on with the config it had. Exits 0 if both take it, 78 if
    /// either refuses it, 66 for a PATH that is not there.
    Check {
        /// The file, instead of the session's own (`$PERSPICAX_CONFIG`, or
        /// else `$XDG_CONFIG_HOME/perspicax/config.toml`).
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
    },
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

    // Before anything else, because it is nothing else: no session, no log,
    // nothing started, only a length of wire to one that is running.
    if let Some(Command::Attach { socket }) = &cli.command {
        let path = attach::socket_path(socket.clone(), |name| std::env::var(name).ok())?;
        return attach::attach(&path);
    }

    // Likewise: this process becomes the lock program, or says why it could
    // not.
    if let Some(Command::Lock { args }) = &cli.command {
        return Err(lock::lock(args));
    }

    // Likewise: an answer about a file, and nothing started to give it.
    if let Some(Command::Config {
        verb: ConfigVerb::Check { path },
    }) = &cli.command
    {
        std::process::exit(check::run(path.as_deref(), BUILT));
    }

    // Before the log is opened and before anything is started, because when it
    // works nothing after it runs: this process becomes `dbus-run-session`,
    // which runs it again under a bus of its own.
    let bus = if cli.session {
        session::ensure_bus()
    } else {
        Ok(())
    };

    // Before anything is started and before any thread runs, so that nothing
    // inherits the agent's wire or is half-way through a line on it; and after
    // the bus, whose `dbus-run-session` would run this again without it.
    let wire = cli
        .mcp
        .then(Wire::take)
        .transpose()
        .context("could not keep stdin and stdout for the agent interface")?;
    // And before the log, which is written to stderr: a stderr that is the wire
    // too -- `2>&1`, or one file for both -- is where every program this starts
    // prints, so any of them could print a reply perspicax did not send (#52).
    if let Some(wire) = &wire
        && wire
            .shares(io::stderr().as_fd())
            .context("could not tell where stderr goes")?
    {
        bail!(
            "stderr is the agent's wire too, and every program perspicax starts prints to \
             stderr: any of them could print a reply perspicax did not send. Give stderr a \
             file or a pipe of its own, or serve the agent on a socket, with `--mcp-socket \
             PATH` and `perspicax attach PATH` as its MCP server"
        );
    }

    // NEVER STDOUT. Under `--mcp` stdout is the JSON-RPC wire, and a single log
    // line written to it corrupts a frame -- which arrives at the client as a
    // parse error with nothing in it pointing back here. So stderr whatever the
    // flags, rather than only under `--mcp`, because a log destination that
    // depends on a flag is one somebody adds a `println!` next to -- with one
    // exception, and that one a file: a display manager keeps a session's
    // stderr wherever it keeps such things, so `--session` has a log of its own.
    // Under `--mcp` that `println!` would now miss the wire anyway, which has
    // just left fd 1 (`wire`).
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

    let ended = run(&cli, wire);
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

/// What this build can honour, as the config's feature check reads it: what
/// the seat's own reading of the file is told too.
const BUILT: Built = Built {
    seat: cfg!(feature = "seat"),
    xwayland: cfg!(feature = "xwayland"),
    capture: cfg!(feature = "capture"),
    svg: cfg!(feature = "svg"),
};

/// Where the agent socket is served, if anywhere. Headless reads no config, so
/// `--mcp-socket` says; a person's session reads `[agent] socket` in the file
/// at `config_path` first, here, since the socket is bound before the seat is
/// taken. The seat reads the file again, for everything else.
fn agent_socket(cli: &Cli, config_path: Option<&Path>) -> Result<Option<PathBuf>> {
    if !cli.on_seat() {
        return Ok(cli.mcp_socket.clone());
    }
    let said = match config_path {
        // In the seat's own words, which are the ones it would refuse it in.
        Some(path) => {
            perspicax_config::load(path, BUILT)
                .map_err(|error| anyhow!("config: {}: {error}", path.display()))?
                .agent
                .socket
        }
        None => AgentSocket::Unsaid,
    };
    // `--mcp` serves the agent that started this process, and a desktop holds
    // one agent at a time: its wire is not taken from under it for a socket.
    if cli.mcp {
        if matches!(said, AgentSocket::On | AgentSocket::At(_)) {
            tracing::info!(
                "[agent] socket is not served beside --mcp, whose agent the desktop is \
                 already serving on stdin and stdout"
            );
        }
        return Ok(None);
    }
    let served = session::agent_socket(cli.mcp_socket.as_deref(), &said, |name| {
        std::env::var(name).ok()
    })?;
    match (&cli.mcp_socket, &served) {
        (Some(flag), None) => tracing::warn!(
            flag = %flag.display(),
            "the config's [agent] socket = false goes over --mcp-socket: none is served"
        ),
        (Some(flag), Some(path)) if path != flag => tracing::warn!(
            flag = %flag.display(),
            served = %path.display(),
            "the config's [agent] socket goes over --mcp-socket"
        ),
        _ => {}
    }
    Ok(served)
}

/// Everything once there is a log: the run, from its first child to its end.
/// `wire` is the agent's, under `--mcp`.
fn run(cli: &Cli, wire: Option<Wire>) -> Result<()> {
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
            // Nobody sits at it, so no window rearranges itself.
            person: false,
            opacity: Default::default(),
            // Every client: nobody sits at it to be asked.
            agent: Default::default(),
        }
    };
    // First, before the accessibility bus is touched: a backend this binary
    // cannot run is the one thing worth saying, and it should not arrive
    // after a registry has been started for nothing.
    backend.ensure_built()?;

    // Second, for the same reason: a socket another session is serving is a
    // reason not to start at all. The file is held to the end of this
    // function, which is what removes it on the way out, however `run` ends.
    let config_path = cli
        .on_seat()
        .then(|| cli.config.clone().or_else(perspicax_config::default_path))
        .flatten();
    let (socket, listener) = agent_socket(cli, config_path.as_deref())?
        .map(perspicax_mcp::Socket::bind)
        .transpose()?
        .unzip();
    let agent_env = session::agent_env(socket.as_ref().map(perspicax_mcp::Socket::path))?;

    // Third, and before the registry, whose start makes the bus start the
    // accessibility bus: a session bus holding the agent's wire would hand it
    // to that, and to everything else it starts (issue #52).
    if let Some(wire) = &wire {
        refuse_a_bus_holding(wire)?;
    }

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
            session::config_env(config_path.as_deref()),
        ]
        .concat(),
        run_for: cli.run_for.map(Duration::from_secs_f64),
        config: config_path,
        // A person's: headless, an agent's run remembers nothing past itself.
        places: cli
            .on_seat()
            .then(|| session::windows_path(|name| std::env::var(name).ok()))
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
    let interface = wire
        .map(Interface::Stdio)
        .or_else(|| listener.map(Interface::Socket));
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
    /// On the wire `main` took from this process's stdin and stdout, to
    /// whatever started it: `--mcp`.
    Stdio(Wire),
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
                Ok(run_then_let_go(runtime, async {
                    match interface {
                        Interface::Stdio(wire) => {
                            perspicax_mcp::serve_over(desk, wire.into_halves()).await
                        }
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

/// How long the agent interface's runtime is given, once a conversation is
/// over, to finish what it still has running.
const LET_GO_AFTER: Duration = Duration::from_millis(500);

/// Run `conversation` on `runtime` to its end, then let the runtime go, waiting
/// no longer than [`LET_GO_AFTER`] for what it still has running.
///
/// Dropping the runtime would wait for all of it, and on the wire that can be
/// forever: `--mcp`'s reads run on its blocking pool, where nothing can call
/// one off, and rmcp's loop has one pending nearly all the time. A
/// conversation that ends with the client closing its end has seen that read
/// return, and nothing is left to wait for. One that ends any other way leaves
/// it waiting on the client, and until the client spoke again the error that
/// ended it would go unlogged, `main` untold, and under `--headless` the stop
/// unasked (issue #117).
///
/// A bound rather than no wait at all, for a reply still being written. A tool
/// call still running on the pool gets the same bound: its answer has nobody
/// left to go to, and its thread runs on, only no longer waited for.
fn run_then_let_go<T>(
    runtime: tokio::runtime::Runtime,
    conversation: impl Future<Output = T>,
) -> T {
    let outcome = runtime.block_on(conversation);
    runtime.shutdown_timeout(LET_GO_AFTER);
    outcome
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

/// Refuse to serve the agent on a wire the session bus's daemon holds too
/// (issue #52).
///
/// `dbus-run-session -- perspicax --mcp` gives its bus daemon the wire before
/// this process runs, where [`Wire::take`] cannot reach it, and the daemon
/// gives it to every service it starts: one a hosted program asks for by name,
/// or writes a `.service` file for, could print a reply perspicax did not send.
///
/// Served all the same when there is no bus, or it cannot be told whether the
/// daemon holds the wire. A daemon that does was started beside this process,
/// as its user, and answers; one that will not say, or that this process cannot
/// look into, is a bus somebody else runs, and a systemd user manager's prints
/// to the journal.
fn refuse_a_bus_holding(wire: &Wire) -> Result<()> {
    let (pid, fd) = match bus_holding(wire) {
        Ok(Some(held)) => held,
        Ok(None) => return Ok(()),
        Err(error) => {
            tracing::warn!(
                "{error:#}; serving the agent's wire without knowing whether the \
                 session bus holds it too"
            );
            return Ok(());
        }
    };
    let held = ["stdin", "stdout", "stderr"][usize::from(fd)];
    bail!(
        "the session bus's daemon (pid {pid}) holds the agent's wire as its {held}, as \
         `dbus-run-session -- perspicax --mcp` leaves it, and every service the bus starts \
         would hold it too: any of them could print a reply perspicax did not send. Give the \
         wire to perspicax alone, on fds 3 and 4,\n\n    \
         dbus-run-session -- sh -c 'exec perspicax --mcp … <&3 >&4 3<&- 4>&-' 3<&0 4>&1 \
         </dev/null >&2\n\n\
         or serve the agent on a socket, with `--mcp-socket PATH` and `perspicax attach PATH` \
         as its MCP server"
    )
}

/// The session bus daemon's pid and which of its descriptors 0, 1 and 2 is
/// open on `wire`; `None` when there is no bus, or the daemon holds none of
/// it. Briefly borrows a runtime to ask, as [`enable_accessibility`] does.
fn bus_holding(wire: &Wire) -> Result<Option<(u32, u8)>> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("no runtime to ask the session bus with")?
        .block_on(async {
            let Ok(connection) = zbus::Connection::session().await else {
                tracing::debug!("no session bus to hold the agent's wire");
                return Ok(None);
            };
            let pid = session::bus_daemon(&connection).await?;
            let fd = wire.held_by(pid).with_context(|| {
                format!("could not look at what the session bus's daemon (pid {pid}) holds")
            })?;
            Ok(fd.map(|fd| (pid, fd)))
        })
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
/// for a zero-sized output should say so, not quietly invent one; nor one
/// wider or taller than the world (#154), whose far edge a window's width
/// could carry past an `i32`.
fn parse_size(raw: &str) -> Result<(i32, i32)> {
    let (width, height) = raw
        .split_once(['x', 'X'])
        .with_context(|| format!("expected WIDTHxHEIGHT, got `{raw}`"))?;
    let width: i32 = width.trim().parse().context("width")?;
    let height: i32 = height.trim().parse().context("height")?;
    if width <= 0 || height <= 0 {
        bail!("an output must have a positive size, got {width}x{height}");
    }
    if width > FAR || height > FAR {
        bail!("an output is at most {FAR} pixels a side, got {width}x{height}");
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use std::os::fd::OwnedFd;

    use clap::{CommandFactory as _, error::ErrorKind};
    use tokio::io::AsyncReadExt as _;

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
    fn a_virtual_output_wider_than_the_world_is_refused() {
        assert_eq!(parse_size("65536x65536").ok(), Some((FAR, FAR)));
        for size in ["65537x1080", "1920x2147483647"] {
            let error = parse_size(size).expect_err(size).to_string();
            assert!(error.contains("at most 65536"), "{size}: {error}");
        }
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
        for backend in ["--headless", "--seat"] {
            assert!(parse(&[backend, "--mcp"]).expect(backend).mcp);
        }
        assert_eq!(
            parse(&["--session", "--mcp"]).err(),
            Some(ErrorKind::ArgumentConflict),
            "a display manager's session has no agent on its stdin"
        );
    }

    #[test]
    fn attaching_needs_no_session_and_takes_none() {
        let attaching = parse(&["attach"]).expect("attach alone");
        assert!(matches!(
            attaching.command,
            Some(Command::Attach { socket: None })
        ));
        let named = parse(&["attach", "/run/user/1000/perspicax-mcp"]).expect("attach PATH");
        assert!(matches!(
            named.command,
            Some(Command::Attach { socket: Some(_) })
        ));
        assert_eq!(
            parse(&["--headless", "attach"]).err(),
            Some(ErrorKind::ArgumentConflict)
        );
    }

    #[test]
    fn locking_needs_no_session_and_passes_its_flags_on() {
        let args = |cli: Cli| match cli.command {
            Some(Command::Lock { args }) => args,
            _ => panic!("not lock"),
        };
        assert!(args(parse(&["lock"]).expect("lock alone")).is_empty());
        assert_eq!(
            args(parse(&["lock", "-f", "--ring-color", "00ff00"]).expect("lock ARGS")),
            ["-f", "--ring-color", "00ff00"]
        );
        assert_eq!(
            parse(&["--seat", "lock"]).err(),
            Some(ErrorKind::ArgumentConflict)
        );
    }

    #[test]
    fn checking_a_config_needs_no_session_and_takes_a_path() {
        let path = |cli: Cli| match cli.command {
            Some(Command::Config {
                verb: ConfigVerb::Check { path },
            }) => path,
            _ => panic!("not config check"),
        };
        assert_eq!(
            path(parse(&["config", "check"]).expect("check alone")),
            None
        );
        assert_eq!(
            path(parse(&["config", "check", "/tmp/config.toml"]).expect("check PATH")),
            Some(PathBuf::from("/tmp/config.toml"))
        );
        assert_eq!(
            parse(&["--seat", "config", "check"]).err(),
            Some(ErrorKind::ArgumentConflict)
        );
        assert!(
            parse(&["config"]).is_err(),
            "config is not a verb on its own"
        );
    }

    #[test]
    fn a_conversation_that_ends_with_a_read_of_the_wire_pending_still_ends() {
        // The client's end of the wire, held open and silent: a read of the
        // other end returns only once it is written to or closed.
        let (input, client) = io::pipe().expect("a pipe for the wire");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let (tell, ended) = mpsc::channel();
        std::thread::spawn(move || {
            let outcome = run_then_let_go(runtime, async move {
                // The wire as `Wire::into_halves` gives it, read as rmcp's
                // loop reads it: in a `select!`, while something else ends
                // the conversation.
                let mut wire = tokio::fs::File::from_std(File::from(OwnedFd::from(input)));
                let mut buffer = [0; 64];
                tokio::select! {
                    _ = wire.read(&mut buffer) => "the client spoke",
                    () = tokio::time::sleep(Duration::from_millis(100)) => "the conversation ended",
                }
            });
            let _ = tell.send(outcome);
        });

        // Far past the bound, so a busy machine cannot fail it: a runtime
        // still waiting on the read would wait forever.
        let outcome = ended.recv_timeout(Duration::from_secs(5));
        // Closed only now, which lets go of a runtime still waiting on it.
        drop(client);
        assert_eq!(outcome, Ok("the conversation ended"));
    }
}
