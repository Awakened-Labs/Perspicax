//! The two cold reads, and the probe that chooses between them.
//!
//! An AT-SPI application may or may not offer `org.a11y.atspi.Cache`, whose
//! `GetItems` hands over an entire application tree in **one** round trip. When
//! it does, reading a window is a single call. When it does not, the only way
//! to learn the tree is to walk it: `Accessible.GetChildren` from the root and
//! then, per node, a call for each property. The distance between those two is
//! the measurement M1 exists to produce.
//!
//! # An application that stops answering
//!
//! Every call made here has a deadline, [`ANSWER`], and every walk has
//! [`Patience`]. Neither is a precaution against something hypothetical:
//! before #51, one `GetExtents` an application left unanswered cost perspicax
//! the whole application, every time it was read, for as long as it ran.
//!
//! # An application that asking freezes
//!
//! And a call can do worse than go unanswered. Asking a Flutter application
//! where one of its nodes is freezes it for 25 s, because its bridge answers
//! by asking itself (#80). So asking where nodes are is [`place`]'s. It knows
//! the node that question would freeze before asking it, and never asks it
//! (#124), and it remembers anything else that froze an application and never
//! asks that again: see [`Spared`].

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use atspi::{
    CacheItem, CoordType, Interface, InterfaceSet, LegacyCacheItem, Role as AtspiRole, State,
    StateSet,
    proxy::{accessible::AccessibleProxy, cache::CacheProxy, component::ComponentProxy},
    zbus::{Connection, proxy::CacheProperties},
};
use perspicax_index::Interner;
use perspicax_node::{Node, ObservedNode, Rect};

use crate::{
    app::{AppRef, ObjectKey},
    error::Error,
    map,
};

/// How an application's tree can actually be read.
///
/// # Why this is decided by what an application *answers*
///
/// The obvious probe is to ask what interfaces an application offers and
/// dispatch on the answer. Measured against real toolkits, that probe is
/// wrong, and wrong in the worst available direction.
///
/// Qt 6.8.2 exports `org.a11y.atspi.Cache` at `/org/a11y/atspi/cache`. It
/// introspects cleanly, it declares `GetItems`, and calling `GetItems` returns
/// an **empty array** -- not an error, not `NotSupported`, just a successful
/// reply containing no nodes. An interface-shaped probe therefore concludes
/// "fast path available", takes it, receives zero nodes, and reports that a
/// window full of widgets is empty. There is no failure to catch and nothing
/// in the reply to distrust.
///
/// GTK gets the same treatment for a different reason. Its cache is real and
/// its answers are honest, but ATK fills it **as accessibles are realised**:
/// read a freshly started `gtk4-widget-factory` and `GetItems` returns 11 of
/// its 278 nodes, successfully and without comment. Walk it once and the cache
/// holds all 278 thereafter. So a cold cache is not a small tree, it is the
/// wrong tree -- and only [`cache_is_complete`] can tell the difference,
/// because the reply cannot.
///
/// So the probe is result-shaped: call the cheap thing, and believe it only if
/// what comes back is the tree it claims to be. A cache that answers with
/// nothing, or with less than it says it has, means "this read will not give
/// you the tree" -- not "this application has no widgets" -- and the walk
/// settles which it was.
///
/// The same call also has two wire formats -- `atspi` models them as
/// [`CacheItem`] and [`LegacyCacheItem`] -- which differ in how they describe
/// parentage, so the probe records which one deserialised as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// One `Cache.GetItems` round trip, modern layout: each item carries its
    /// parent and its index among that parent's children, but no child list.
    Cache,
    /// One `Cache.GetItems` round trip, legacy layout: each item carries an
    /// explicit list of its children.
    LegacyCache,
    /// Recursive `Accessible.GetChildren` plus per-node property reads. Six
    /// round trips per node, and the only option for an application whose
    /// cache is absent, empty, or still filling up. Measured: 219 nodes for
    /// the Qt widget gallery, 278 for a cold `gtk4-widget-factory`.
    Walk,
}

impl Strategy {
    /// Whether this strategy reads the tree in a single round trip.
    #[must_use]
    pub fn is_bulk(self) -> bool {
        matches!(self, Self::Cache | Self::LegacyCache)
    }
}

/// One accessible as a cold read produced it, before any id is minted.
///
/// Transport-shaped on purpose: it holds AT-SPI's own vocabulary and AT-SPI's
/// own keys, so that assembling a tree and translating a tree stay two
/// separable problems -- the first is pure and testable without a bus, and the
/// second is where [`map`] earns its keep.
#[derive(Debug, Clone)]
pub struct RawNode {
    /// Where this accessible lives.
    pub key: ObjectKey,
    /// Its parent, absent for an application root.
    pub parent: Option<ObjectKey>,
    /// Its position among its parent's children, when the source said: in a
    /// walk, its place in the list that held it, and in the modern cache, the
    /// item's own. Not what the node says when asked `GetIndexInParent`, and
    /// so not what tells a plug: [`place`] asks for that. See [`Spared`].
    pub index: Option<i32>,
    /// Its children, in order. Empty until [`link_children`] runs for a source
    /// that reports parentage the other way round.
    pub children: Vec<ObjectKey>,
    /// AT-SPI's role.
    pub role: AtspiRole,
    /// AT-SPI's state bits.
    pub states: StateSet,
    /// The interfaces this accessible offers.
    pub interfaces: InterfaceSet,
    /// The accessible's name -- what a selector's bare word matches.
    pub label: String,
    /// The accessible's longer description.
    pub description: String,
    /// Bounds in node space, present only when geometry was asked for -- it
    /// costs a round trip per node on both toolkits. See [`place`].
    pub bounds: Option<Rect>,
    /// How many children the application *says* this node has.
    ///
    /// The modern cache layout reports a count rather than a list, which turns
    /// out to be the most useful number in the whole item: comparing it against
    /// the children actually delivered is what [`cache_is_complete`] does, and
    /// it costs nothing to keep.
    pub declared_children: Option<i32>,
}

/// Whether a cache read delivered the tree it claims to describe.
///
/// # The measurement this exists for
///
/// GTK's cache is populated by ATK **as accessibles are realised**, not when
/// the window is built. Read it from a freshly started `gtk4-widget-factory`
/// and `Cache.GetItems` returns 11 nodes. Walk the same application and there
/// are 278. Walk it once and the cache thereafter holds all 278.
///
/// So a cold cache is not a small cache, it is a different answer -- 4% of the
/// tree, returned successfully, with nothing in the reply to suggest anything
/// is missing. A reader that trusts it reports that a window full of widgets
/// has eleven things in it, and no amount of care further up can recover what
/// was never sent.
///
/// The check is free. Each item already carries the child *count* its
/// application declares, so comparing that against the children the cache
/// actually delivered needs no extra round trip -- a node claiming nine
/// children and supplying none is a cache that is still filling up.
///
/// Being wrong in the safe direction costs a walk; being wrong in the other
/// direction costs the tree. So any shortfall is a shortfall.
#[must_use]
pub fn cache_is_complete(nodes: &[RawNode]) -> bool {
    nodes.iter().all(|node| {
        node.declared_children
            .is_none_or(|declared| i64::from(declared) <= node.children.len() as i64)
    })
}

/// Rebuild child lists for a source that reports only parent and index.
///
/// The modern [`CacheItem`] layout gives each node its parent and its position
/// among that parent's children, and a child *count* -- but never the children
/// themselves. The tree is real and fully determined; it is just written down
/// edge-by-edge from the wrong end, so it has to be inverted before anything
/// can traverse it.
///
/// Ordering is the part that matters. `[n]` in a selector has to mean the same
/// node twice running, so children are sorted by the index the application
/// reported and ties break on the object path -- deterministic, and stable
/// across reads, which a hash-map iteration order would not be.
///
/// Nodes that are already carrying children (the legacy layout, or a walk) are
/// left exactly as they are.
pub fn link_children(nodes: &mut [RawNode]) {
    let mut by_parent: HashMap<ObjectKey, Vec<(i32, ObjectKey)>> = HashMap::new();
    for node in nodes.iter() {
        if !node.children.is_empty() {
            continue;
        }
        if let Some(parent) = &node.parent {
            by_parent
                .entry(parent.clone())
                .or_default()
                .push((node.index.unwrap_or(i32::MAX), node.key.clone()));
        }
    }

    for children in by_parent.values_mut() {
        children.sort_by(|(left_index, left_key), (right_index, right_key)| {
            left_index
                .cmp(right_index)
                .then_with(|| left_key.path().cmp(right_key.path()))
        });
    }

    for node in nodes {
        if node.children.is_empty()
            && let Some(children) = by_parent.get(&node.key)
        {
            node.children = children.iter().map(|(_, key)| key.clone()).collect();
        }
    }
}

/// Everything beneath `root` inclusive, in the order the nodes were given.
///
/// A cache read returns an application's whole tree; a snapshot may have been
/// asked for a window inside it. Filtering afterwards rather than asking for
/// less is not a compromise -- there is no AT-SPI call that returns a subtree
/// -- and it keeps one code path for both.
///
/// A node whose parent chain does not reach `root` is dropped. A cycle in the
/// reported structure cannot loop this: descent is breadth-first over a
/// visited set, because a bridge is not a trusted source of structure.
#[must_use]
pub fn subtree(nodes: Vec<RawNode>, root: &ObjectKey) -> Vec<RawNode> {
    let by_key: HashMap<&ObjectKey, &RawNode> =
        nodes.iter().map(|node| (&node.key, node)).collect();
    if !by_key.contains_key(root) {
        return Vec::new();
    }

    let mut keep: std::collections::HashSet<ObjectKey> = std::collections::HashSet::new();
    let mut queue = vec![root.clone()];
    while let Some(key) = queue.pop() {
        if !keep.insert(key.clone()) {
            continue;
        }
        if let Some(node) = by_key.get(&key) {
            queue.extend(node.children.iter().cloned());
        }
    }

    nodes
        .into_iter()
        .filter(|node| keep.contains(&node.key))
        .collect()
}

/// Turn a cold read into nodes the index can hold.
///
/// Every node comes out
/// [`Origin::Unattributed`](perspicax_node::Origin::Unattributed) and
/// [`Visibility::Unknown`](perspicax_node::Visibility::Unknown) -- that is
/// [`ObservedNode::unjoined`]'s whole job, and in M1 there is no `HostView` to
/// change either. Which means every node produced here is un-actable, and the
/// refusal gate says so. That is the correct M1 behaviour, and it is asserted
/// as a test rather than worked around.
///
/// Defunct nodes are dropped rather than translated: `State::Defunct` is the
/// bus saying the object behind this reference is already gone, and minting an
/// id for it would put a node in the index that can never be read again.
pub fn to_observed(raw: Vec<RawNode>, interner: &mut Interner<ObjectKey>) -> Vec<ObservedNode> {
    // Two passes: every key must have an id before any child list is written,
    // because a parent is routinely reported before its children.
    let live: Vec<RawNode> = raw
        .into_iter()
        .filter(|node| !node.states.contains(State::Defunct))
        .collect();
    for node in &live {
        let _ = interner.intern(node.key.clone());
    }

    live.into_iter()
        .map(|raw| {
            let id = interner.intern(raw.key.clone());
            let mut node = Node::new(map::role(raw.role, raw.states));

            if !raw.label.is_empty() {
                node.set_label(raw.label);
            }
            if !raw.description.is_empty() {
                node.set_description(raw.description);
            }
            if let Some(bounds) = raw.bounds {
                node.set_bounds(bounds);
            }
            map::apply_states(raw.states, &mut node);
            map::apply_actions(raw.states, raw.interfaces, &mut node);

            // A child the read did not include -- filtered out of a subtree,
            // or defunct -- must not appear in a parent's child list, or the
            // index would hold an edge to a node it does not have.
            let children: Vec<_> = raw
                .children
                .iter()
                .filter_map(|key| interner.get(key))
                .collect();
            if !children.is_empty() {
                node.set_children(children);
            }

            ObservedNode::unjoined(id, node)
        })
        .collect()
}

/// How long a call waits for its answer before it is treated as failed.
///
/// Set once, on the connection every read goes over, rather than around each
/// call, so that no call made through this crate can forget it. A healthy call
/// answers in about two milliseconds -- the M1 latency table reads a Qt tree
/// with geometry, some 1,500 calls, in about three seconds -- so a second is
/// five hundred times that, and still short against a read's own budget.
pub const ANSWER: Duration = Duration::from_secs(1);

/// How many unanswered calls a walk sits through before it stops asking. See
/// [`Patience`].
pub const STRIKES: u8 = 3;

/// How much silence a walk sits through, and what the application's last walk
/// taught it.
///
/// # Why a deadline per call is not enough
///
/// [`ANSWER`] bounds a call, not a read. An application that has stopped
/// answering leaves every call after that unanswered, so a tree of 64 nodes
/// costs 64 deadlines, and a walk that waits out each one is a walk that does
/// not finish in any time worth having. So a walk counts the calls an
/// application leaves unanswered, and after [`STRIKES`] of them it stops
/// asking: the rest of the tree is not read, and what was read is kept.
///
/// Stopping early costs actability and nothing worse. A node the walk never
/// reached is not in the index -- it is not guessed at.
///
/// Asking where nodes are is not a walk, and sits through no silence at all:
/// see [`place`].
///
/// # Why it remembers
///
/// An application that ran a walk out of patience is given **one** unanswered
/// call in its next walk, not [`STRIKES`]. Re-reads are the keeper's, inline,
/// and every second one spends waiting is a second in which no other
/// application's signals are drained -- while a walk that ran out last time
/// will almost certainly run out again. One call is still asked, so an
/// application that has started answering is believed at once, and a walk in
/// which it does not run out puts its patience back to full. A walk's calls
/// ask what a node is, which an application answers without asking anyone
/// else, so asking again costs it nothing.
///
/// The memory is one application's, never a toolkit's. Nothing here knows
/// what any toolkit is, because any bridge can stop answering any call.
///
/// # And a budget
///
/// A read may also be given a moment at which it stops asking whatever the
/// count, which is how an application that answers everything, slowly, is
/// still read in bounded time. What was read by then is kept, rather than the
/// whole read thrown away for being late.
#[derive(Debug, Clone)]
pub struct Patience {
    /// Unanswered calls this read will still sit through.
    left: u8,
    /// How many this read was given.
    given: u8,
    /// When this read stops asking, if it has a budget.
    until: Option<Instant>,
    /// Why this read stopped asking, once it has. Read by the next
    /// [`begin`](Patience::begin): it is the memory.
    gave_up: Option<GaveUp>,
}

/// Why a walk stopped asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaveUp {
    /// The application left `calls` calls unanswered, which was all the walk
    /// had.
    Unanswered {
        /// How many it was given: [`STRIKES`], or one after a read that ran
        /// out the same way.
        calls: u8,
    },
    /// The read's budget ran out.
    OutOfTime,
}

impl Default for Patience {
    fn default() -> Self {
        Self::new()
    }
}

impl Patience {
    /// Full patience, for an application not read yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            left: STRIKES,
            given: STRIKES,
            until: None,
            gave_up: None,
        }
    }

    /// Start a read, which stops asking at `until` if it has a budget.
    ///
    /// Patience is full again, unless the last read ran out of it by silence:
    /// then this read is given one unanswered call. Running out of *time* is
    /// not held against an application, since it says how long its tree is
    /// rather than whether it answers.
    pub fn begin(&mut self, until: Option<Instant>) {
        let silenced = matches!(self.gave_up, Some(GaveUp::Unanswered { .. }));
        self.given = if silenced { 1 } else { STRIKES };
        self.left = self.given;
        self.until = until;
        self.gave_up = None;
    }

    /// Whether this read is still asking.
    fn holds(&mut self) -> bool {
        if self.gave_up.is_none() && self.until.is_some_and(|until| Instant::now() >= until) {
            self.gave_up = Some(GaveUp::OutOfTime);
        }
        self.gave_up.is_none()
    }

    /// Weigh a call's result. One the application never answered costs a
    /// strike, and the last strike ends the asking.
    fn heard<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        if result.as_ref().is_err_and(Error::is_unanswered) {
            self.left = self.left.saturating_sub(1);
            if self.left == 0 && self.gave_up.is_none() {
                self.gave_up = Some(GaveUp::Unanswered { calls: self.given });
            }
        }
        result
    }

    /// Why this read stopped asking, or `None` if it never did.
    #[must_use]
    pub fn gave_up(&self) -> Option<GaveUp> {
        self.gave_up
    }

    /// Whether the application left any call of this read unanswered.
    #[must_use]
    pub fn heard_silence(&self) -> bool {
        self.left < self.given
    }
}

/// Read an application's tree, choosing a strategy by what it answers.
///
/// Returns the strategy that worked alongside the nodes, because the choice is
/// itself a result: it is the difference the M1 latency table is measuring, and
/// a caller that cannot see which path ran cannot report it.
///
/// A walk sits through as much silence as `patience` allows; see [`walk`].
///
/// # Errors
///
/// [`Error::Call`] if the walk's own root could not be read. A cache that
/// answers with an error is not an error -- it is the probe doing its job.
pub async fn cold_read(
    connection: &Connection,
    app: &AppRef,
    patience: &mut Patience,
) -> Result<(Strategy, Vec<RawNode>), Error> {
    if let Some((strategy, nodes)) = try_cache(connection, app).await {
        return Ok((strategy, nodes));
    }
    tracing::debug!(
        app = app.name(),
        toolkit = app.toolkit(),
        "no usable cache; falling back to a per-node walk"
    );
    Ok((
        Strategy::Walk,
        walk(connection, app.root(), patience).await?,
    ))
}

/// Read with a strategy already chosen, skipping the probe.
///
/// The measurement path -- see [`AtspiIngest::forcing`](crate::AtspiIngest::forcing).
/// A cache strategy that an application cannot actually serve returns an empty
/// vector rather than falling back, because a fallback here would quietly turn
/// the two numbers being compared into the same number.
///
/// # Errors
///
/// [`Error::Call`] if a walk's root cannot be read.
pub async fn read_with(
    connection: &Connection,
    app: &AppRef,
    strategy: Strategy,
    patience: &mut Patience,
) -> Result<Vec<RawNode>, Error> {
    match strategy {
        Strategy::Walk => walk(connection, app.root(), patience).await,
        // The *raw* cache read, deliberately skipping the completeness gate
        // that [`cold_read`] applies. A caller naming a strategy is measuring
        // it, and answering "nothing" because the cache is cold would hide the
        // exact number they asked for -- the 11-against-278 that
        // [`cache_is_complete`] exists to describe.
        Strategy::Cache | Strategy::LegacyCache => Ok(cache_items(connection, app)
            .await
            .filter(|(used, _)| *used == strategy)
            .map(|(_, nodes)| nodes)
            .unwrap_or_default()),
    }
}

/// Try the one-round-trip read, in both of its wire formats.
///
/// `None` means "this application has no usable fast path", which covers four
/// distinct situations deliberately treated alike: no `Cache` interface, a
/// `Cache` that errors, a `Cache` that cheerfully returns nothing (Qt -- see
/// [`Strategy`]), and a `Cache` that returns a tree it admits is incomplete
/// (GTK, when cold -- see [`cache_is_complete`]).
///
/// All four are the same fact from a caller's point of view: *this read will
/// not give you the tree*. Distinguishing them would only invite a caller to
/// handle one of them optimistically.
async fn try_cache(connection: &Connection, app: &AppRef) -> Option<(Strategy, Vec<RawNode>)> {
    let (strategy, nodes) = cache_items(connection, app).await?;
    if cache_is_complete(&nodes) {
        return Some((strategy, nodes));
    }
    tracing::debug!(
        app = app.name(),
        nodes = nodes.len(),
        "Cache.GetItems answered with a partial tree; it is still filling up"
    );
    None
}

/// The one-round-trip read with no judgement applied: whatever the cache says,
/// in whichever of the two layouts it says it in.
///
/// Separated from [`try_cache`] so that "what does the cache contain?" and
/// "is that the tree?" stay two questions. The probe needs the second; a
/// measurement needs the first.
async fn cache_items(connection: &Connection, app: &AppRef) -> Option<(Strategy, Vec<RawNode>)> {
    let proxy = CacheProxy::builder(connection)
        .destination(app.root().bus().to_owned())
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()?;

    // The modern layout first: it is what at-spi2-core and every ATK-bridged
    // toolkit speak. A deserialisation failure here is the legacy layout
    // answering, not a broken application, so it falls through rather than up.
    match proxy.get_items().await {
        Ok(items) if !items.is_empty() => {
            let mut nodes: Vec<RawNode> = items.iter().filter_map(from_cache_item).collect();
            link_children(&mut nodes);
            return Some((Strategy::Cache, nodes));
        }
        Ok(_) => tracing::debug!(app = app.name(), "Cache.GetItems answered with no nodes"),
        Err(error) => tracing::debug!(app = app.name(), %error, "Cache.GetItems (modern) failed"),
    }

    match proxy.get_legacy_items().await {
        Ok(items) if !items.is_empty() => {
            let nodes: Vec<RawNode> = items.iter().filter_map(from_legacy_cache_item).collect();
            Some((Strategy::LegacyCache, nodes))
        }
        Ok(_) => None,
        Err(error) => {
            tracing::debug!(app = app.name(), %error, "Cache.GetItems (legacy) failed");
            None
        }
    }
}

/// Walk a tree node by node, because nothing cheaper is on offer.
///
/// **Six D-Bus round trips per node**, and there is no way to do better over
/// this interface: role, name, description, states, interfaces and children are
/// six separate calls, and none of them is bulk. That figure is the point --
/// it is what the M1 measurement is for, and shaving it would blur the number
/// rather than improve it.
///
/// Breadth-first over a visited set, iterative rather than recursive: an
/// application is not a trusted source of structure, so neither a cycle nor a
/// pathological depth may take the process down with it.
///
/// A node that fails mid-walk is skipped rather than fatal -- a window closing
/// while it is being read is ordinary, and losing the whole tree to it would
/// be worse than losing the node. The root is the exception: if that cannot be
/// read there is nothing to return and the caller should hear why.
///
/// A node that goes unanswered is skipped the same way, and costs `patience`
/// a strike. Once patience runs out, by silence or by its budget, the walk
/// stops and returns the nodes it has: the rest of the tree is missing, and
/// what was read is kept. The root is always asked, so a read that returns at
/// all returns something.
///
/// # Errors
///
/// [`Error::Call`] if `root` itself cannot be read, including when it does
/// not answer: [`Error::is_unanswered`].
pub async fn walk(
    connection: &Connection,
    root: &ObjectKey,
    patience: &mut Patience,
) -> Result<Vec<RawNode>, Error> {
    let mut nodes = Vec::new();
    let mut seen: std::collections::HashSet<ObjectKey> = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([(root.clone(), None::<ObjectKey>, 0i32)]);

    while let Some((key, parent, index)) = queue.pop_front() {
        if !nodes.is_empty() && !patience.holds() {
            break;
        }
        if !seen.insert(key.clone()) {
            continue;
        }
        let is_root = nodes.is_empty() && parent.is_none();
        let node = match patience.heard(read_one(connection, &key, parent, index).await) {
            Ok(node) => node,
            Err(error) if is_root => return Err(error),
            Err(error) => {
                tracing::debug!(
                    path = key.path(),
                    %error,
                    "skipping a node that vanished, or went unanswered, mid-walk"
                );
                continue;
            }
        };
        for (position, child) in node.children.iter().enumerate() {
            queue.push_back((
                child.clone(),
                Some(key.clone()),
                i32::try_from(position).unwrap_or(i32::MAX),
            ));
        }
        nodes.push(node);
    }

    Ok(nodes)
}

/// One node's worth of the walk: the six calls, in one place so the cost is
/// countable by reading it.
async fn read_one(
    connection: &Connection,
    key: &ObjectKey,
    parent: Option<ObjectKey>,
    index: i32,
) -> Result<RawNode, Error> {
    let proxy = accessible(connection, key).await?;
    Ok(RawNode {
        key: key.clone(),
        parent,
        index: Some(index),
        children: proxy
            .get_children()
            .await?
            .iter()
            .filter_map(ObjectKey::from_owned)
            .collect(),
        role: proxy.get_role().await?,
        states: proxy.get_state().await?,
        interfaces: proxy.get_interfaces().await?,
        label: proxy.name().await?,
        description: proxy.description().await?,
        bounds: None,
        declared_children: None,
    })
}

/// An `Accessible` proxy pointed at one object.
///
/// # Why every proxy in this crate disables property caching
///
/// zbus caches properties by default, and a caching proxy issues
/// `org.freedesktop.DBus.Properties.GetAll` for its interface the moment it is
/// built. **That call segfaults Qt 6.8.2's AT-SPI adaptor.** Measured, not
/// suspected: `GetAll("org.a11y.atspi.Accessible")` against the Qt widget
/// gallery returns `NoReply -- message recipient disconnected` and leaves a
/// core behind, while `Get` of the same properties one at a time is fine.
///
/// So a reader built the obvious way crashes every Qt application it looks at,
/// on construction, before reading a single node. An observer that destroys
/// what it observes is not a subtle performance issue -- it is the sharpest
/// possible version of the rule this project keeps restating, which is that
/// reading a tree must not perturb it.
///
/// `atspi` reaches the same conclusion internally; its own `AccessibleProxy`
/// construction passes `CacheProperties::No`. This crate does it at every
/// proxy it builds, not just this one.
///
/// # Errors
///
/// [`Error::Call`] if the bus name or path is not well formed.
pub async fn accessible<'a>(
    connection: &Connection,
    key: &ObjectKey,
) -> Result<AccessibleProxy<'a>, Error> {
    Ok(AccessibleProxy::builder(connection)
        .destination(key.bus().to_owned())?
        .path(key.path().to_owned())?
        .cache_properties(CacheProperties::No)
        .build()
        .await?)
}

/// The nodes a desktop's reads never ask where they are again, nor anything
/// beneath them. Shared by every ingest reading a desktop, as
/// [`Ids`](crate::Ids) is, so that it lasts as long as an application does and
/// not only as long as one reading of it.
///
/// # Why asking can be the harm
///
/// A Flutter Linux application embeds an `AtkPlug` in an `AtkSocket` of its
/// own process. at-spi2-core's bridge answers `GetExtents` on that plug by
/// asking the socket, over D-Bus and blocking -- which is asking itself, so its
/// main thread waits on a call only it could answer until libdbus gives up,
/// 25 s later, and the answer is `(-1, -1, -1, -1)`. Every Flutter node takes
/// its extents from its parent's, up to the plug, so asking where any of them
/// is freezes the whole application, and a frozen client is disconnected by
/// the compositor once input fills its buffer (#80). A call sent while it is
/// frozen waits in its queue and freezes it again when its turn comes,
/// however long ago perspicax stopped waiting for it.
///
/// # Knowing the plug before asking it
///
/// So the plug is put here before it is asked where it is (#124), from three
/// things its application says at once and without asking anyone. [`place`]
/// asks top-down, so it has each of them by the plug's turn:
///
/// 1. **The node that lists it has no place.** A bare socket answers
///    `GetExtents` at once, with nothing in it. A plug's own answer is its
///    socket's, forwarded, so once the socket has said it is nowhere, asking
///    the plug can only cost. It also keeps the extra question to the few
///    children of nodes with no place.
/// 2. **It is on its lister's own bus name.** at-spi2-atk holds one
///    connection per process, and that is the connection the forwarded call
///    would block. A plug embedded from another process, as WebKitGTK's is,
///    has its call answered there, and is asked as any node is.
/// 3. **Its own `GetIndexInParent` is -1.** ATK gives it no parent, though a
///    node of its own process lists it: the socket's link, which the bridge
///    makes and ATK knows nothing of.
///
/// None of the three is enough alone. Measured on ParadoxShift, the socket
/// says -1 too, and so do GTK 3's own header bar fillers: it is their
/// listers' places that keep them asked.
///
/// # And learning from the first silence
///
/// A plug the three do not mark is asked, and freezes its application once:
/// one whose socket has a place, as Flutter's would once
/// flutter/flutter#153560 is fixed while at-spi2-core still asks itself.
/// So the first `GetExtents` an application leaves unanswered ends the asking,
/// and the node it was asked of is remembered here: it, and every node beneath
/// it, keep no bounds and are refused as `unplaced` from then on, which fails
/// closed. For a plug that is the plug itself, since it is asked before
/// anything its silence reaches.
///
/// | | at-spi2-core asks itself | at-spi2-core fixed |
/// |---|---|---|
/// | the socket has no place (today) | known: never asked, never frozen | known: never asked, and nothing lost |
/// | the socket has a place | asked: frozen once, then spared | asked, and answers |
///
/// # Why only after a walk answered in full
///
/// Then the silence began when the application was asked where something is,
/// which is the only evidence there is that asking is what silenced it. An
/// application already silent during the walk is busy, or frozen by something
/// else, and its silence says nothing about the node it was asked of.
///
/// # A wrong spare
///
/// Either way, a node put here stays for the life of its process, and costs
/// it its subtree's bounds. That is the price of never asking twice, and the
/// rules above keep it to a node that fits the shape of a plug or went silent
/// straight after a clean walk.
///
/// A unique bus name is never reused, so an [`ObjectKey`] here names one node
/// of one process, and nothing here knows what a toolkit is.
pub type Spared = Arc<Mutex<HashSet<ObjectKey>>>;

/// How asking each node where it is ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placed {
    /// Every node that offers `Component`, and is neither spared nor beneath
    /// a node that is, was asked.
    All,
    /// The read's budget ran out first. Not held against anything: it says how
    /// long a tree is, not whether its application answers.
    OutOfTime,
    /// The node went unanswered when asked where it is, straight after a walk
    /// the application answered in full, so it, and everything beneath it,
    /// are spared from now on. See [`Spared`].
    Spared(ObjectKey),
    /// A call about the node went unanswered that asking where it is cannot
    /// be blamed for: the walk before it went unanswered too, so the
    /// application was silent already, or the call asked only its index.
    /// Nothing is held against the node.
    Unanswered(ObjectKey),
}

/// What one pass of [`place`] came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placing {
    /// How the asking ended.
    pub ended: Placed,
    /// The plugs it found that no pass had found before, each spared before
    /// it was asked where it is: see [`Spared`]. Only the new ones, so a
    /// caller can say so once for as long as the application runs.
    pub plugs: Vec<ObjectKey>,
}

/// Ask each node where it is, top-down from `root`, until one goes unanswered
/// or the read's budget runs out at `until`.
///
/// A node that offers no `Component` is not asked -- an application root
/// offers none, and atk-bridge prints a CRITICAL for every `GetExtents` it is
/// sent anyway -- and neither is a node in `spared`, or beneath one.
///
/// Nor is a plug its own application embedded, nor anything beneath it. A
/// node listed by one that has just answered with no place, on that node's own
/// bus, is asked its index among its parent's children first, and one that
/// says -1 is spared there and then. A pass rooted beneath a socket would
/// not know its plug, having no lister for it to ask about; every caller reads
/// from an application's root.
///
/// A node with no usable size to give, or that fails, is left without bounds.
/// The first node that does not answer ends the asking, and is spared if
/// `walk`, the read's walk, was answered in full: see [`Spared`].
pub async fn place(
    connection: &Connection,
    nodes: &mut [RawNode],
    root: &ObjectKey,
    walk: &Patience,
    spared: &Spared,
    until: Option<Instant>,
) -> Placing {
    let order = placing_order(nodes, root, &remembered(spared));
    let mut plugs = Vec::new();
    // The nodes this pass found to have no place, and the nodes it leaves
    // unasked along with everything beneath them.
    let (mut nowhere, mut left) = (HashSet::new(), HashSet::new());
    for Turn { node, lister } in order {
        if lister.is_some_and(|lister| left.contains(&lister)) {
            left.insert(node);
            continue;
        }
        if until.is_some_and(|until| Instant::now() >= until) {
            return Placing {
                ended: Placed::OutOfTime,
                plugs,
            };
        }
        let key = nodes[node].key.clone();
        let may_be_a_plug = lister.is_some_and(|lister| {
            nowhere.contains(&lister) && nodes[lister].key.bus() == key.bus()
        });
        if may_be_a_plug {
            let index = index_in_parent(connection, &key).await;
            tracing::debug!(
                path = key.path(),
                ?index,
                "asked a node its index: it is listed, on its own bus, by a node with no place"
            );
            match index {
                Ok(-1) => {
                    left.insert(node);
                    if remembered(spared).insert(key.clone()) {
                        plugs.push(key);
                    }
                    continue;
                }
                Err(error) if error.is_unanswered() => {
                    return Placing {
                        ended: Placed::Unanswered(key),
                        plugs,
                    };
                }
                Ok(_) | Err(_) => {}
            }
        }
        if !nodes[node].interfaces.contains(Interface::Component) {
            continue;
        }
        match extents(connection, &key).await {
            Ok(bounds) => {
                if bounds.is_none() {
                    nowhere.insert(node);
                }
                nodes[node].bounds = bounds;
            }
            Err(error) if error.is_unanswered() => {
                let ended = if walk.heard_silence() {
                    Placed::Unanswered(key)
                } else {
                    remembered(spared).insert(key.clone());
                    Placed::Spared(key)
                };
                return Placing { ended, plugs };
            }
            Err(_) => {}
        }
    }
    Placing {
        ended: Placed::All,
        plugs,
    }
}

/// The spared nodes, locked. A panic cannot leave a set half made, so a
/// poisoned lock is still a set worth reading.
fn remembered(spared: &Spared) -> std::sync::MutexGuard<'_, HashSet<ObjectKey>> {
    spared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One node's turn in [`place`], beside the node that listed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Turn {
    /// The node, by its index in the read's nodes.
    node: usize,
    /// The node whose children it was found among, or `None` for the pass's
    /// root.
    lister: Option<usize>,
}

/// The order in which `nodes` take their turns to be placed: top-down from
/// `root`, a parent before its children, whatever order the read delivered
/// them in -- a cache's is its own. Each turn names the node that listed it,
/// so that [`place`] can leave out everything beneath a node it leaves out,
/// and can ask about a node what its lister answered. Leaves out a spared
/// node and everything beneath it.
///
/// A node that offers no `Component` has its turn too, though it is not asked
/// where it is: a plug that offered none would still have beneath it the
/// nodes whose asking freezes.
///
/// Breadth-first over a visited set, as [`walk`] is: a bridge is not a
/// trusted source of structure. A node listed twice has one turn, beside the
/// first node to list it.
fn placing_order(nodes: &[RawNode], root: &ObjectKey, spared: &HashSet<ObjectKey>) -> Vec<Turn> {
    let at: HashMap<&ObjectKey, usize> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (&node.key, index))
        .collect();
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([(root, None)]);
    while let Some((key, lister)) = queue.pop_front() {
        if spared.contains(key) || !seen.insert(key) {
            continue;
        }
        let Some(&node) = at.get(key) else {
            continue;
        };
        order.push(Turn { node, lister });
        queue.extend(nodes[node].children.iter().map(|child| (child, Some(node))));
    }
    order
}

/// What a node says its index among its parent's children is, as its toolkit
/// counts them: -1 for one with no parent of its own. Asked only of a node
/// that may be a plug, since that answer is what tells one; see [`Spared`].
///
/// # Errors
///
/// [`Error::Call`] if the node has gone, cannot say, or does not answer
/// before [`ANSWER`]: [`Error::is_unanswered`].
pub async fn index_in_parent(connection: &Connection, key: &ObjectKey) -> Result<i32, Error> {
    Ok(accessible(connection, key)
        .await?
        .get_index_in_parent()
        .await?)
}

/// One node's bounds in node space -- window-relative, from whichever origin
/// its toolkit calls the window's -- or `None` if it answers with no usable
/// size.
///
/// Separate from the tree read, and separately measured, because **no bulk
/// geometry API exists on either toolkit**. `Cache.GetItems` carries role,
/// name, states and parentage and no extents whatsoever, so geometry is a
/// round trip per node even on the fast path. That asymmetry is why the M1
/// latency table reports "cold tree" and "cold tree + geometry" as two numbers
/// rather than one.
///
/// [`CoordType::Window`] always, never `Screen`. Measured 2026-09-04 against
/// the same two applications under a GNOME Xorg session and a GNOME Wayland
/// session, asking each node for both coordinate types:
///
/// | | Qt `Screen` | GTK `Screen` |
/// |---|---|---|
/// | Xorg | correct -- a real screen offset | **wrong** -- `0,0` for a child its own `Window` call puts at `10,57` |
/// | Wayland | degrades to window-relative | **wrong** -- same as above |
///
/// Qt is honest: it reports true screen coordinates where it can know them and
/// falls back to window-relative under Wayland, where a client genuinely
/// cannot. GTK answers `Screen` with numbers that disagree with its own
/// `Window` answer on *both* session types.
///
/// So `Screen` is not merely unavailable under Wayland, it is unreliable
/// everywhere -- and unreliable in the shape that matters, since a plausible
/// wrong box is what makes a system click in the wrong place. Only a
/// `HostView` turns window-relative bounds into anything global, once the
/// index has measured which origin "window" meant: see
/// `perspicax_index::Index::window_origin`.
///
/// # Errors
///
/// [`Error::Call`] if the node has no `Component`, has gone, or does not
/// answer before [`ANSWER`] -- that last one is [`Error::is_unanswered`], and
/// it is the one a caller must not simply shrug off: it costs the whole
/// deadline every time it is asked, and it may be what froze the application.
/// See [`Spared`].
pub async fn extents(connection: &Connection, key: &ObjectKey) -> Result<Option<Rect>, Error> {
    let proxy = ComponentProxy::builder(connection)
        .destination(key.bus().to_owned())?
        .path(key.path().to_owned())?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    Ok(map::extents_to_rect(
        proxy.get_extents(CoordType::Window).await?,
    ))
}

/// The modern cache layout, which reports parentage from the child's end.
///
/// `atspi`'s field names for the two strings are a trap worth naming: the wire
/// order after the interface list is *name*, role, *description*, states, and
/// the crate calls those two `short_name` and `name`. So `short_name` is the
/// accessible's name -- what a selector's bare word matches -- and `name` is
/// its description.
pub(crate) fn from_cache_item(item: &CacheItem) -> Option<RawNode> {
    Some(RawNode {
        key: ObjectKey::from_owned(&item.object)?,
        parent: ObjectKey::from_owned(&item.parent),
        index: Some(item.index),
        children: Vec::new(),
        role: item.role,
        states: item.states,
        interfaces: item.ifaces,
        label: item.short_name.clone(),
        description: item.name.clone(),
        bounds: None,
        declared_children: Some(item.children),
    })
}

/// The legacy layout, which reports its children directly and so needs no
/// inversion. Qt declares this shape; see [`Strategy`] for what it returns.
fn from_legacy_cache_item(item: &LegacyCacheItem) -> Option<RawNode> {
    Some(RawNode {
        key: ObjectKey::from_owned(&item.object)?,
        parent: ObjectKey::from_owned(&item.parent),
        index: None,
        children: item
            .children
            .iter()
            .filter_map(ObjectKey::from_owned)
            .collect(),
        role: item.role,
        states: item.states,
        interfaces: item.ifaces,
        label: item.short_name.clone(),
        description: item.name.clone(),
        bounds: None,
        declared_children: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(path: &str, parent: Option<&str>, index: i32) -> RawNode {
        RawNode {
            key: ObjectKey::new(":1.2", path),
            parent: parent.map(|p| ObjectKey::new(":1.2", p)),
            index: Some(index),
            children: Vec::new(),
            role: AtspiRole::Panel,
            states: StateSet::empty(),
            interfaces: InterfaceSet::empty(),
            label: String::new(),
            description: String::new(),
            bounds: None,
            declared_children: None,
        }
    }

    /// The modern cache layout writes every edge from the child's end. The
    /// tree is fully determined; it just has to be turned around.
    #[test]
    fn parent_and_index_rebuild_an_ordered_child_list() {
        let mut nodes = vec![
            raw("/root", None, -1),
            raw("/b", Some("/root"), 1),
            raw("/a", Some("/root"), 0),
            raw("/c", Some("/root"), 2),
        ];
        link_children(&mut nodes);

        let root = nodes.iter().find(|n| n.key.path() == "/root").unwrap();
        let paths: Vec<_> = root.children.iter().map(|k| k.path()).collect();
        assert_eq!(
            paths,
            ["/a", "/b", "/c"],
            "children must follow the reported index"
        );
    }

    /// Two children claiming the same index is malformed input, not a panic.
    /// It has to resolve the same way every read, or `[n]` stops meaning one
    /// node.
    #[test]
    fn a_duplicate_index_still_orders_deterministically() {
        let mut first = vec![
            raw("/root", None, -1),
            raw("/z", Some("/root"), 0),
            raw("/a", Some("/root"), 0),
        ];
        let mut second = vec![
            raw("/root", None, -1),
            raw("/a", Some("/root"), 0),
            raw("/z", Some("/root"), 0),
        ];
        link_children(&mut first);
        link_children(&mut second);

        let order = |nodes: &[RawNode]| {
            nodes
                .iter()
                .find(|n| n.key.path() == "/root")
                .unwrap()
                .children
                .iter()
                .map(|k| k.path().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&first), order(&second));
        assert_eq!(order(&first), ["/a", "/z"]);
    }

    /// The legacy layout already said who its children are. Nothing to invert.
    #[test]
    fn an_explicit_child_list_is_left_alone() {
        let mut nodes = vec![RawNode {
            children: vec![ObjectKey::new(":1.2", "/only")],
            ..raw("/root", None, -1)
        }];
        link_children(&mut nodes);
        assert_eq!(nodes[0].children.len(), 1);
    }

    #[test]
    fn a_subtree_keeps_descendants_and_drops_everything_else() {
        let mut nodes = vec![
            raw("/root", None, -1),
            raw("/window", Some("/root"), 0),
            raw("/button", Some("/window"), 0),
            raw("/other", Some("/root"), 1),
        ];
        link_children(&mut nodes);

        let kept = subtree(nodes, &ObjectKey::new(":1.2", "/window"));
        let mut paths: Vec<_> = kept.iter().map(|n| n.key.path().to_owned()).collect();
        paths.sort();
        assert_eq!(paths, ["/button", "/window"]);
    }

    /// A bridge is not a trusted source of structure. A cycle in what it
    /// reports must terminate, not hang.
    #[test]
    fn a_cycle_in_the_reported_tree_terminates() {
        let mut nodes = vec![
            RawNode {
                children: vec![ObjectKey::new(":1.2", "/b")],
                ..raw("/a", None, 0)
            },
            RawNode {
                children: vec![ObjectKey::new(":1.2", "/a")],
                ..raw("/b", Some("/a"), 0)
            },
        ];
        link_children(&mut nodes);
        assert_eq!(subtree(nodes, &ObjectKey::new(":1.2", "/a")).len(), 2);
    }

    #[test]
    fn asking_for_a_root_that_was_not_read_yields_nothing() {
        let nodes = vec![raw("/root", None, -1)];
        assert!(subtree(nodes, &ObjectKey::new(":1.2", "/absent")).is_empty());
    }

    /// A cold GTK cache declares children it has not sent. Measured: 11 nodes
    /// delivered where a walk of the same window finds 278.
    #[test]
    fn a_cache_that_declares_more_children_than_it_sent_is_incomplete() {
        let nodes = vec![RawNode {
            declared_children: Some(9),
            ..raw("/window", None, 0)
        }];
        assert!(!cache_is_complete(&nodes));
    }

    #[test]
    fn a_cache_that_delivered_what_it_declared_is_complete() {
        let mut nodes = vec![
            RawNode {
                declared_children: Some(2),
                ..raw("/window", None, 0)
            },
            RawNode {
                declared_children: Some(0),
                ..raw("/a", Some("/window"), 0)
            },
            RawNode {
                declared_children: Some(0),
                ..raw("/b", Some("/window"), 1)
            },
        ];
        link_children(&mut nodes);
        assert!(cache_is_complete(&nodes));
    }

    /// A source that reports children directly rather than counting them makes
    /// no claim to check, and must not be failed for it.
    #[test]
    fn a_source_that_declares_no_count_is_not_judged_incomplete() {
        let nodes = vec![raw("/window", None, 0)];
        assert!(cache_is_complete(&nodes));
    }

    /// What zbus returns when the reading connection's deadline runs out.
    fn unanswered() -> Result<(), Error> {
        Err(Error::Call(atspi::zbus::Error::from(std::io::Error::from(
            std::io::ErrorKind::TimedOut,
        ))))
    }

    /// A call that fails at once costs no time, so it costs no patience.
    #[test]
    fn only_an_unanswered_call_is_a_strike() {
        assert!(unanswered().unwrap_err().is_unanswered());
        assert!(!Error::UnknownRoot(1).is_unanswered());

        let mut patience = Patience::new();
        patience.begin(None);
        for _ in 0..10 {
            let _ = patience.heard(Err::<(), _>(Error::UnknownRoot(1)));
        }
        assert!(patience.holds());
        assert_eq!(patience.gave_up(), None);
    }

    #[test]
    fn a_pass_stops_asking_at_its_last_strike_and_says_so() {
        let mut patience = Patience::new();
        patience.begin(None);
        for _ in 1..STRIKES {
            let _ = patience.heard(unanswered());
            assert!(patience.holds());
        }
        let _ = patience.heard(unanswered());
        assert!(!patience.holds());
        assert_eq!(
            patience.gave_up(),
            Some(GaveUp::Unanswered { calls: STRIKES })
        );
    }

    /// The keeper's re-read of an application that went silent waits one
    /// deadline, not three -- and the read after one in which it answered
    /// waits the full three again, so an application that recovers is not
    /// held to its worst read forever.
    #[test]
    fn patience_run_out_by_silence_is_one_strike_until_a_read_does_not_run_out() {
        let mut patience = Patience::new();
        patience.begin(None);
        for _ in 0..STRIKES {
            let _ = patience.heard(unanswered());
        }

        patience.begin(None);
        let _ = patience.heard(unanswered());
        assert_eq!(patience.gave_up(), Some(GaveUp::Unanswered { calls: 1 }));

        patience.begin(None);
        let _ = patience.heard(Ok(()));
        assert_eq!(patience.gave_up(), None, "it answered");

        patience.begin(None);
        let _ = patience.heard(unanswered());
        assert!(patience.holds(), "and is given its full patience again");
    }

    #[test]
    fn a_walk_that_heard_silence_says_so_until_the_next_begins() {
        let mut patience = Patience::new();
        patience.begin(None);
        let _ = patience.heard(Ok(()));
        assert!(!patience.heard_silence());
        let _ = patience.heard(unanswered());
        assert!(patience.heard_silence(), "one unanswered call is silence");
        patience.begin(None);
        assert!(!patience.heard_silence());
    }

    /// A node that can say where it is.
    fn placeable(path: &str, children: &[&str]) -> RawNode {
        RawNode {
            interfaces: InterfaceSet::new(Interface::Component),
            children: children
                .iter()
                .map(|child| ObjectKey::new(":1.2", child))
                .collect(),
            ..raw(path, None, 0)
        }
    }

    /// Each turn `placing_order` gives, as the path of its node and of the
    /// node that listed it.
    fn turns<'a>(nodes: &'a [RawNode], spared: &[&str]) -> Vec<(&'a str, Option<&'a str>)> {
        let spared = spared
            .iter()
            .map(|path| ObjectKey::new(":1.2", path))
            .collect();
        placing_order(nodes, &ObjectKey::new(":1.2", "/root"), &spared)
            .into_iter()
            .map(|turn| {
                (
                    nodes[turn.node].key.path(),
                    turn.lister.map(|lister| nodes[lister].key.path()),
                )
            })
            .collect()
    }

    fn ordered<'a>(nodes: &'a [RawNode], spared: &[&str]) -> Vec<&'a str> {
        turns(nodes, spared)
            .into_iter()
            .map(|(node, _)| node)
            .collect()
    }

    /// A cache holds a tree in its own order. A parent is asked first
    /// anyway, so the first node of a frozen branch to be asked is the one
    /// nearest the root.
    #[test]
    fn a_parent_is_asked_where_it_is_before_its_children() {
        let mut nodes = vec![
            placeable("/root", &["/plug", "/ok"]),
            placeable("/plug", &["/square"]),
            placeable("/ok", &[]),
            placeable("/square", &[]),
        ];
        nodes.reverse();
        assert_eq!(ordered(&nodes, &[]), ["/root", "/plug", "/ok", "/square"]);
    }

    #[test]
    fn a_spared_node_and_everything_beneath_it_are_not_asked() {
        let nodes = vec![
            placeable("/root", &["/plug", "/ok"]),
            placeable("/plug", &["/square"]),
            placeable("/square", &["/deeper"]),
            placeable("/deeper", &[]),
            placeable("/ok", &[]),
        ];
        assert_eq!(ordered(&nodes, &["/plug"]), ["/root", "/ok"]);
    }

    /// An application root offers no `Component`, and its windows do. The
    /// root has its turn anyway, though it is not asked where it is: a plug
    /// that offered none would still be what its children freeze on.
    #[test]
    fn a_node_that_offers_no_component_has_its_turn_and_so_do_its_children() {
        let nodes = vec![
            RawNode {
                interfaces: InterfaceSet::empty(),
                ..placeable("/root", &["/frame"])
            },
            placeable("/frame", &[]),
        ];
        assert_eq!(ordered(&nodes, &[]), ["/root", "/frame"]);
    }

    #[test]
    fn a_cycle_in_the_reported_tree_is_asked_once_round() {
        let nodes = vec![placeable("/root", &["/a"]), placeable("/a", &["/root"])];
        assert_eq!(ordered(&nodes, &[]), ["/root", "/a"]);
    }

    /// What a plug is known by is what its lister answered, so each turn
    /// says whose children it was found among.
    #[test]
    fn each_node_has_its_turn_beside_the_node_that_listed_it() {
        let nodes = vec![
            placeable("/root", &["/socket", "/ok"]),
            placeable("/socket", &["/plug"]),
            placeable("/ok", &[]),
            placeable("/plug", &[]),
        ];
        assert_eq!(
            turns(&nodes, &[]),
            [
                ("/root", None),
                ("/socket", Some("/root")),
                ("/ok", Some("/root")),
                ("/plug", Some("/socket")),
            ]
        );
    }

    #[test]
    fn a_node_listed_twice_has_one_turn_beside_the_first_to_list_it() {
        let nodes = vec![
            placeable("/root", &["/a", "/b"]),
            placeable("/a", &["/shared"]),
            placeable("/b", &["/shared"]),
            placeable("/shared", &[]),
        ];
        assert_eq!(
            turns(&nodes, &[]),
            [
                ("/root", None),
                ("/a", Some("/root")),
                ("/b", Some("/root")),
                ("/shared", Some("/a")),
            ]
        );
    }

    /// Running out of time says how long a tree is, not whether its
    /// application answers, so it is not held against the next read.
    #[test]
    fn a_read_out_of_time_is_not_held_against_the_next() {
        let mut patience = Patience::new();
        patience.begin(Some(Instant::now()));
        assert!(!patience.holds());
        assert_eq!(patience.gave_up(), Some(GaveUp::OutOfTime));

        patience.begin(None);
        let _ = patience.heard(unanswered());
        assert!(patience.holds());
    }

    /// The M1 invariant, stated where the nodes are made. No compositor exists,
    /// so nothing this crate produces may be acted on.
    #[test]
    fn every_node_a_cold_read_produces_is_unattributed_and_unjudged() {
        let mut nodes = vec![raw("/root", None, -1), raw("/child", Some("/root"), 0)];
        link_children(&mut nodes);
        let mut interner = Interner::new();

        let observed = to_observed(nodes, &mut interner);
        assert_eq!(observed.len(), 2);
        for node in &observed {
            assert_eq!(node.origin, perspicax_node::Origin::Unattributed);
            assert_eq!(node.visibility, perspicax_node::Visibility::Unknown);
            assert!(node.surface.is_none());
            assert_eq!(
                perspicax_index::check_actable(node).unwrap_err(),
                perspicax_index::Refusal::Unattributed,
                "M1 has no compositor, so every node must be refused"
            );
        }
    }

    #[test]
    fn a_defunct_node_is_dropped_rather_than_given_an_id() {
        let nodes = vec![
            raw("/live", None, 0),
            RawNode {
                states: [State::Defunct].into_iter().collect(),
                ..raw("/dead", None, 1)
            },
        ];
        let mut interner = Interner::new();
        let observed = to_observed(nodes, &mut interner);
        assert_eq!(observed.len(), 1);
        assert_eq!(interner.len(), 1);
    }

    /// A parent must not keep an edge to a child that was filtered out, or the
    /// index holds a dangling id.
    #[test]
    fn a_child_outside_the_read_is_not_left_in_its_parents_child_list() {
        let nodes = vec![RawNode {
            children: vec![ObjectKey::new(":1.2", "/gone")],
            ..raw("/root", None, -1)
        }];
        let mut interner = Interner::new();
        let observed = to_observed(nodes, &mut interner);
        assert!(observed[0].node.children().is_empty());
    }
}
