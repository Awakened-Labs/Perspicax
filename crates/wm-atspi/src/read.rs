//! The two cold reads, and the probe that chooses between them.
//!
//! An AT-SPI application may or may not offer `org.a11y.atspi.Cache`, whose
//! `GetItems` hands over an entire application tree in **one** round trip. When
//! it does, reading a window is a single call. When it does not, the only way
//! to learn the tree is to walk it: `Accessible.GetChildren` from the root and
//! then, per node, a call for each property. The distance between those two is
//! the measurement M1 exists to produce.

use std::collections::HashMap;

use atspi::{
    CacheItem, CoordType, InterfaceSet, LegacyCacheItem, Role as AtspiRole, State, StateSet,
    proxy::{accessible::AccessibleProxy, cache::CacheProxy, component::ComponentProxy},
    zbus::{Connection, proxy::CacheProperties},
};
use wm_index::Interner;
use wm_node::{Node, ObservedNode, Rect};

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
    /// Its position among its parent's children, when the source said.
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
    /// Window-relative bounds, present only when geometry was asked for --
    /// it costs a round trip per node on both toolkits. See [`extents`].
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
/// Every node comes out [`Origin::Unattributed`](wm_node::Origin::Unattributed)
/// and [`Visibility::Unknown`](wm_node::Visibility::Unknown) -- that is
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

/// Read an application's tree, choosing a strategy by what it answers.
///
/// Returns the strategy that worked alongside the nodes, because the choice is
/// itself a result: it is the difference the M1 latency table is measuring, and
/// a caller that cannot see which path ran cannot report it.
///
/// # Errors
///
/// [`Error::Call`] if the walk's own root could not be read. A cache that
/// answers with an error is not an error -- it is the probe doing its job.
pub async fn cold_read(
    connection: &Connection,
    app: &AppRef,
) -> Result<(Strategy, Vec<RawNode>), Error> {
    if let Some((strategy, nodes)) = try_cache(connection, app).await {
        return Ok((strategy, nodes));
    }
    tracing::debug!(
        app = app.name(),
        toolkit = app.toolkit(),
        "no usable cache; falling back to a per-node walk"
    );
    Ok((Strategy::Walk, walk(connection, app.root()).await?))
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
) -> Result<Vec<RawNode>, Error> {
    match strategy {
        Strategy::Walk => walk(connection, app.root()).await,
        Strategy::Cache | Strategy::LegacyCache => Ok(try_cache(connection, app)
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
            if cache_is_complete(&nodes) {
                return Some((Strategy::Cache, nodes));
            }
            tracing::debug!(
                app = app.name(),
                nodes = nodes.len(),
                "Cache.GetItems answered with a partial tree; it is still filling up"
            );
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
/// # Errors
///
/// [`Error::Call`] if `root` itself cannot be read.
pub async fn walk(connection: &Connection, root: &ObjectKey) -> Result<Vec<RawNode>, Error> {
    let mut nodes = Vec::new();
    let mut seen: std::collections::HashSet<ObjectKey> = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([(root.clone(), None::<ObjectKey>, 0i32)]);

    while let Some((key, parent, index)) = queue.pop_front() {
        if !seen.insert(key.clone()) {
            continue;
        }
        let is_root = nodes.is_empty() && parent.is_none();
        let node = match read_one(connection, &key, parent, index).await {
            Ok(node) => node,
            Err(error) if is_root => return Err(error),
            Err(error) => {
                tracing::debug!(path = key.path(), %error, "skipping a node that vanished mid-walk");
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

/// One node's window-relative bounds, or `None` if it does not have any.
///
/// Separate from the tree read, and separately measured, because **no bulk
/// geometry API exists on either toolkit**. `Cache.GetItems` carries role,
/// name, states and parentage and no extents whatsoever, so geometry is a
/// round trip per node even on the fast path. That asymmetry is why the M1
/// latency table reports "cold tree" and "cold tree + geometry" as two numbers
/// rather than one.
///
/// [`CoordType::Window`] always, never `Screen`: a Wayland client cannot know
/// where it sits on screen, and the bus will answer a `Screen` request with a
/// confident fiction.
pub async fn extents(connection: &Connection, key: &ObjectKey) -> Option<Rect> {
    let proxy = ComponentProxy::builder(connection)
        .destination(key.bus().to_owned())
        .ok()?
        .path(key.path().to_owned())
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()?;
    proxy
        .get_extents(CoordType::Window)
        .await
        .ok()
        .and_then(map::extents_to_rect)
}

/// The modern cache layout, which reports parentage from the child's end.
///
/// `atspi`'s field names for the two strings are a trap worth naming: the wire
/// order after the interface list is *name*, role, *description*, states, and
/// the crate calls those two `short_name` and `name`. So `short_name` is the
/// accessible's name -- what a selector's bare word matches -- and `name` is
/// its description.
fn from_cache_item(item: &CacheItem) -> Option<RawNode> {
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
            assert_eq!(node.origin, wm_node::Origin::Unattributed);
            assert_eq!(node.visibility, wm_node::Visibility::Unknown);
            assert!(node.surface.is_none());
            assert_eq!(
                wm_index::check_actable(node).unwrap_err(),
                wm_index::Refusal::Unattributed,
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
