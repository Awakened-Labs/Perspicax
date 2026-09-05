//! `wm` -- the composition root.
//!
//! One binary, because the pieces have to share a process: the semantic index
//! is only worth anything joined to the compositor that can say what is
//! visible, and an agent interface that had to reach either of them over IPC
//! would be paying for a boundary that buys nothing.
//!
//! # Running it
//!
//! ```sh
//! wm --headless --spawn gtk4-widget-factory --run-for 10
//! ```
//!
//! It needs `XDG_RUNTIME_DIR` set, which is where the Wayland socket goes. Over
//! SSH that is `export XDG_RUNTIME_DIR=/run/user/$(id -u)`, the same variable
//! `wm-probe` needs for the accessibility bus and for the same reason: a login
//! shell has a session, and an SSH command does not.
//!
//! Set `RUST_LOG=info` for a running account of clients connecting and windows
//! mapping; that log is currently the only way to watch a compositor that
//! deliberately draws nothing.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use wm::{observe, session};
use wm_compositor::{Config, Facts, Stop};

#[derive(Parser)]
#[command(
    name = "wm",
    about = "An agent-native Wayland compositor: it tracks what is on screen instead of drawing it.",
    version
)]
struct Cli {
    /// Run with no window system at all: a virtual output, no rendering, no
    /// GPU.
    ///
    /// The only mode there is. `--nested`, which runs inside an existing
    /// session and shows you pixels, is named in the plan and not built:
    /// nothing this milestone has to prove needs a picture, and a renderer is
    /// the single largest dependency a compositor can acquire. The flag is
    /// required rather than defaulted so that the day a second mode exists,
    /// no script silently changes meaning.
    #[arg(long)]
    headless: bool,

    /// The virtual output's size, as `WIDTHxHEIGHT`. Every global rectangle the
    /// index reports is in this space.
    #[arg(long, default_value = "1920x1080", value_parser = parse_size)]
    size: (i32, i32),

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
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    if !cli.headless {
        bail!("no backend selected: pass --headless (see --help; it is the only one)");
    }

    // Before anything is spawned, and in this order. A registry that arrives
    // after its clients is a registry GTK has already given up on.
    let _registry = session::Registry::ensure()?;
    if let Err(error) = enable_accessibility() {
        tracing::warn!("{error:#}");
    }

    let config = Config {
        size: cli.size,
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
    };

    // Created here rather than inside the compositor, because the thread that
    // reads these facts is not the thread that publishes them.
    let facts = Facts::new();
    let stop = Stop::new();

    if let Some(after) = cli.dump_tree.map(Duration::from_secs_f64) {
        dump_when_ready(facts.clone(), stop.clone(), after);
    }

    wm_compositor::run(&config, &facts, &stop).context("the compositor stopped")
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
        .block_on(wm_atspi::enable())
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
                Ok(reports) => observe::report(&reports, &facts.read()),
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
