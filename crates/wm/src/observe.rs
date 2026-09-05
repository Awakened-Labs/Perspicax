//! Reading the desktop this compositor is hosting, and keeping it read.
//!
//! This is where the two halves of the project meet. An accessibility bridge
//! describes trees and knows nothing about surfaces; a compositor holds
//! surfaces and cannot read a widget. Everything the project claims over a
//! library that scrapes AT-SPI from outside happens in the handful of lines
//! below that put a bridge's window next to a host's surface and decide whether
//! they are the same thing.
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
//!
//! # One index, and one id space inside it
//!
//! Every application lands in the **same** [`Index`], minting ids from the same
//! [`Ids`]. That was not true until M3 slice 5, and the difference is invisible
//! until ids leave the process: AT-SPI numbers object paths per application, so
//! `/org/a11y/atspi/accessible/1` exists in every program on the bus, and an
//! interner each meant two applications both minted id 1. Nothing above could
//! tell those two nodes apart -- least of all the MCP server, which hands an
//! agent an id and takes it back.
//!
//! # Read once, then listen
//!
//! [`observe`] is the expensive half and runs once: seconds against a cold Qt
//! tree. After it, each [`App`] is still attached and still subscribed, so
//! [`App::changes`] takes what the bus has volunteered without asking anything.
//! Polling a tree is the failure mode this project exists to remove, and the
//! subscription was opened before the snapshot precisely so that nothing can
//! change in the gap between the two.

use std::{collections::HashSet, time::Duration};

use anyhow::{Context as _, Result};
use wm_atspi::{AppRef, AtspiIngest, Ids};
use wm_compositor::Facts;
use wm_index::{
    Change, Finding, HostFacts, Index, Ingest as _, Join, SurfaceClaim, SurfaceFacts, Tally,
    WindowClaim, join,
};
use wm_node::{NodeId, ObservedNode, Origin, Role, SurfaceId};

/// How long one application gets to be read before it is given up on.
///
/// Generous: the M1 measurements put a cold Qt tree with geometry at about
/// three seconds, and a loaded machine is slower. It exists because the
/// alternative was observed rather than imagined -- `gnome-shell` on the test
/// bed answers the registry and then fails its own peer handshake, and reading
/// it never returns. One application must not be able to hide the desktop, and
/// saying so in a doc comment is not the same as arranging it.
const PER_APP: Duration = Duration::from_secs(20);

/// One application this compositor drew: what it turned out to be, and the
/// live connection it is still being read through.
pub struct App {
    /// Its name on the accessibility bus.
    pub name: String,
    /// The toolkit it reports. Diagnostic only.
    pub toolkit: String,
    /// Its root node, in the desktop's shared index.
    pub root: NodeId,
    /// Its toplevels, bound to surfaces.
    pub joins: Vec<Join>,
    /// Its toplevels that could not be bound, and why.
    pub findings: Vec<Finding>,
    /// Still attached, and still subscribed. This is what makes staying
    /// current a matter of listening rather than reading again.
    ingest: AtspiIngest,
}

impl App {
    /// Every node of this application, in tree order, root first.
    ///
    /// The index is the desktop's, so an application is a subtree of it rather
    /// than a thing of its own -- and this is how a caller asks for its part.
    #[must_use]
    pub fn nodes(&self, index: &Index) -> Vec<NodeId> {
        let mut nodes = vec![self.root];
        nodes.extend(index.descendants(self.root));
        nodes
    }

    /// Take what this application has volunteered since the last call.
    ///
    /// Volunteered, not fetched: this drains a queue of signals that already
    /// arrived and asks the application nothing, so it cannot block and on a
    /// quiet desktop it returns immediately.
    ///
    /// # Errors
    ///
    /// Fails if the accessibility bus connection has gone.
    pub async fn changes(&mut self) -> Result<Vec<Change>> {
        self.ingest
            .drain_changes()
            .await
            .with_context(|| format!("could not drain changes from {}", self.name))
    }

    /// Read this application's whole tree again.
    ///
    /// The answer to [`Change::SubtreeInvalidated`], which says a subtree
    /// changed shape and cannot be trusted until it is read. Expensive, and
    /// deliberately the caller's decision rather than something
    /// [`App::changes`] does on its own.
    ///
    /// # Errors
    ///
    /// Fails if the application cannot be read -- it has exited, or the bus
    /// has gone.
    pub async fn reread(&mut self) -> Result<Vec<ObservedNode>> {
        let root = self.root;
        tokio::time::timeout(PER_APP, self.ingest.snapshot(root))
            .await
            .with_context(|| format!("gave up re-reading {}", self.name))?
            .with_context(|| format!("could not re-read {}", self.name))
    }

    /// Attribute this application's windows to their surfaces again.
    ///
    /// Cheap -- a tree walk, no I/O -- and necessary after any change, because
    /// a node the bus has just volunteered arrives unjoined and therefore
    /// un-actable. Every node under a window was drawn on that window's
    /// surface, which is [`Index::join_subtree`]'s own rule, so re-running it
    /// is the whole of re-attribution.
    pub fn rejoin(&self, index: &mut Index, facts: &HostFacts) {
        for join in &self.joins {
            let origin = facts
                .surface(join.surface)
                .map_or(Origin::Unattributed, |facts| facts.origin.clone());
            index.join_subtree(join.node, join.surface, &origin);
        }
    }
}

/// What one read of the desktop found.
pub struct Reading {
    /// Every application's tree, in one index and one id space.
    pub index: Index,
    /// The applications, still attached.
    pub apps: Vec<App>,
    /// What judging the lot decided.
    pub tally: Tally,
    /// How many nodes sit under rendering that arrived after this read and
    /// that no semantic event explained.
    pub unexplained: usize,
}

/// Read every application on the accessibility bus and join it to the host.
///
/// # Errors
///
/// Fails only if the accessibility bus itself cannot be reached. An individual
/// application that cannot be read is reported and skipped: one misbehaving
/// toolkit must not be able to hide the desktop.
pub async fn observe(facts: &Facts) -> Result<Reading> {
    let bus = wm_atspi::on_the_bus()
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

    // One id map for the desktop, handed to every ingest below. See the module
    // documentation for what an interner each costs.
    let ids = Ids::default();
    let mut index = Index::new();
    let mut apps = Vec::new();
    let mut skipped = 0;

    for app in bus {
        if !app.bus_pid().is_some_and(|pid| ours.contains(&pid)) {
            skipped += 1;
            continue;
        }
        let (name, toolkit) = (app.name().to_owned(), app.toolkit().to_owned());
        // By reference, not by name. A desktop can be running two copies of one
        // program -- the test bed was, one of them a leftover -- and resolving
        // by name reads the first twice while never reaching the second.
        match tokio::time::timeout(PER_APP, read_app(app, &ids, &mut index, facts)).await {
            Ok(Ok(mut app)) => {
                app.toolkit = toolkit;
                apps.push(app);
            }
            Ok(Err(error)) => tracing::warn!(app = %name, %error, "could not read application"),
            Err(_) => tracing::warn!(app = %name, "gave up reading application"),
        }
    }
    if skipped > 0 {
        tracing::info!(
            skipped,
            "applications on the bus that this host did not draw"
        );
    }

    // Judged and staled against the host as it is NOW, which is the whole
    // point: the difference between when each tree was read and now is exactly
    // what a reader needs to be told it missed.
    let now = facts.read();
    let tally = index.judge(&now);
    let unexplained = index.under_damage(&now).len();

    Ok(Reading {
        index,
        apps,
        tally,
        unexplained,
    })
}

/// Read one application into the desktop's index, and join it.
async fn read_app(app: AppRef, ids: &Ids, index: &mut Index, facts: &Facts) -> Result<App> {
    let name = app.name().to_owned();
    let mut ingest = AtspiIngest::attach(app)
        .await?
        .sharing(ids)
        .with_geometry(true);
    let root = ingest.root_id();

    // The state of the host *before* the read, kept so the reconciliation
    // below can be honest about what this tree does and does not contain.
    let before = facts.read();
    let surfaces: Vec<SurfaceClaim> = before.surfaces().iter().map(SurfaceFacts::claim).collect();

    let nodes = ingest.snapshot(root).await?;
    index.ingest_snapshot(nodes);

    let windows = toplevels(index, root, ingest.app().bus_pid());
    let (joins, findings) = join(&windows, &surfaces);

    let app = App {
        name,
        toolkit: String::new(),
        root,
        joins,
        findings,
        ingest,
    };
    app.rejoin(index, &before);

    for join in &app.joins {
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

    Ok(app)
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
pub fn report(reading: &Reading, host: &HostFacts) {
    let index = &reading.index;
    for app in &reading.apps {
        println!(
            "\n{} ({}) -- {} nodes",
            app.name,
            if app.toolkit.is_empty() {
                "unknown toolkit"
            } else {
                &app.toolkit
            },
            app.nodes(index).len()
        );

        for join in &app.joins {
            let title = index
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

            // The measurement M2 owes: node space against the window geometry
            // the host placed. A bridge whose window node reports its own
            // extents as `0,0 -> w,h` is measuring from the same origin the
            // compositor calls the window's, and `node_space_offset` is
            // genuinely zero. Anything else is the shadow margin, and printing
            // both is how that stops being a guess.
            let node_bounds = index.get(join.node).and_then(|node| node.bounds());
            let surface = index.get(join.node).and_then(|node| node.surface);
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

        // The two numbers M2 exists to produce, for one node each, so a human
        // can see the join actually reached the leaves.
        if let Some(sample) = sample_node(index, app) {
            let node = index.get(sample).expect("just found");
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

    println!(
        "\n{}\n{} of them under rendering no semantic event explained",
        reading.tally, reading.unexplained
    );
}

/// A leaf worth showing: something with a label, a role that is not a
/// container, and bounds.
fn sample_node(index: &Index, app: &App) -> Option<NodeId> {
    app.nodes(index).into_iter().find(|id| {
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
