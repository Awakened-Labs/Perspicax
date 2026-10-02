//! The desktop, as the agent interface sees it.
//!
//! `perspicax-mcp` describes what it needs as a trait with three methods and
//! never implements it, so that the agent-facing surface stays portable -- a
//! GNOME extension or a KWin plugin satisfies the same three and gets the same
//! six tools. This is that trait satisfied, here in the composition root,
//! because this is the only place that holds the index, the host's facts and
//! the compositor's inbound channel at once.
//!
//! # Semantics are cached; visibility is not
//!
//! The index holds a tree the accessibility bus volunteered, and re-reading it
//! costs seconds. Whether a node can be *seen* is a different question with a
//! different cost: it is arithmetic over rectangles the host has already
//! published, and it is microseconds. So the two are refreshed on different
//! schedules, and [`Desk::current`] is where that rule lives -- **every read
//! and every act re-judges first**, so a window raised a moment ago is
//! accounted for even though nobody has re-read a tree since.
//!
//! Without that, the milestone's own demo would fail in the most misleading
//! possible way: the windows overlap after the trees were read, and a verdict
//! frozen at read time would confidently report the covered node as visible.
//!
//! # Acting takes the lock, and that is correct rather than convenient
//!
//! [`Desktop::act`] holds the index for the whole dispatch, including the 200
//! ms the damage window costs. That is not a concession to a simple lock
//! strategy: a tree read from the middle of a dispatch describes a moment that
//! never existed, and an agent cannot meaningfully observe a desktop mid-click.
//! Acts serialise against reads because acts serialise against reality.

use std::sync::{Mutex, MutexGuard};

use perspicax_compositor::{ActError, Facts, Host};
use perspicax_index::{
    Delta, HostFacts, Index, Receipt, Selector, Shot, ShotTarget, Verb, WindowReceipt, WindowVerb,
    check_readable,
};
use perspicax_mcp::{Denied, Desktop};
use perspicax_node::SurfaceId;

use crate::act;

/// Everything the MCP server reaches the desktop through.
pub struct Desk {
    /// The desktop's one index, and one id space. Behind a `Mutex` because the
    /// server's tools and the thread that keeps the index current are different
    /// threads by construction -- the compositor's loop is not `Send` and
    /// cannot host either of them.
    index: Mutex<Index>,
    /// What the compositor publishes outward.
    facts: Facts,
    /// And the way back in.
    host: Host,
}

impl Desk {
    /// An empty desktop, ready to be served before anything has been read.
    ///
    /// Empty rather than absent, and the distinction is the reason this is
    /// worth a constructor of its own: an MCP client connects and completes its
    /// handshake long before a toolkit has finished populating its
    /// accessibility tree, and a server that refused to start until it had
    /// something to say would look like one that had hung. `window_list` on an
    /// empty desk answers `count: 0`, which is true.
    #[must_use]
    pub fn new(facts: &Facts, host: &Host) -> Self {
        Self {
            index: Mutex::new(Index::new()),
            facts: facts.clone(),
            host: host.clone(),
        }
    }

    /// Replace the index with a freshly read one.
    ///
    /// Wholesale, and only at the start: the deltas a client has not drained go
    /// with it, and a replacement therefore reads to a subscriber as though
    /// every node had just appeared. That is honest for the first read, when
    /// every node *has*, and wrong afterwards -- which is why staying current
    /// is [`Desk::update`]'s job and not this one's.
    pub fn publish(&self, index: Index) {
        *self.lock() = index;
    }

    /// Change the index under the lock.
    ///
    /// One critical section for however many steps a refresh takes -- applying
    /// changes, re-joining the nodes they added, re-reading an invalidated
    /// subtree -- because a reader that caught the index between them would see
    /// nodes that exist and have not yet been attributed to anybody.
    pub fn update(&self, change: impl FnOnce(&mut Index, &HostFacts)) {
        let facts = self.facts.read();
        change(&mut self.lock(), &facts);
    }

    /// What the compositor is publishing right now.
    #[must_use]
    pub fn facts(&self) -> HostFacts {
        self.facts.read()
    }

    /// The index, with its visibility verdicts brought up to date.
    ///
    /// See the module documentation: judging is the cheap half and is therefore
    /// done on every access, so no caller ever reads a verdict older than its
    /// own call.
    fn current(&self) -> (MutexGuard<'_, Index>, HostFacts) {
        let facts = self.facts.read();
        let mut index = self.lock();
        index.judge(&facts);
        (index, facts)
    }

    /// The index, locked.
    ///
    /// A poisoned index is unrecoverable rather than an error to report: a
    /// thread panicked partway through a refresh, so the tree may hold nodes
    /// that have been added and not attributed, and the gate's whole
    /// fail-closed property assumes it is looking at a consistent one.
    fn lock(&self) -> MutexGuard<'_, Index> {
        self.index
            .lock()
            .expect("a panic left the index half-updated")
    }
}

impl Desktop for Desk {
    fn read(&self, visit: &mut dyn FnMut(&Index, &HostFacts)) {
        let (index, facts) = self.current();
        visit(&index, &facts);
    }

    fn deltas(&self) -> Vec<Delta> {
        self.lock().take_deltas()
    }

    fn act(&self, selector: &Selector, verb: &Verb) -> Result<Receipt, Denied> {
        let (index, _) = self.current();
        act::act(
            &index,
            &self.host,
            &self.facts,
            selector,
            verb,
            act::DAMAGE_WINDOW,
        )
        .map_err(denied)
    }

    fn act_window(&self, surface: SurfaceId, verb: WindowVerb) -> Result<WindowReceipt, Denied> {
        act::act_window(&self.host, &self.facts, surface, verb, act::DAMAGE_WINDOW).map_err(denied)
    }

    /// A window's picture is gated like reading it; a monitor's is not,
    /// because whatever the agent may not see in it is painted over by the
    /// compositor, which holds the same consent.
    fn capture(&self, target: ShotTarget) -> Result<Shot, Denied> {
        if let ShotTarget::Window(surface) = &target {
            check_readable(&self.facts.read(), *surface)?;
        }
        self.host.capture(target).map_err(|error| match error {
            ActError::NotBuilt(feature) => Denied::NotBuilt(feature.to_owned()),
            error => Denied::Undispatched(error.to_string()),
        })
    }
}

/// A failure to act, as the MCP server reports it.
fn denied(failure: act::Failure) -> Denied {
    match failure {
        // The gate's answer, carried through unchanged: it names what is in
        // the way and what would clear it, and paraphrasing it here would
        // cost the agent exactly the part it can act on.
        act::Failure::Refused(refusal) => Denied::Refused(refusal),
        // A statement about this compositor rather than about the target.
        // Flattened to its message because `perspicax-mcp` deliberately
        // cannot see the crate the type comes from, and because there is
        // nothing an agent can do with it but report it.
        act::Failure::Dispatch(error) => Denied::Undispatched(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use perspicax_compositor::Requests;
    use perspicax_index::{Consent, PointerButton, Refusal, SurfaceFacts};
    use perspicax_node::{
        Node, NodeId, ObservedNode, Origin, ProcessOrigin, Rect, Role, SurfaceId,
    };

    use super::*;

    const WINDOW: SurfaceId = SurfaceId(1);
    const OVERLAY: SurfaceId = SurfaceId(2);
    const FRAME: NodeId = NodeId(1);
    const CANCEL: NodeId = NodeId(2);

    fn origin() -> Origin {
        Origin::Process(Box::new(ProcessOrigin {
            pid: 4242,
            exe: Some("/usr/bin/gtk4-widget-factory".to_owned()),
            cgroup: None,
            sandbox: None,
        }))
    }

    /// One window, nothing over it.
    fn clear() -> HostFacts {
        HostFacts::bottom_to_top(
            [SurfaceFacts::new(WINDOW, Rect::new(0.0, 0.0, 400.0, 300.0)).owned_by(origin())],
            1,
        )
        .with_consent(Consent::Everyone)
    }

    /// The same window, with something over the button.
    fn covered() -> HostFacts {
        HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(WINDOW, Rect::new(0.0, 0.0, 400.0, 300.0)).owned_by(origin()),
                SurfaceFacts::new(OVERLAY, Rect::new(0.0, 0.0, 200.0, 200.0)).owned_by(origin()),
            ],
            2,
        )
        .with_consent(Consent::Everyone)
    }

    /// A window with one button, joined and judged against `facts`.
    fn tree(facts: &HostFacts) -> Index {
        let mut frame = Node::new(Role::Window);
        frame.set_label("Widget Factory");
        frame.set_bounds(Rect::new(0.0, 0.0, 400.0, 300.0));
        frame.set_children(vec![CANCEL]);

        let mut cancel = Node::new(Role::Button);
        cancel.set_label("Cancel");
        cancel.set_bounds(Rect::new(10.0, 10.0, 90.0, 40.0));

        let mut index = Index::new();
        index.ingest_snapshot([
            ObservedNode::unjoined(FRAME, frame),
            ObservedNode::unjoined(CANCEL, cancel),
        ]);
        index.join_subtree(FRAME, WINDOW, &origin());
        index.judge(facts);
        index
    }

    /// A desk over a screen, with a host nobody is listening to and a patience
    /// short enough that asserting so costs no wall clock.
    fn desk(facts: &HostFacts) -> Desk {
        let facts = Facts::of(facts.clone());
        let host = Host::new(&facts, &Requests::new()).waiting(Duration::from_millis(20));
        Desk::new(&facts, &host)
    }

    fn selector() -> Selector {
        Selector::parse("button:Cancel").expect("parses")
    }

    /// An MCP client connects and finishes its handshake long before a toolkit
    /// has finished populating its accessibility tree. A server that had
    /// nothing to say until then would look like one that had hung.
    #[test]
    fn a_desk_with_nothing_on_it_still_answers() {
        let desk = desk(&HostFacts::default());
        let mut nodes = None;
        desk.read(&mut |index, facts| nodes = Some((index.len(), facts.surfaces().len())));
        assert_eq!(nodes, Some((0, 0)));
        assert!(desk.deltas().is_empty());
    }

    /// The rule the module exists to state: semantics are cached and visibility
    /// is not. The index below was judged against a clear screen and says the
    /// button is visible; by the time it is read, something is over it. A
    /// verdict frozen at read time is exactly how the milestone's own demo
    /// would fail while looking like it passed.
    #[test]
    fn a_read_judges_against_the_screen_as_it_is_now() {
        let index = tree(&clear());
        assert!(
            index
                .actable(CANCEL)
                .is_ok_and(|node| node.visibility.is_actable()),
            "the fixture must start out believing the button is visible"
        );

        let desk = desk(&covered());
        desk.publish(index);

        let mut refusal = None;
        desk.read(&mut |index, _| refusal = index.actable(CANCEL).err());
        assert_eq!(refusal, Some(Refusal::Occluded { by: OVERLAY }));
    }

    /// And acting re-judges too, so a stale verdict cannot be the thing that
    /// lets a click through. Nothing is dispatched: the refusal happens before
    /// the host is ever asked, which is what the order of these two assertions
    /// is really pinning.
    #[test]
    fn an_act_is_refused_on_a_verdict_no_older_than_itself() {
        let desk = desk(&covered());
        desk.publish(tree(&clear()));

        assert_eq!(
            desk.act(&selector(), &Verb::Click(PointerButton::Left)),
            Err(Denied::Refused(Refusal::Occluded { by: OVERLAY }))
        );
    }

    /// A compositor that does not answer is not a refusal. An agent told
    /// "occluded" goes and raises a window; there is no window here, and
    /// nothing it could do differently.
    #[test]
    fn a_compositor_that_is_not_there_is_undispatched_rather_than_refused() {
        let desk = desk(&clear());
        desk.publish(tree(&clear()));

        let outcome = desk.act(&selector(), &Verb::Focus);
        assert_eq!(
            outcome,
            Err(Denied::Undispatched(
                "no compositor loop answered".to_owned()
            ))
        );
    }

    /// Draining, not reading. The first read takes the two nodes the fixture
    /// ingested; the second has nothing to say because nothing has happened.
    #[test]
    fn deltas_are_taken_once() {
        let desk = desk(&clear());
        desk.publish(tree(&clear()));

        assert_eq!(desk.deltas().len(), 2);
        assert!(desk.deltas().is_empty());
    }

    /// `update` is one critical section for however many steps a refresh takes,
    /// and it hands over the host's facts so that re-attribution does not need
    /// a second lock to find an origin.
    #[test]
    fn an_update_sees_the_index_and_the_screen_together() {
        let desk = desk(&clear());
        desk.publish(tree(&clear()));

        desk.update(|index, facts| {
            assert_eq!(facts.surfaces().len(), 1);
            // The change the refresh loop applies, through the same door.
            index.apply(perspicax_index::Change::SubtreeInvalidated { root: FRAME });
        });

        let mut refusal = None;
        desk.read(&mut |index, _| refusal = index.actable(CANCEL).err());
        assert_eq!(refusal, Some(Refusal::Stale { frames: 0 }));
    }

    /// A window's picture is gated like reading it, here, before the
    /// compositor is asked: the host below has nobody listening, so a
    /// picture that reached it would come back `Unreachable`, not refused.
    #[test]
    fn a_picture_of_a_window_the_agent_may_not_read_is_refused_before_it_is_taken() {
        let desk = desk(&clear().with_consent(Consent::Spawned(vec![1])));
        assert!(matches!(
            desk.capture(ShotTarget::Window(WINDOW)),
            Err(Denied::Refused(Refusal::NoCapability { .. }))
        ));
        assert_eq!(
            desk.capture(ShotTarget::Window(SurfaceId(99))),
            Err(Denied::Refused(Refusal::NotFound))
        );
    }
}
