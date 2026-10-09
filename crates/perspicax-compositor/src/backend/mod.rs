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
#[cfg(any(feature = "seat", feature = "capture"))]
pub(crate) mod cursor;
#[cfg(any(feature = "seat", test))]
mod hatch;
#[cfg(any(feature = "seat", test))]
mod pointer;
#[cfg(feature = "seat")]
pub(crate) mod seat;

use smithay::{
    input::keyboard::LedState,
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            LoopHandle,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::{DisplayHandle, backend::GlobalId},
    },
    utils::Transform,
};

use perspicax_index::Consent;
use perspicax_policy::{Access, MouseBindings, Place, Shape};

use crate::{Config, Error, FRAME_INTERVAL, state::Compositor};

/// Where the compositor puts its output and gets its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// No window system at all: virtual outputs, no renderer, and input only
    /// from [`crate::Host`]. What CI and an agent with no person present run.
    Headless {
        /// The virtual monitors, at least one. Placed the way a seat's are,
        /// by [`perspicax_policy::arrange`], so a desk of several monitors can
        /// be tested on a machine that has one, or none.
        outputs: Vec<Virtual>,
        /// How many workspaces and how they relate to the monitors, as a
        /// seat's `[workspaces]` table would say. The default is one.
        workspaces: Shape,
        /// Which clients may use the protocols that reach past their own
        /// windows, as a seat's `[protocols]` table would say. The default
        /// is any client: headless hosts only what it was told to start.
        access: Access,
        /// Whether a person sits at it, as one does at a seat: windows may
        /// then maximize, go fullscreen and minimize themselves, `activated`
        /// follows the keyboard, and the rest of what only a person gets.
        /// For tests of exactly that; an agent's desk has nobody, and the
        /// default is `false`. See [`Backend::with_person`].
        person: bool,
    },
    /// A real session: the outputs the GPU has connected, the keyboards and
    /// pointers libinput finds, device access negotiated through libseat.
    /// Needs the `seat` feature.
    Seat,
}

/// One virtual monitor of a headless compositor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Virtual {
    /// Its connector name, as a client sees it in `wl_output.name`, and as
    /// another virtual monitor names it to be placed beside it.
    pub name: String,
    /// Its size in pixels, at scale 1.
    pub size: (i32, i32),
    /// Where it goes, as a seat's `[[output]]` rule would say.
    pub place: Place,
}

impl Virtual {
    /// The `nth` virtual monitor, counting from 1, at `size`, placed to the
    /// right of the ones before it.
    #[must_use]
    pub fn numbered(nth: usize, size: (i32, i32)) -> Self {
        Self {
            name: format!("HEADLESS-{nth}"),
            size,
            place: Place::Auto,
        }
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::headless((1920, 1080))
    }
}

impl Backend {
    /// Headless, with one virtual monitor of `size`: what CI has always run.
    #[must_use]
    pub fn headless(size: (i32, i32)) -> Self {
        Self::Headless {
            outputs: vec![Virtual::numbered(1, size)],
            workspaces: Shape::default(),
            access: Access::open(),
            person: false,
        }
    }

    /// The same, with a person sitting at it, so a test can see what a
    /// person gets on a seat. A seat has one already.
    #[must_use]
    pub fn with_person(mut self) -> Self {
        if let Self::Headless { person, .. } = &mut self {
            *person = true;
        }
        self
    }

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
    pub fn ensure_built(&self) -> Result<(), Error> {
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
    /// Virtual outputs, each held with its placement and its global until
    /// it is unplugged: dropping the global would withdraw the `wl_output`
    /// from clients that already bound it.
    Headless {
        outputs: Vec<Plugged>,
        workspaces: Shape,
        /// Boxed: a rule for each protocol makes it most of this variant,
        /// and the seat's is boxed already.
        access: Box<Access>,
        /// Whether a person sits at it. See [`Running::has_person`].
        person: bool,
        /// Virtual monitors a display tool turned off, kept so it can turn
        /// them on again.
        dark: Vec<Virtual>,
        /// What pictures are drawn with, made the first time one is asked
        /// for. See [`crate::capture`]. Boxed, as the seat's session is: it
        /// is large, and the rest of this is not.
        #[cfg(feature = "capture")]
        pictures: Option<Box<Pictures>>,
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
        match &config.backend {
            Backend::Headless {
                outputs,
                workspaces,
                access,
                person,
            } => Ok(Self::Headless {
                outputs: outputs.iter().map(|out| plug(display, out)).collect(),
                workspaces: *workspaces,
                access: Box::new(access.clone()),
                person: *person,
                dark: Vec::new(),
                #[cfg(feature = "capture")]
                pictures: None,
            }),
            #[cfg(feature = "seat")]
            Backend::Seat => Ok(Self::Seat(Box::new(seat::Session::open(
                event_loop,
                config.config.clone(),
            )?))),
            #[cfg(not(feature = "seat"))]
            Backend::Seat => unreachable!("ensure_built refuses a backend this build left out"),
        }
    }

    /// Every lit output, with where its config says it goes. What
    /// [`Compositor::arrange_outputs`] places.
    pub(crate) fn placements(&self) -> Vec<(Output, Place)> {
        match self {
            Self::Headless { outputs, .. } => outputs
                .iter()
                .map(|plugged| (plugged.output.clone(), plugged.place.clone()))
                .collect(),
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.placements(),
        }
    }

    /// How many workspaces, and whether they span the monitors.
    pub(crate) fn workspace_shape(&self) -> Shape {
        match self {
            Self::Headless { workspaces, .. } => *workspaces,
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.settings.workspaces,
        }
    }

    /// Who may use the protocols that reach past their own windows: the
    /// person's `[protocols]` on a seat.
    pub(crate) fn access(&self) -> Access {
        match self {
            Self::Headless { access, .. } => Access::clone(access),
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.settings.protocols.clone(),
        }
    }

    /// How dragging a window to an edge snaps it: the person's config on a
    /// seat. Headless nobody drags anything.
    pub(crate) fn snapping(&self) -> Option<perspicax_policy::Snapping> {
        match self {
            Self::Headless { .. } => None,
            #[cfg(feature = "seat")]
            Self::Seat(session) => Some(session.settings.snapping),
        }
    }

    /// How hard the edges of screens and panels hold a window being moved:
    /// the person's config on a seat. Headless nobody drags anything.
    pub(crate) fn resistance(&self) -> Option<perspicax_policy::Resistance> {
        match self {
            Self::Headless { .. } => None,
            #[cfg(feature = "seat")]
            Self::Seat(session) => Some(session.settings.resistance),
        }
    }

    /// Who draws window frames, and how they look: the person's config on a
    /// seat. Headless takes the defaults, so the frames a seat would draw are
    /// in the facts CI tests, though nothing is drawn.
    pub(crate) fn decorations(&self) -> perspicax_policy::Decorations {
        match self {
            Self::Headless { .. } => perspicax_policy::Decorations::default(),
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.settings.decorations,
        }
    }

    /// The focus policy: the person's `[focus]` on a seat. Headless has
    /// none, for nobody points at anything there.
    pub(crate) fn focus(&self) -> Option<perspicax_policy::Focus> {
        match self {
            Self::Headless { .. } => None,
            #[cfg(feature = "seat")]
            Self::Seat(session) => Some(session.settings.focus),
        }
    }

    /// What the mouse's buttons and wheel are bound to when the compositor
    /// starts: the person's `[mouse]` on a seat, and nothing headless.
    pub(crate) fn mouse(&self) -> MouseBindings {
        match self {
            Self::Headless { .. } => MouseBindings::default(),
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.settings.mouse.clone(),
        }
    }

    /// How soon, in milliseconds, a second click makes a double-click: the
    /// person's `double-click-ms` on a seat.
    pub(crate) fn double_click_ms(&self) -> u32 {
        match self {
            Self::Headless { .. } => perspicax_policy::DOUBLE_CLICK_MS,
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.settings.pointer.double_click_ms,
        }
    }

    /// Whether this backend keeps client buffers after commit, to draw from.
    /// The seat's renderer does, and so does headless when it can take
    /// pictures, which are drawn from the buffers last committed. Otherwise
    /// headless never looks at a pixel and releases each buffer as it
    /// arrives. See [`Compositor`]'s `commit`.
    pub(crate) fn keeps_buffers(&self) -> bool {
        match self {
            Self::Headless { .. } => cfg!(feature = "capture"),
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

    /// Light the keyboards' lock keys as xkb has them. Headless has no
    /// keyboard with lights.
    pub(crate) fn light(
        &mut self,
        #[cfg_attr(not(feature = "seat"), expect(unused_variables, reason = "the seat's"))]
        leds: LedState,
    ) {
        match self {
            Self::Headless { .. } => {}
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.light(leds),
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
                state.arrange_outputs();
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
    /// the keyboard, interactive moves and resizes, a window maximizing,
    /// going fullscreen or minimizing itself, popup grabs. Headless keeps
    /// the deterministic M2 behaviour its tests are written against, unless
    /// a test seats a person to see what a person gets. See `crate::shell`.
    pub(crate) fn has_person(&self) -> bool {
        match self {
            Self::Headless { person, .. } => *person,
            #[cfg(feature = "seat")]
            Self::Seat(_) => true,
        }
    }

    /// Start what the session runs besides its windows: Xwayland, if built
    /// and wanted, then `ready` -- the agent's programs -- and (on a seat)
    /// the autostart list, once `DISPLAY` exists. Without Xwayland, `ready`
    /// runs at once. Headless starts Xwayland only when the caller asked for
    /// it, and has no autostart.
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
        ready: impl FnOnce(&mut Compositor) + 'static,
    ) -> Result<(), Error> {
        match state.backend {
            #[cfg(feature = "xwayland")]
            Self::Headless { .. } if headless_xwayland => {
                crate::xwayland::start(state, event_loop, ready)
            }
            Self::Headless { .. } => {
                ready(state);
                Ok(())
            }
            #[cfg(feature = "seat")]
            Self::Seat(_) => seat::populate(state, event_loop, ready),
        }
    }

    /// Collect the session's own children that have exited.
    pub(crate) fn reap(&mut self) {
        match self {
            Self::Headless { .. } => {}
            #[cfg(feature = "seat")]
            Self::Seat(session) => session.reap(),
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

/// What a headless compositor draws pictures with: pixman, in software, and
/// the pointer's images, read the first time a picture asks for the pointer,
/// so a screen recording of a headless session shows the pointer a seat
/// would.
#[cfg(feature = "capture")]
pub(crate) struct Pictures {
    pub(crate) renderer: smithay::backend::renderer::pixman::PixmanRenderer,
    pub(crate) cursor: Option<cursor::Cursor>,
}

/// A virtual output a headless compositor is running.
pub(crate) struct Plugged {
    pub(crate) output: Output,
    pub(crate) place: Place,
    pub(crate) global: GlobalId,
}

/// Bring a virtual monitor up and advertise it. Where it goes is decided
/// afterwards, with every other output, by [`Compositor::arrange_outputs`].
pub(crate) fn plug(display: &DisplayHandle, virtual_output: &Virtual) -> Plugged {
    let output = Output::new(
        virtual_output.name.clone(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "perspicax".to_owned(),
            model: "virtual".to_owned(),
        },
    );
    let mode = Mode {
        size: virtual_output.size.into(),
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        None,
    );
    output.set_preferred(mode);
    let global = output.create_global::<Compositor>(display);
    Plugged {
        output,
        place: virtual_output.place.clone(),
        global,
    }
}

impl Compositor {
    /// Carry out a [`crate::Command`].
    pub(crate) fn command(&mut self, command: &crate::Command) {
        match command {
            crate::Command::Plug(virtual_output) => self.plug_virtual(virtual_output),
            crate::Command::Unplug(name) => self.unplug_virtual(name),
            crate::Command::Perform(action) => self.perform(action),
            crate::Command::Protocols(access) => {
                self.set_access(access.clone());
                self.reconfigure_shells();
            }
            crate::Command::ReconfigureShell => self.reconfigure_shells(),
            crate::Command::Keymap(keymap) => {
                if let Err(error) = self.set_keymap(keymap, None) {
                    tracing::warn!(%error, ?keymap, "xkb could not compile that keymap");
                }
            }
            crate::Command::LayoutSwitching(switching) => {
                self.layout_memory.set_switching(*switching);
            }
            crate::Command::MouseBindings(bindings) => self.mouse = bindings.clone(),
            crate::Command::Click { at, button, mods } => {
                self.stand_in_click(*at, *button, *mods);
            }
            crate::Command::Scroll { at, v120, mods } => {
                self.stand_in_scroll(*at, *v120, *mods);
            }
        }
    }

    /// Plug a virtual monitor into a headless compositor, the way a person
    /// plugs one into a desk. A seat's monitors are its hardware's, so there
    /// this is refused in the log and changes nothing.
    pub(crate) fn plug_virtual(&mut self, virtual_output: &Virtual) {
        let outputs = match &mut self.backend {
            Running::Headless { outputs, .. } => outputs,
            #[cfg(feature = "seat")]
            Running::Seat(_) => {
                tracing::warn!(
                    name = virtual_output.name,
                    "only a headless compositor has virtual outputs"
                );
                return;
            }
        };
        if outputs
            .iter()
            .any(|plugged| plugged.output.name() == virtual_output.name)
        {
            tracing::warn!(name = virtual_output.name, "already plugged in");
            return;
        }
        outputs.push(plug(&self.display, virtual_output));
        self.arrange_outputs();
    }

    /// Unplug a virtual monitor. Its windows are rescued onto the monitors
    /// that remain, as they are when a seat's monitor is unplugged.
    pub(crate) fn unplug_virtual(&mut self, name: &str) {
        let outputs = match &mut self.backend {
            Running::Headless { outputs, .. } => outputs,
            #[cfg(feature = "seat")]
            Running::Seat(_) => {
                tracing::warn!(name, "only a headless compositor has virtual outputs");
                return;
            }
        };
        let Some(at) = outputs
            .iter()
            .position(|plugged| plugged.output.name() == name)
        else {
            tracing::warn!(name, "no virtual output by that name");
            return;
        };
        if outputs.len() == 1 {
            tracing::warn!(
                name,
                "the last output stays: a compositor with none has nowhere to put a window"
            );
            return;
        }
        let gone = outputs.remove(at);
        self.workspaces.forget_output(&gone.output.name());
        crate::layers::close_on(&gone.output);
        self.space.unmap_output(&gone.output);
        self.display.remove_global::<Compositor>(gone.global);
        self.arrange_outputs();
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
