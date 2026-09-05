//! Reading the desktop this compositor is hosting, and saying what it found.
//!
//! This is where the two halves of the project meet for the first time. An
//! accessibility bridge describes trees and knows nothing about surfaces; a
//! compositor holds surfaces and cannot read a widget. Everything the project
//! claims over a library that scrapes AT-SPI from outside happens in the
//! handful of lines below that put a bridge's window next to a host's surface
//! and decide whether they are the same thing.
//!
//! The order is deliberate and each step depends on the one before it:
//!
//! 1. Read each application's tree off the accessibility bus, **with geometry**
//!    -- which costs a round trip per node and is the reason the M1 latency
//!    table exists. Without bounds there is nothing to judge.
//! 2. Offer its toplevels as [`WindowClaim`]s and the host's surfaces as
//!    [`SurfaceClaim`]s, and let `wm_index::join` weigh them.
//! 3. Attribute every joined subtree, which is the first moment any node in
//!    this system has ever had an `Origin`.
//! 4. Judge the lot against the host's published facts.
//!
//! A window that does not join keeps `Origin::Unattributed`, every node under
//! it stays un-actable, and the reason appears as a [`Finding`] rather than in
//! a log nobody reads.

use std::{collections::HashSet, time::Duration};

use anyhow::{Context as _, Result};
use wm_atspi::{AppRef, AtspiIngest};
use wm_compositor::Facts;
use wm_index::{
    Finding, HostFacts, Index, Ingest as _, Join, SurfaceClaim, SurfaceFacts, Tally, WindowClaim,
    join,
};
use wm_node::{NodeId, Origin, Role, SurfaceId};

/// How long one application gets to be read before it is given up on.
///
/// Generous: the M1 measurements put a cold Qt tree with geometry at about
/// three seconds, and a loaded machine is slower. It exists because the
/// alternative was observed rather than imagined -- `gnome-shell` on the test
/// bed answers the registry and then fails its own peer handshake, and reading
/// it never returns. One application must not be able to hide the desktop, and
/// saying so in a doc comment is not the same as arranging it.
const PER_APP: Duration = Duration::from_secs(20);

/// What one application turned out to be.
pub struct AppReport {
    /// Its name on the accessibility bus.
    pub name: String,
    /// The toolkit it reports. Diagnostic only.
    pub toolkit: String,
    /// Its whole tree, attributed and judged.
    pub index: Index,
    /// Its toplevels, bound to surfaces.
    pub joins: Vec<Join>,
    /// Its toplevels that could not be bound, and why.
    pub findings: Vec<Finding>,
    /// What judging its nodes decided.
    pub tally: Tally,
    /// How many of its nodes sit under rendering that arrived after this read
    /// and that no semantic event explained.
    pub unexplained: usize,
}

/// Read every application on the accessibility bus and join it to the host.
///
/// # Errors
///
/// Fails only if the accessibility bus itself cannot be reached. An individual
/// application that cannot be read is reported and skipped: one misbehaving
/// toolkit must not be able to hide the desktop.
pub async fn observe(facts: &Facts) -> Result<Vec<AppReport>> {
    let apps = wm_atspi::on_the_bus()
        .await
        .context("the accessibility bus could not be reached")?;

    // Only applications this host actually drew. The pid is the join's gate, so
    // an application whose process owns none of our surfaces cannot bind to one
    // however promising its windows look -- and reading it would cost a D-Bus
    // round trip per node to reach that conclusion. On the test bed the
    // difference is the whole GNOME session: gnome-shell, seven gsd-* daemons,
    // and two leftover copies of the demo applications, none of which this
    // compositor is hosting.
    let ours: HashSet<u32> = facts
        .read()
        .surfaces()
        .iter()
        .filter_map(|surface| surface.claim().pid)
        .collect();

    let mut reports = Vec::new();
    let mut skipped = 0;
    for app in apps {
        if !app.bus_pid().is_some_and(|pid| ours.contains(&pid)) {
            skipped += 1;
            continue;
        }
        let (name, toolkit) = (app.name().to_owned(), app.toolkit().to_owned());
        // By reference, not by name. A desktop can be running two copies of one
        // program -- the test bed was, one of them a leftover -- and resolving
        // by name reads the first twice while never reaching the second.
        match tokio::time::timeout(PER_APP, read_app(app, facts)).await {
            Ok(Ok(mut report)) => {
                report.toolkit = toolkit;
                reports.push(report);
            }
            Ok(Err(error)) => {
                tracing::warn!(app = %name, %error, "could not read application");
            }
            Err(_) => tracing::warn!(app = %name, "gave up reading application"),
        }
    }
    if skipped > 0 {
        tracing::info!(
            skipped,
            "applications on the bus that this host did not draw"
        );
    }
    Ok(reports)
}

/// Read one application, join it, and judge it.
async fn read_app(app: AppRef, facts: &Facts) -> Result<AppReport> {
    let name = app.name().to_owned();
    let mut ingest = AtspiIngest::attach(app).await?.with_geometry(true);
    let root = ingest.root_id();

    // The state of the host *before* the read, kept so the reconciliation
    // below can be honest about what this tree does and does not contain.
    let before = facts.read();
    let surfaces: Vec<SurfaceClaim> = before.surfaces().iter().map(SurfaceFacts::claim).collect();

    let nodes = ingest.snapshot(root).await?;

    let mut index = Index::new();
    index.ingest_snapshot(nodes);

    let windows = toplevels(&index, root, ingest.app().bus_pid());
    let (joins, findings) = join(&windows, &surfaces);

    for join in &joins {
        let origin = before
            .surface(join.surface)
            .map_or(Origin::Unattributed, |facts| facts.origin.clone());
        index.join_subtree(join.node, join.surface, &origin);

        // Credited with the generation the surface was at when the read
        // STARTED. A GTK tree takes tens of milliseconds at best, during which
        // that application repaints its window perhaps twice; crediting the
        // read with where the counter finished would quietly claim those
        // frames had been seen.
        let generation = before
            .surface(join.surface)
            .map_or(0, |facts| facts.damage_generation);
        index.reconcile(join.surface, generation);
    }

    // Judged and staled against the host as it is NOW, which is the whole
    // point: the difference between then and now is exactly what a reader
    // needs to be told it missed.
    let now = facts.read();
    let tally = index.judge(&now);
    let unexplained = index.under_damage(&now).len();

    Ok(AppReport {
        name,
        toolkit: String::new(),
        index,
        joins,
        findings,
        tally,
        unexplained,
    })
}

/// An application's toplevels, as claims to be weighed.
///
/// **The children of the root, whatever role they carry.** Searching for
/// `Role::Window` would find GTK's `frame` and miss the Qt widget gallery
/// entirely, because Qt offers its main window as an AT-SPI `dialog` -- and the
/// symptom would be a toolkit silently reporting no windows rather than an
/// error anyone could chase.
fn toplevels(index: &Index, root: NodeId, bus_pid: Option<u32>) -> Vec<WindowClaim> {
    let Some(node) = index.get(root) else {
        return Vec::new();
    };
    node.node
        .children()
        .iter()
        .filter_map(|child| index.get(*child))
        .map(|window| WindowClaim {
            node: window.id,
            title: window.node.label().map(Into::into),
            bus_pid,
            // Not plumbed, and the reason is a real one rather than an
            // omission: this ingest connects to the bus after the windows it
            // is looking at have already mapped, so the activation that would
            // correlate with the host's focus has long since gone by. Making
            // that evidence available means an ingest that is running before
            // the first window appears -- which is the shape M3 needs anyway,
            // and is not this slice.
            active_at: None,
        })
        .collect()
}

/// Print what was found, in the order somebody debugging it would want.
pub fn report(reports: &[AppReport], host: &HostFacts) {
    for app in reports {
        println!(
            "\n{} ({}) -- {} nodes",
            app.name,
            if app.toolkit.is_empty() {
                "unknown toolkit"
            } else {
                &app.toolkit
            },
            app.index.len()
        );

        for join in &app.joins {
            let title = app
                .index
                .get(join.node)
                .and_then(|node| node.node.label())
                .unwrap_or_default()
                .to_owned();
            println!(
                "  window {} {:?} -> surface {}  [{}]",
                join.node.0,
                title,
                join.surface.0,
                join.evidence
                    .iter()
                    .map(|evidence| format!("{evidence:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            // The measurement this milestone owes: node space against the
            // window geometry the host placed. A bridge whose window node
            // reports its own extents as `0,0 -> w,h` is measuring from the
            // same origin the compositor calls the window's, and
            // `node_space_offset` is genuinely zero. Anything else is the
            // shadow margin, and printing both is how that stops being a
            // guess.
            let node_bounds = app.index.get(join.node).and_then(|node| node.bounds());
            let surface = app.index.get(join.node).and_then(|node| node.surface);
            println!(
                "    damage: {} frames",
                surface
                    .and_then(|id| host.surface(id))
                    .map_or(0, |facts| facts.damage_generation),
            );
            println!(
                "    node space: {:?}   host geometry: {:?}",
                node_bounds.map(|b| (b.x0, b.y0, b.x1, b.y1)),
                surface.and_then(|id| host.surface(id)).map(|facts| (
                    facts.geometry.x0,
                    facts.geometry.y0,
                    facts.geometry.x1,
                    facts.geometry.y1
                )),
            );
        }
        for finding in &app.findings {
            println!("  FINDING: {finding}");
        }
        println!(
            "  {}\n  {} of them under rendering no semantic event explained",
            app.tally, app.unexplained
        );

        // The two numbers M2 exists to produce, for one node each, so a human
        // can see the join actually reached the leaves.
        if let Some(sample) = sample_node(&app.index) {
            let node = app.index.get(sample).expect("just found");
            println!(
                "  e.g. node {} {:?} {:?} on {:?}: {:?} / {:?}",
                sample.0,
                node.node.role(),
                node.node.label().unwrap_or_default(),
                node.surface.map_or(0, |s: SurfaceId| s.0),
                node.origin,
                node.visibility,
            );
        }
    }
}

/// A leaf worth showing: something with a label, a role that is not a
/// container, and bounds.
fn sample_node(index: &Index) -> Option<NodeId> {
    index.preorder().into_iter().find(|id| {
        index.get(*id).is_some_and(|node| {
            node.node.label().is_some()
                && node.bounds().is_some()
                && matches!(
                    node.node.role(),
                    Role::Button | Role::CheckBox | Role::Label
                )
        })
    })
}
