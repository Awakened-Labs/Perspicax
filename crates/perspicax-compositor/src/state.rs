//! The compositor's state, and the protocol handlers that mutate it.
//!
//! Smithay's shape is one state struct plus a handler trait per protocol, wired
//! together by `delegate_*!`. Nothing here is unusual for a Wayland compositor;
//! the two decisions worth reading are in [`Compositor::commit`] and
//! [`Compositor::send_frames`], and both follow from this compositor not
//! drawing anything.

use std::{
    collections::{HashMap, HashSet},
    os::unix::net::UnixStream,
    sync::{Arc, OnceLock},
    time::Instant,
};

use perspicax_node::{Origin, Rect, SurfaceId};

use crate::{act::Keys, backend::Running, facts::Facts, origin};

use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{Space, Window},
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{KeyboardHandle, XkbConfig},
        pointer::{CursorImageStatus, PointerHandle},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_surface::WlSurface},
        },
    },
    utils::{SERIAL_COUNTER, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, Damage,
            SurfaceAttributes, TraversalAction, with_states, with_surface_tree_downward,
        },
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
            XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

/// How far each new toplevel is offset from the last, so two windows opened in
/// a row are not stacked exactly on top of each other and invisible to a test
/// about overlap. A placement policy, and a placeholder for one: the arc's
/// occlusion demo needs to *choose* where windows go, and will replace this.
const CASCADE: i32 = 32;

/// How many damaged regions to remember per surface.
///
/// A history, not a log. At the ~41 frames a second an idle GTK application
/// produces this is about six seconds of memory, which is far longer than any
/// read of a tree takes; a reader further behind than this is told the whole
/// surface changed, which is true and is the safe direction to be wrong in.
const DAMAGE_HISTORY: usize = 256;

/// Everything this compositor knows.
pub struct Compositor {
    /// Kept so a new client can be inserted from an event callback.
    pub(crate) display: DisplayHandle,
    pub(crate) compositor: CompositorState,
    pub(crate) xdg_shell: XdgShellState,
    pub(crate) shm: ShmState,
    /// Held, not read. These two are RAII handles for protocol globals:
    /// dropping the `OutputManagerState` withdraws `xdg_output`, and dropping
    /// the `Seat` withdraws `wl_seat` -- so a client that connected a moment
    /// earlier would watch the desktop lose features it had already bound. `expect` rather
    /// than `allow`: the next slice dispatches focus through the seat, and this
    /// should start complaining the moment that makes it live.
    #[expect(dead_code, reason = "RAII handle for the xdg_output global")]
    pub(crate) output_manager: OutputManagerState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) data_device: DataDeviceState,
    #[expect(dead_code, reason = "RAII handle for the wl_seat global")]
    pub(crate) seat: Seat<Self>,
    /// `None` only if xkb could not compile a default keymap, which would mean
    /// the session has no usable keyboard layout at all. Focus still works
    /// without one; nothing else does.
    pub(crate) keyboard: Option<KeyboardHandle<Self>>,
    /// The pointer capability. Absent from M2 entirely, because a compositor
    /// that only reads the screen never needs to move anything; added here
    /// because clicking is the whole of what M3 is for.
    pub(crate) pointer: Option<PointerHandle<Self>>,
    /// Which key produces which character, on this seat's layout. Built once at
    /// construction: it is a pure function of the keymap, and rebuilding it per
    /// keystroke would put a keymap compile inside an agent's latency budget.
    pub(crate) keys: Keys,
    /// Where damage landed on each surface, oldest first, tagged with the
    /// generation it arrived at. Monotonic and bounded: the index compares
    /// generations against what it has reconciled, and a counter that went
    /// backwards would make a stale node look current.
    damage: HashMap<SurfaceId, Vec<(u64, Rect)>>,
    /// Surfaces that have presented a buffer at least once.
    ///
    /// A toplevel exists from the moment a client asks for one; it is on screen
    /// from the moment it puts something in it. The gap between the two is
    /// where a node would otherwise be judged visible on a window with nothing
    /// in it. Recorded here rather than read from the surface, because this
    /// compositor releases every buffer the instant it arrives and so never has
    /// one to look at afterwards.
    presented: HashSet<SurfaceId>,
    /// When each surface was last given keyboard focus. The host's half of the
    /// focus correlation the join uses to tell two windows of one process
    /// apart -- see `perspicax_index::join`.
    focused_at: HashMap<SurfaceId, Instant>,
    /// Windows and their z-order. `Space::elements()` iterates back to front,
    /// which is the order `perspicax_index::HostFacts` wants, so the two agree
    /// by construction rather than by a conversion someone has to keep right.
    pub(crate) space: Space<Window>,
    /// What this compositor last told the rest of the process. See
    /// [`crate::facts`] for why the boundary is a published copy.
    pub(crate) facts: Facts,
    /// Bumped on every publication, so a reader can tell two snapshots apart
    /// without comparing them.
    pub(crate) generation: u64,
    /// Minted per toplevel and never reused. A retired id addressing a new
    /// window is the failure `Refusal::Stale` exists to prevent, one layer up.
    next_surface: u64,
    /// The backend this compositor was brought up on, and everything it owns:
    /// the outputs, and on a seat the session, GPU and input devices.
    pub(crate) backend: Running,
    /// The linux-dmabuf global's state. Present in every seat build and
    /// advertised only when the seat backend runs, because GPU clients hand
    /// their buffers over as dmabufs and a renderer is what imports them.
    #[cfg(feature = "seat")]
    pub(crate) dmabuf: smithay::wayland::dmabuf::DmabufState,
    placed: i32,
    started: Instant,
}

impl Compositor {
    /// Bring up every global a stock GTK or Qt client expects to find, on
    /// whatever outputs the backend starts with.
    pub(crate) fn new(display: &DisplayHandle, backend: Running, facts: Facts) -> Self {
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(display, "perspicax-seat");
        let keyboard = match seat.add_keyboard(XkbConfig::default(), 200, 25) {
            Ok(keyboard) => Some(keyboard),
            Err(error) => {
                tracing::warn!(%error, "no keyboard: xkb could not compile a default keymap");
                None
            }
        };
        // A pointer is unconditional where a keyboard is not: `add_pointer`
        // cannot fail, because there is no keymap to compile.
        let pointer = Some(seat.add_pointer());

        let mut space = Space::default();
        for output in backend.initial_outputs() {
            let at = output.current_location();
            space.map_output(&output, at);
        }

        Self {
            display: display.clone(),
            compositor: CompositorState::new::<Self>(display),
            xdg_shell: XdgShellState::new::<Self>(display),
            shm: ShmState::new::<Self>(display, Vec::new()),
            output_manager: OutputManagerState::new_with_xdg_output::<Self>(display),
            data_device: DataDeviceState::new::<Self>(display),
            seat_state,
            seat,
            space,
            backend,
            #[cfg(feature = "seat")]
            dmabuf: smithay::wayland::dmabuf::DmabufState::new(),
            keyboard,
            pointer,
            keys: Keys::from_default_layout(),
            damage: HashMap::new(),
            presented: HashSet::new(),
            focused_at: HashMap::new(),
            facts,
            generation: 0,
            next_surface: 0,
            placed: 0,
            started: Instant::now(),
        }
    }

    /// Accept a connection, and settle who is on the other end of it before
    /// that client can do anything else.
    ///
    /// The ordering is awkward and unavoidable: a client's state has to be
    /// constructed before the `Client` that would answer for its credentials
    /// exists. So the origin is written once, immediately after insertion and
    /// before the loop dispatches a single request, into a slot whose empty
    /// state means `Unattributed` -- which is refused. A failure to attribute
    /// therefore costs the client its ability to be acted on, and never
    /// silently grants it somebody else's identity.
    pub(crate) fn insert_client(&mut self, stream: UnixStream) {
        let client = match self
            .display
            .insert_client(stream, Arc::new(ClientState::default()))
        {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "could not insert client");
                return;
            }
        };

        let origin = client
            .get_credentials(&self.display)
            .map_or(Origin::Unattributed, |credentials| {
                origin::of_pid(credentials.pid)
            });
        if let Some(state) = client.get_data::<ClientState>() {
            state.attribute(origin.clone());
        }
        tracing::info!(id = ?client.id(), ?origin, "client connected");
    }

    /// The next surface id. Monotonic, and never handed out twice.
    fn mint_surface_id(&mut self) -> SurfaceId {
        self.next_surface += 1;
        SurfaceId(self.next_surface)
    }

    /// Tell every surface it may draw again.
    ///
    /// A compositor that renders sends these after presenting a frame. This one
    /// never presents, so the callbacks come from a timer instead -- and they
    /// are not optional. A Wayland client draws once and then waits for
    /// permission to draw again; with no frame callbacks the tree is one frame
    /// old forever, no damage ever arrives, and the whole M2 signal is a
    /// straight line at zero. The clock is deliberately the compositor's own
    /// monotonic clock rather than the wall, because that is what the protocol
    /// says the argument means.
    pub(crate) fn send_frames(&self) {
        let elapsed = u32::try_from(self.started.elapsed().as_millis() % u128::from(u32::MAX))
            .unwrap_or(u32::MAX);
        for window in self.space.elements() {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };
            with_surface_tree_downward(
                toplevel.wl_surface(),
                (),
                |_, _, &()| TraversalAction::DoChildren(()),
                |_, states, &()| {
                    for callback in states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .frame_callbacks
                        .drain(..)
                    {
                        callback.done(elapsed);
                    }
                },
                |_, _, &()| true,
            );
        }
    }

    /// Give a surface keyboard focus, and remember when.
    pub(crate) fn focus_surface(&mut self, surface: WlSurface, id: SurfaceId) {
        let Some(keyboard) = self.keyboard.clone() else {
            return;
        };
        keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
        self.focused_at.insert(id, Instant::now());
    }

    /// When this surface was last focused.
    pub(crate) fn focused_at(&self, id: SurfaceId) -> Option<Instant> {
        self.focused_at.get(&id).copied()
    }

    /// How many frames of damage this surface has taken.
    pub(crate) fn damage_generation(&self, id: SurfaceId) -> u64 {
        self.damage
            .get(&id)
            .and_then(|history| history.last())
            .map_or(0, |(generation, _)| *generation)
    }

    /// Where damage landed on this surface, oldest first.
    pub(crate) fn damage_history(&self, id: SurfaceId) -> Vec<(u64, Rect)> {
        self.damage.get(&id).cloned().unwrap_or_default()
    }

    /// Whether this surface has ever had anything in it.
    pub(crate) fn has_presented(&self, id: SurfaceId) -> bool {
        self.presented.contains(&id)
    }

    /// The window whose toplevel owns this surface, if any.
    /// The window carrying this id, if it is still mapped.
    ///
    /// The id lives in the window's user data, put there by `new_toplevel`, so
    /// this is the inverse of the only place ids are ever handed out.
    pub(crate) fn window_for_id(&self, id: SurfaceId) -> Option<Window> {
        self.space
            .elements()
            .find(|window| window.user_data().get::<SurfaceId>() == Some(&id))
            .cloned()
    }

    /// Which surface holds the keyboard right now.
    ///
    /// Asked of the seat rather than remembered, because a client can lose
    /// focus for reasons this compositor did not initiate -- a destroyed
    /// surface being the ordinary one -- and a cached answer would then
    /// describe a window that is gone.
    pub(crate) fn focused_surface(&self) -> Option<SurfaceId> {
        let surface = self.keyboard.as_ref()?.current_focus()?;
        let window = self.window_for(&surface)?;
        window.user_data().get::<SurfaceId>().copied()
    }

    /// When this compositor started. The base of every event timestamp it
    /// sends, so synthetic input is stamped on the same clock as frame
    /// callbacks rather than on a second one that could disagree.
    pub(crate) fn started_at(&self) -> Instant {
        self.started
    }

    fn window_for(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == surface)
            })
            .cloned()
    }
}

impl CompositorHandler for Compositor {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("every client this compositor inserts carries a ClientState")
            .compositor
    }

    /// A client has finished describing a new state for a surface.
    ///
    /// Headless, the buffer is released immediately, which a rendering
    /// compositor would not do. It holds a buffer until it has drawn from it,
    /// and Smithay therefore releases the *previous* buffer when a newer one
    /// arrives -- a scheme that quietly requires the client to own at least
    /// two. The headless compositor never reads a pixel, so it is finished with
    /// a buffer the instant it arrives, and saying so keeps a single-buffered
    /// client running instead of stalled against a compositor that had no use
    /// for its contents in the first place.
    ///
    /// On a seat the renderer does read it, so the buffer goes to Smithay's
    /// renderer bookkeeping instead and is released once it has been drawn.
    /// Everything this compositor *records* -- damage, presentation -- is the
    /// same on both paths, so the facts a seat publishes are the facts CI
    /// tested.
    fn commit(&mut self, surface: &WlSurface) {
        let renders = self.backend.renders();
        let whole = declared_geometry(surface);
        let (presented, damaged) = with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            let current = attributes.current();

            // Read, not drained, when a renderer is going to want the same
            // damage next; it drains what it uses, and the rest is cleared
            // below so no commit's damage is ever counted twice.
            let scale = f64::from(current.buffer_scale.max(1));
            let mut damaged: Vec<Rect> = current
                .damage
                .iter()
                .map(|damage| match *damage {
                    Damage::Surface(rect) => to_rect(rect, 1.0),
                    // Buffer coordinates are the surface's multiplied by the
                    // scale the client declared, so dividing is what puts them
                    // back into the space every other rectangle here uses.
                    Damage::Buffer(rect) => to_rect(rect, scale),
                })
                .collect();

            let presented = !matches!(current.buffer, Some(BufferAssignment::Removed));
            if !renders {
                current.damage.clear();
                if let Some(BufferAssignment::NewBuffer(buffer)) = current.buffer.take() {
                    buffer.release();
                }
            }

            // A commit that presents a buffer without saying which part of it
            // changed has changed all of it as far as anyone here can tell.
            if damaged.is_empty() && presented {
                damaged.extend(whole);
            }
            (presented, damaged)
        });
        #[cfg(feature = "seat")]
        if renders {
            smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
            // What the renderer did not take -- damage committed without a new
            // buffer -- is cleared here, as the headless path clears it.
            with_states(surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .damage
                    .clear();
            });
        }

        if let Some(window) = self.window_for(surface) {
            window.on_commit();
            if let Some(id) = window.user_data().get::<SurfaceId>().copied() {
                if presented {
                    self.presented.insert(id);
                } else {
                    self.presented.remove(&id);
                }
                if !damaged.is_empty() {
                    let history = self.damage.entry(id).or_default();
                    let generation = history.last().map_or(0, |(g, _)| *g) + 1;
                    history.extend(damaged.into_iter().map(|rect| (generation, rect)));
                    if history.len() > DAMAGE_HISTORY {
                        history.drain(..history.len() - DAMAGE_HISTORY);
                    }
                }
            }
        }
        self.space.refresh();
        self.backend.committed();
        self.publish_facts();
    }
}

impl XdgShellHandler for Compositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Activated from the start: a toolkit that believes it is unfocused
        // renders differently, and M2 is about reading what is on screen.
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
        });
        surface.send_configure();

        // Taken before the toplevel is moved into the window.
        let wl_surface = surface.wl_surface().clone();
        let window = Window::new_wayland_window(surface);
        let id = self.mint_surface_id();
        window.user_data().insert_if_missing(|| id);

        let at = (self.placed * CASCADE, self.placed * CASCADE);
        self.placed += 1;
        self.space.map_element(window, at, true);
        tracing::info!(surface = id.0, at = ?at, "toplevel mapped");

        // Focus what just appeared. A newly mapped window taking focus is what
        // every desktop does, and here it does double duty: it is the host half
        // of the focus correlation, and it makes the toolkit's own idea of
        // which window is active agree with ours -- which is the pair of
        // observations the join weighs.
        self.focus_surface(wl_surface, id);
        self.publish_facts();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for(surface.wl_surface()) {
            self.space.unmap_elem(&window);
        }
        self.publish_facts();
    }

    /// Popups are accepted and configured, and deliberately not placed yet. A
    /// menu is its own surface while its accessible nodes hang off the
    /// toplevel, so where it belongs is a question this arc measures rather
    /// than guesses at.
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        if let Err(error) = surface.send_configure() {
            tracing::warn!(%error, "could not configure popup");
        }
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
}

impl SeatHandler for Compositor {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}

impl ShmHandler for Compositor {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}

impl BufferHandler for Compositor {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl OutputHandler for Compositor {}

impl SelectionHandler for Compositor {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Compositor {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}

impl ClientDndGrabHandler for Compositor {}
impl ServerDndGrabHandler for Compositor {}

/// Per-client state: the protocol's, and this client's provenance.
///
/// The origin is settled once, when the connection is accepted, and never
/// re-derived. Credentials are a fact about a socket at the instant it was
/// accepted; asking again later against a pid the kernel may since have
/// recycled would be worse information wearing a fresher timestamp.
#[derive(Default)]
pub(crate) struct ClientState {
    compositor: CompositorClientState,
    origin: OnceLock<Origin>,
}

impl ClientState {
    /// Record who this client is. Once, and only the first time: an identity
    /// that could be overwritten is not an attestation.
    fn attribute(&self, origin: Origin) {
        let _ = self.origin.set(origin);
    }

    /// Who this client is, or `Unattributed` if it was never established.
    pub(crate) fn origin(&self) -> Origin {
        self.origin.get().cloned().unwrap_or_default()
    }
}

impl ClientData for ClientState {
    fn initialized(&self, _id: ClientId) {}
    fn disconnected(&self, _id: ClientId, _reason: DisconnectReason) {}
}

delegate_compositor!(Compositor);
delegate_data_device!(Compositor);
delegate_output!(Compositor);
delegate_seat!(Compositor);
delegate_shm!(Compositor);
delegate_xdg_shell!(Compositor);

/// A smithay rectangle in the surface's own coordinates, divided by `scale`.
fn to_rect<Kind>(rect: smithay::utils::Rectangle<i32, Kind>, scale: f64) -> Rect {
    Rect::new(
        f64::from(rect.loc.x) / scale,
        f64::from(rect.loc.y) / scale,
        f64::from(rect.loc.x + rect.size.w) / scale,
        f64::from(rect.loc.y + rect.size.h) / scale,
    )
}

/// The window geometry a client declared, in surface-local coordinates.
///
/// Used as the extent of a commit that presented a buffer and described no
/// damage. A client that has not declared one yet has nothing on screen for
/// damage to be about.
pub(crate) fn declared_geometry(surface: &WlSurface) -> Option<Rect> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
            .map(|geometry| to_rect(geometry, 1.0))
    })
}
