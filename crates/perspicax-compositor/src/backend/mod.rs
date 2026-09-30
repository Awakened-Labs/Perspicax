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

#[cfg(any(feature = "seat", test))]
mod connectors;
#[cfg(feature = "seat")]
mod cursor;
#[cfg(any(feature = "seat", test))]
mod hatch;
#[cfg(any(feature = "seat", test))]
mod pointer;
#[cfg(feature = "seat")]
pub(crate) mod seat;

use smithay::{
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            LoopHandle,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::DisplayHandle,
    },
    utils::Transform,
};

use perspicax_index::Consent;

use crate::{Config, Error, FRAME_INTERVAL, state::Compositor};

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

/// A backend, brought up: what the compositor holds for as long as it runs.
///
/// An enum on [`Compositor`] rather than a type parameter, because the two
/// differ in a handful of places -- where outputs come from, when clients are
/// told to draw, whether a buffer is kept after commit -- and a generic would
/// thread a parameter through every protocol handler to express that.
pub(crate) enum Running {
    /// One virtual output, created once and never changed.
    Headless {
        /// Mapped at the origin once, then held: dropping it would withdraw
        /// the `wl_output` global from clients that already bound it.
        output: Output,
    },
    /// The session, the GPU and the outputs on it. Boxed because it is large
    /// and the headless variant is not.
    #[cfg(feature = "seat")]
    Seat(Box<seat::Session>),
}

impl Running {
    /// Bring a backend up, as far as it can go before the compositor exists.
    ///
    /// # Errors
    ///
    /// [`Error::NotBuilt`] for a backend this build left out, and
    /// [`Error::Seat`] for a seat that would not come up.
    pub(crate) fn start(
        config: &Config,
        display: &DisplayHandle,
        #[cfg_attr(not(feature = "seat"), expect(unused_variables, reason = "the seat's"))]
        event_loop: &LoopHandle<'static, Compositor>,
    ) -> Result<Self, Error> {
        config.backend.ensure_built()?;
        match config.backend {
            Backend::Headless { size } => Ok(Self::headless(display, size)),
            #[cfg(feature = "seat")]
            Backend::Seat => Ok(Self::Seat(Box::new(seat::Session::open(
                event_loop,
                config.config.clone(),
            )?))),
            #[cfg(not(feature = "seat"))]
            Backend::Seat => unreachable!("ensure_built refuses a backend this build left out"),
        }
    }

    /// The headless backend: one output of `size`, advertised to clients, at
    /// the origin of the global space.
    pub(crate) fn headless(display: &DisplayHandle, size: (i32, i32)) -> Self {
        let output = Output::new(
            "perspicax-headless".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "perspicax".to_owned(),
                model: "virtual".to_owned(),
            },
        );
        let mode = Mode {
            size: size.into(),
            refresh: 60_000,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        output.create_global::<Compositor>(display);
        Self::Headless { output }
    }

    /// The outputs to map when the compositor is built. The seat has none
    /// yet: its outputs arrive from the first connector scan, by the same path
    /// a hotplugged monitor takes.
    pub(crate) fn initial_outputs(&self) -> Vec<Output> {
        match self {
            Self::Headless { output } => vec![output.clone()],
            #[cfg(feature = "seat")]
            Self::Seat(_) => Vec::new(),
        }
    }

    /// Whether this backend reads client buffers after commit. The renderer
    /// does; headless never looks at a pixel and releases each buffer as it
    /// arrives. See [`Compositor`]'s `commit`.
    pub(crate) fn renders(&self) -> bool {
        match self {
            Self::Headless { .. } => false,
            #[cfg(feature = "seat")]
            Self::Seat(_) => true,
        }
    }

    /// Something on screen may have changed: a surface committed, the pointer
    /// moved, the cursor image changed. Headless draws nothing, so only the
    /// seat has anything to do.
    pub(crate) fn redraw(&mut self) {
        match self {
            Self::Headless { .. } => {}
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.request_frames(),
        }
    }

    /// Finish bringing the backend up, now the compositor exists, and start
    /// telling clients when to draw.
    ///
    /// Headless, that is a timer: nothing is ever presented, so nothing else
    /// would ever tell a client it may draw again. On a seat it is the
    /// monitors' vblank, armed by the first connector scan.
    ///
    /// # Errors
    ///
    /// [`Error::EventLoop`] if the timer cannot be added; [`Error::Seat`] if
    /// no monitor could be lit.
    pub(crate) fn attach(
        state: &mut Compositor,
        event_loop: &LoopHandle<'static, Compositor>,
    ) -> Result<(), Error> {
        match state.backend {
            Self::Headless { .. } => {
                event_loop
                    .insert_source(Timer::immediate(), |_, (), state: &mut Compositor| {
                        state.send_frames();
                        TimeoutAction::ToDuration(FRAME_INTERVAL)
                    })
                    .map_err(|error| Error::EventLoop(error.to_string()))?;
                Ok(())
            }
            #[cfg(feature = "seat")]
            Self::Seat(_) => seat::attach(state),
        }
    }

    /// Whose applications an agent may act on, before anything is spawned.
    ///
    /// Headless: everyone. Nobody sits at a headless compositor, and its
    /// socket exists for the agent that started it. On a seat, only what
    /// perspicax itself spawns for an agent. The person launched everything
    /// else, and nobody has asked them.
    pub(crate) fn consent(&self) -> Consent {
        match self {
            Self::Headless { .. } => Consent::Everyone,
            #[cfg(feature = "seat")]
            Self::Seat(_) => Consent::Spawned(Vec::new()),
        }
    }

    /// Whether a person sits at this seat. It decides everything that exists
    /// for a person and would get in an agent's way: `activated` following
    /// the keyboard, interactive moves and resizes, maximize, minimize,
    /// popup grabs. Headless keeps the deterministic M2 behaviour its tests
    /// are written against. See `crate::shell`.
    pub(crate) fn has_person(&self) -> bool {
        self.renders()
    }

    /// Start what the session runs besides its windows: Xwayland, if built
    /// and wanted, then (on a seat) the autostart list once `DISPLAY` exists.
    /// Headless starts Xwayland only when the caller asked for it, and has no
    /// autostart.
    ///
    /// # Errors
    ///
    /// [`Error::Seat`] if Xwayland will not start at all.
    pub(crate) fn populate(
        state: &mut Compositor,
        #[cfg_attr(
            not(any(feature = "seat", feature = "xwayland")),
            expect(unused_variables, reason = "the seat's and Xwayland's")
        )]
        event_loop: &LoopHandle<'static, Compositor>,
        #[cfg_attr(
            not(feature = "xwayland"),
            expect(unused_variables, reason = "Xwayland's")
        )]
        headless_xwayland: bool,
    ) -> Result<(), Error> {
        match state.backend {
            #[cfg(feature = "xwayland")]
            Self::Headless { .. } if headless_xwayland => {
                crate::xwayland::start(state, event_loop, |_| {})
            }
            Self::Headless { .. } => Ok(()),
            #[cfg(feature = "seat")]
            Self::Seat(_) => seat::populate(state, event_loop),
        }
    }

    /// Whether the person at the keyboard asked the session to end.
    pub(crate) fn exit_requested(&self) -> bool {
        match self {
            Self::Headless { .. } => false,
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.exit_requested(),
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
