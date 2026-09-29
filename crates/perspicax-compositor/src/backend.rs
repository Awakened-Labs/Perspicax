//! Which window system the compositor sits on.
//!
//! Two, and they differ in exactly one direction: where the pixels go and
//! where the input comes from. Everything between -- the protocol handlers,
//! the facts, the act path -- is the same code whichever backend is running,
//! which is the property worth protecting. A compositor that behaved one way
//! headless and another on a seat would be one whose CI tested the wrong
//! thing.
//!
//! The seat backend is a cargo feature because it costs C libraries (libseat,
//! udev, DRM, GBM, EGL, GLES, libinput) that the headless build and the four
//! gates do not need. A binary built without it still knows the backend's
//! *name*, so that asking for it produces an error saying how to get it rather
//! than a flag the parser has never heard of.

use crate::Error;

/// Where the compositor puts its output and gets its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// No window system at all: one virtual output of this size in pixels, no
    /// renderer, and input only from [`crate::Host`]. What CI and an agent
    /// with no person present run.
    Headless {
        /// The virtual output's size. Every global rect in
        /// [`perspicax_index::HostFacts`] is expressed in this space.
        size: (i32, i32),
    },
    /// A real session: the outputs the GPU has connected, the keyboards and
    /// pointers libinput finds, device access negotiated through libseat.
    /// Needs the `seat` feature.
    Seat,
}

impl Default for Backend {
    fn default() -> Self {
        Self::Headless { size: (1920, 1080) }
    }
}

impl Backend {
    /// Refuse a backend this binary was built without, naming the feature
    /// that would provide it.
    ///
    /// Separate from [`crate::run`] so a caller can ask before doing anything
    /// expensive -- the binary checks this before it touches the
    /// accessibility bus, so a missing feature is the first thing reported
    /// rather than the last.
    ///
    /// # Errors
    ///
    /// [`Error::NotBuilt`] when the backend's feature is off.
    pub fn ensure_built(self) -> Result<(), Error> {
        match self {
            Self::Headless { .. } => Ok(()),
            Self::Seat if cfg!(feature = "seat") => Ok(()),
            Self::Seat => Err(Error::NotBuilt {
                backend: "seat",
                feature: "seat",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_needs_no_feature() {
        assert!(Backend::default().ensure_built().is_ok());
    }

    #[cfg(not(feature = "seat"))]
    #[test]
    fn a_seat_without_the_feature_names_the_feature() {
        let error = Backend::Seat.ensure_built().unwrap_err();
        assert!(
            error.to_string().contains("`seat` cargo feature"),
            "the message has to say how to get the backend, got: {error}"
        );
    }

    #[cfg(feature = "seat")]
    #[test]
    fn a_seat_with_the_feature_is_built() {
        assert!(Backend::Seat.ensure_built().is_ok());
    }
}
