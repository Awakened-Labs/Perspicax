//! The node cache, and the deltas it computes.
//!
//! Everything here is ordinary bookkeeping with one unusual requirement: it
//! must be possible to answer "what changed?" without re-reading anything. An
//! accessibility bridge that is asked for a whole tree on every tick is the
//! performance failure this project exists to remove, so the cache holds the
//! last known state and emits a [`Delta`] only where the new state actually
//! differs from it.

use std::collections::{HashMap, HashSet};

use perspicax_node::{NodeId, ObservedNode, Origin, Rect, SurfaceId, Vec2, Visibility};

use crate::{
    Change, Consent, Refusal, check_actable,
    host::overlaps,
    host::{HostFacts, Judgement, Tally, judge},
    selector::Selector,
};

/// Something an agent subscribed to changes would want to hear about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delta {
    /// A node the cache had not seen before.
    Added { id: NodeId },
    /// A node whose contents differ from what the cache held.
    Updated { id: NodeId },
    /// A node that is gone. Emitted for every node of a removed subtree, not
    /// just its root, so a subscriber never has to infer descendants.
    Removed { id: NodeId },
    /// A subtree that can no longer be trusted and has not yet been re-read.
    Invalidated { root: NodeId },
}

/// The semantic index's cache of what is on screen.
#[derive(Debug, Default)]
pub struct Index {
    nodes: HashMap<NodeId, ObservedNode>,
    /// Child -> parent. Populated from each node's own child list, so a child
    /// can be recorded before it arrives.
    parents: HashMap<NodeId, NodeId>,
    /// The damage generation each surface's nodes were last read at. A
    /// surface absent from here has never been reconciled, which is generation
    /// zero and therefore behind everything.
    reconciled: HashMap<SurfaceId, u64>,
    /// Each surface's window node: the root of the subtree a join bound to it,
    /// whose own extents say where that window is in its bridge's node space.
    /// Believed only while that node is still joined to the surface; see
    /// [`Index::window_node`].
    windows: HashMap<SurfaceId, NodeId>,
    /// Nodes that cannot be trusted, and how many frames of surface damage
    /// each one owes. Zero is a real value and the common one: an
    /// accessibility-side invalidation says the tree changed shape, which is
    /// staleness with no damage behind it.
    stale: HashMap<NodeId, u32>,
    pending: Vec<Delta>,
    /// The host's consent policy as of the last [`Index::judge`]. Taken with
    /// the visibility verdicts, so the two describe the same instant.
    consent: Consent,
}

impl Index {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many nodes are cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The cached node with this id.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&ObservedNode> {
        self.nodes.get(&id)
    }

    /// Whether this node's subtree has been invalidated and not yet re-read.
    #[must_use]
    pub fn is_stale(&self, id: NodeId) -> bool {
        self.stale.contains_key(&id)
    }

    /// How many nodes of `root`'s subtree, `root` included, are stale: what a
    /// re-read left unread.
    ///
    /// The subtree as its child lists describe it now. A node a re-read no
    /// longer finds under its parent keeps its mark and is not counted, since
    /// no read of this subtree will reach it again -- and a count that held
    /// it would leave the subtree owed a read for ever.
    #[must_use]
    pub fn stale_in(&self, root: NodeId) -> usize {
        if !self.nodes.contains_key(&root) {
            return 0;
        }
        core::iter::once(root)
            .chain(self.descendants(root))
            .filter(|id| self.stale.contains_key(id))
            .count()
    }

    /// Take everything that has changed since the last call.
    ///
    /// Draining rather than reading: a subscriber that is told twice about one
    /// change cannot tell it happened once.
    pub fn take_deltas(&mut self) -> Vec<Delta> {
        core::mem::take(&mut self.pending)
    }

    /// Load a bulk read from an [`Ingest`](crate::Ingest) into the cache.
    pub fn ingest_snapshot(&mut self, nodes: impl IntoIterator<Item = ObservedNode>) {
        for node in nodes {
            self.upsert(node);
        }
    }

    /// Apply one change volunteered by an ingest path.
    pub fn apply(&mut self, change: Change) {
        match change {
            Change::Upserted { id, node } => self.upsert(ObservedNode::unjoined(id, *node)),
            Change::Removed { id } => self.remove_subtree(id),
            Change::SubtreeInvalidated { root } => self.invalidate(root),
        }
    }

    /// Insert or update one node.
    ///
    /// Two rules live here, and both are about not lying to the layers above:
    ///
    /// - **An unchanged node produces no delta.** Otherwise `subscribe`
    ///   degenerates into a poll wearing a push's clothes.
    /// - **A changed node loses its visibility but keeps its origin.** If a
    ///   node's contents moved, its geometry may have moved with them, so the
    ///   old visibility verdict is worthless and the node must fail closed
    ///   until a `HostView` judges it again. Its owning process, by contrast,
    ///   did not change, so re-deciding provenance would be busywork.
    pub fn upsert(&mut self, mut node: ObservedNode) {
        let id = node.id;

        if let Some(existing) = self.nodes.get(&id)
            && existing.node == node.node
        {
            self.stale.remove(&id);
            return;
        }

        let existed = match self.nodes.get(&id) {
            Some(previous) => {
                node.origin = previous.origin.clone();
                node.surface = previous.surface;
                node.visibility = Visibility::Unknown;
                true
            }
            None => false,
        };

        for child in node.node.children() {
            self.parents.insert(*child, id);
        }

        self.nodes.insert(id, node);
        self.stale.remove(&id);
        self.pending.push(if existed {
            Delta::Updated { id }
        } else {
            Delta::Added { id }
        });
    }

    /// Remove a node and everything beneath it.
    fn remove_subtree(&mut self, root: NodeId) {
        let mut doomed = self.descendants(root);
        doomed.push(root);
        for id in doomed {
            if self.nodes.remove(&id).is_some() {
                self.pending.push(Delta::Removed { id });
            }
            self.parents.remove(&id);
            self.stale.remove(&id);
        }
        let nodes = &self.nodes;
        self.windows.retain(|_, window| nodes.contains_key(window));
    }

    /// Mark a subtree as no longer trustworthy.
    ///
    /// The nodes stay cached deliberately. A stale tree is still the best
    /// description of the screen anyone has, and throwing it away would leave
    /// `observe` with nothing to say between an invalidation and the re-read
    /// that answers it. Marking is what lets the layers above tell the
    /// difference between "gone" and "not to be acted on yet".
    fn invalidate(&mut self, root: NodeId) {
        if !self.nodes.contains_key(&root) {
            return;
        }
        self.stale.insert(root, 0);
        for id in self.descendants(root) {
            self.stale.insert(id, 0);
        }
        self.pending.push(Delta::Invalidated { root });
    }

    /// Node ids with no cached parent, in ascending id order.
    #[must_use]
    pub fn roots(&self) -> Vec<NodeId> {
        let mut roots: Vec<NodeId> = self
            .nodes
            .keys()
            .filter(|id| !self.parents.contains_key(id))
            .copied()
            .collect();
        roots.sort_unstable();
        roots
    }

    /// Every cached node in tree order: each root in ascending id order, then
    /// each node's children in the order the node itself lists them.
    ///
    /// Determinism is the requirement, not the traversal: `[n]` in a selector
    /// has to mean the same node twice running, and a `HashMap`'s iteration
    /// order would make it mean whatever it liked.
    #[must_use]
    pub fn preorder(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut seen = HashSet::new();
        for root in self.roots() {
            self.walk(root, &mut out, &mut seen);
        }

        // Every cached node must appear, including ones no root can reach.
        // A cycle in the reported structure leaves its members parented to
        // each other and therefore rootless, and dropping them here would let
        // a bridge delete nodes from `observe` just by describing them badly.
        // They are visited in id order, after everything properly rooted.
        let mut unreachable: Vec<NodeId> = self
            .nodes
            .keys()
            .filter(|id| !seen.contains(*id))
            .copied()
            .collect();
        unreachable.sort_unstable();
        for id in unreachable {
            self.walk(id, &mut out, &mut seen);
        }

        out
    }

    fn walk(&self, id: NodeId, out: &mut Vec<NodeId>, seen: &mut HashSet<NodeId>) {
        // A malformed tree can name a cycle; a bridge is not a trusted source
        // of structure, so refusing to loop forever is not optional.
        if !seen.insert(id) {
            return;
        }
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        out.push(id);
        for child in node.node.children() {
            self.walk(*child, out, seen);
        }
    }

    /// Every node beneath `root`, not including `root` itself, in tree order.
    #[must_use]
    pub fn descendants(&self, root: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        seen.insert(root);
        if let Some(node) = self.nodes.get(&root) {
            for child in node.node.children() {
                self.walk(*child, &mut out, &mut seen);
            }
        }
        out
    }

    /// Every node this selector matches, in tree order.
    #[must_use]
    pub fn resolve_all(&self, selector: &Selector) -> Vec<NodeId> {
        let order = self.preorder();
        let mut frontier: Option<Vec<NodeId>> = None;

        for segment in selector.segments() {
            let candidates: Vec<NodeId> = match &frontier {
                // The first segment searches the whole tree.
                None => order.clone(),
                // Later segments search the DESCENDANTS of the previous match,
                // not its children -- see the grammar notes on `>`.
                Some(previous) => {
                    let reachable: HashSet<NodeId> = previous
                        .iter()
                        .flat_map(|id| self.descendants(*id))
                        .collect();
                    order
                        .iter()
                        .copied()
                        .filter(|id| reachable.contains(id))
                        .collect()
                }
            };

            let mut matched: Vec<NodeId> = candidates
                .into_iter()
                .filter(|id| self.nodes.get(id).is_some_and(|node| segment.matches(node)))
                .collect();

            if let Some(nth) = segment.nth() {
                matched = matched.into_iter().nth(nth).into_iter().collect();
            }

            if matched.is_empty() {
                return Vec::new();
            }
            frontier = Some(matched);
        }

        frontier.unwrap_or_default()
    }

    /// Resolve a selector to exactly one node.
    ///
    /// # Errors
    ///
    /// [`Refusal::NotFound`] when nothing matched, and
    /// [`Refusal::AmbiguousSelector`] carrying the count when several did --
    /// so an agent can narrow the selector or index into it rather than guess.
    pub fn resolve(&self, selector: &Selector) -> Result<NodeId, Refusal> {
        let matches = self.resolve_all(selector);
        match matches.len() {
            0 => Err(Refusal::NotFound),
            1 => Ok(matches[0]),
            n => Err(Refusal::AmbiguousSelector { matches: n }),
        }
    }

    /// Attribute one node to the surface it was drawn on and the process that
    /// drew it.
    ///
    /// Visibility is deliberately not set here. Knowing which surface a node
    /// belongs to is not the same as knowing whether it can be seen, and a join
    /// that quietly implied `Visible` would be the whole failure of this
    /// project in one line. [`Index::judge`] is what answers that, against
    /// facts, and a node between the two calls is `Unknown` and refused.
    ///
    /// Private, so that every attribution goes through
    /// [`Index::join_subtree`] and names the window it is measured from.
    fn join(&mut self, id: NodeId, surface: SurfaceId, origin: &Origin) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.surface = Some(surface);
            node.origin = origin.clone();
        }
    }

    /// Attribute a window -- a node and everything beneath it -- to a surface,
    /// and say how many nodes were attributed.
    ///
    /// The natural unit: an accessibility bridge reports one tree per window,
    /// and every node in it was drawn on that window's surface. The exception
    /// is a menu or a combo popup, which is its own surface while its
    /// accessible nodes hang off the toplevel -- so a caller that has resolved
    /// a popup joins it separately rather than letting this walk cover it.
    ///
    /// `root` becomes the surface's window node, the one its subtree's
    /// coordinates are measured from: see [`Index::window_origin`].
    pub fn join_subtree(&mut self, root: NodeId, surface: SurfaceId, origin: &Origin) -> usize {
        if self.nodes.contains_key(&root) {
            self.windows.insert(surface, root);
        }
        let mut joined = 0;
        for id in core::iter::once(root).chain(self.descendants(root)) {
            if self.nodes.contains_key(&id) {
                self.join(id, surface, origin);
                joined += 1;
            }
        }
        joined
    }

    /// The node a join bound to this surface as its window.
    ///
    /// Only while it is still joined there. A join is never undone -- a
    /// re-join binds afresh and leaves an earlier attribution standing -- so a
    /// window node can be recorded for a surface it has since been bound away
    /// from, and believing the record then would measure one window from
    /// another's origin.
    #[must_use]
    pub fn window_node(&self, surface: SurfaceId) -> Option<NodeId> {
        let window = *self.windows.get(&surface)?;
        self.nodes
            .get(&window)
            .filter(|node| node.surface == Some(surface))
            .map(|_| window)
    }

    /// Where this surface's window geometry begins in its bridge's node
    /// space: its window node's own extents origin.
    ///
    /// # Measured, because toolkits disagree
    ///
    /// AT-SPI's `Window` coordinates are relative to "the window", and
    /// toolkits do not agree on where a window begins. GTK 4 and Qt 6 measure
    /// from the xdg window geometry, the visible frame, and report their window
    /// node at `(0, 0)` (2026-09-04). Firefox 148 measures from its buffer,
    /// client-side shadow and all, and reports its window node at the shadow's
    /// width, `(26, 23)` on the first seat it was tried on (issue #45).
    /// Assuming either convention for every toolkit puts the other's clicks
    /// beside their target, so neither is assumed: each window says where it
    /// is in its own coordinates, and that is subtracted.
    ///
    /// Read from the cached tree on every call rather than kept, so it is
    /// always as current as the nodes it corrects: the window node and its
    /// descendants were read together, and a cache behind the screen is behind
    /// it consistently. A change of this origin alone -- a maximized Firefox
    /// drops its shadow -- moves every node of the window while only the
    /// window node is reported [`Delta::Updated`].
    ///
    /// `None`, failing closed, when the surface has no window node or its
    /// window node reports no extents with any area: there is nothing to
    /// measure from, and guessing is how #45 happened.
    #[must_use]
    pub fn window_origin(&self, surface: SurfaceId) -> Option<Vec2> {
        let bounds = self.get(self.window_node(surface)?)?.node_space_bounds()?;
        (!bounds.is_empty()).then(|| bounds.origin().to_vec2())
    }

    /// A node's bounds in **window space**: relative to the origin of its
    /// window's geometry, whatever origin its toolkit measures from.
    ///
    /// The one space every toolkit is brought into, and the one everything
    /// downstream of the index uses: the visibility verdict, the rect an act
    /// is aimed at, the receipt's damage witness and what an agent is shown.
    /// `None` for a node with no bounds, no surface, or a window with no
    /// [`window_origin`](Index::window_origin), all of which are refused.
    #[must_use]
    pub fn window_bounds(&self, id: NodeId) -> Option<Rect> {
        let node = self.nodes.get(&id)?;
        let bounds = node.node_space_bounds()?;
        Some(bounds - self.window_origin(node.surface?)?)
    }

    /// Judge every cached node against a host's published facts, and report
    /// what that pass decided.
    ///
    /// Stale nodes are judged like any other. Their geometry is suspect, which
    /// is exactly why staleness is a separate refusal consulted by
    /// [`Index::actable`] -- the verdict here stays the best available
    /// description of the screen, and the gate above it is what refuses to act
    /// on a description it distrusts.
    pub fn judge(&mut self, facts: &HostFacts) -> Tally {
        self.consent = facts.consent().clone();
        // Decided first and written after: a node's window bounds are read
        // from its window node, which is one of the nodes being written.
        let verdicts: Vec<(NodeId, Judgement)> = self
            .nodes
            .iter()
            .map(|(id, node)| {
                let verdict = match (node.surface, self.window_bounds(*id)) {
                    (Some(surface), Some(rect)) => judge(facts, surface, rect),
                    // No surface joined, a bridge that reported no extents,
                    // or a window that reported none to measure them from.
                    // All are "nobody has judged this", which is what
                    // `Unknown` means and what the gate refuses.
                    _ => Judgement {
                        visibility: Visibility::Unknown,
                        unproven: false,
                    },
                };
                (*id, verdict)
            })
            .collect();
        let mut tally = Tally::default();
        for (id, verdict) in verdicts {
            tally.record(&verdict);
            if let Some(node) = self.nodes.get_mut(&id) {
                node.visibility = verdict.visibility;
            }
        }
        tally
    }

    /// Record that this surface's nodes were read as of `generation`.
    ///
    /// Called with the generation observed **before** the read started, not
    /// after. A tree takes milliseconds at best and seconds at worst to read,
    /// and anything that changed while it was being read is not in it --
    /// crediting the read with the generation it finished at would silently
    /// swallow exactly the frames a reader most needs to know it missed.
    pub fn reconcile(&mut self, surface: SurfaceId, generation: u64) {
        self.reconciled.insert(surface, generation);
    }

    /// Every node sitting under pixels that changed since this index read it.
    ///
    /// # This is a measurement, not a refusal, and the difference was measured
    ///
    /// The obvious use is staleness: pixels under a node changed, so do not
    /// trust its rectangle. Against real toolkits that rule is unusable.
    /// `gtk4-widget-factory` sitting idle with nobody touching it damages its
    /// whole window about **41 times a second**; Qt's gallery manages one small
    /// region every two seconds. Applying damage as a refusal therefore leaves
    /// 273 of a GTK application's 275 nodes un-actable at all times, with no
    /// re-read fast enough to ever catch up -- and it is not even true, because
    /// what those frames contain is a repaint of the same widgets.
    ///
    /// So damage does not decide actability here. [`Refusal::Stale`] is
    /// produced by the accessibility feed saying a subtree changed, which is a
    /// statement about *meaning*. What this returns is the other half: the
    /// nodes under rendering that the semantic feed did not explain.
    ///
    /// That difference is the whole product. Damage says something changed on
    /// screen; accessibility events say what changed. **Damage with no
    /// accompanying event is a surface that rendered without explaining
    /// itself** -- which for GTK and Qt is a repaint and can be ignored, and
    /// for a Flutter or canvas surface is the precise definition of a region no
    /// bridge can describe, and the only honest trigger for a vision fallback.
    /// Counting it on toolkits that *do* explain themselves is how that trigger
    /// gets a baseline to be compared against.
    ///
    /// Scoped to the node rather than to the surface, because the region is
    /// the information a compositor uniquely has: a spinner repainting its own
    /// corner has not touched the button across the window.
    #[must_use]
    pub fn under_damage(&self, facts: &HostFacts) -> Vec<NodeId> {
        let unreconciled: HashMap<SurfaceId, Vec<Rect>> = facts
            .surfaces()
            .iter()
            .filter_map(|surface| {
                let reconciled = self
                    .reconciled
                    .get(&surface.id)
                    .copied()
                    .unwrap_or_default();
                Some((surface.id, surface.damage_since(reconciled)?))
            })
            .collect();

        let mut under = Vec::new();
        for id in self.preorder() {
            let Some(node) = self.nodes.get(&id) else {
                continue;
            };
            let (Some(surface_id), Some(bounds)) = (node.surface, self.window_bounds(id)) else {
                continue;
            };
            let (Some(regions), Some(surface)) =
                (unreconciled.get(&surface_id), facts.surface(surface_id))
            else {
                continue;
            };
            let global = surface.to_global(bounds);
            if regions.iter().any(|region| overlaps(global, *region)) {
                under.push(id);
            }
        }
        under
    }

    /// The node with this id, if an agent may act on it.
    ///
    /// Staleness is checked **before** origin and visibility, and the order is
    /// the point: a stale node's attribution and geometry were computed from
    /// data the index already knows to be behind the screen. Reporting
    /// `Occluded` from a rect that has since moved would be a confident answer
    /// derived from something we have just admitted we do not trust, and an
    /// agent told "stale" can wait and re-read, whereas one told "occluded" will
    /// go and raise the wrong window.
    ///
    /// # Errors
    ///
    /// [`Refusal::NotFound`] for an id this index does not hold,
    /// [`Refusal::Stale`] for one behind the screen, then whatever
    /// [`check_actable`] says about the node itself -- its origin, the
    /// compositor's visibility verdict, and [`Refusal::NotShowing`] when its
    /// application says it is not shown -- and last
    /// [`Refusal::NoCapability`] for an application the host's [`Consent`]
    /// does not cover. Consent comes last so a refusal names the nearer
    /// obstacle: a covered button is reported as covered, whoever drew it.
    pub fn actable(&self, id: NodeId) -> Result<&ObservedNode, Refusal> {
        let node = self.nodes.get(&id).ok_or(Refusal::NotFound)?;
        if let Some(frames) = self.stale.get(&id) {
            return Err(Refusal::Stale { frames: *frames });
        }
        check_actable(node)?;
        if !self.consent.permits(&node.origin) {
            return Err(Refusal::NoCapability {
                origin: Box::new(node.origin.clone()),
            });
        }
        Ok(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use perspicax_node::{Node, Origin, ProcessOrigin, Role};

    /// A window with a File menu holding Open and Save, plus two Close buttons
    /// so ambiguity and indexing have something to bite on.
    fn sample() -> Index {
        let mut index = Index::new();
        index.ingest_snapshot([
            observed(1, Role::Window, Some("Text Editor"), &[2, 5, 6]),
            observed(2, Role::Menu, Some("File"), &[3, 4]),
            observed(3, Role::MenuItem, Some("Open"), &[]),
            observed(4, Role::MenuItem, Some("Save"), &[]),
            observed(5, Role::Button, Some("Close"), &[]),
            observed(6, Role::Button, Some("Close"), &[]),
        ]);
        index.take_deltas();
        index
    }

    fn observed(id: u64, role: Role, label: Option<&str>, children: &[u64]) -> ObservedNode {
        let mut node = Node::new(role);
        if let Some(label) = label {
            node.set_label(label);
        }
        node.set_children(children.iter().map(|c| NodeId(*c)).collect::<Vec<_>>());
        ObservedNode::unjoined(NodeId(id), node)
    }

    fn sel(s: &str) -> Selector {
        Selector::parse(s).unwrap()
    }

    #[test]
    fn a_snapshot_populates_the_cache_and_its_structure() {
        let index = sample();
        assert_eq!(index.len(), 6);
        assert_eq!(index.roots(), vec![NodeId(1)]);
        assert_eq!(
            index.preorder(),
            [1, 2, 3, 4, 5, 6].map(NodeId).to_vec(),
            "traversal must be document order, not hash order"
        );
    }

    #[test]
    fn resolves_a_unique_selector() {
        let index = sample();
        assert_eq!(index.resolve(&sel("menu:File")), Ok(NodeId(2)));
        assert_eq!(index.resolve(&sel("Save")), Ok(NodeId(4)));
    }

    /// The M3 demo's own selector, against a tree shaped like a real one.
    #[test]
    fn resolves_the_demo_selector_through_a_descendant_step() {
        let index = sample();
        assert_eq!(index.resolve(&sel("menu:File>Open")), Ok(NodeId(3)));
    }

    /// `>` skips toolkit scaffolding. Here the menu's items are wrapped in a
    /// generic container that no user would ever name, exactly as GTK and Qt
    /// each do differently.
    #[test]
    fn descendant_matching_skips_intermediate_containers() {
        let mut index = Index::new();
        index.ingest_snapshot([
            observed(1, Role::Menu, Some("File"), &[2]),
            observed(2, Role::GenericContainer, None, &[3]),
            observed(3, Role::MenuItem, Some("Open"), &[]),
        ]);
        assert_eq!(index.resolve(&sel("menu:File>Open")), Ok(NodeId(3)));
    }

    #[test]
    fn an_ambiguous_selector_reports_how_many_matched() {
        let index = sample();
        assert_eq!(
            index.resolve(&sel("button:Close")),
            Err(Refusal::AmbiguousSelector { matches: 2 })
        );
    }

    #[test]
    fn an_index_disambiguates_and_is_zero_based_in_tree_order() {
        let index = sample();
        assert_eq!(index.resolve(&sel("button:Close[0]")), Ok(NodeId(5)));
        assert_eq!(index.resolve(&sel("button:Close[1]")), Ok(NodeId(6)));
        assert_eq!(
            index.resolve(&sel("button:Close[2]")),
            Err(Refusal::NotFound)
        );
    }

    #[test]
    fn a_selector_matching_nothing_is_not_found() {
        let index = sample();
        assert_eq!(index.resolve(&sel("Nonexistent")), Err(Refusal::NotFound));
        assert_eq!(
            index.resolve(&sel("menu:File>Quit")),
            Err(Refusal::NotFound)
        );
    }

    #[test]
    fn a_first_read_is_all_additions() {
        let mut index = Index::new();
        index.ingest_snapshot([observed(1, Role::Button, Some("OK"), &[])]);
        assert_eq!(index.take_deltas(), vec![Delta::Added { id: NodeId(1) }]);
    }

    /// The rule the whole cache exists for.
    #[test]
    fn re_reading_an_unchanged_node_produces_no_delta() {
        let mut index = sample();
        index.ingest_snapshot([observed(3, Role::MenuItem, Some("Open"), &[])]);
        assert!(
            index.take_deltas().is_empty(),
            "an unchanged re-read produced a delta -- subscribe would be a poll"
        );
    }

    #[test]
    fn a_changed_node_produces_exactly_one_update() {
        let mut index = sample();
        index.apply(Change::Upserted {
            id: NodeId(3),
            node: Box::new({
                let mut n = Node::new(Role::MenuItem);
                n.set_label("Open...");
                n
            }),
        });
        assert_eq!(index.take_deltas(), vec![Delta::Updated { id: NodeId(3) }]);
        assert_eq!(index.get(NodeId(3)).unwrap().node.label(), Some("Open..."));
    }

    /// Content moved, so the old visibility verdict is worthless -- but the
    /// process that drew it did not change, so provenance survives.
    #[test]
    fn an_update_resets_visibility_but_keeps_origin() {
        let mut index = sample();

        let judged = index.nodes.get_mut(&NodeId(5)).unwrap();
        judged.visibility = Visibility::Visible;
        judged.origin = Origin::Process(Box::new(ProcessOrigin {
            pid: 9182,
            exe: Some("/usr/bin/gtk4-widget-factory".into()),
            cgroup: None,
            sandbox: None,
        }));

        index.apply(Change::Upserted {
            id: NodeId(5),
            node: Box::new({
                let mut n = Node::new(Role::Button);
                n.set_label("Dismiss");
                n
            }),
        });

        let after = index.get(NodeId(5)).unwrap();
        assert_eq!(after.visibility, Visibility::Unknown, "must fail closed");
        assert!(
            matches!(after.origin, Origin::Process(_)),
            "origin survives"
        );
    }

    #[test]
    fn removing_a_node_removes_its_whole_subtree() {
        let mut index = sample();
        index.apply(Change::Removed { id: NodeId(2) });

        let deltas = index.take_deltas();
        assert_eq!(deltas.len(), 3, "the menu and both of its items");
        for id in [2, 3, 4].map(NodeId) {
            assert!(deltas.contains(&Delta::Removed { id }));
            assert!(index.get(id).is_none());
        }
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn invalidation_marks_a_subtree_without_discarding_it() {
        let mut index = sample();
        index.apply(Change::SubtreeInvalidated { root: NodeId(2) });

        assert_eq!(
            index.take_deltas(),
            vec![Delta::Invalidated { root: NodeId(2) }]
        );
        for id in [2, 3, 4].map(NodeId) {
            assert!(index.is_stale(id));
            assert!(
                index.get(id).is_some(),
                "a stale node is still the best we have"
            );
        }
        assert!(!index.is_stale(NodeId(5)), "a sibling is unaffected");
    }

    #[test]
    fn re_reading_a_stale_node_clears_its_mark() {
        let mut index = sample();
        index.apply(Change::SubtreeInvalidated { root: NodeId(2) });
        assert!(index.is_stale(NodeId(3)));

        index.ingest_snapshot([observed(3, Role::MenuItem, Some("Open"), &[])]);
        assert!(!index.is_stale(NodeId(3)));
    }

    /// What a keeper asks after a re-read (#41): did it reach everything the
    /// invalidation marked? One that stopped short -- the application went
    /// quiet partway -- leaves the rest marked, and only they count.
    #[test]
    fn a_re_read_that_stops_short_leaves_only_what_it_missed_stale() {
        let mut index = sample();
        index.apply(Change::SubtreeInvalidated { root: NodeId(1) });
        assert_eq!(index.stale_in(NodeId(1)), 6, "the whole window");

        // The read reached the window and its menu, and no further.
        index.ingest_snapshot([
            observed(1, Role::Window, Some("Text Editor"), &[2, 5, 6]),
            observed(2, Role::Menu, Some("File"), &[3, 4]),
        ]);
        assert_eq!(index.stale_in(NodeId(1)), 4, "Open, Save and both Closes");
        assert_eq!(index.stale_in(NodeId(2)), 2, "and within any subtree");

        index.ingest_snapshot([
            observed(3, Role::MenuItem, Some("Open"), &[]),
            observed(4, Role::MenuItem, Some("Save"), &[]),
            observed(5, Role::Button, Some("Close"), &[]),
            observed(6, Role::Button, Some("Close"), &[]),
        ]);
        assert_eq!(index.stale_in(NodeId(1)), 0, "a read that reached it all");
    }

    /// A node a re-read no longer finds under its parent is not owed a read.
    /// No read of the tree will reach it again, so counting it would leave the
    /// tree owed one forever.
    #[test]
    fn a_node_its_parent_no_longer_lists_is_not_counted_stale() {
        let mut index = sample();
        index.apply(Change::SubtreeInvalidated { root: NodeId(2) });
        // The menu lost Save, and nothing said so.
        index.ingest_snapshot([
            observed(2, Role::Menu, Some("File"), &[3]),
            observed(3, Role::MenuItem, Some("Open"), &[]),
        ]);
        assert!(
            index.is_stale(NodeId(4)),
            "still marked: nothing removed it"
        );
        assert_eq!(index.stale_in(NodeId(1)), 0, "but no longer the window's");
        assert_eq!(
            index.stale_in(NodeId(99)),
            0,
            "and nothing of a node not held"
        );
    }

    /// A bridge is not a trusted source of structure. A cycle in the child
    /// lists must neither hang the traversal nor swallow the nodes: every
    /// member of the cycle is parented to another member, so none of them is a
    /// root, and a traversal that only started from roots would report an
    /// empty screen.
    #[test]
    fn a_cyclic_tree_terminates_and_still_yields_every_node() {
        let mut index = Index::new();
        index.ingest_snapshot([
            observed(1, Role::GenericContainer, None, &[2]),
            observed(2, Role::GenericContainer, None, &[1]),
        ]);

        assert!(index.roots().is_empty(), "a cycle has no parentless node");
        assert_eq!(index.preorder(), vec![NodeId(1), NodeId(2)]);
    }

    /// The same guarantee, stated as an invariant rather than a special case.
    #[test]
    fn preorder_visits_every_cached_node_exactly_once() {
        let index = sample();
        let order = index.preorder();
        assert_eq!(order.len(), index.len());
        assert_eq!(
            order.iter().collect::<HashSet<_>>().len(),
            index.len(),
            "a node was visited twice"
        );
    }

    // --- the join, the judgement, and the gate that consults both ------------

    use crate::host::{HostFacts, SurfaceFacts};
    use perspicax_node::{Rect, SurfaceId};

    fn placed(id: u64, label: &str, bounds: Rect) -> ObservedNode {
        let mut node = Node::new(Role::Button);
        node.set_label(label);
        node.set_bounds(bounds);
        ObservedNode::unjoined(NodeId(id), node)
    }

    fn origin() -> Origin {
        Origin::Process(Box::new(ProcessOrigin {
            pid: 9182,
            exe: Some("/usr/bin/gtk4-widget-factory".into()),
            cgroup: None,
            sandbox: None,
        }))
    }

    /// One 400x300 window holding two buttons, the second of which sits under
    /// where an overlapping surface will be placed.
    fn joined() -> Index {
        let mut index = Index::new();
        let mut root = Node::new(Role::Window);
        root.set_label("Text Editor");
        root.set_children(vec![NodeId(2), NodeId(3)]);
        root.set_bounds(Rect::new(0.0, 0.0, 400.0, 300.0));
        index.ingest_snapshot([
            ObservedNode::unjoined(NodeId(1), root),
            placed(2, "Open", Rect::new(10.0, 10.0, 90.0, 40.0)),
            placed(3, "Cancel", Rect::new(200.0, 200.0, 280.0, 240.0)),
        ]);
        index.take_deltas();
        index.join_subtree(NodeId(1), SurfaceId(1), &origin());
        index
    }

    fn desktop(cover: Option<SurfaceFacts>) -> HostFacts {
        let window = SurfaceFacts::new(SurfaceId(1), Rect::new(0.0, 0.0, 400.0, 300.0));
        match cover {
            Some(cover) => HostFacts::bottom_to_top([window, cover], 1),
            None => HostFacts::bottom_to_top([window], 1),
        }
        .with_consent(Consent::Everyone)
    }

    #[test]
    fn joining_a_subtree_attributes_every_node_beneath_it() {
        let index = joined();
        assert_eq!(index.len(), 3);
        for id in [1, 2, 3] {
            let node = index.get(NodeId(id)).unwrap();
            assert_eq!(node.surface, Some(SurfaceId(1)));
            assert_eq!(node.origin, origin());
        }
    }

    /// Knowing which surface drew a node is not knowing whether it can be
    /// seen. Between the join and the judgement every node is still refused.
    #[test]
    fn a_join_does_not_imply_visibility() {
        let index = joined();
        assert_eq!(
            index.get(NodeId(2)).unwrap().visibility,
            Visibility::Unknown
        );
        assert_eq!(
            index.actable(NodeId(2)).unwrap_err(),
            Refusal::Unjudged,
            "attributed but unjudged is still un-actable"
        );
    }

    #[test]
    fn judging_fills_in_what_the_host_could_see() {
        let mut index = joined();
        let cover = SurfaceFacts::new(SurfaceId(2), Rect::new(180.0, 180.0, 400.0, 300.0));
        let tally = index.judge(&desktop(Some(cover)));

        assert!(index.actable(NodeId(2)).is_ok(), "Open is in the clear");
        assert_eq!(
            index.actable(NodeId(3)).unwrap_err(),
            Refusal::Occluded { by: SurfaceId(2) },
            "Cancel is under the other window, and the refusal says which"
        );

        assert_eq!(tally.judged, 3);
        assert_eq!(
            tally.occluded, 2,
            "Cancel and the window root, whose own rect is the whole window \
             and so is partly covered too. Partial coverage occludes: nothing \
             here knows which point of a node an agent would aim at, and the \
             half that answers 'no' is the safe half to be wrong on"
        );
        assert_eq!(
            tally.unproven, 2,
            "both rest on policy, because the covering surface declared no \
             opaque region at all"
        );
    }

    /// A bridge that reports no extents leaves a node unjudged rather than
    /// judged visible. There is no rect, so there is no answer -- and the
    /// refusal says it is the application's silence, not the judging, that is
    /// missing.
    #[test]
    fn a_node_with_no_bounds_is_not_judged() {
        let mut index = joined();
        let tally = index.judge(&desktop(None));
        assert_eq!(
            index.get(NodeId(1)).unwrap().visibility,
            Visibility::Visible
        );
        assert_eq!(tally.unjudged, 0);

        let mut index = Index::new();
        index.ingest_snapshot([observed(9, Role::Button, Some("No bounds"), &[])]);
        index.join_subtree(NodeId(9), SurfaceId(1), &origin());
        let tally = index.judge(&desktop(None));
        assert_eq!(tally.unjudged, 1);
        assert_eq!(index.actable(NodeId(9)).unwrap_err(), Refusal::Unplaced);
    }

    /// Staleness outranks every other verdict, and the order is the point: an
    /// occlusion computed from a rect the index already knows has moved is a
    /// confident answer derived from data it has just admitted it distrusts.
    #[test]
    fn actable_reports_staleness_before_anything_else() {
        let mut index = joined();
        let cover = SurfaceFacts::new(SurfaceId(2), Rect::new(180.0, 180.0, 400.0, 300.0));
        index.judge(&desktop(Some(cover)));
        index.apply(Change::SubtreeInvalidated { root: NodeId(1) });

        assert_eq!(
            index.actable(NodeId(3)).unwrap_err(),
            Refusal::Stale { frames: 0 },
            "the occlusion verdict is still cached, and staleness still wins"
        );
        assert_eq!(
            index.get(NodeId(3)).unwrap().visibility,
            Visibility::Occluded { by: SurfaceId(2) },
            "and the description is kept, because a stale tree is still the \
             best account of the screen anyone has"
        );
    }

    /// Zero frames is a real value, not a placeholder: the accessibility bus
    /// said the tree changed shape, and no surface was damaged. The message
    /// has to read that way rather than as "stale by 0 frames".
    #[test]
    fn an_accessibility_invalidation_is_staleness_with_no_damage() {
        let mut index = joined();
        index.apply(Change::SubtreeInvalidated { root: NodeId(1) });
        assert_eq!(
            index.actable(NodeId(2)).unwrap_err().to_string(),
            "node's subtree was invalidated and has not been re-read yet"
        );
        assert_eq!(
            Refusal::Stale { frames: 4 }.to_string(),
            "node is stale by 4 frame(s) of damage"
        );
    }

    #[test]
    fn actable_refuses_an_id_this_index_never_held() {
        assert_eq!(
            joined().actable(NodeId(404)).unwrap_err(),
            Refusal::NotFound
        );
    }

    /// The measured case, in miniature: an application repainting one corner
    /// of itself does not make the rest of the window unsafe. Without this,
    /// GTK's ~41 frames a second of idle repainting would refuse every node it
    /// has, permanently, with no read fast enough to recover.
    #[test]
    fn damage_is_reported_where_it_lands_and_not_across_the_window() {
        let mut index = joined();
        let spinner = Rect::new(200.0, 200.0, 280.0, 240.0);
        let facts = HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(SurfaceId(1), Rect::new(0.0, 0.0, 400.0, 300.0))
                    .damaging([(1, spinner)]),
            ],
            2,
        )
        .with_consent(Consent::Everyone);
        index.judge(&facts);
        index.reconcile(SurfaceId(1), 0);

        assert_eq!(
            index.under_damage(&facts),
            vec![NodeId(1), NodeId(3)],
            "the window root and Cancel, which sit under the repainted corner"
        );
        assert!(
            index.actable(NodeId(3)).is_ok(),
            "and being repainted is not by itself a reason to refuse: an \
             application that redraws itself has not necessarily changed"
        );
    }

    /// Damage far apart is counted where each frame of it landed. A caret
    /// blinking in the window's corner and a menu hanging below the window
    /// leave Open and Cancel between them explained: neither was drawn on.
    #[test]
    fn damage_far_apart_leaves_what_lies_between_explained() {
        let mut index = joined();
        let facts = HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(SurfaceId(1), Rect::new(0.0, 0.0, 400.0, 300.0)).damaging([
                    (1, Rect::new(2.0, 2.0, 6.0, 8.0)),
                    (2, Rect::new(300.0, 320.0, 420.0, 400.0)),
                ]),
            ],
            2,
        )
        .with_consent(Consent::Everyone);
        index.judge(&facts);
        index.reconcile(SurfaceId(1), 0);

        assert_eq!(
            index.under_damage(&facts),
            vec![NodeId(1)],
            "only the window root, under the caret"
        );
    }

    /// Reconciling at the generation a read *finished* at would swallow the
    /// frames that arrived during it. This is the same test one generation
    /// later, and nothing should be stale.
    #[test]
    fn a_read_that_has_caught_up_reports_no_damage() {
        let mut index = joined();
        let facts = HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(SurfaceId(1), Rect::new(0.0, 0.0, 400.0, 300.0))
                    .damaging([(1, Rect::new(200.0, 200.0, 280.0, 240.0))]),
            ],
            2,
        )
        .with_consent(Consent::Everyone);
        index.judge(&facts);
        index.reconcile(SurfaceId(1), 1);

        assert!(index.under_damage(&facts).is_empty());
        assert!(index.actable(NodeId(3)).is_ok());
    }

    #[test]
    fn a_host_that_publishes_no_consent_makes_nothing_actable() {
        let mut index = joined();
        index.judge(&desktop(None).with_consent(Consent::default()));
        assert_eq!(
            index.actable(NodeId(2)).unwrap_err(),
            Refusal::NoCapability {
                origin: Box::new(origin())
            }
        );
    }

    #[test]
    fn consent_to_spawned_processes_covers_those_pids_and_no_others() {
        let mut index = joined();
        index.judge(&desktop(None).with_consent(Consent::Spawned(vec![9182])));
        assert!(
            index.actable(NodeId(2)).is_ok(),
            "9182 is the one we spawned"
        );

        index.judge(&desktop(None).with_consent(Consent::Spawned(vec![1])));
        assert!(matches!(
            index.actable(NodeId(2)),
            Err(Refusal::NoCapability { .. })
        ));
    }

    /// Consent is asked last, so the refusal names the nearer obstacle: an
    /// agent told "no capability" about a covered button would stop, when
    /// "occluded" is what it could actually act on by asking the person.
    #[test]
    fn a_covered_node_is_reported_as_covered_before_consent_is_asked() {
        let mut index = joined();
        let cover = SurfaceFacts::new(SurfaceId(2), Rect::new(0.0, 0.0, 400.0, 300.0));
        index.judge(&desktop(Some(cover)).with_consent(Consent::Nobody));
        assert!(matches!(
            index.actable(NodeId(2)),
            Err(Refusal::Occluded { .. })
        ));
    }

    /// Issue #32, in miniature. Firefox draws its menu bar, hidden until Alt
    /// is pressed, over the same rectangle as its tab strip, on the same
    /// surface. The compositor judges both nodes visible, because it judges
    /// the rectangle and the rectangle is uncovered; only the application
    /// knows which of the two is drawn there, and it says.
    #[test]
    fn of_two_nodes_drawn_over_one_rectangle_only_the_one_showing_is_actable() {
        let strip = Rect::new(0.0, 0.0, 400.0, 26.0);
        let mut window = Node::new(Role::Window);
        window.set_children(vec![NodeId(2), NodeId(3)]);
        window.set_bounds(Rect::new(0.0, 0.0, 400.0, 300.0));
        let mut menu_bar = placed(2, "File", strip);
        menu_bar.node.set_hidden();
        let tab_strip = placed(3, "Tabs", strip);

        let mut index = Index::new();
        index.ingest_snapshot([
            ObservedNode::unjoined(NodeId(1), window),
            menu_bar,
            tab_strip,
        ]);
        index.join_subtree(NodeId(1), SurfaceId(1), &origin());
        index.judge(&desktop(None));

        for id in [2, 3] {
            assert_eq!(
                index.get(NodeId(id)).unwrap().visibility,
                Visibility::Visible,
                "the compositor's verdict is about the rectangle, and it is right"
            );
        }
        assert_eq!(
            index.actable(NodeId(2)).unwrap_err(),
            Refusal::NotShowing,
            "a press on File would have landed on the tab strip"
        );
        assert!(index.actable(NodeId(3)).is_ok());
    }

    /// The application's word about its own node is nearer than the person's
    /// consent to the application: a hidden menu is reported as hidden
    /// whoever may drive it.
    #[test]
    fn a_node_not_showing_is_reported_as_such_before_consent_is_asked() {
        let mut index = Index::new();
        let mut hidden = placed(1, "File", Rect::new(0.0, 0.0, 40.0, 26.0));
        hidden.node.set_hidden();
        index.ingest_snapshot([hidden]);
        index.join_subtree(NodeId(1), SurfaceId(1), &origin());
        index.judge(&desktop(None).with_consent(Consent::Nobody));
        assert_eq!(index.actable(NodeId(1)).unwrap_err(), Refusal::NotShowing);
    }

    /// A node can outlive the surface it was read from -- a window closes
    /// between a read and a judgement. That is `Unknown`, and refused.
    #[test]
    fn a_node_whose_surface_the_host_no_longer_knows_is_unjudged() {
        let mut index = joined();
        index.judge(&desktop(None));
        assert!(index.actable(NodeId(2)).is_ok());

        let tally = index.judge(&HostFacts::default());
        assert_eq!(tally.unjudged, 3);
        assert_eq!(index.actable(NodeId(2)).unwrap_err(), Refusal::Unjudged);
    }

    // --- window space: each window measured from its own node (#45) ---------

    /// Firefox's window as issue #45 measured it on a seat. Firefox measures
    /// from its buffer, so its window node sits at the shadow's width,
    /// (26, 23), and the Paint button is where Firefox said, (50, 217), 85x41.
    /// The host placed the 800x600 window geometry at (753, 51), with the
    /// buffer starting the shadow's width up and to the left of it.
    fn firefox() -> Index {
        let mut frame = Node::new(Role::Window);
        frame.set_label("issue 33");
        frame.set_children(vec![NodeId(2)]);
        frame.set_bounds(Rect::new(26.0, 23.0, 826.0, 623.0));
        let mut index = Index::new();
        index.ingest_snapshot([
            ObservedNode::unjoined(NodeId(1), frame),
            placed(2, "Paint", Rect::new(50.0, 217.0, 135.0, 258.0)),
        ]);
        index.join_subtree(NodeId(1), SurfaceId(1), &origin());
        index
    }

    fn firefox_window() -> SurfaceFacts {
        SurfaceFacts::new(SurfaceId(1), Rect::new(753.0, 51.0, 1553.0, 651.0))
            .with_buffer_origin(Vec2::new(727.0, 28.0))
    }

    fn firefox_desktop(above: Option<SurfaceFacts>) -> HostFacts {
        HostFacts::bottom_to_top([firefox_window()].into_iter().chain(above), 1)
            .with_consent(Consent::Everyone)
    }

    /// The screenshot in #45 shows Paint drawn at (24, 194)-(109, 235) in the
    /// window: where Firefox said, less where Firefox's window begins.
    #[test]
    fn a_window_measured_from_its_buffer_is_placed_from_its_own_node() {
        let index = firefox();
        assert_eq!(
            index.window_origin(SurfaceId(1)),
            Some(Vec2::new(26.0, 23.0))
        );
        assert_eq!(
            index.window_bounds(NodeId(2)),
            Some(Rect::new(24.0, 194.0, 109.0, 235.0)),
            "the button where it is drawn"
        );
        assert_eq!(
            index.window_bounds(NodeId(1)),
            Some(Rect::new(0.0, 0.0, 800.0, 600.0)),
            "and the window node is the window"
        );
    }

    /// GTK and Qt measure from the window geometry, and nothing moves.
    #[test]
    fn a_window_measured_from_its_geometry_is_unchanged() {
        let index = joined();
        assert_eq!(index.window_origin(SurfaceId(1)), Some(Vec2::ZERO));
        assert_eq!(
            index.window_bounds(NodeId(3)),
            index.get(NodeId(3)).unwrap().node_space_bounds()
        );
    }

    /// The verdict is reached where the button is drawn. A window over its
    /// drawn place covers it; one over the place its raw bounds would put it,
    /// were they read as the window's, does not -- which is the click that
    /// landed 2.5 px below Paint in #45, judged visible on the wrong spot.
    #[test]
    fn a_node_is_judged_where_its_window_drew_it() {
        // Paint is drawn at (777, 245)-(862, 286) on the desk; raw bounds
        // read as window space would put it at (803, 268)-(888, 309).
        let over_drawn = SurfaceFacts::new(SurfaceId(2), Rect::new(770.0, 240.0, 800.0, 265.0));
        let over_raw = SurfaceFacts::new(SurfaceId(2), Rect::new(870.0, 290.0, 900.0, 320.0));

        let mut index = firefox();
        index.judge(&firefox_desktop(Some(over_drawn)));
        assert_eq!(
            index.actable(NodeId(2)).unwrap_err(),
            Refusal::Occluded { by: SurfaceId(2) }
        );

        index.judge(&firefox_desktop(Some(over_raw)));
        assert!(index.actable(NodeId(2)).is_ok(), "nothing covers Paint");
    }

    /// Damage arrives surface-local, which for Firefox is its own node space:
    /// a repaint of Paint is at (50, 217) in the buffer. The receipt's witness
    /// and `under_damage` both find it on Paint, and not a repaint where the
    /// raw bounds would have put it.
    #[test]
    fn damage_where_the_window_drew_a_node_is_on_that_node() {
        let index = firefox();
        let paint = index.window_bounds(NodeId(2)).unwrap();

        let repainted = firefox_window().damaging([(1, Rect::new(50.0, 217.0, 135.0, 258.0))]);
        assert!(repainted.damage_touches(0, paint), "on target");
        let facts = HostFacts::bottom_to_top([repainted], 2).with_consent(Consent::Everyone);
        assert!(index.under_damage(&facts).contains(&NodeId(2)));

        // In the buffer, (140, 262)-(170, 290) is clear of Paint and inside
        // where its raw bounds, read as window space, would have put it.
        let beside = firefox_window().damaging([(1, Rect::new(140.0, 262.0, 170.0, 290.0))]);
        assert!(!beside.damage_touches(0, paint), "not on target");
        let facts = HostFacts::bottom_to_top([beside], 2).with_consent(Consent::Everyone);
        assert!(!index.under_damage(&facts).contains(&NodeId(2)));
    }

    /// A window whose own node says nothing about where it is -- no extents,
    /// or none with any area -- gives nothing to measure from, so nothing on
    /// it is placed, judged or acted on. Assuming it measures from the window
    /// geometry is how every Firefox click came to miss.
    #[test]
    fn a_window_whose_own_node_reports_no_extents_is_unjudged_throughout() {
        for frame_bounds in [None, Some(Rect::new(26.0, 23.0, 26.0, 23.0))] {
            let mut frame = Node::new(Role::Window);
            frame.set_children(vec![NodeId(2)]);
            if let Some(bounds) = frame_bounds {
                frame.set_bounds(bounds);
            }
            let mut index = Index::new();
            index.ingest_snapshot([
                ObservedNode::unjoined(NodeId(1), frame),
                placed(2, "Paint", Rect::new(50.0, 217.0, 135.0, 258.0)),
            ]);
            index.join_subtree(NodeId(1), SurfaceId(1), &origin());

            assert_eq!(index.window_origin(SurfaceId(1)), None);
            assert_eq!(index.window_bounds(NodeId(2)), None);
            let tally = index.judge(&firefox_desktop(None));
            assert_eq!(tally.unjudged, 2, "{frame_bounds:?}");
            assert_eq!(index.actable(NodeId(2)).unwrap_err(), Refusal::Unjudged);
        }
    }

    #[test]
    fn a_removed_window_is_no_longer_measured_from() {
        let mut index = firefox();
        index.apply(Change::Removed { id: NodeId(1) });
        assert_eq!(index.window_origin(SurfaceId(1)), None);
        assert!(index.windows.is_empty());
    }

    /// A join is never undone, so a surface can still have a window node
    /// recorded that has since been bound to another surface. It is not that
    /// surface's window any more, and is not measured from.
    #[test]
    fn a_window_node_joined_away_is_not_its_old_surfaces_window() {
        let mut index = firefox();
        index.join_subtree(NodeId(1), SurfaceId(2), &origin());
        assert_eq!(index.window_node(SurfaceId(1)), None);
        assert_eq!(index.window_node(SurfaceId(2)), Some(NodeId(1)));
        assert_eq!(
            index.window_bounds(NodeId(2)),
            Some(Rect::new(24.0, 194.0, 109.0, 235.0)),
            "measured from the window it is joined to now"
        );
    }
}
