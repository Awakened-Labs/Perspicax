//! The node cache, and the deltas it computes.
//!
//! Everything here is ordinary bookkeeping with one unusual requirement: it
//! must be possible to answer "what changed?" without re-reading anything. An
//! accessibility bridge that is asked for a whole tree on every tick is the
//! performance failure this project exists to remove, so the cache holds the
//! last known state and emits a [`Delta`] only where the new state actually
//! differs from it.

use std::collections::{HashMap, HashSet};

use wm_node::{NodeId, ObservedNode, Visibility};

use crate::{Change, Refusal, selector::Selector};

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
    stale: HashSet<NodeId>,
    pending: Vec<Delta>,
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
        self.stale.contains(&id)
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
        self.stale.insert(root);
        for id in self.descendants(root) {
            self.stale.insert(id);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_node::{Node, Origin, ProcessOrigin, Role};

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
}
