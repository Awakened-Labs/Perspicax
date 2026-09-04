//! The push half: accessibility signals in, [`Change`]s out.
//!
//! Slice 3 could read a tree. This is what keeps one, and the rule it exists to
//! honour is the crate's oldest: **never poll a tree**. A full read costs a
//! round trip per node on the slow path, so re-reading to find out whether
//! anything moved is the failure this project was started over. The bus already
//! says what changed. This listens.
//!
//! # Why translation does no I/O at all
//!
//! [`translate`] is a pure function, and that is a design choice rather than an
//! accident of what was easy.
//!
//! `Cache.AddAccessible` carries an entire [`CacheItem`](atspi::CacheItem) --
//! role, states, interfaces, name, description, parentage -- so a node that
//! appears can be built completely from the signal that announced it. Nothing
//! to fetch.
//!
//! The other four signals carry a *delta*, not a node: which state flipped,
//! which characters changed. Rebuilding a full node from one of those would
//! mean going back to the bus, and doing that inside a drain would make the
//! cost of noticing a change proportional to the change rate of the whole
//! desktop, at a moment when the caller only asked what happened.
//!
//! So they become [`Change::SubtreeInvalidated`] instead: the index keeps what
//! it has, marks it untrustworthy, and whoever wants the new value pays for it
//! when they want it. That is what `Delta::Invalidated` is for, and it is why
//! [`Index::invalidate`](wm_index::Index) deliberately keeps the stale nodes
//! rather than dropping them -- a stale tree is still the best description of
//! the screen anyone has.
//!
//! The dividend is that every rule in this module is testable against a
//! synthetic event with no bus, no desktop and no application.

use std::pin::Pin;

use atspi::{
    AccessibilityConnection, AtspiError, Event,
    events::{
        cache::{AddAccessibleEvent, RemoveAccessibleEvent},
        object::{ChildrenChangedEvent, PropertyChangeEvent, StateChangedEvent, TextChangedEvent},
    },
};
use futures_util::{FutureExt, Stream, StreamExt};
use wm_index::{Change, Interner};
use wm_node::Node;

use crate::{app::ObjectKey, error::Error, map, read::from_cache_item};

/// A live subscription to one application's accessibility signals.
///
/// # The bound worth knowing about
///
/// zbus delivers signals through a broadcast queue that holds **64 messages by
/// default**, and a consumer that falls behind loses the overflow *silently* --
/// `async_broadcast` skips them and the stream yields the next message as if
/// nothing happened. Nothing in this crate can detect that.
///
/// Two things keep it from mattering here, and neither is a fix. Signals are
/// filtered to a single application, so a busy desktop's other programs cost
/// nothing; and only five signal types are subscribed rather than the whole
/// `org.a11y.atspi.Event` surface. What remains is a real obligation on the
/// caller: [`drain_changes`](wm_index::Ingest::drain_changes) must be called
/// regularly, not occasionally.
///
/// Making loss detectable needs a sequence number the protocol does not
/// provide, or a periodic reconciliation read. Both are M2's problem, and the
/// honest thing for M1 is to write the bound down rather than imply it is not
/// there.
pub struct Subscription {
    /// This subscription's **own** connection to the accessibility bus, held
    /// for as long as the stream is -- see [`Subscription::open`] for why it is
    /// not the one the reads go over.
    _bus: AccessibilityConnection,
    stream: Pin<Box<dyn Stream<Item = Result<Event, AtspiError>> + Send>>,
    /// The unique bus name of the application this ingest is reading. The
    /// accessibility bus carries every application's signals; all but one
    /// application's are somebody else's business.
    app_bus: Box<str>,
}

impl core::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Subscription")
            .field("app_bus", &self.app_bus)
            .finish_non_exhaustive()
    }
}

impl Subscription {
    /// Subscribe to the six signals that can change a tree.
    ///
    /// Six, not everything: `object:children-changed` for shape,
    /// `object:state-changed`, `object:text-changed` and
    /// `object:property-change` for contents, and the two `Cache` signals for
    /// nodes arriving and leaving. The rest of AT-SPI's event surface --
    /// mouse, keyboard, focus, document, terminal -- describes things
    /// happening *in* a tree rather than *to* it, and subscribing to them
    /// would spend the queue this depends on.
    ///
    /// # Why this opens its own connection to the bus
    ///
    /// Because sharing one deadlocks, and it took a hung test to notice.
    ///
    /// zbus hands every incoming message to each stream open on a connection
    /// through a bounded queue. Registering these signals is a *registry-wide*
    /// instruction -- at-spi2-registryd starts applications emitting them --
    /// and reading a cold GTK tree realises hundreds of accessibles, each
    /// announcing itself. That traffic fills the queue in the middle of the
    /// walk that provoked it, and once it is full the connection stops
    /// dispatching, so the replies the walk is waiting for never arrive. The
    /// read blocks on signals it caused, and nothing times out.
    ///
    /// Giving signals their own connection dissolves it. The reading
    /// connection carries no match rules and therefore has no queue to fill;
    /// this one has no outstanding method calls to stall. The cost is one
    /// extra bus connection per ingest, which is the cheapest thing in this
    /// crate by a wide margin.
    ///
    /// # Errors
    ///
    /// [`Error::Bus`] if the connection could not be opened or a match rule
    /// could not be registered.
    pub async fn open(app_bus: &str) -> Result<Self, Error> {
        let bus = AccessibilityConnection::new().await?;
        // The stream is taken *before* the registrations complete so that a
        // signal arriving during setup is queued rather than missed.
        let stream = Box::pin(bus.event_stream());

        bus.register_event::<AddAccessibleEvent>().await?;
        bus.register_event::<RemoveAccessibleEvent>().await?;
        bus.register_event::<ChildrenChangedEvent>().await?;
        bus.register_event::<StateChangedEvent>().await?;
        bus.register_event::<TextChangedEvent>().await?;
        bus.register_event::<PropertyChangeEvent>().await?;

        Ok(Self {
            _bus: bus,
            stream,
            app_bus: app_bus.into(),
        })
    }

    /// Take every signal already queued, and no more.
    ///
    /// Non-blocking by construction: `now_or_never` resolves the stream's next
    /// item only if it is already there, so a quiet desktop returns an empty
    /// vector immediately instead of parking the caller. A drain that could
    /// wait would turn "what has happened?" into "wait until something does",
    /// which is a different question with a much worse failure mode.
    pub fn drain(&mut self, interner: &mut Interner<ObjectKey>) -> Vec<Change> {
        let mut changes = Vec::new();
        while let Some(item) = self.stream.next().now_or_never() {
            match item {
                // The stream ended: the bus went away. Nothing further will
                // arrive, and saying so is the caller's business, not a panic.
                None => break,
                Some(Ok(event)) => {
                    if let Some(change) = translate(&event, &self.app_bus, interner) {
                        changes.push(change);
                    }
                }
                // A signal this version of `atspi` cannot decode is not a
                // reason to drop the ones after it.
                Some(Err(error)) => {
                    tracing::debug!(%error, "undecodable accessibility signal");
                }
            }
        }
        changes
    }
}

/// One accessibility signal as a [`Change`], or `None` if it says nothing this
/// index needs to hear.
///
/// Pure, and deliberately so -- see the module documentation. `None` covers
/// three cases that are all genuinely "no change to report": a signal from
/// another application, a signal about a node this ingest has never interned,
/// and a signal type that does not alter a tree.
///
/// The second is the one worth naming. An event about an unknown object must
/// **not** mint an id: the accessibility bus describes an entire application,
/// and a snapshot may have been taken of one window inside it. Interning on
/// hearsay would fill the index with nodes nobody asked for and no snapshot
/// would ever confirm. [`Change::Upserted`] is the sole exception, because an
/// `AddAccessible` is by definition about something new.
pub fn translate(
    event: &Event,
    app_bus: &str,
    interner: &mut Interner<ObjectKey>,
) -> Option<Change> {
    match event {
        Event::Cache(atspi::events::CacheEvents::Add(added)) => {
            let raw = from_cache_item(&added.node_added)?;
            if raw.key.bus() != app_bus {
                return None;
            }
            let id = interner.intern(raw.key.clone());
            let mut node = Node::new(map::role(raw.role, raw.states));
            if !raw.label.is_empty() {
                node.set_label(raw.label);
            }
            if !raw.description.is_empty() {
                node.set_description(raw.description);
            }
            map::apply_states(raw.states, &mut node);
            map::apply_actions(raw.states, raw.interfaces, &mut node);
            Some(Change::Upserted {
                id,
                node: Box::new(node),
            })
        }

        Event::Cache(atspi::events::CacheEvents::Remove(removed)) => {
            let key = ObjectKey::from_owned(&removed.node_removed)?;
            if key.bus() != app_bus {
                return None;
            }
            let id = interner.get(&key)?;
            // Retired, never reissued. A toolkit hands the same object path
            // straight back out to the next dialog, and an agent still holding
            // this id must not find it addressing that dialog instead.
            interner.retire(id);
            Some(Change::Removed { id })
        }

        // Shape changed: the parent's child list is no longer what was read.
        Event::Object(atspi::events::ObjectEvents::ChildrenChanged(changed)) => {
            invalidate(&changed.item, app_bus, interner)
        }

        // Contents changed. The signal carries a delta, not a node, so the
        // honest report is "what you hold is stale" rather than a guess at
        // what it became.
        Event::Object(atspi::events::ObjectEvents::StateChanged(changed)) => {
            invalidate(&changed.item, app_bus, interner)
        }
        Event::Object(atspi::events::ObjectEvents::TextChanged(changed)) => {
            invalidate(&changed.item, app_bus, interner)
        }
        Event::Object(atspi::events::ObjectEvents::PropertyChange(changed)) => {
            invalidate(&changed.item, app_bus, interner)
        }

        _ => None,
    }
}

/// Mark a known node's subtree stale, if the node is one of ours.
fn invalidate(
    item: &atspi::ObjectRefOwned,
    app_bus: &str,
    interner: &Interner<ObjectKey>,
) -> Option<Change> {
    let key = ObjectKey::from_owned(item)?;
    if key.bus() != app_bus {
        return None;
    }
    Some(Change::SubtreeInvalidated {
        root: interner.get(&key)?,
    })
}

#[cfg(test)]
mod tests {
    use atspi::{
        CacheItem, InterfaceSet, ObjectRef, ObjectRefOwned, Operation, Role as AtspiRole, State,
        StateSet,
        events::{CacheEvents, ObjectEvents},
    };
    use wm_node::Role;

    use super::*;

    const APP: &str = ":1.2";

    fn object(bus: &'static str, path: &'static str) -> ObjectRefOwned {
        ObjectRef::from_static_str_unchecked(bus, path).into()
    }

    fn cache_item(path: &'static str, role: AtspiRole, name: &str) -> CacheItem {
        CacheItem {
            object: object(APP, path),
            app: object(APP, "/org/a11y/atspi/accessible/root"),
            parent: object(APP, "/org/a11y/atspi/accessible/root"),
            index: 0,
            children: 0,
            ifaces: InterfaceSet::empty(),
            short_name: name.to_owned(),
            role,
            name: String::new(),
            states: [
                State::Showing,
                State::Visible,
                State::Sensitive,
                State::Enabled,
            ]
            .into_iter()
            .collect::<StateSet>(),
        }
    }

    fn added(path: &'static str, role: AtspiRole, name: &str) -> Event {
        Event::Cache(CacheEvents::Add(AddAccessibleEvent {
            item: object(APP, "/org/a11y/atspi/accessible/root"),
            node_added: cache_item(path, role, name),
        }))
    }

    fn removed(bus: &'static str, path: &'static str) -> Event {
        Event::Cache(CacheEvents::Remove(RemoveAccessibleEvent {
            item: object(APP, "/org/a11y/atspi/accessible/root"),
            node_removed: object(bus, path),
        }))
    }

    fn state_changed(bus: &'static str, path: &'static str) -> Event {
        Event::Object(ObjectEvents::StateChanged(StateChangedEvent {
            item: object(bus, path),
            state: State::Checked,
            enabled: true,
        }))
    }

    fn children_changed(path: &'static str) -> Event {
        Event::Object(ObjectEvents::ChildrenChanged(ChildrenChangedEvent {
            item: object(APP, path),
            operation: Operation::Insert,
            index_in_parent: 0,
            child: object(APP, "/new"),
        }))
    }

    /// An add carries a whole cache item, so a complete node comes out of the
    /// signal with nothing fetched. This is the reason `translate` is pure.
    #[test]
    fn an_added_node_is_built_entirely_from_its_signal() {
        let mut interner = Interner::new();
        let change = translate(
            &added("/button", AtspiRole::Button, "Cancel"),
            APP,
            &mut interner,
        )
        .expect("an add is a change");

        match change {
            Change::Upserted { id, node } => {
                assert_eq!(id, NodeIdOf(&interner, "/button"));
                assert_eq!(node.role(), Role::Button);
                assert_eq!(node.label(), Some("Cancel"));
                assert!(!node.is_hidden(), "the item reported Showing and Visible");
            }
            other => panic!("expected an upsert, got {other:?}"),
        }
    }

    /// Retired, never reissued -- the rule `Interner` exists for, exercised by
    /// the signal that actually triggers it in the wild.
    #[test]
    fn a_removed_node_retires_its_id_rather_than_freeing_it() {
        let mut interner = Interner::new();
        translate(
            &added("/dialog-ok", AtspiRole::Button, "OK"),
            APP,
            &mut interner,
        )
        .unwrap();
        let before = interner.get(&ObjectKey::new(APP, "/dialog-ok")).unwrap();

        let change = translate(&removed(APP, "/dialog-ok"), APP, &mut interner);
        assert_eq!(change, Some(Change::Removed { id: before }));
        assert!(interner.get(&ObjectKey::new(APP, "/dialog-ok")).is_none());

        // The toolkit hands the same path back for a different widget.
        let after = interner.intern(ObjectKey::new(APP, "/dialog-ok"));
        assert_ne!(before, after);
    }

    /// A delta signal says what is no longer trustworthy, not what it became.
    /// Rebuilding the node here would mean going back to the bus mid-drain.
    #[test]
    fn a_contents_signal_invalidates_rather_than_guesses() {
        let mut interner = Interner::new();
        translate(
            &added("/check", AtspiRole::CheckBox, "Bold"),
            APP,
            &mut interner,
        )
        .unwrap();
        let id = interner.get(&ObjectKey::new(APP, "/check")).unwrap();

        assert_eq!(
            translate(&state_changed(APP, "/check"), APP, &mut interner),
            Some(Change::SubtreeInvalidated { root: id })
        );
    }

    #[test]
    fn a_shape_signal_invalidates_the_parent_that_changed() {
        let mut interner = Interner::new();
        translate(&added("/box", AtspiRole::Panel, ""), APP, &mut interner).unwrap();
        let id = interner.get(&ObjectKey::new(APP, "/box")).unwrap();

        assert_eq!(
            translate(&children_changed("/box"), APP, &mut interner),
            Some(Change::SubtreeInvalidated { root: id })
        );
    }

    /// The accessibility bus carries every application's signals. All but one
    /// application's are somebody else's business, and must not mint ids here.
    #[test]
    fn another_applications_signals_are_not_this_ingests_business() {
        let mut interner = Interner::new();
        assert_eq!(
            translate(&state_changed(":1.99", "/other"), APP, &mut interner),
            None
        );
        assert_eq!(
            translate(&removed(":1.99", "/other"), APP, &mut interner),
            None
        );
        assert_eq!(interner.minted(), 0, "a foreign signal minted an id");
    }

    /// A signal about a node no snapshot has ever seen is not a change to
    /// report -- and interning on hearsay would fill the index with nodes
    /// nothing will ever confirm.
    #[test]
    fn a_signal_about_an_unknown_node_mints_nothing() {
        let mut interner = Interner::new();
        assert_eq!(
            translate(&state_changed(APP, "/never-read"), APP, &mut interner),
            None
        );
        assert_eq!(interner.minted(), 0);
    }

    /// Test-only helper: the id a path was interned under, for assertion.
    #[expect(non_snake_case, reason = "reads as a value at the call site")]
    fn NodeIdOf(interner: &Interner<ObjectKey>, path: &'static str) -> wm_node::NodeId {
        interner.get(&ObjectKey::new(APP, path)).expect("interned")
    }
}
