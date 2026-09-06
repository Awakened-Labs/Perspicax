//! Stable identity for nodes.
//!
//! An ingest path names nodes in whatever vocabulary its transport uses --
//! AT-SPI2 addresses an object by a `(bus name, object path)` pair, a future
//! Wayland protocol will hand over an AccessKit id directly. Neither is a
//! [`NodeId`], and neither should leak upward: the layers above this crate
//! address a node by one opaque integer, cheaply, and never learn what it was
//! called underneath.
//!
//! [`Interner`] performs that translation, and its single interesting property
//! is what it refuses to do -- see [`Interner::retire`].

use std::{collections::HashMap, hash::Hash};

use perspicax_node::NodeId;

/// A bidirectional map from an ingest path's own key type to [`NodeId`].
///
/// Generic in the key so that this crate stays transport-agnostic:
/// `perspicax-atspi` instantiates it over an AT-SPI object reference, a fast
/// path would instantiate it over something else, and the index itself never
/// learns the difference.
#[derive(Debug)]
pub struct Interner<K> {
    forward: HashMap<K, NodeId>,
    reverse: HashMap<NodeId, K>,
    next: u64,
}

impl<K> Default for Interner<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K> Interner<K> {
    /// An empty interner.
    ///
    /// Ids start at 1, so a zero arriving from anywhere is recognisable as a
    /// value this interner did not mint rather than a plausible first node.
    #[must_use]
    pub fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
            next: 1,
        }
    }

    /// How many keys are currently interned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    /// Whether nothing is currently interned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// How many ids have ever been minted, including retired ones.
    ///
    /// Differs from [`len`](Self::len) by exactly the number of retirements,
    /// which is the cheapest way to see the never-reuse rule holding.
    #[must_use]
    pub fn minted(&self) -> u64 {
        self.next - 1
    }
}

impl<K: Eq + Hash + Clone> Interner<K> {
    /// The id for `key`, minting one if this is the first time it is seen.
    ///
    /// Idempotent for a live key: interning the same key twice returns the same
    /// id, which is what makes an id "stable for the node's lifetime".
    pub fn intern(&mut self, key: K) -> NodeId {
        if let Some(&id) = self.forward.get(&key) {
            return id;
        }
        let id = NodeId(self.next);
        self.next += 1;
        self.forward.insert(key.clone(), id);
        self.reverse.insert(id, key);
        id
    }

    /// The id for `key`, if it is currently interned.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<NodeId> {
        self.forward.get(key).copied()
    }

    /// The key an id was minted for, if it is still live.
    #[must_use]
    pub fn key(&self, id: NodeId) -> Option<&K> {
        self.reverse.get(&id)
    }

    /// Retire an id, returning the key it was minted for.
    ///
    /// **The id is not returned to circulation, ever.** This is the whole point
    /// of the type. Toolkits recycle their own handles freely -- an AT-SPI
    /// object path belonging to a closed dialog is very likely to be handed
    /// straight back out to the next one -- and if this map recycled ids to
    /// match, an agent holding a node id from before the dialog closed would
    /// find it silently addressing a different widget in a different dialog.
    /// That is not a stale reference, which [`Refusal::Stale`] exists to catch;
    /// it is a reference that has quietly become valid again while meaning
    /// something else, which nothing downstream can detect.
    ///
    /// So a key that is retired and later re-interned gets a **new** id. It is
    /// a new node, and the numbering says so.
    ///
    /// [`Refusal::Stale`]: crate::Refusal::Stale
    pub fn retire(&mut self, id: NodeId) -> Option<K> {
        let key = self.reverse.remove(&id)?;
        self.forward.remove(&key);
        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent_for_a_live_key() {
        let mut interner = Interner::new();
        let first = interner.intern("dialog/button-1");
        let again = interner.intern("dialog/button-1");
        assert_eq!(first, again);
        assert_eq!(interner.len(), 1);
        assert_eq!(interner.minted(), 1);
    }

    #[test]
    fn distinct_keys_get_distinct_ids() {
        let mut interner = Interner::new();
        let a = interner.intern("a");
        let b = interner.intern("b");
        assert_ne!(a, b);
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn ids_start_at_one_so_zero_is_recognisably_foreign() {
        let mut interner = Interner::new();
        assert_eq!(interner.intern("first"), NodeId(1));
    }

    #[test]
    fn round_trips_in_both_directions() {
        let mut interner = Interner::new();
        let id = interner.intern("widget");
        assert_eq!(interner.get(&"widget"), Some(id));
        assert_eq!(interner.key(id), Some(&"widget"));
    }

    /// The rule the type exists for. A toolkit reusing an object path must not
    /// be able to resurrect a node id an agent is still holding.
    #[test]
    fn a_recycled_key_gets_a_new_id_never_the_old_one() {
        let mut interner = Interner::new();
        let before = interner.intern("dialog/ok");

        assert_eq!(interner.retire(before), Some("dialog/ok"));
        assert!(interner.is_empty());
        assert_eq!(interner.key(before), None);

        // The toolkit hands the very same path back out for a different widget.
        let after = interner.intern("dialog/ok");
        assert_ne!(
            before, after,
            "a retired id was reissued -- an agent's stale handle would now \
             address a different widget with no way to notice"
        );
        assert_eq!(interner.minted(), 2);
        assert_eq!(interner.len(), 1);
    }

    #[test]
    fn retiring_an_unknown_id_is_not_an_error() {
        let mut interner: Interner<&str> = Interner::new();
        assert_eq!(interner.retire(NodeId(42)), None);
    }
}
