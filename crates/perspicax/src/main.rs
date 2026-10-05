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
//! It needs `XDG_RUNTIME_DIR` set, which is where the Wayland socket goes. Over
//! SSH that is `export XDG_RUNTIME_DIR=/run/user/$(id -u)`, the same variable
//! `perspicax-probe` needs for the accessibility bus and for the same reason: a login
//! shell has a session, and an SSH command does not.
//!
//! Set `RUST_LOG=info` for a running account of clients connecting and windows
//! mapping; headless, that log is the only way to watch a compositor that
//! deliberately draws nothing.

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use perspicax::{desk::Desk, keep::keep_current, observe, session};
use perspicax_compositor::{Backend, Config, Facts, Host, Requests, Stop, Virtual};

#[derive(Parser)]
#[command(
    name = "perspicax",
    about = "An agent-native Wayland compositor: everything on screen, as a typed API.",
    version,
    group = clap::ArgGroup::new("backend").required(true).args(["headless", "seat"])
)]
struct Cli {
    /// Run with no window system at all: a virtual output, no rendering, no
    /// GPU. What CI runs, and what an agent with no person present wants.
    ///
    /// One of `--headless` or `--seat` is required rather than defaulted, so
    /// that no script written against one silently starts meaning the other.
    #[arg(long)]
    headless: bool,

    /// Run as a real session on this machine's seat: the connected monitors,
    /// the keyboards and pointers, device access through libseat. Start it
    /// from a TTY. Needs a build with `--features seat` (or `desktop`).
    #[arg(long)]
    seat: bool,

    /// A virtual output's size, as `WIDTHxHEIGHT`. Repeat it for more than one
    /// monitor: they are named `HEADLESS-1`, `HEADLESS-2`, ... and placed left
    /// to right in the order given. Every global rectangle the index reports
    /// is in the space they make. Headless only: a seat's outputs are the size
    /// its monitors are.
    #[arg(
        long,
        default_value = "1920x1080",
        value_parser = parse_size,
        conflicts_with = "seat"
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
    /// goes to stderr in every mode; see the note in `main`.
    ///
    /// The session ends when the client does: an agent-driven compositor with
    /// no agent left has nothing to host.
    #[arg(long)]
    mcp: bool,

    /// How long `--mcp` lets an application draw before reading it.
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

fn main() -> Result<()> {
    // STDERR, AND NOT STDOUT. Under `--mcp` stdout is the JSON-RPC wire, and a
    // single log line written to it corrupts a frame -- which arrives at the
    // client as a parse error with nothing in it pointing back here. Stderr
    // unconditionally rather than only under `--mcp`, because a log destination
    // that depends on a flag is one somebody adds a `println!` next to.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let backend = if cli.seat {
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

    // Before anything is spawned, and in this order. A registry that arrives
    // after its clients is a registry GTK has already given up on.
    let _registry = session::Registry::ensure()?;
    if let Err(error) = enable_accessibility() {
        tracing::warn!("{error:#}");
    }

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
        env: session::accessibility_env(),
        run_for: cli.run_for.map(Duration::from_secs_f64),
        config: cli
            .seat
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

    if let Some(after) = cli.dump_tree.map(Duration::from_secs_f64) {
        dump_when_ready(facts.clone(), stop.clone(), after);
    }

    if cli.mcp {
        let desk = Arc::new(Desk::new(&facts, &Host::new(&facts, &requests)));
        serve(Arc::clone(&desk), stop.clone());
        keep_current(
            desk,
            facts.clone(),
            stop.clone(),
            Duration::from_secs_f64(cli.settle),
        );
    }

    perspicax_compositor::run(&config, &facts, &requests, &stop).context("the compositor stopped")
}

/// Serve the agent interface on its own thread, with its own runtime.
///
/// The same arrangement `dump_when_ready` uses and for the same reason: Wayland
/// state is not `Send` and the calloop loop must not be blocked, while an MCP
/// server spends its life waiting on a pipe.
fn serve(desk: Arc<Desk>, stop: Stop) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(%error, "no runtime for the agent interface");
                stop.request();
                return;
            }
        };

        if let Err(error) = runtime.block_on(perspicax_mcp::serve(desk)) {
            tracing::error!(%error, "the agent interface stopped");
        }
        // The client going away ends the session. This process exists to be
        // driven, and there is nobody left to drive it.
        stop.request();
    });
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
