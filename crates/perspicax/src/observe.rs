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
//!    [`SurfaceClaim`]s, and let `perspicax_index::join` weigh them.
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
//! Reading is the expensive half: seconds against a cold Qt tree. After it,
//! each [`App`] is still attached and still subscribed, so [`App::changes`]
//! takes what the bus has volunteered without asking anything. Polling a tree
//! is the failure mode this project exists to remove, and the subscription was
//! opened before the snapshot precisely so that nothing can change in the gap
//! between the two.
//!
//! [`observe`] reads every application drawing on the host, once. Keeping that
//! current as applications come and go is [`crate::keep`]'s, which reads each
//! newcomer with [`read`] and puts it in the index with [`Snapshot::admit`]:
//! the read awaits the bus and touches no index, and the admitting touches the
//! index and awaits nothing, so the index is never held across a wait.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use perspicax_atspi::{AppRef, AtspiIngest, Ids, Spared};
use perspicax_compositor::Facts;
use perspicax_index::{
    Change, Finding, HostFacts, Index, Ingest as _, Join, SurfaceClaim, SurfaceFacts, Tally,
    WindowClaim, join, join::MenuFinding,
};
use perspicax_node::{NodeId, ObservedNode, Origin, Role, SurfaceId, Vec2};

/// How long one application gets to be read: once to connect to it, and once
/// to read it.
///
/// Generous: the M1 measurements put a cold Qt tree with geometry at about
/// three seconds, and a loaded machine is slower. It exists because the
/// alternative was observed rather than imagined -- `gnome-shell` on the test
/// bed answers the registry and then fails its own peer handshake, and reading
/// it never returns. One application must not be able to hide the desktop, and
/// saying so in a doc comment is not the same as arranging it.
///
/// Connecting is cut off when it runs out, since a connection that never
/// finishes has nothing in it. Reading is not: the read is given this as its
/// budget ([`AtspiIngest::within`]) and stops asking when it is spent,
/// keeping what it read. Until #51 it was cut off too, and a Flutter
/// application -- whose `GetExtents` takes 25 s -- lost its whole tree to the
/// one call it left unanswered, every read, for as long as it ran. Every call
/// has a deadline of its own now, so the read ends when its budget does.
const PER_APP: Duration = Duration::from_secs(20);

/// One application this compositor drew: what it turned out to be, and the
/// live connection it is still being read through.
pub struct App {
    /// Its name on the accessibility bus.
    pub name: String,
    /// The toolkit it reports. Diagnostic only.
    pub toolkit: String,
    /// Its process, as the bus reports it: the pid that admitted it to be
    /// read, because it owns surfaces on this host.
    pub pid: u32,
    /// Its root node, in the desktop's shared index.
    pub root: NodeId,
    /// Its toplevels, bound to surfaces.
    pub joins: Vec<Join>,
    /// Its toplevels that could not be bound, and why.
    pub findings: Vec<Finding>,
    /// Its open menus no part of its tree could be bound to, and why
    /// (issue #112).
    pub menus: Vec<MenuFinding>,
    /// Its open menus it has been read again for, since each opened: one
    /// read for each, and no more. See [`App::owes_a_read_for_a_menu`].
    menus_read: HashSet<SurfaceId>,
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
    /// A re-read that runs out of [`PER_APP`] returns what it read by then.
    /// A node it did not reach keeps the mark the invalidation gave it, and
    /// is refused as stale until a later read reaches it -- which the keeper
    /// sees to, reading the application again until one does
    /// ([`crate::keep::Owed`]).
    ///
    /// # Errors
    ///
    /// Fails if the application cannot be read -- it has exited, the bus has
    /// gone, or its root does not answer.
    pub async fn reread(&mut self) -> Result<Vec<ObservedNode>> {
        let root = self.root;
        self.ingest
            .snapshot(root)
            .await
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
        // A window's walk takes back every node beneath it, its open menus'
        // included, so those go back to their popups after it.
        index.restamp_menus(facts);
    }

    /// Bind this application's windows to the host's surfaces again, then
    /// attribute them: [`App::rejoin`], after weighing the windows afresh.
    ///
    /// For when either side changed since the last weighing: a window the
    /// tree gained, a surface the host mapped, one monitor's desktop joined
    /// by a shell when the monitor is plugged in. Cheap, like `rejoin`: the
    /// weighing is over a handful of windows and surfaces, with no I/O.
    pub fn relink(&mut self, index: &mut Index, facts: &HostFacts) {
        let windows = toplevels(index, self.root, Some(self.pid));
        (self.joins, self.findings) = join(&windows, &surfaces(facts));
        self.rejoin(index, facts);
        // Its open menus last, measured against the windows just joined.
        let windows: Vec<(NodeId, SurfaceId)> = self
            .joins
            .iter()
            .map(|join| (join.node, join.surface))
            .collect();
        self.menus = index.join_popups(self.root, self.pid, &windows, facts);
    }

    /// The menus this application has open, as the host describes them.
    pub fn popups<'a>(&self, facts: &'a HostFacts) -> impl Iterator<Item = &'a SurfaceFacts> {
        let pid = self.pid;
        facts.surfaces().iter().filter(move |surface| {
            surface.hangs_from().is_some() && surface.claim().pid == Some(pid)
        })
    }

    /// Whether this application should be read again for a menu it has
    /// open that nothing of its tree is bound to yet: once for each menu,
    /// a moment after it opens.
    ///
    /// GTK 4 announces a menu it opens, and the join binds it from that. GTK
    /// 3 says nothing of a context menu, an untitled window of its own, so
    /// the menu would stay refused as unplaced for as long as it was open,
    /// measured on 2026-10-14. A read after it opened finds it. The moment
    /// is for the announcement to arrive first, and once is enough: a menu
    /// that a read made after it opened does not bind -- a tooltip, or a
    /// menu its toolkit does not describe -- is not read for again.
    pub fn owes_a_read_for_a_menu(
        &mut self,
        index: &Index,
        facts: &HostFacts,
        now: Instant,
    ) -> bool {
        let open: Vec<(SurfaceId, Option<Instant>)> = self
            .popups(facts)
            .filter(|popup| popup.mapped)
            .map(|popup| (popup.id, popup.mapped_at))
            .collect();
        menus_to_read(
            &mut self.menus_read,
            &open,
            |popup| index.menu_of(popup).is_some(),
            now,
        )
    }
}

/// One application, read off the bus and not yet in any index.
pub struct Snapshot {
    name: String,
    toolkit: String,
    pid: u32,
    root: NodeId,
    nodes: Vec<ObservedNode>,
    /// The host as it was when the read began.
    before: HostFacts,
    /// When the read began: what every node it holds is credited with being
    /// seen at, so that a menu that opened while it was under way is not
    /// taken to be in it.
    began: Instant,
    ingest: AtspiIngest,
}

impl Snapshot {
    /// The process this application belongs to.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Put this application into `index`, its windows bound to the surfaces
    /// the host had when the read began.
    pub fn admit(self, index: &mut Index) -> App {
        index.ingest_read(self.nodes, self.began);
        let mut app = App {
            name: self.name,
            toolkit: self.toolkit,
            pid: self.pid,
            root: self.root,
            joins: Vec::new(),
            findings: Vec::new(),
            menus: Vec::new(),
            menus_read: HashSet::new(),
            ingest: self.ingest,
        };
        app.relink(index, &self.before);
        for join in &app.joins {
            // Where the toolkit's coordinates begin, once per window read: a
            // toolkit with a convention nobody has measured yet shows up here
            // as a number, and not as clicks beside their targets.
            match index.window_origin(join.surface) {
                Some(origin) => tracing::info!(
                    app = app.name,
                    window = join.node.0,
                    surface = join.surface.0,
                    x = origin.x,
                    y = origin.y,
                    "the window begins here in its own coordinates"
                ),
                None => tracing::warn!(
                    app = app.name,
                    window = join.node.0,
                    surface = join.surface.0,
                    "the window node reports no extents, so nothing on the \
                     window can be placed and every node on it is refused"
                ),
            }
            // Credited with the generation the surface was at when the read
            // STARTED. A GTK tree takes tens of milliseconds at best, during
            // which that application repaints its window perhaps twice;
            // crediting the read with where the counter finished would
            // quietly claim those frames had been seen.
            let generation = self
                .before
                .surface(join.surface)
                .map_or(0, |facts| facts.damage_generation);
            index.reconcile(join.surface, generation);
        }
        // Its open menus too, which its windows' nodes may be joined to.
        for popup in app.popups(&self.before) {
            index.reconcile(popup.id, popup.damage_generation);
        }
        app
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
    // Only applications this host actually drew. The pid is the join's gate, so
    // an application whose process owns none of our surfaces cannot bind to one
    // however promising its windows look -- and reading it would cost a D-Bus
    // round trip per node to reach that conclusion. On the test bed the
    // difference is the whole GNOME session: gnome-shell, seven gsd-* daemons,
    // and two leftover copies of the demo applications, none of which this
    // compositor is hosting.
    let ours = drawing(&facts.read());

    // One id map for the desktop, and one set of nodes never asked where they
    // are, handed to every ingest below. See the module documentation for
    // what an interner each costs.
    let (ids, spared) = (Ids::default(), Spared::default());
    let mut index = Index::new();
    let apps = read(&ours, &ids, &spared, facts)
        .await?
        .snapshots
        .into_iter()
        .map(|snapshot| snapshot.admit(&mut index))
        .collect();

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

/// Every process that owns a surface on the host.
#[must_use]
pub fn drawing(facts: &HostFacts) -> HashSet<u32> {
    facts
        .surfaces()
        .iter()
        .filter_map(|surface| surface.claim().pid)
        .collect()
}

/// What reading a set of processes found.
#[derive(Default)]
pub struct Reads {
    /// The applications that were read.
    pub snapshots: Vec<Snapshot>,
    /// Every process asked for that is on the accessibility bus, read or
    /// not. One that is on the bus and was not read is an application that
    /// could not be read, which is a different thing to say about it than
    /// that it has not joined the bus yet.
    pub on_the_bus: HashSet<u32>,
}

/// Read every application on the accessibility bus whose process is one of
/// `pids`, minting ids from `ids` and never asking where a node in `spared`
/// is.
///
/// # Errors
///
/// Fails only if the accessibility bus itself cannot be reached. An
/// individual application that cannot be read is reported and skipped: one
/// misbehaving toolkit must not be able to hide the desktop.
pub async fn read(pids: &HashSet<u32>, ids: &Ids, spared: &Spared, facts: &Facts) -> Result<Reads> {
    let bus = perspicax_atspi::on_the_bus()
        .await
        .context("the accessibility bus could not be reached")?;
    let mut reads = Reads::default();
    let mut skipped = 0;
    for app in bus {
        let Some(pid) = app.bus_pid().filter(|pid| pids.contains(pid)) else {
            skipped += 1;
            continue;
        };
        reads.on_the_bus.insert(pid);
        let name = app.name().to_owned();
        // By reference, not by name. A desktop can be running two copies of one
        // program -- the test bed was, one of them a leftover -- and resolving
        // by name reads the first twice while never reaching the second.
        match snapshot(app, pid, ids, spared, facts).await {
            Ok(snapshot) => reads.snapshots.push(snapshot),
            Err(error) => tracing::warn!(app = %name, "could not read application: {error:#}"),
        }
    }
    if skipped > 0 {
        tracing::debug!(skipped, "applications on the bus that were not asked for");
    }
    Ok(reads)
}

/// Read one application's tree, with geometry, in about [`PER_APP`] at most.
async fn snapshot(
    app: AppRef,
    pid: u32,
    ids: &Ids,
    spared: &Spared,
    facts: &Facts,
) -> Result<Snapshot> {
    let (name, toolkit) = (app.name().to_owned(), app.toolkit().to_owned());
    let mut ingest = tokio::time::timeout(PER_APP, AtspiIngest::attach(app))
        .await
        .context("gave up connecting to it")??
        .sharing(ids)
        .sparing(spared)
        .with_geometry(true)
        .within(PER_APP);
    let root = ingest.root_id();

    // The state of the host *before* the read, kept so the reconciliation
    // can be honest about what this tree does and does not contain.
    let before = facts.read();
    let began = Instant::now();
    let nodes = ingest.snapshot(root).await?;
    Ok(Snapshot {
        name,
        toolkit,
        pid,
        root,
        nodes,
        before,
        began,
        ingest,
    })
}

/// Every surface a window could have been drawn on, as claims to be weighed.
///
/// Not a popup, however much it shares its window's process: offered as one,
/// it would make a window with no title of its own ambiguous whenever a menu
/// of it was open.
fn surfaces(facts: &HostFacts) -> Vec<SurfaceClaim> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.hangs_from().is_none())
        .map(SurfaceFacts::claim)
        .collect()
}

/// How long an open menu may stay unbound before its application is read
/// again for it: about one turn of the keeper, for the toolkit's own
/// announcement of the menu to arrive first.
const MENU_GRACE: Duration = Duration::from_millis(250);

/// Which of `open` menus, each with when it opened, are owed a read now:
/// none bound, open longer than [`MENU_GRACE`], and not read for already
/// (`read`, which keeps those still open, these added). Whether any are.
fn menus_to_read(
    read: &mut HashSet<SurfaceId>,
    open: &[(SurfaceId, Option<Instant>)],
    bound: impl Fn(SurfaceId) -> bool,
    now: Instant,
) -> bool {
    read.retain(|done| open.iter().any(|(popup, _)| popup == done));
    let owed: Vec<SurfaceId> = open
        .iter()
        .filter(|(popup, mapped_at)| {
            !read.contains(popup)
                && !bound(*popup)
                && mapped_at.is_some_and(|at| now.saturating_duration_since(at) >= MENU_GRACE)
        })
        .map(|(popup, _)| *popup)
        .collect();
    read.extend(owed.iter().copied());
    !owed.is_empty()
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
        // A menu GTK 3 puts beside the windows, as an untitled window of its
        // own, is a popup's, not a window to weigh. Weighed, it binds to
        // the application's one surface as well as its window does, and the
        // two are both demoted as contested: the whole window unattributed
        // for as long as a context menu or combo list is open.
        .filter(|child| index.menu_window(**child).is_none())
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

            // Where this window's coordinates begin, as its own node says,
            // beside where the host placed it. The index subtracts the first
            // from every node on the window (`Index::window_origin`), and
            // printing all three is how a toolkit with a convention of its own
            // shows up as a number rather than as clicks beside their targets.
            let node_bounds = index
                .get(join.node)
                .and_then(ObservedNode::node_space_bounds);
            let facts = host.surface(join.surface);
            println!(
                "    damage: {} frames",
                facts.map_or(0, |facts| facts.damage_generation),
            );
            println!(
                "    node space: {:?}   host geometry: {:?}",
                node_bounds.map(|b| (b.x0, b.y0, b.x1, b.y1)),
                facts.map(|facts| (
                    facts.geometry.x0,
                    facts.geometry.y0,
                    facts.geometry.x1,
                    facts.geometry.y1
                )),
            );
            println!(
                "    window origin: {}",
                window_origin(index, join.surface, facts)
            );
        }
        for finding in &app.findings {
            println!("  FINDING: {finding}");
        }
        // Its open menus, each with what of its tree is bound to it, and in
        // which frame: whether its toolkit measures it from the menu or from
        // its window, the question #112 had to answer per toolkit.
        for popup in app.popups(host) {
            match index.menu_of(popup.id) {
                Some(menu) => println!(
                    "  menu {} from surface {:?} <- nodes {:?}, measured in {:?}, from {:?}",
                    popup.id.0,
                    popup.hangs_from().map(|from| from.0),
                    menu.roots.iter().map(|root| root.0).collect::<Vec<_>>(),
                    menu.frame,
                    (menu.origin.x, menu.origin.y),
                ),
                None => println!(
                    "  menu {} from surface {:?}: nothing bound",
                    popup.id.0,
                    popup.hangs_from().map(|from| from.0),
                ),
            }
        }
        for finding in &app.menus {
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

/// Where a window's coordinates begin in its toolkit's node space, and which
/// convention that is.
///
/// Two are known. GTK and Qt measure from the window geometry, so their
/// window node is at `(0, 0)`; Firefox measures from its buffer, so its window
/// node is the shadow's width in, which is the host's `geometry` less its
/// `buffer_origin`. Anything else, or a window node whose size is not the
/// geometry's, is named, because the index's rule -- subtract the window
/// node's origin -- assumes the window node is the window.
fn window_origin(index: &Index, surface: SurfaceId, facts: Option<&SurfaceFacts>) -> String {
    let Some(origin) = index.window_origin(surface) else {
        return "unmeasured: the window node reports no extents, so every node \
                on it is refused"
            .to_owned();
    };
    let mut said = format!("({}, {})", origin.x, origin.y);
    let Some(facts) = facts else {
        return said;
    };
    let shadow = facts.geometry.origin() - facts.buffer_origin.to_point();
    said.push_str(if origin == Vec2::ZERO {
        ", measured from the window geometry"
    } else if origin == shadow {
        ", measured from the buffer: the shadow's width"
    } else {
        ", which is neither the window geometry nor the buffer: CHECK THIS TOOLKIT"
    });
    let window = index
        .window_node(surface)
        .and_then(|node| index.get(node))
        .and_then(ObservedNode::node_space_bounds);
    if let Some(window) = window
        && window.size() != facts.geometry.size()
    {
        said.push_str(&format!(
            "; its size {}x{} is not the geometry's {}x{}",
            window.width(),
            window.height(),
            facts.geometry.width(),
            facts.geometry.height()
        ));
    }
    said
}

/// A leaf worth showing: something with a label, a role that is not a
/// container, and bounds.
fn sample_node(index: &Index, app: &App) -> Option<NodeId> {
    app.nodes(index).into_iter().find(|id| {
        index.get(*id).is_some_and(|node| {
            node.node.label().is_some()
                && node.node_space_bounds().is_some()
                && matches!(
                    node.node.role(),
                    Role::Button | Role::CheckBox | Role::Label
                )
        })
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use perspicax_node::{Node, ObservedNode, Rect, Role, SurfaceId};

    use super::*;

    fn node(id: u64, role: Role, label: Option<&str>, children: &[u64]) -> ObservedNode {
        let mut node = Node::new(role);
        if let Some(label) = label {
            node.set_label(label);
        }
        node.set_children(children.iter().map(|c| NodeId(*c)).collect::<Vec<_>>());
        ObservedNode::unjoined(NodeId(id), node)
    }

    /// GTK 3 puts an open context menu or combo list beside the application's
    /// windows, as an untitled window holding the menu. Weighed as a window,
    /// it binds to the application's one surface as well as the window does,
    /// and both are demoted as contested: the whole window unattributed while
    /// the menu is open, as slice 1 of #112 found on main. It is not weighed.
    #[test]
    fn a_menu_beside_an_applications_windows_is_not_one_of_them() {
        let mut index = Index::new();
        index.ingest_snapshot([
            node(100, Role::Application, Some("menus3.py"), &[1, 8]),
            node(1, Role::Window, Some("menus3"), &[]),
            node(8, Role::Window, None, &[9]),
            node(9, Role::Menu, None, &[]),
        ]);
        let claims = toplevels(&index, NodeId(100), Some(9182));
        assert_eq!(
            claims.iter().map(|claim| claim.node).collect::<Vec<_>>(),
            [NodeId(1)]
        );
    }

    /// An open menu nothing is bound to is owed one read, a moment after it
    /// opens, and no more: one that a read did not bind is a tooltip, or a
    /// menu its toolkit does not describe, and reading again will not change
    /// that. One bound needs none, and one that closed is forgotten, so it is
    /// owed a read again if it reopens unbound.
    #[test]
    fn an_unbound_menu_is_owed_one_read_a_moment_after_it_opens() {
        let opened = Instant::now();
        let menu = SurfaceId(5);
        let mut read = HashSet::new();
        let open = [(menu, Some(opened))];
        let unbound = |_| false;

        assert!(!menus_to_read(&mut read, &open, unbound, opened), "not yet");
        let later = opened + MENU_GRACE;
        assert!(menus_to_read(&mut read, &open, unbound, later));
        assert!(!menus_to_read(&mut read, &open, unbound, later), "once");

        assert!(
            !menus_to_read(&mut HashSet::new(), &open, |_| true, later),
            "a bound menu needs none"
        );
        assert!(!menus_to_read(&mut read, &[], unbound, later));
        assert!(read.is_empty(), "a closed menu is forgotten");
    }

    /// Nor is a popup a surface a window could be on.
    #[test]
    fn a_popup_is_not_a_surface_a_window_could_be_on() {
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let facts = HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(SurfaceId(1), area),
                SurfaceFacts::new(SurfaceId(5), area).hung_from(SurfaceId(1), Instant::now()),
            ],
            1,
        );
        assert_eq!(
            surfaces(&facts)
                .iter()
                .map(|claim| claim.surface)
                .collect::<Vec<_>>(),
            [SurfaceId(1)]
        );
    }
}
