//! One small desktop, shared by every test in this crate.
//!
//! Two surfaces and three nodes, arranged so that each of the three answers the
//! gate can give is present at once: a button that is visible and actable, a
//! button covered by the surface above it, and -- the case no tool that reads an
//! accessibility bus from outside can produce -- a surface with a window on it
//! that no bridge has described.
//!
//! Built from the real types rather than mocked. `Index`, `HostFacts` and
//! `judge` are the production ones; only [`Desktop`] is faked, and only because
//! there is no compositor in a unit test to be the other side of it.

use std::sync::Mutex;

use perspicax_index::{Consent, Delta, HostFacts, Index, Receipt, Selector, SurfaceFacts, Verb};
use perspicax_node::{Node, NodeId, ObservedNode, Origin, ProcessOrigin, Rect, Role, SurfaceId};

use crate::{Denied, Desktop};

/// The window's surface, the one with a tree on it.
pub(crate) const WINDOW: SurfaceId = SurfaceId(1);
/// The surface on top of it, which nothing describes.
pub(crate) const OVERLAY: SurfaceId = SurfaceId(2);

/// The window node.
pub(crate) const FRAME: NodeId = NodeId(1);
/// A button in the clear.
pub(crate) const CANCEL: NodeId = NodeId(2);
/// A button under [`OVERLAY`].
pub(crate) const BURIED: NodeId = NodeId(3);

/// Who drew the window.
pub(crate) fn origin() -> Origin {
    Origin::Process(Box::new(ProcessOrigin {
        pid: 4242,
        exe: Some("/usr/bin/gtk4-widget-factory".to_owned()),
        cgroup: None,
        sandbox: None,
    }))
}

/// The host's published facts: a window, and an undescribed surface over it.
///
/// Bottom to top, so the overlay is second and therefore above.
pub(crate) fn facts() -> HostFacts {
    HostFacts::bottom_to_top(
        [
            SurfaceFacts::new(WINDOW, Rect::new(0.0, 0.0, 400.0, 300.0))
                .owned_by(origin())
                .titled("Widget Factory")
                // One frame of damage on the top-left corner, arriving after
                // the index read the tree. `under_damage` is what turns that
                // into the number `screenshot` refuses with.
                .damaged(41)
                .damaging([(41, Rect::new(0.0, 0.0, 100.0, 60.0))]),
            SurfaceFacts::new(OVERLAY, Rect::new(100.0, 100.0, 300.0, 250.0)).owned_by(
                Origin::Process(Box::new(ProcessOrigin {
                    pid: 5150,
                    exe: None,
                    cgroup: None,
                    sandbox: None,
                })),
            ),
        ],
        7,
    )
    .with_consent(Consent::Everyone)
}

/// The window's tree, joined to [`WINDOW`] and judged against [`facts`].
pub(crate) fn index() -> Index {
    let mut frame = Node::new(Role::Window);
    frame.set_label("Widget Factory");
    frame.set_bounds(Rect::new(0.0, 0.0, 400.0, 300.0));
    frame.set_children(vec![CANCEL, BURIED]);

    let mut cancel = Node::new(Role::Button);
    cancel.set_label("Cancel");
    cancel.set_description("Discard the changes");
    cancel.set_bounds(Rect::new(10.0, 10.0, 90.0, 40.0));

    let mut buried = Node::new(Role::Button);
    buried.set_label("Apply");
    buried.set_bounds(Rect::new(120.0, 120.0, 200.0, 160.0));
    buried.set_disabled();

    let mut index = Index::new();
    index.ingest_snapshot([
        ObservedNode::unjoined(FRAME, frame),
        ObservedNode::unjoined(CANCEL, cancel),
        ObservedNode::unjoined(BURIED, buried),
    ]);
    index.join_subtree(FRAME, WINDOW, &origin());
    // One frame behind the host, which is what makes `under_damage` non-empty.
    index.reconcile(WINDOW, 40);
    index.judge(&facts());
    index
}

/// A [`Desktop`] with no compositor behind it.
///
/// It answers reads from the fixture above and answers acts with whatever it
/// was built to answer, recording what it was asked so a test can assert that
/// the server passed the selector through rather than inventing one.
pub(crate) struct Fake {
    index: Mutex<Index>,
    facts: HostFacts,
    answer: Result<Receipt, Denied>,
    asked: Mutex<Vec<(String, Verb)>>,
}

impl Fake {
    /// A desktop whose every act answers `answer`.
    pub(crate) fn answering(answer: Result<Receipt, Denied>) -> Self {
        Self {
            index: Mutex::new(index()),
            facts: facts(),
            answer,
            asked: Mutex::new(Vec::new()),
        }
    }

    /// What this desktop has been asked to do, in order.
    pub(crate) fn asked(&self) -> Vec<(String, Verb)> {
        self.asked.lock().expect("no test poisons this").clone()
    }
}

impl Desktop for Fake {
    fn read(&self, visit: &mut dyn FnMut(&Index, &HostFacts)) {
        visit(
            &self.index.lock().expect("no test poisons this"),
            &self.facts,
        );
    }

    fn deltas(&self) -> Vec<Delta> {
        self.index
            .lock()
            .expect("no test poisons this")
            .take_deltas()
    }

    fn act(&self, selector: &Selector, verb: &Verb) -> Result<Receipt, Denied> {
        self.asked
            .lock()
            .expect("no test poisons this")
            .push((selector.to_string(), verb.clone()));
        self.answer.clone()
    }
}
