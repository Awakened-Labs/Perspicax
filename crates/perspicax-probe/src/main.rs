//! `perspicax-probe` -- the tool that makes a milestone demonstrable.
//!
//! `dump` prints a semantic tree, `time` records what reading one costs, and
//! `explain` says why a node was refused. The latency numbers are not
//! decoration: the case for ever building a faster ingest path rests on them,
//! so they get measured from M1 rather than asserted.
//!
//! # Running it
//!
//! It needs a graphical session with an accessibility bus, and over SSH that
//! means two variables nothing will remind you about:
//!
//! ```sh
//! export XDG_RUNTIME_DIR=/run/user/$(id -u)
//! export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
//! perspicax-probe dump --app gtk4-widget-factory
//! ```
//!
//! Without them the bus looks empty rather than absent, which is a confusing
//! hour if nobody wrote it down.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use perspicax_atspi::{AtspiIngest, Strategy};
use perspicax_index::{Index, Ingest, Selector, check_actable};
use perspicax_node::{NodeId, ObservedNode};

#[derive(Parser)]
#[command(
    name = "perspicax-probe",
    about = "Read a semantic tree off the accessibility bus, time it, or ask why a node was refused.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print an application's semantic tree.
    Dump {
        /// The application's name on the accessibility bus.
        #[arg(long)]
        app: String,
        /// Emit JSON instead of an indented tree.
        #[arg(long)]
        json: bool,
        /// Also read per-node bounds, at one round trip per node.
        #[arg(long)]
        geometry: bool,
    },
    /// Measure what reading that tree costs.
    Time {
        /// The application's name on the accessibility bus.
        #[arg(long)]
        app: String,
        /// How many times to repeat each measurement. The fastest run is
        /// reported, because the thing being measured is a cost floor and
        /// every source of noise on a live desktop only ever adds.
        #[arg(long, default_value_t = 5)]
        runs: u32,
    },
    /// Resolve a selector and say whether it may be acted on.
    Explain {
        /// The application's name on the accessibility bus.
        #[arg(long)]
        app: String,
        /// A selector: `button:Cancel`, `menu:File>Open`, `dialog>button[1]`.
        #[arg(long)]
        selector: String,
    },
    /// List the applications currently on the accessibility bus.
    Apps,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Dump {
            app,
            json,
            geometry,
        } => dump(&app, json, geometry).await,
        Command::Time { app, runs } => time(&app, runs).await,
        Command::Explain { app, selector } => explain(&app, &selector).await,
        Command::Apps => apps().await,
    }
}

/// Read a tree and print it.
async fn dump(app: &str, json: bool, geometry: bool) -> Result<()> {
    let mut ingest = AtspiIngest::connect(app)
        .await
        .with_context(|| format!("connecting to {app:?}"))?
        .with_geometry(geometry);
    let root = ingest.root_id();

    let started = Instant::now();
    let nodes = ingest.snapshot(root).await.context("reading the tree")?;
    let elapsed = started.elapsed();

    let mut index = Index::new();
    index.ingest_snapshot(nodes);

    if json {
        println!("{}", serde_json::to_string_pretty(&as_json(&index))?);
        return Ok(());
    }

    for id in index.preorder() {
        let Some(node) = index.get(id) else { continue };
        println!("{}", line(&index, id, node));
    }
    println!();
    println!(
        "{} nodes via {:?} in {}",
        index.len(),
        ingest.strategy().context("no strategy was recorded")?,
        human(elapsed),
    );
    // Not a footnote. With no compositor there is no `HostView`, so no node
    // has an origin or a visibility, and every one of them is refused. That is
    // M1 working, not M1 missing something.
    println!("every node is Unattributed and un-actable: no compositor exists yet");
    Ok(())
}

/// One line of the indented tree.
fn line(index: &Index, id: NodeId, node: &ObservedNode) -> String {
    let depth = depth_of(index, id);
    let label = node
        .node
        .label()
        .map(|label| format!(" {label:?}"))
        .unwrap_or_default();
    let bounds = node
        .bounds()
        .map(|r| {
            format!(
                "  [{:.0},{:.0} {:.0}x{:.0}]",
                r.x0,
                r.y0,
                r.x1 - r.x0,
                r.y1 - r.y0
            )
        })
        .unwrap_or_default();
    let stale = if index.is_stale(id) { " (stale)" } else { "" };
    format!(
        "{:indent$}{:?}{label}{bounds}{stale}   #{}",
        "",
        node.node.role(),
        id.0,
        indent = depth * 2,
    )
}

/// How deep a node sits, by counting ancestors. Quadratic in principle and
/// irrelevant in practice: this runs once per node of a tree a human is about
/// to read, and the alternative is threading depth through the index for the
/// benefit of one printer.
fn depth_of(index: &Index, id: NodeId) -> usize {
    let mut depth = 0;
    for root in index.roots() {
        if root == id {
            return 0;
        }
        if let Some(found) = descend(index, root, id, 1) {
            depth = found;
            break;
        }
    }
    depth
}

fn descend(index: &Index, from: NodeId, target: NodeId, depth: usize) -> Option<usize> {
    let node = index.get(from)?;
    for child in node.node.children() {
        if *child == target {
            return Some(depth);
        }
        if let Some(found) = descend(index, *child, target, depth + 1) {
            return Some(found);
        }
    }
    None
}

/// The tree as JSON, in a shape this project owns.
///
/// Deliberately not a serialisation of AccessKit's `Node`: that would make the
/// upstream crate's internal representation part of this tool's output, and a
/// consumer would start depending on it. These are the fields M1 has something
/// to say about, including the two that are always empty and are the point.
fn as_json(index: &Index) -> serde_json::Value {
    let nodes: Vec<serde_json::Value> = index
        .preorder()
        .into_iter()
        .filter_map(|id| {
            let node = index.get(id)?;
            Some(serde_json::json!({
                "id": id.0,
                "role": format!("{:?}", node.node.role()),
                "label": node.node.label(),
                "description": node.node.description(),
                "children": node.node.children().iter().map(|c| c.0).collect::<Vec<_>>(),
                "bounds": node.bounds().map(|r| serde_json::json!({
                    "x": r.x0, "y": r.y0, "width": r.x1 - r.x0, "height": r.y1 - r.y0,
                    "space": "window-relative",
                })),
                "origin": format!("{:?}", node.origin),
                "visibility": format!("{:?}", node.visibility),
                "actable": check_actable(node).is_ok(),
                "stale": index.is_stale(id),
            }))
        })
        .collect();
    serde_json::json!({ "nodes": nodes })
}

/// Measure what a tree costs, by each route that can produce one.
///
/// Four numbers, and the gaps between them are the deliverable:
///
/// - **probe** -- what a caller actually pays, strategy chosen by the probe.
/// - **cache** -- the one-round-trip read, forced. May return fewer nodes than
///   exist, or none at all; the node count next to the time says which, and a
///   fast answer to the wrong question is not a fast answer.
/// - **walk** -- `Accessible.GetChildren` plus six property reads per node.
/// - **walk + geometry** -- the same, plus `Component.GetExtents` per node,
///   because no bulk geometry API exists on either toolkit.
/// - **drain** -- the standing cost of *staying* current instead of re-reading.
async fn time(app: &str, runs: u32) -> Result<()> {
    println!("{app}  (best of {runs})");

    // First, on the application exactly as found. This is the only row that
    // reflects a cold cache, and it is also the only row that cannot be
    // compared with the others -- see the note printed underneath.
    let (first_time, first_nodes, strategy) = {
        let mut ingest = AtspiIngest::connect(app).await?;
        let root = ingest.root_id();
        let started = Instant::now();
        let nodes = ingest.snapshot(root).await?;
        (started.elapsed(), nodes.len(), ingest.strategy())
    };
    row(
        "first read",
        first_time,
        first_nodes,
        format!("{strategy:?}"),
    );

    // Then warm the application, untimed, so every row below is measuring the
    // same thing. Without this the numbers come out in the order they were
    // taken rather than in the order of what they cost: reading a cold GTK
    // tree realises hundreds of accessibles, and whichever strategy ran first
    // pays for all of them. Measured before this warm-up existed, "walk +
    // geometry" came out nearly twice as fast as "walk", which is impossible
    // -- geometry is a strictly extra round trip per node.
    {
        let mut ingest = AtspiIngest::connect(app).await?.forcing(Strategy::Walk);
        let root = ingest.root_id();
        ingest.snapshot(root).await?;
    }

    for (label, strategy, geometry) in [
        ("cache", Strategy::Cache, false),
        ("walk", Strategy::Walk, false),
        ("walk + geometry", Strategy::Walk, true),
    ] {
        let mut best = Duration::MAX;
        let mut counted = 0;
        for _ in 0..runs.max(1) {
            let mut ingest = AtspiIngest::connect(app)
                .await?
                .forcing(strategy)
                .with_geometry(geometry);
            let root = ingest.root_id();
            let started = Instant::now();
            let nodes = ingest.snapshot(root).await?;
            best = best.min(started.elapsed());
            counted = nodes.len();
        }
        row(label, best, counted, String::new());
    }

    // The number the whole argument turns on. A drain asks the application
    // nothing: it reads signals that already arrived. Compare it to the rows
    // above, which is the difference between staying current and starting over.
    let mut ingest = AtspiIngest::connect(app).await?;
    let root = ingest.root_id();
    ingest.snapshot(root).await?;
    let mut drained = 0;
    let started = Instant::now();
    for _ in 0..100 {
        drained += ingest.drain_changes().await?.len();
    }
    let per_call = started.elapsed() / 100;
    println!(
        "  {:<16} {:>10}  per call, {drained} change(s) seen over 100 calls",
        "drain",
        human(per_call)
    );
    println!();
    println!(
        "  `first read` is the application as found, measured once: it is the only row\n\
         that can see a cold cache, and so the only one that cannot be repeated. The\n\
         rows below it follow a warm-up walk and compare with each other. A cache row\n\
         reporting fewer nodes than the walk is a cache that has not filled up yet,\n\
         and a fast answer to the wrong question is not a fast answer."
    );
    Ok(())
}

fn row(label: &str, elapsed: Duration, nodes: usize, note: String) {
    println!(
        "  {label:<16} {:>10}  {nodes:>5} nodes  {note}",
        human(elapsed)
    );
}

/// Durations a person can compare at a glance, which `Debug` does not give:
/// `1.2ms` and `987.6µs` sort visually the wrong way round.
fn human(elapsed: Duration) -> String {
    let micros = elapsed.as_secs_f64() * 1_000_000.0;
    if micros >= 1_000_000.0 {
        format!("{:.2}s", micros / 1_000_000.0)
    } else if micros >= 1_000.0 {
        format!("{:.1}ms", micros / 1_000.0)
    } else {
        format!("{micros:.0}us")
    }
}

/// Resolve a selector, then ask the gate whether it may be acted on.
///
/// In M1 the answer is always no, and printing it is the point of the
/// subcommand. There is no compositor, so nothing has an origin, so
/// `check_actable` refuses -- and it refuses by *naming the reason*, which is
/// what makes a refusal recoverable rather than a dead end.
async fn explain(app: &str, selector: &str) -> Result<()> {
    let parsed = Selector::parse(selector)
        .map_err(|error| anyhow::anyhow!("{error}"))
        .with_context(|| format!("parsing selector {selector:?}"))?;

    let mut ingest = AtspiIngest::connect(app).await?;
    let root = ingest.root_id();
    let mut index = Index::new();
    index.ingest_snapshot(ingest.snapshot(root).await?);

    println!("selector  {selector}");
    println!(
        "matches   {} of {} nodes",
        index.resolve_all(&parsed).len(),
        index.len()
    );

    match index.resolve(&parsed) {
        Err(refusal) => println!("REFUSED   {refusal}"),
        Ok(id) => {
            let node = index.get(id).context("resolved a node the index lost")?;
            println!(
                "node      #{} {:?} {:?}",
                id.0,
                node.node.role(),
                node.node.label()
            );
            match check_actable(node) {
                Ok(()) => println!("ACTABLE   a compositor has judged this node"),
                Err(refusal) => {
                    println!("REFUSED   {refusal}");
                    println!();
                    println!(
                        "This is M1 behaving correctly. Origin and Visibility are a compositor's\n\
                         to fill, no compositor exists yet, and the gate fails closed rather than\n\
                         guessing. Every node reads the same way."
                    );
                }
            }
        }
    }
    Ok(())
}

/// What is on the bus -- the first thing to run when `--app` finds nothing.
async fn apps() -> Result<()> {
    let apps = perspicax_atspi::on_the_bus()
        .await
        .context("reading the bus")?;
    if apps.is_empty() {
        println!("nothing is on the accessibility bus.");
        println!(
            "If the desktop visibly has windows, this process is talking to a different bus --\n\
             over SSH that means XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS are unset."
        );
        return Ok(());
    }
    for app in &apps {
        // The pid is the bus daemon's, from peer credentials -- and still not
        // an attribution. See `perspicax_atspi::AppRef`.
        println!(
            "{:<28} toolkit={:<8} bus={:<8} pid={}",
            app.name(),
            app.toolkit(),
            app.root().bus(),
            app.bus_pid()
                .map_or_else(|| "?".to_owned(), |pid| pid.to_string()),
        );
    }
    Ok(())
}
