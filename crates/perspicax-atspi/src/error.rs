//! What can go wrong between the accessibility bus and a node.
//!
//! `thiserror` rather than `anyhow`, because this is a library: a caller that
//! wants to react to "that application is not on the bus" differently from
//! "the bus itself is unreachable" must be able to match on the difference.

use thiserror::Error;

/// A failure reading the accessibility bus.
#[derive(Debug, Error)]
pub enum Error {
    /// The accessibility bus could not be reached or spoken to.
    ///
    /// The usual cause is environmental rather than programmatic: no graphical
    /// session, or `a11y` disabled. Over SSH it is almost always a missing
    /// `XDG_RUNTIME_DIR` / `DBUS_SESSION_BUS_ADDRESS`, which produces an empty
    /// desktop rather than an obvious failure -- so the message names the bus
    /// explicitly instead of letting a generic D-Bus error stand in for it.
    #[error("could not reach the accessibility bus: {0}")]
    Bus(#[source] atspi::AtspiError),

    /// A D-Bus method call failed.
    #[error("accessibility bus call failed: {0}")]
    Call(#[source] atspi::zbus::Error),

    /// No application on the bus answers to that name.
    ///
    /// Carries every name that *is* present, for the same reason
    /// [`Refusal`](perspicax_index::Refusal) variants carry their context: an
    /// agent or a human told only "no" can only guess, whereas one handed the
    /// list can correct a typo without a second round trip.
    #[error("no application named {name:?} on the accessibility bus (present: {})",
            if available.is_empty() { "none".to_owned() } else { available.join(", ") })]
    NoSuchApp {
        /// The name that was asked for.
        name: String,
        /// Every application name the registry did report.
        available: Vec<String>,
    },

    /// A snapshot was asked for from a node this ingest never minted.
    ///
    /// Distinct from a node that has been retired: an id from another
    /// [`Interner`](perspicax_index::Interner) is a programming error, not a
    /// race.
    #[error("node {0} was not minted by this ingest")]
    UnknownRoot(u64),
}

impl From<atspi::AtspiError> for Error {
    fn from(error: atspi::AtspiError) -> Self {
        Self::Bus(error)
    }
}

impl From<atspi::zbus::Error> for Error {
    fn from(error: atspi::zbus::Error) -> Self {
        Self::Call(error)
    }
}
