//! Bus-side identity: where an accessible lives, and which application it
//! belongs to.
//!
//! Neither type leaves this crate's vocabulary by accident. [`ObjectKey`] is
//! the key `wm-index`'s [`Interner`](wm_index::Interner) is instantiated over,
//! so it is translated into an opaque [`NodeId`](wm_node::NodeId) before any
//! layer above sees it; [`AppRef`] holds the one piece of information this
//! crate is deliberately *not* allowed to pass upward at all.

use atspi::{ObjectRef, ObjectRefOwned};

/// Where one accessible lives on the accessibility bus.
///
/// A unique connection name plus an object path -- AT-SPI's own coordinates,
/// and the natural interner key. `atspi::ObjectRef` would be the obvious type
/// to reuse, but it is borrowed and implements neither `Hash` nor a derived
/// `PartialEq`, so it cannot key a map. This owns its two strings instead.
///
/// `Box<str>` rather than `String`: one of these exists per node in the tree
/// and neither field is ever appended to, so the capacity word is pure waste
/// at a few thousand nodes per application.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectKey {
    bus: Box<str>,
    path: Box<str>,
}

impl ObjectKey {
    /// The key for a bus name and object path.
    #[must_use]
    pub fn new(bus: &str, path: &str) -> Self {
        Self {
            bus: bus.into(),
            path: path.into(),
        }
    }

    /// The unique connection name of the application serving this object.
    #[must_use]
    pub fn bus(&self) -> &str {
        &self.bus
    }

    /// The object path within that application.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The key for a borrowed reference, or `None` if it is the null ref.
    ///
    /// Null is not a defensive `Option` here, it is a value that genuinely
    /// arrives: an application root reports its parent as `("",
    /// /org/a11y/atspi/null)`, and that is how a tree says "this is the top".
    /// A conversion that could not express it would have to invent a key for
    /// nothing.
    #[must_use]
    pub fn from_ref(object: &ObjectRef<'_>) -> Option<Self> {
        Some(Self::new(object.name_as_str()?, object.path_as_str()))
    }

    /// The key for an owned reference, or `None` if it is the null ref.
    #[must_use]
    pub fn from_owned(object: &ObjectRefOwned) -> Option<Self> {
        Some(Self::new(object.name()?.as_str(), object.path().as_str()))
    }
}

/// One application, as the accessibility bus describes it.
///
/// # Why the pid is in this crate and goes no further
///
/// `org.a11y.atspi.Application` exposes an id, a toolkit name and two version
/// strings -- and no process id at all. The pid here comes from a different and
/// better place: `GetConnectionUnixProcessID` on the accessibility bus's own
/// daemon, which answers from the peer credentials of the socket the
/// application connected on. That is a kernel-attested number, not a
/// self-report, and it is worth being precise that this crate uses the
/// stronger of the two available sources.
///
/// It still does not make an [`Origin`](wm_node::Origin), for a reason that
/// survives the number being true: it attributes a **bus connection**, not a
/// **surface**. Knowing which process joined the accessibility bus says nothing
/// about which process drew the window a node claims to live in, and a
/// compositor is the only thing that can answer the second question. An
/// application may also serve accessibility for trees it did not draw -- that
/// is what `org.a11y.atspi.Socket` is for -- so even an honest pid can name
/// the wrong author.
///
/// So it is kept here, in this crate's own vocabulary. In M2 the compositor
/// will hold a pid from the *Wayland* client's credentials, and this one
/// becomes useful for exactly one thing: a correlation hint when joining an
/// accessibility tree to a surface. A hint that agrees is evidence; a hint that
/// disagrees is a finding. Neither is an attribution, and until a `HostView`
/// exists every node this crate produces is
/// [`Origin::Unattributed`](wm_node::Origin::Unattributed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppRef {
    name: String,
    toolkit: String,
    root: ObjectKey,
    bus_pid: Option<u32>,
}

impl AppRef {
    /// Describe an application from what the registry reported about it.
    #[must_use]
    pub fn new(name: String, toolkit: String, root: ObjectKey, bus_pid: Option<u32>) -> Self {
        Self {
            name,
            toolkit,
            root,
            bus_pid,
        }
    }

    /// The application's own name for itself, as `--app` matches against.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The toolkit string the application reports -- `"GTK"`, `"Qt"`, and so
    /// on. Diagnostic only: the read strategy is chosen by what the
    /// application actually *answers*, never by what it calls itself. See
    /// [`Strategy`](crate::Strategy).
    #[must_use]
    pub fn toolkit(&self) -> &str {
        &self.toolkit
    }

    /// The application's root accessible.
    #[must_use]
    pub fn root(&self) -> &ObjectKey {
        &self.root
    }

    /// The pid the accessibility bus daemon observed for this connection.
    ///
    /// True, and still not an attribution -- read the type-level note before
    /// using it for anything. It exists for M2's focus-correlation join and for
    /// diagnostics, and it must never become an [`Origin`](wm_node::Origin).
    #[must_use]
    pub fn bus_pid(&self) -> Option<u32> {
        self.bus_pid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_distinct_per_path_within_one_application() {
        let a = ObjectKey::new(":1.2", "/org/a11y/atspi/accessible/1");
        let b = ObjectKey::new(":1.2", "/org/a11y/atspi/accessible/2");
        assert_ne!(a, b);
        assert_eq!(a.bus(), ":1.2");
    }

    /// Two applications recycle object paths freely -- `/root` exists in every
    /// one of them -- so the bus name has to be part of the key.
    #[test]
    fn the_same_path_in_two_applications_is_two_keys() {
        assert_ne!(
            ObjectKey::new(":1.2", "/org/a11y/atspi/accessible/root"),
            ObjectKey::new(":1.3", "/org/a11y/atspi/accessible/root"),
        );
    }

    /// The pid is reachable from this crate and from nowhere else. If this
    /// ever compiles against `wm_node::Origin`, the boundary has moved.
    #[test]
    fn a_bus_pid_never_becomes_an_origin() {
        let app = AppRef::new(
            "gtk4-widget-factory".to_owned(),
            "GTK".to_owned(),
            ObjectKey::new(":1.2", "/org/a11y/atspi/accessible/root"),
            Some(4242),
        );
        assert_eq!(app.bus_pid(), Some(4242));
        assert_eq!(app.toolkit(), "GTK");
    }
}
