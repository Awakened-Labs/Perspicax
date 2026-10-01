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
    time::{Duration, Instant},
};

use perspicax_index::Consent;
use perspicax_node::{Origin, Rect, SurfaceId};
use perspicax_policy::Workspaces;

use smithay::wayland::seat::WaylandFocus;

use crate::{act::Keys, backend::Running, facts::Facts, focus::FocusTarget, origin, shell};

use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_primary_selection,
    delegate_seat, delegate_shm, delegate_xdg_activation, delegate_xdg_shell,
    desktop::{PopupKind, PopupManager, Space, Window},
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{KeyboardHandle, XkbConfig},
        pointer::{CursorImageStatus, GrabStartData, PointerHandle},
    },
    reexports::{
        calloop::LoopHandle,
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, DisplayHandle, Resource as _,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{
                wl_buffer::WlBuffer, wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface,
            },
        },
    },
    utils::{SERIAL_COUNTER, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, Damage,
            SurfaceAttributes, TraversalAction, with_states, with_surface_tree_downward,
        },
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
            primary_selection::{
                PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
            },
        },
        session_lock::SessionLockManagerState,
        shell::wlr_layer::WlrLayerShellState,
        shell::xdg::{
            PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
            XdgShellState,
        },
        shm::{ShmHandler, ShmState},
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};

/// How many damaged regions to remember per surface.
///
/// A history, not a log. At the ~41 frames a second an idle GTK application
/// produces this is about six seconds of memory, which is far longer than any
/// read of a tree takes; a reader further behind than this is told the whole
/// surface changed, which is true and is the safe direction to be wrong in.
const DAMAGE_HISTORY: usize = 256;

/// How long after the person's last key or pointer event an agent's act is
/// refused as racing them. Long enough to cover the gap between keystrokes of
/// someone typing steadily, short enough that an agent waiting for a pause is
/// not waiting long.
const PERSON_QUIET: Duration = Duration::from_millis(1500);

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
    /// Whose applications an agent may act on, published with every
    /// snapshot. Starts as the backend's policy; [`Compositor::grant`] adds
    /// what `run` spawns.
    pub(crate) consent: Consent,
    /// When the person at the seat last pressed a key or moved the pointer.
    /// `None` headless, where nobody is at the seat, and so never a reason to
    /// refuse an agent there.
    person_at: Option<Instant>,
    /// What the focused client asked the pointer to look like. Only the seat
    /// draws it; headless records it and nothing reads it.
    pub(crate) cursor: CursorImageStatus,
    /// Popups (menus, tooltips), tracked so they render and hit-test with
    /// their window and so a grab can dismiss them.
    pub(crate) popups: PopupManager,
    /// Windows that are open and not on screen: minimized, or on a
    /// workspace that is not showing. Unmapped from the space, kept here in
    /// the order they went, and described to the index as unmapped rather
    /// than forgotten. Which of the two is in each window's
    /// [`shell::Placement`]; see `shell::workspaces` for the one rule that
    /// decides what is in the space and what is here.
    pub(crate) parked: Vec<Window>,
    /// Which workspace every window is on and which each monitor shows.
    pub(crate) workspaces: Workspaces<SurfaceId, String>,
    /// The window being moved with the pointer, while it is: an edge flip
    /// takes it along to the next workspace.
    pub(crate) dragging: Option<Window>,
    /// Where the window being dragged would snap if let go now. See
    /// `shell::snap`.
    pub(crate) snap_preview: Option<shell::SnapPreview>,
    /// RAII handles for the primary-selection and xdg-activation globals.
    primary_selection: PrimarySelectionState,
    activation: XdgActivationState,
    /// Panels, launchers, wallpapers: the layer-shell global. See
    /// [`crate::layers`].
    pub(crate) layer_shell: WlrLayerShellState,
    /// The session-lock global, and the lock itself while one is held. See
    /// [`crate::lock`].
    pub(crate) lock_manager: SessionLockManagerState,
    pub(crate) lock: Option<crate::lock::Locked>,
    /// Idle notification (swayidle) and the surfaces inhibiting it.
    pub(crate) idle: IdleNotifierState<Self>,
    #[expect(dead_code, reason = "RAII handle for the idle-inhibit global")]
    idle_inhibit: IdleInhibitManagerState,
    pub(crate) inhibitors: Vec<WlSurface>,
    /// Xwayland, its window manager, and the xwayland-shell global. See
    /// [`crate::xwayland`].
    #[cfg(feature = "xwayland")]
    pub(crate) xwayland: crate::xwayland::Xwayland,
    #[cfg(feature = "xwayland")]
    pub(crate) xwayland_shell: Option<smithay::wayland::xwayland_shell::XWaylandShellState>,
    /// The event loop, for the handlers that must schedule work on it.
    #[cfg_attr(
        not(feature = "xwayland"),
        expect(dead_code, reason = "Xwayland's selections")
    )]
    pub(crate) loop_handle: LoopHandle<'static, Self>,
    /// How to start a program against this compositor. Set by `run` once the
    /// socket exists, so `None` only before any client could connect.
    pub(crate) launch: Option<crate::Launch>,
    /// How many windows have been placed, for the cascade.
    pub(crate) placed: u32,
    started: Instant,
}

impl Compositor {
    /// Bring up every global a stock GTK or Qt client expects to find, on
    /// whatever outputs the backend starts with.
    pub(crate) fn new(
        display: &DisplayHandle,
        event_loop: LoopHandle<'static, Self>,
        backend: Running,
        facts: Facts,
    ) -> Self {
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

        let consent = backend.consent();
        let workspace_shape = backend.workspace_shape();
        // Empty: outputs are mapped by `arrange_outputs`, once every one
        // the backend starts with is known, so the first is placed knowing
        // about the rest.
        let space = Space::default();

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
            consent,
            person_at: None,
            cursor: CursorImageStatus::default_named(),
            generation: 0,
            next_surface: 0,
            popups: PopupManager::default(),
            parked: Vec::new(),
            workspaces: Workspaces::new(workspace_shape),
            dragging: None,
            snap_preview: None,
            primary_selection: PrimarySelectionState::new::<Self>(display),
            activation: XdgActivationState::new::<Self>(display),
            layer_shell: WlrLayerShellState::new::<Self>(display),
            // Any client may lock. A lock client is the one kind that must
            // work when everything else has gone wrong.
            lock_manager: SessionLockManagerState::new::<Self, _>(display, |_| true),
            lock: None,
            idle: IdleNotifierState::new(display, event_loop.clone()),
            #[cfg(feature = "xwayland")]
            xwayland: crate::xwayland::Xwayland::default(),
            #[cfg(feature = "xwayland")]
            xwayland_shell: None,
            loop_handle: event_loop,
            idle_inhibit: IdleInhibitManagerState::new::<Self>(display),
            inhibitors: Vec::new(),
            launch: None,
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
    pub(crate) fn mint_surface_id(&mut self) -> SurfaceId {
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
        let windows = self.space.elements().filter_map(shell::surface_of);
        let layers = self
            .layers_in(&[
                crate::layers::BELOW[0],
                crate::layers::BELOW[1],
                crate::layers::ABOVE[0],
                crate::layers::ABOVE[1],
            ])
            .into_iter()
            .map(|(layer, _)| layer.wl_surface().clone());
        let covers = self
            .lock
            .iter()
            .flat_map(|locked| &locked.surfaces)
            .map(|cover| cover.surface.wl_surface().clone());
        for surface in windows.chain(layers).chain(covers).collect::<Vec<_>>() {
            with_surface_tree_downward(
                &surface,
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

    /// Extend consent to processes this compositor started. Meaningful only
    /// under [`Consent::Spawned`]: headless already consents to everyone.
    pub(crate) fn grant(&mut self, pids: impl IntoIterator<Item = u32>) {
        if let Consent::Spawned(granted) = &mut self.consent {
            granted.extend(pids);
            self.publish_facts();
        }
    }

    /// The person at the seat just used it.
    #[cfg_attr(
        not(feature = "seat"),
        expect(dead_code, reason = "the seat's input path")
    )]
    pub(crate) fn person_used_seat(&mut self) {
        self.person_at = Some(Instant::now());
        self.idle.notify_activity(&self.seat);
    }

    /// The surface holding the keyboard, whatever kind it is.
    pub(crate) fn keyboard_focus(&self) -> Option<WlSurface> {
        self.keyboard
            .as_ref()?
            .current_focus()?
            .wl_surface()
            .map(std::borrow::Cow::into_owned)
    }

    /// Give the keyboard to a surface that is not a window: a launcher, a
    /// lock screen.
    pub(crate) fn focus_plain(&mut self, surface: WlSurface) {
        if let Some(keyboard) = self.keyboard.clone() {
            keyboard.set_focus(self, Some(surface.into()), SERIAL_COUNTER.next_serial());
        }
    }

    /// A window closed. If it held the keyboard, the keyboard goes to the
    /// window now on top, as on any desktop; a launcher or lock screen that
    /// holds it keeps it. Only with a person at the seat: headless keeps the
    /// M2 contract that focus moves only when something asks it to.
    ///
    /// `closing` is the closed window's surface, compared rather than asked
    /// whether it is alive: a client destroys its window role before the
    /// surface, so at this point the surface still is, and an aliveness check
    /// concluded the keyboard was still held. Found on the first hardware run.
    pub(crate) fn refocus_after_close(&mut self, closing: Option<&WlSurface>) {
        if !self.backend.has_person() {
            return;
        }
        let held = self
            .keyboard_focus()
            .is_some_and(|focus| focus.is_alive() && Some(&focus) != closing);
        if !held {
            self.focus_top_window();
        }
    }

    /// Give the keyboard back to the topmost window, or to nothing.
    pub(crate) fn focus_top_window(&mut self) {
        let top = self
            .space
            .elements()
            .last()
            .and_then(|window| Some((shell::surface_of(window)?, shell::id_of(window)?)));
        match top {
            Some((surface, id)) => self.focus_surface(surface, id),
            None => {
                if let Some(keyboard) = self.keyboard.clone() {
                    keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
                }
            }
        }
    }

    /// Whether an agent acting now would race the person at the seat.
    pub(crate) fn person_is_active(&self) -> bool {
        self.person_at.is_some_and(|at| at.elapsed() < PERSON_QUIET)
    }

    /// Give a surface keyboard focus, and remember when.
    ///
    /// An X11 window is focused as an X11 window, so the X server's input
    /// focus moves with it. See [`crate::focus`].
    pub(crate) fn focus_surface(&mut self, surface: WlSurface, id: SurfaceId) {
        let Some(keyboard) = self.keyboard.clone() else {
            return;
        };
        let target = self.focus_target(surface, id);
        keyboard.set_focus(self, Some(target), SERIAL_COUNTER.next_serial());
        self.focused_at.insert(id, Instant::now());
    }

    /// The focus target for a window's surface: the X11 window itself if
    /// that is what it is, the surface otherwise.
    #[cfg_attr(not(feature = "xwayland"), expect(unused_variables, reason = "X11's"))]
    fn focus_target(&self, surface: WlSurface, id: SurfaceId) -> FocusTarget {
        #[cfg(feature = "xwayland")]
        if let Some(x11) = self
            .window_for_id(id)
            .and_then(|window| window.x11_surface().cloned())
        {
            return FocusTarget::X11(x11);
        }
        FocusTarget::Wayland(surface)
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

    /// Record that a surface has something in it.
    #[cfg_attr(
        not(feature = "xwayland"),
        expect(dead_code, reason = "Xwayland's late association")
    )]
    pub(crate) fn mark_presented(&mut self, id: SurfaceId) {
        self.presented.insert(id);
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
        let surface = self.keyboard_focus()?;
        let window = self.window_for(&surface)?;
        window.user_data().get::<SurfaceId>().copied()
    }

    /// When this compositor started. The base of every event timestamp it
    /// sends, so synthetic input is stamped on the same clock as frame
    /// callbacks rather than on a second one that could disagree.
    pub(crate) fn started_at(&self) -> Instant {
        self.started
    }

    /// The window whose toplevel is this surface, mapped or parked.
    pub(crate) fn window_for(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .chain(&self.parked)
            .find(|window| shell::is_toplevel_of(window, surface))
            .cloned()
    }
}

impl CompositorHandler for Compositor {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    /// Every client carries one of two kinds of data: ours, inserted in
    /// `insert_client`, or -- for the one client this compositor did not
    /// accept from its socket -- the data Smithay gives the Xwayland it
    /// spawned.
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        #[cfg(feature = "xwayland")]
        if let Some(xwayland) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &xwayland.compositor_state;
        }
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

        self.popups.commit(surface);
        // A window or a layer surface: both are described to the index, so
        // both keep the same presentation and damage records.
        let id = match self.window_for(surface) {
            Some(window) => {
                window.on_commit();
                self.settle_resize(&window);
                shell::id_of(&window)
            }
            None => self.layer_committed(surface),
        };
        #[cfg(feature = "xwayland")]
        if id.is_none() && presented {
            with_states(surface, |states| {
                states
                    .data_map
                    .insert_if_missing_threadsafe(|| crate::xwayland::PresentedUnclaimed);
            });
        }
        if let Some(id) = id {
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
        self.space.refresh();
        self.backend.redraw();
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

        let at = self.place_new();
        self.space.map_element(window.clone(), at, true);
        self.adopt(&window);
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
            self.forget_window(&window);
        }
        self.refocus_after_close(Some(surface.wl_surface()));
        self.backend.redraw();
        self.publish_facts();
    }

    /// Popups are tracked on both backends, so they render and hit-test with
    /// their window. Only with a person at the seat are they also kept on
    /// screen: headless, a menu's placement is left to its positioner, because
    /// a menu is its own surface while its accessible nodes hang off the
    /// toplevel, and where it belongs is a question M2 measured rather than
    /// guessed at.
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        if self.backend.has_person() {
            self.constrain(&surface);
        }
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface.clone())) {
            tracing::warn!(%error, "could not track popup");
        }
        if let Err(error) = surface.send_configure() {
            tracing::warn!(%error, "could not configure popup");
        }
    }

    /// A menu wants the keyboard and pointer until it is dismissed. Only a
    /// person makes that request meaningful: headless, the act path addresses
    /// surfaces directly and a grab would only get in its way.
    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        if !self.backend.has_person() {
            return;
        }
        if let Some(seat) = Seat::<Self>::from_resource(&seat) {
            self.grab_popup(surface, &seat, serial);
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.positioner = positioner;
        });
        if self.backend.has_person() {
            self.constrain(&surface);
        }
        surface.send_repositioned(token);
    }

    /// A titlebar drag. Honoured only if the serial is the button press that
    /// is still holding the pointer, which is what stops a client from
    /// starting a move nobody asked for.
    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        let Some((window, start)) = self.interactive(&surface, &seat, serial) else {
            return;
        };
        self.start_move(&window, start, serial);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some((window, start)) = self.interactive(&surface, &seat, serial) else {
            return;
        };
        self.start_resize(&window, shell::edges(edges), start, serial);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if self.backend.has_person() {
            self.fill(&surface, xdg_toplevel::State::Maximized, None);
        } else {
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if self.backend.has_person() {
            self.unfill(&surface, xdg_toplevel::State::Maximized, None);
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        if self.backend.has_person() {
            self.fill(&surface, xdg_toplevel::State::Fullscreen, output.as_ref());
        } else {
            surface.send_configure();
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if self.backend.has_person() {
            self.unfill(&surface, xdg_toplevel::State::Fullscreen, None);
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if !self.backend.has_person() {
            return;
        }
        if let Some(window) = self.window_for(surface.wl_surface()) {
            self.minimize(&window);
        }
    }
}

impl Compositor {
    /// The window and grab start for an interactive move or resize, if the
    /// request is one to honour: a person at the seat, a window we know, and a
    /// serial that is the press currently holding the pointer.
    fn interactive(
        &self,
        surface: &ToplevelSurface,
        seat: &WlSeat,
        serial: Serial,
    ) -> Option<(Window, GrabStartData<Self>)> {
        if !self.backend.has_person() {
            return None;
        }
        let pointer = Seat::<Self>::from_resource(seat)?.get_pointer()?;
        if !pointer.has_grab(serial) {
            return None;
        }
        let start = pointer.grab_start_data()?;
        // The press must have landed on this client's window, or a client
        // could drag a window it does not own.
        let (focus, _) = start.focus.as_ref()?;
        if !focus.same_client_as(&surface.wl_surface().id()) {
            return None;
        }
        Some((self.window_for(surface.wl_surface())?, start))
    }
}

impl SeatHandler for Compositor {
    type KeyboardFocus = FocusTarget;
    type PointerFocus = FocusTarget;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    /// On a seat, the window holding the keyboard is the one that looks
    /// active, and no other. Headless leaves every toplevel activated from the
    /// start (see `new_toplevel`), because the toolkits it hosts for reading
    /// render differently when they believe they are in the background.
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&FocusTarget>) {
        let focused = focused
            .and_then(WaylandFocus::wl_surface)
            .map(std::borrow::Cow::into_owned);
        let focused = focused.as_ref();
        tracing::debug!(surface = ?focused.map(|surface| surface.id()), "keyboard focus changed");
        // The clipboard and the primary selection belong to whoever has the
        // keyboard: a client may only read a selection while focused. On both
        // backends, because an agent pasting is as real as a person pasting.
        let client = focused.and_then(|surface| self.display.get_client(surface.id()).ok());
        set_data_device_focus(&self.display, seat, client.clone());
        set_primary_focus(&self.display, seat, client);

        if !self.backend.has_person() {
            return;
        }
        for window in self.space.elements() {
            let active = shell::surface_of(window).as_ref() == focused;
            // An X11 window is told at once; an xdg toplevel needs the
            // configure that carries its new state.
            if window.set_activated(active) {
                tracing::debug!(window = ?shell::id_of(window), active, "activation changed");
                if let Some(toplevel) = window.toplevel() {
                    toplevel.send_pending_configure();
                }
            }
        }
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor = image;
        self.backend.redraw();
    }
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

    /// A Wayland client copied something: offer it to X clients too.
    #[cfg(feature = "xwayland")]
    fn new_selection(
        &mut self,
        target: smithay::wayland::selection::SelectionTarget,
        source: Option<smithay::wayland::selection::SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(error) = wm.new_selection(target, source.map(|source| source.mime_types()))
        {
            tracing::warn!(%error, ?target, "could not offer a selection to X11");
        }
    }

    /// An X client is pasting what a Wayland client copied.
    #[cfg(feature = "xwayland")]
    fn send_selection(
        &mut self,
        target: smithay::wayland::selection::SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        let handle = self.loop_handle.clone();
        if let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(error) = wm.send_selection(target, mime_type, fd, handle)
        {
            tracing::warn!(%error, ?target, "could not hand a selection to X11");
        }
    }
}

impl DataDeviceHandler for Compositor {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}

impl ClientDndGrabHandler for Compositor {}
impl ServerDndGrabHandler for Compositor {}

impl PrimarySelectionHandler for Compositor {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection
    }
}

impl XdgActivationHandler for Compositor {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation
    }

    /// A client asking for one of its windows to be brought forward: an
    /// application opened from a launcher, a link opened in a browser that
    /// was already running. Granted only for a fresh token minted from the
    /// person's own input (see `perspicax_policy::grants_activation`), so a
    /// window cannot pull focus to itself while someone is typing elsewhere.
    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        self.activation.remove_token(&token);
        if !perspicax_policy::grants_activation(data.timestamp.elapsed(), data.serial.is_some()) {
            tracing::debug!("activation refused: no recent input behind the token");
            return;
        }
        let Some(window) = self.window_for(&surface) else {
            return;
        };
        let Some(id) = shell::id_of(&window) else {
            return;
        };
        self.restore(&window);
        self.space.raise_element(&window, false);
        self.focus_surface(surface, id);
        self.backend.redraw();
        self.publish_facts();
    }
}

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
delegate_primary_selection!(Compositor);
delegate_xdg_activation!(Compositor);

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
