//! One AccessKit adapter per surface, serving its tree on the accessibility
//! bus.
//!
//! AccessKit runs the bus on a thread of its own, with a runtime of its own,
//! so the shell's loop never waits on D-Bus. That thread asks for a tree only
//! when the bus has accessibility turned on, which may be long after the
//! surface was drawn, so the latest tree is kept where it can reach it.
//!
//! Dropping a surface's [`Served`] takes its window off the bus.

use std::sync::{Arc, Mutex, PoisonError};

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};
use accesskit_unix::Adapter;

/// A surface's tree, on the bus.
pub(crate) struct Served {
    adapter: Adapter,
    latest: Latest,
}

impl Served {
    /// Serve `tree`, once the bus asks for it.
    pub(crate) fn new(tree: TreeUpdate) -> Self {
        let latest = Latest(Arc::new(Mutex::new(tree)));
        Self {
            adapter: Adapter::new(latest.clone(), Inert, Inert),
            latest,
        }
    }

    /// Serve `tree` from now on.
    pub(crate) fn show(&mut self, tree: TreeUpdate) {
        *self.latest.0.lock().unwrap_or_else(PoisonError::into_inner) = tree.clone();
        self.adapter.update_if_active(|| tree);
    }
}

/// The tree as last built, for the adapter's thread to start from.
#[derive(Clone)]
struct Latest(Arc<Mutex<TreeUpdate>>);

impl ActivationHandler for Latest {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        Some(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
    }
}

/// Nothing on the shell can be acted on through the bus yet.
struct Inert;

impl ActionHandler for Inert {
    fn do_action(&mut self, request: ActionRequest) {
        tracing::debug!(action = ?request.action, "the shell offers no actions yet");
    }
}

impl DeactivationHandler for Inert {
    fn deactivate_accessibility(&mut self) {}
}
