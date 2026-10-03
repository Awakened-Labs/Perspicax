//! One AccessKit adapter per surface, serving its tree on the accessibility
//! bus.
//!
//! AccessKit runs the bus on a thread of its own, with a runtime of its own,
//! so the shell's loop never waits on D-Bus. That thread asks for a tree only
//! when the bus has accessibility turned on, which may be long after the
//! surface was drawn, so the latest tree is kept where it can reach it.
//!
//! What an assistive technology asks of a tree, to click a menu item say,
//! arrives on that thread too, and is passed to the shell's loop rather than
//! acted on there.
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
    /// Serve `tree`, once the bus asks for it, with nothing on it to act on.
    #[cfg(feature = "wallpaper")]
    pub(crate) fn new(tree: TreeUpdate) -> Self {
        Self::acting(tree, Inert)
    }

    /// Serve `tree`, once the bus asks for it, passing what is asked of it
    /// to `actions`, on AccessKit's thread.
    pub(crate) fn acting(tree: TreeUpdate, actions: impl ActionHandler + Send + 'static) -> Self {
        let latest = Latest(Arc::new(Mutex::new(tree)));
        Self {
            adapter: Adapter::new(latest.clone(), actions, Inert),
            latest,
        }
    }

    /// Serve `tree` from now on.
    pub(crate) fn show(&mut self, tree: TreeUpdate) {
        *self.latest.0.lock().unwrap_or_else(PoisonError::into_inner) = tree.clone();
        self.adapter.update_if_active(|| tree);
    }

    /// Say whether the surface has the keyboard, so that an assistive
    /// technology follows the focus into it.
    #[cfg(feature = "menus")]
    pub(crate) fn focused(&mut self, focused: bool) {
        self.adapter.update_window_focus_state(focused);
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

/// Nothing on the surface to act on: a wallpaper.
struct Inert;

impl ActionHandler for Inert {
    fn do_action(&mut self, request: ActionRequest) {
        tracing::debug!(action = ?request.action, "nothing here to act on");
    }
}

impl DeactivationHandler for Inert {
    fn deactivate_accessibility(&mut self) {}
}
