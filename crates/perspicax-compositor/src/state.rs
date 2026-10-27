//! The compositor's state, and the protocol handlers that mutate it.
//!
//! Smithay's shape is one state struct plus a handler trait per protocol, wired
//! together by `delegate_*!`. Nothing here is unusual for a Wayland compositor;
//! the two decisions worth reading are in [`Compositor::commit`] and
//! [`Compositor::send_frames`], and both follow from this compositor not
//! drawing anything.

use std::{
    collections::{HashMap, HashSet},
    os::{fd::OwnedFd, unix::net::UnixStream},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use perspicax_node::{Origin, Rect, SurfaceId, Vec2};
use perspicax_policy::Workspaces;

use smithay::wayland::seat::WaylandFocus;

use crate::{
    act::Keys,
    backend::Running,
    damage,
    facts::Facts,
    focus::FocusTarget,
    framed::Framed,
    geometry, origin,
    owed::{Event, Owed, Paid, Plan},
    popup,
    shell::{self, Fill},
    stall::{Change, Stall},
};

use smithay::{
    delegate_cursor_shape, delegate_data_device, delegate_output, delegate_primary_selection,
    delegate_relative_pointer, delegate_seat, delegate_xdg_activation,
    desktop::{PopupGrab, PopupKind, PopupManager, Space, Window, find_popup_root_surface},
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{KeyboardHandle, LedState, XkbConfig},
        pointer::{CursorImageStatus, GrabStartData, PointerHandle},
    },
    reexports::{
        calloop::LoopHandle,
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, DisplayHandle, Resource as _,
            backend::{ClientData, ClientId, DisconnectReason, ObjectId},
            protocol::{
                wl_buffer::WlBuffer, wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface,
            },
        },
    },
    utils::{IsAlive as _, SERIAL_COUNTER, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, is_sync_subsurface, with_surface_tree_downward,
        },
        cursor_shape::CursorShapeManagerState,
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        output::{OutputHandler, OutputManagerState},
        pointer_constraints::PointerConstraintsState,
        relative_pointer::RelativePointerManagerState,
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
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
        tablet_manager::TabletSeatHandler,
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
/// not waiting long. A button held down is not a pause, however still the
/// hand holding it: see [`Compositor::person_is_active`].
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
    /// `wp_cursor_shape_v1`: a client names the pointer it wants and this
    /// compositor draws it from the theme's cursor, so every application's
    /// pointer is the same one. Held for its global, as the one above is.
    #[expect(dead_code, reason = "RAII handle for the cursor-shape global")]
    pub(crate) cursor_shape: CursorShapeManagerState,
    /// `zwp_relative_pointer_manager_v1`: the mouse's own motion, sent beside
    /// the pointer's (see [`Compositor::travel`]), so a game turns its camera
    /// by how far the mouse went, not by how far the pointer could. Held for
    /// its global, as the ones above are.
    #[expect(dead_code, reason = "RAII handle for the relative-pointer global")]
    relative_pointer: RelativePointerManagerState,
    /// `zwp_pointer_constraints_v1`: a game holding the pointer in place, or
    /// inside its window. Which window may, and when, is [`crate::hold`]'s.
    #[expect(dead_code, reason = "RAII handle for the pointer-constraints global")]
    pointer_constraints: PointerConstraintsState,
    /// `wp_viewporter`: what size a surface's buffer is shown at, which a
    /// client drawing at a fractional scale says ([`crate::scale`]). Held
    /// for its global, as the ones above are.
    #[expect(dead_code, reason = "the viewporter global's id")]
    viewporter: smithay::reexports::wayland_server::backend::GlobalId,
    /// `wp_fractional_scale_manager_v1`: which scale each surface should
    /// draw at ([`crate::scale`]). Held for its global, as the ones above
    /// are.
    #[expect(dead_code, reason = "RAII handle for the fractional-scale global")]
    fractional_scale: smithay::wayland::fractional_scale::FractionalScaleManagerState,
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
    pub(crate) space: Space<Framed>,
    /// Who draws each window's frame: the two protocols that negotiate it.
    /// See [`crate::decorations`].
    pub(crate) decorations: crate::decorations::DecorationStates,
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
    /// snapshot: everyone headless, and on a seat only what was granted,
    /// each for as long as its process lives. See [`crate::grants`].
    pub(crate) grants: crate::grants::Grants,
    /// When the person at the seat last pressed a key or moved the pointer.
    /// `None` headless, where nobody is at the seat, and so never a reason to
    /// refuse an agent there.
    person_at: Option<Instant>,
    /// The pointer buttons the person at the seat has down, by Linux code: a
    /// drag, a resize or a selection under way. Written by the seat's
    /// libinput and by a test's stand-ins for a hand, never by an agent,
    /// whose presses go straight to the pointer and come up in the same
    /// turn.
    pub(crate) person_buttons: Vec<u32>,
    /// Where the person's attention has been: what an activation token is
    /// good for only until it moves on (#257). Turned by the person's
    /// presses, from the seat's libinput and a test's stand-ins, never an
    /// agent's, and by the keys going to another client, whoever moved them.
    pub(crate) attention: perspicax_policy::Attention<Serial, ClientId>,
    /// What the focused client asked the pointer to look like. Only the seat
    /// draws it; headless records it and nothing reads it.
    pub(crate) cursor: CursorImageStatus,
    /// Popups (menus, tooltips), tracked so they render and hit-test with
    /// their window and so a grab can dismiss them.
    pub(crate) popups: PopupManager,
    /// Each popup's id, by its surface: minted in `new_popup` for each
    /// `xdg_popup` and gone with it. A client may give its surface a new
    /// popup -- GTK 4 does, each time a popover opens again -- and the new
    /// menu gets a new id, so nothing addressed to the old one, its nodes or
    /// an act, ever reaches the new one.
    pub(crate) popup_ids: HashMap<ObjectId, SurfaceId>,
    /// When each popup was last put on screen: what the index joins a menu
    /// to it only from a read made after.
    pub(crate) popups_shown: HashMap<SurfaceId, Instant>,
    /// The last grab a window's menu took of the keyboard and pointer: what
    /// [`Self::close_menu`] closes. Kept once the menu has gone, when
    /// smithay has forgotten it and closing it again does nothing.
    pub(crate) menu: Option<PopupGrab<Self>>,
    /// Windows that are open and not on screen: minimized, or on a
    /// workspace that is not showing. Unmapped from the space, kept here in
    /// the order they went, and described to the index as unmapped rather
    /// than forgotten. Which of the two is in each window's
    /// [`shell::Placement`]; see `shell::workspaces` for the one rule that
    /// decides what is in the space and what is here.
    pub(crate) parked: Vec<Framed>,
    /// Windows their clients unmapped, by attaching no buffer: closed to
    /// everyone but their clients, which still hold them and may ask for
    /// them again. Kept in the order they went. See `shell::withdrawn`.
    pub(crate) withdrawn: Vec<shell::Withdrawn>,
    /// Layer surfaces their clients unmapped the same way: gone to everyone
    /// but their clients, which may ask for them again. See
    /// [`crate::layers`].
    pub(crate) withdrawn_layers: Vec<crate::layers::Withdrawn>,
    /// Which workspace every window is on and which each monitor shows.
    pub(crate) workspaces: Workspaces<SurfaceId, String>,
    /// Which windows are tabs of one another, and which tab of each group is
    /// in front. See `shell::tabs`.
    pub(crate) tabs: perspicax_policy::Groups<SurfaceId>,
    /// Whose the keyboard layout is, and each window's own when it is
    /// theirs. See `crate::keyboard`.
    pub(crate) layout_memory: perspicax_policy::LayoutMemory<SurfaceId>,
    /// The names the keyboard's keymap was compiled from.
    pub(crate) keymap: crate::Keymap,
    /// The window being moved or resized with the pointer, while it is:
    /// every switch takes it along to the workspace its monitor shows, and an
    /// edge flip carries one being moved.
    pub(crate) hand: Option<shell::Hand>,
    /// How many times the pointer has jumped rather than moved: carried round
    /// the desk by an edge flip, or left where it was while the monitors
    /// moved under it. A move that sees this change knows its next motion is
    /// a jump, which no edge holds (see `shell::resist`).
    pub(crate) desk_jumps: u64,
    /// The `[mouse]` bindings in force: the config's on a seat, as of the
    /// last reload; headless, what [`crate::Command::MouseBindings`] said.
    pub(crate) mouse: perspicax_policy::MouseBindings,
    /// The person's presses, for double-clicks and for the releases of
    /// presses a `[mouse]` binding took. See `crate::mouse`.
    pub(crate) clicks: perspicax_policy::Clicks,
    /// The wheel's notches toward a `[mouse]` binding, horizontal then
    /// vertical, gathered from one scroll event to the next.
    pub(crate) wheel: [perspicax_policy::Notches; 2],
    /// Where the window being dragged would snap if let go now. See
    /// `shell::snap`.
    pub(crate) snap_preview: Option<shell::SnapPreview>,
    /// The titlebar a tab being dragged would join if let go now: drawn like
    /// a snap preview. See `shell::tabs`.
    #[cfg(feature = "seat")]
    pub(crate) tab_drop: Option<smithay::utils::Rectangle<i32, smithay::utils::Logical>>,
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
    /// What each output shows where nothing else is: the lock screen's
    /// colour and logo, and the splash's. See [`crate::backdrop`].
    #[cfg(any(feature = "seat", feature = "capture"))]
    pub(crate) backdrop: crate::backdrop::Backdrop,
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
    /// Who may use the protocols that reach past their own windows. See
    /// [`crate::access`].
    pub(crate) gate: crate::access::Gate,
    /// Every window, as the taskbar protocols have been told it. See
    /// [`crate::toplevels`].
    pub(crate) toplevels: crate::toplevels::Toplevels,
    /// The workspaces, as the pager protocol has been told them. See
    /// [`crate::pager`].
    pub(crate) pager: crate::pager::Pager,
    /// The monitors, as the display tools have been told them. See
    /// [`crate::output_management`].
    pub(crate) displays: crate::output_management::Displays,
    /// The desktop shells listening for the menus a person asks for. See
    /// [`crate::shell_protocol`].
    pub(crate) shells: crate::shell_protocol::Shells,
    /// The settings app's capture of the next chord, if one waits. See
    /// [`crate::chord_capture`].
    pub(crate) captures: crate::chord_capture::Captures,
    /// The shell asked to end the session: the person chose to log out.
    pub(crate) exit_asked: bool,
    /// Screenshot tools' frames waiting for something to change. See
    /// [`crate::screencopy`].
    #[cfg(feature = "capture")]
    pub(crate) screencopy: crate::screencopy::Screencopy,
    /// The event loop, for the handlers that must schedule work on it.
    pub(crate) loop_handle: LoopHandle<'static, Self>,
    /// How to start a program against this compositor. Set by `run` once the
    /// socket exists, so `None` only before any client could connect.
    pub(crate) launch: Option<crate::Launch>,
    /// The agent's programs, started by `--spawn`: reaped as they exit and
    /// killed when `run` ends. The person's are the seat session's.
    pub(crate) spawned: Vec<std::process::Child>,
    /// A `--spawn` command that would not start, waiting for `run` to return
    /// it. It may have been tried only once Xwayland was ready, inside the
    /// loop, where nothing can return an error.
    pub(crate) spawn_failed: Option<crate::Error>,
    /// How many windows have been placed, for the cascade.
    pub(crate) placed: u32,
    /// Where each program's last window closed, by app id, so that its next
    /// one opens there (see `shell::remember`). The person's history: never
    /// in the facts.
    pub(crate) places: perspicax_policy::Places,
    /// Where the places are kept between sessions, if they are (see
    /// `remembered`).
    pub(crate) places_kept: Option<std::path::PathBuf>,
    /// The places changed since they were last written there.
    pub(crate) places_owed: bool,
    started: Instant,
    /// The clients this compositor accepted, each with its state, until
    /// they go: flushed one by one, to see which have stopped reading
    /// (see [`Compositor::flush_each`]). Xwayland is smithay's, and not here.
    clients: Vec<(ClientId, Arc<ClientState>)>,
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

        let grants = crate::grants::Grants::new(backend.by_grant(), backend.never().clone());
        let workspace_shape = backend.workspace_shape();
        let gate = crate::access::Gate::new(backend.access());
        let mouse = backend.mouse();
        let toplevels = crate::toplevels::Toplevels::new(display, &gate);
        let pager = crate::pager::Pager::new(display, &gate);
        let displays = crate::output_management::Displays::new(display, &gate);
        let shells = crate::shell_protocol::Shells::new(display, &gate);
        let captures = crate::chord_capture::Captures::new(display, &gate);
        #[cfg(feature = "capture")]
        let screencopy = crate::screencopy::Screencopy::new(display, &gate);
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
            cursor_shape: CursorShapeManagerState::new::<Self>(display),
            relative_pointer: RelativePointerManagerState::new::<Self>(display),
            pointer_constraints: PointerConstraintsState::new::<Self>(display),
            viewporter: crate::scale::offer_viewporter(display),
            fractional_scale: smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<
                Self,
            >(display),
            data_device: DataDeviceState::new::<Self>(display),
            seat_state,
            seat,
            space,
            decorations: crate::decorations::DecorationStates::new(display, &backend.decorations()),
            backend,
            #[cfg(feature = "seat")]
            dmabuf: smithay::wayland::dmabuf::DmabufState::new(),
            keyboard,
            pointer,
            keys: Keys::new(&crate::Keymap::default()),
            damage: HashMap::new(),
            presented: HashSet::new(),
            focused_at: HashMap::new(),
            facts,
            grants,
            person_at: None,
            person_buttons: Vec::new(),
            attention: perspicax_policy::Attention::default(),
            cursor: CursorImageStatus::default_named(),
            generation: 0,
            next_surface: 0,
            popups: PopupManager::default(),
            popup_ids: HashMap::new(),
            popups_shown: HashMap::new(),
            menu: None,
            parked: Vec::new(),
            withdrawn: Vec::new(),
            withdrawn_layers: Vec::new(),
            workspaces: Workspaces::new(workspace_shape),
            tabs: perspicax_policy::Groups::default(),
            layout_memory: perspicax_policy::LayoutMemory::default(),
            keymap: crate::Keymap::default(),
            hand: None,
            desk_jumps: 0,
            mouse,
            clicks: perspicax_policy::Clicks::default(),
            wheel: Default::default(),
            snap_preview: None,
            #[cfg(feature = "seat")]
            tab_drop: None,
            primary_selection: PrimarySelectionState::new::<Self>(display),
            activation: XdgActivationState::new::<Self>(display),
            layer_shell: WlrLayerShellState::new::<Self>(display),
            // Any client may lock. A lock client is the one kind that must
            // work when everything else has gone wrong. Only the lock that
            // holds the session may unlock it: see `crate::lock`.
            lock_manager: SessionLockManagerState::new::<Self, _>(display, |_| true),
            lock: None,
            #[cfg(any(feature = "seat", feature = "capture"))]
            backdrop: crate::backdrop::Backdrop::default(),
            idle: IdleNotifierState::new(display, event_loop.clone()),
            #[cfg(feature = "xwayland")]
            xwayland: crate::xwayland::Xwayland::default(),
            #[cfg(feature = "xwayland")]
            xwayland_shell: None,
            gate,
            toplevels,
            pager,
            displays,
            shells,
            captures,
            exit_asked: false,
            #[cfg(feature = "capture")]
            screencopy,
            loop_handle: event_loop,
            idle_inhibit: IdleInhibitManagerState::new::<Self>(display),
            inhibitors: Vec::new(),
            launch: None,
            spawned: Vec::new(),
            spawn_failed: None,
            placed: 0,
            places: perspicax_policy::Places::default(),
            places_kept: None,
            places_owed: false,
            started: Instant::now(),
            clients: Vec::new(),
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
        let state = Arc::new(ClientState::default());
        let client = match self.display.insert_client(stream, state.clone()) {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "could not insert client");
                return;
            }
        };

        let (origin, pidfd) = client
            .get_credentials(&self.display)
            .map_or((Origin::Unattributed, None), |credentials| {
                origin::attest(credentials.pid)
            });
        state.attribute(origin.clone(), pidfd);
        tracing::info!(id = ?client.id(), ?origin, "client connected");
        if let Origin::Process(process) = &origin {
            self.grant_lineage(process.pid, &state);
            self.stamp_started(process.pid, &state);
        }
        self.clients.push((client.id(), state));
    }

    /// Record a client's process, with when it started, and the person's
    /// press that started it, if one did (#259): a binding's, or a click or
    /// an Enter at what it descends from. Asked once, as it connects, while
    /// whatever started it most likely runs yet; its windows ask the stamp.
    fn stamp_started(&self, pid: u32, client: &ClientState) {
        let lineage = origin::lineage(pid);
        let Some(&process) = lineage.first() else {
            return;
        };
        let _ = client.process.set(process);
        if let Some(turn) = self.attention.started(&lineage, Instant::now()) {
            tracing::debug!(pid, "started by the person's latest press");
            let _ = client.started_on.set(turn);
        }
    }

    /// Flush what waits for each client this compositor accepted, and say
    /// when one has stopped reading it, and when it reads again: the line
    /// before the one saying it was cut off, if it is (see [`crate::stall`]).
    /// A client that has gone is forgotten. It cannot be told by its flush,
    /// which for a client cleaned up is `Ok`, and for one cut off but not yet
    /// cleaned up still tries its socket.
    ///
    /// A client whose flush went through reads again, and is paid what it
    /// was owed of the pointer meanwhile (see [`crate::owed`]): here, from
    /// the loop, where nothing holds the pointer's lock. Whether or not its
    /// stall was told, which only one of a second or more is.
    pub(crate) fn flush_each(&mut self) {
        let mut backend = self.display.backend_handle();
        let now = Instant::now();
        let mut owed = Vec::new();
        self.clients.retain(|(id, client)| {
            if client.gone.load(Ordering::Relaxed) {
                return false;
            }
            let flushed = backend.flush(Some(id.clone()));
            match client.stall.flushed(&flushed, now) {
                Some(Change::Stopped(waited)) => tracing::warn!(
                    ?id,
                    origin = ?client.origin(),
                    ?waited,
                    "a client has stopped reading; what is sent to it is held, until too much is"
                ),
                Some(Change::Reading(waited)) => {
                    tracing::info!(?id, origin = ?client.origin(), ?waited, "a client reads again");
                }
                None => {}
            }
            if flushed.is_ok()
                && let Some(debt) = client.owed.take()
            {
                owed.push(debt);
            }
            true
        });
        for (target, paid) in owed {
            self.pay_owed(&target, &paid);
        }
    }

    /// Pay a client what it was owed for `target`, if the pointer is still
    /// on it. A surface gone since stays the pointer's until it next moves,
    /// and is owed nothing; one the pointer left was told so, which forgot
    /// what it was owed.
    fn pay_owed(&mut self, target: &FocusTarget, paid: &Paid) {
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        if !target.alive() || pointer.current_focus().as_ref() != Some(target) {
            return;
        }
        let seat = self.seat.clone();
        target.deliver(&seat, self, paid);
    }

    /// The next surface id. Monotonic, and never handed out twice.
    pub(crate) fn mint_surface_id(&mut self) -> SurfaceId {
        self.next_surface += 1;
        SurfaceId(self.next_surface)
    }

    /// Count every window on the monitors it is on now, before any is told
    /// it may draw again.
    ///
    /// The screen is drawn from where each window is (`shell::stack`), but a
    /// seat tells only the windows smithay counts on the monitor it drew, and
    /// smithay counts only when asked
    /// ([`Space::refresh`](smithay::desktop::Space::refresh)). Asked only
    /// after a commit, a window brought back onto the screen -- by a switch,
    /// a restore, a tab, a monitor that went -- was drawn at once but not
    /// counted, so not told it may draw, nor which monitor it is on, until
    /// some client drew. Once none did, GTK and Chromium, which wait for that
    /// word with no limit, never drew again (#99). Counting before telling
    /// keeps the two in step, whatever put the window there.
    ///
    /// Which scale each surface should draw at is settled with it
    /// ([`Compositor::settle_scales`]), for the same reason: whatever moved
    /// a window, or changed its monitor's scale, it is told before it next
    /// draws.
    pub(crate) fn settle_outputs(&mut self) {
        self.space.refresh();
        self.settle_scales();
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
    ///
    /// Every window in the space is told, counted on a monitor or not, but
    /// one rolled up to its titlebar, whose client is not drawn. Each is
    /// counted first, as the seat counts before it tells: so a window is
    /// told which monitor it is on as soon as it is told it may draw.
    pub(crate) fn send_frames(&mut self) {
        self.settle_outputs();
        let elapsed = u32::try_from(self.started.elapsed().as_millis() % u128::from(u32::MAX))
            .unwrap_or(u32::MAX);
        let windows = self
            .space
            .elements()
            .filter(|window| !shell::is_shaded(window))
            .filter_map(shell::surface_of);
        let layers = self
            .layers_in(&crate::layers::ALL)
            .into_iter()
            .map(|(layer, _)| layer.wl_surface().clone());
        let covers = self
            .lock
            .iter()
            .flat_map(|locked| &locked.surfaces)
            .map(|cover| cover.surface.wl_surface().clone());
        // And a window or panel its client unmapped: it may be waiting for
        // the word before it asks for it again, as GTK waits for every frame.
        let withdrawn = self
            .withdrawn
            .iter()
            .map(|withdrawn| withdrawn.toplevel.wl_surface().clone())
            .chain(
                self.withdrawn_layers
                    .iter()
                    .map(|withdrawn| withdrawn.surface.wl_surface().clone()),
            );
        for surface in windows
            .chain(layers)
            .chain(covers)
            .chain(withdrawn)
            .collect::<Vec<_>>()
        {
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

    /// Give the keyboard to a surface as it is: a launcher, a lock screen,
    /// or the menu or window a closed menu hung from. A window focused for
    /// itself goes through [`Self::focus_surface`]. While the session is
    /// locked, only to the lock's own surfaces.
    pub(crate) fn focus_plain(&mut self, surface: WlSurface) {
        if !self.may_take_keyboard(&surface) {
            return;
        }
        if let Some(keyboard) = self.keyboard.clone() {
            keyboard.set_focus(self, Some(surface.into()), SERIAL_COUNTER.next_serial());
        }
    }

    /// Whether `surface` may be given the keyboard: anything may, unless the
    /// session is locked, when only the holder's lock surfaces may (#127).
    /// What has the keyboard then has the password as it is typed, and a
    /// window behind the lock -- a chat window, a terminal, a dialog that
    /// opened -- would send it with the Enter that was meant to unlock.
    ///
    /// Nor, while a launcher holds the keyboard, may anything but a launcher
    /// (#131): layer-shell has the top-most layer that wants every key keep
    /// them, and a window that opened under it, was brought forward, clicked
    /// or pointed at, would have what was being typed at the launcher. The
    /// window is raised all the same, and has the keys once the launcher
    /// goes, as one that opened behind the lock has them at unlock.
    ///
    /// [`Self::focus_plain`] and [`Self::focus_surface`] ask, which is every
    /// way to the keyboard but a menu's grab, refused while locked (#82) and
    /// to a client without the keys (#187).
    fn may_take_keyboard(&self, surface: &WlSurface) -> bool {
        match &self.lock {
            Some(locked) => locked.covers(surface),
            None => {
                self.is_launcher(surface)
                    || !self
                        .keyboard_root()
                        .is_some_and(|held| self.is_launcher(&held))
            }
        }
    }

    /// Whether the keyboard is with one of `client`'s surfaces: a window, a
    /// menu of one, a launcher, a lock surface.
    ///
    /// The serial a client names is believed only from it (#187), or, for
    /// an activation token, from the client the person's latest press was
    /// sent to, naming that press (#257). An activation token carries one,
    /// and so does a menu's grab, to say what input it answers, but nothing
    /// stops a client naming one it was never sent, or one it was sent long
    /// ago. The client with the keys is the one in use: what it asks for, the person is behind, a link opened in
    /// it, an item picked from its launcher or its menu. Any other client is
    /// asking for itself, which is the focus stealing both protocols leave
    /// the compositor to refuse. How old the serial is proves nothing more,
    /// since the client with the keys can always name the one it was sent as
    /// it took them, and asking would refuse real input: a click that
    /// focuses a window is pressed before the keys come to it, and a
    /// window's own menu takes them and gives them back with new serials.
    ///
    /// An X11 window's request to be active names an X time as its input,
    /// which any X client can learn, and Xwayland is one client for them
    /// all: the time is believed while it has the keys (#256).
    pub(crate) fn keys_are_with(&self, client: &ClientId) -> bool {
        self.keyboard_focus()
            .and_then(|surface| self.display.get_client(surface.id()).ok())
            .is_some_and(|holder| holder.id() == *client)
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

    /// Take the keyboard from whatever has it. Smithay 0.7 tells
    /// `focus_changed` of a new focus but not of none, so what has to follow
    /// is done here: no window looks active any more, nor is named so to X
    /// clients. Every focus cleared to none goes through this.
    pub(crate) fn clear_focus(&mut self) {
        if let Some(keyboard) = self.keyboard.clone() {
            keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
        }
        // Nor does anything have the person's attention (#257).
        self.attention.keys_to(None, false);
        // No window is lit or undimmed now. See `focus_changed`.
        self.backend.redraw();
        self.show_active(None);
        // Nor in use, so none may hold the pointer.
        self.settle_hold_later();
        #[cfg(feature = "xwayland")]
        self.tell_active_x11(None);
    }

    /// On a seat, make the window in use, `in_use`, the one that looks
    /// active, and no other -- parked windows too: one minimized or left on
    /// another workspace while it had the keyboard would otherwise go on
    /// saying it has it. A window whose own menu has the keyboard is still
    /// in use, as on sway and Plasma.
    pub(crate) fn show_active(&self, in_use: Option<SurfaceId>) {
        if !self.backend.has_person() {
            return;
        }
        for window in self.space.elements().chain(&self.parked) {
            let active = in_use.is_some_and(|id| shell::id_of(window) == Some(id));
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

    /// Give the keyboard back to the topmost window, or to nothing. Never to
    /// an X11 window that places itself, which an application's menu is,
    /// and which is on top of everything when it opens (#181).
    ///
    /// Topmost in the stack, which is the order the windows were raised in:
    /// the window raised last, whether or not one kept on top is drawn in
    /// front of it (`shell::stacking`). The keyboard goes where it went
    /// before any window was kept on top. Passing over a window that opened
    /// without the keys (#259), which the person never gave them to, and
    /// raising the window they go to over it, so they go where the person
    /// can see.
    ///
    /// While the session is locked, or a launcher holds the keyboard,
    /// [`Self::focus_surface`] refuses the window the keyboard. Giving it to
    /// nothing is not refused, and need not be: no caller asks while a lock
    /// surface or a launcher holds the keyboard, only once what held it has
    /// gone, or while a window or a menu holds it.
    pub(crate) fn focus_top_window(&mut self) {
        let Some(window) = self.window_for_keys() else {
            self.clear_focus();
            return;
        };
        let (Some(surface), Some(id)) = (shell::surface_of(&window), shell::id_of(&window)) else {
            self.clear_focus();
            return;
        };
        let topmost = self
            .space
            .elements()
            .rev()
            .find(|other| !shell::places_itself(other));
        if topmost != Some(&window) {
            self.space.raise_element(&window, false);
        }
        self.focus_surface(surface, id);
    }

    /// The window the keys go back to: the topmost but for a window that
    /// places itself, or one that opened without them (#259), unless every
    /// window did.
    pub(crate) fn window_for_keys(&self) -> Option<Framed> {
        let windows = || {
            self.space
                .elements()
                .rev()
                .filter(|window| !shell::places_itself(window))
        };
        windows()
            .find(|window| !shell::wants_attention(window))
            .or_else(|| windows().next())
            .cloned()
    }

    /// Whether an agent acting now would race the person at the seat: they
    /// used it moments ago, or they have a button down. A drag held still is
    /// still a drag, and an agent's click in the middle of it would carry the
    /// window off to where the agent aimed and end the drag under their hand
    /// (#73).
    pub(crate) fn person_is_active(&self) -> bool {
        !self.person_buttons.is_empty()
            || self.person_at.is_some_and(|at| at.elapsed() < PERSON_QUIET)
    }

    /// A button of the person's went down or came up: `down` is whether it
    /// is down now.
    pub(crate) fn person_button(&mut self, code: u32, down: bool) {
        if !down {
            self.person_buttons.retain(|&button| button != code);
        } else if !self.person_buttons.contains(&code) {
            self.person_buttons.push(code);
        }
    }

    /// Give a surface keyboard focus, and remember when.
    ///
    /// An X11 window is focused as an X11 window, so the X server's input
    /// focus moves with it. See [`crate::focus`].
    ///
    /// Never while the session is locked: a window is no lock surface. It
    /// is not focused, and has the keyboard once the session unlocks if it
    /// is the window on top then, as one that opened behind the lock is.
    pub(crate) fn focus_surface(&mut self, surface: WlSurface, id: SurfaceId) {
        if !self.may_take_keyboard(&surface) {
            return;
        }
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

    /// Record one frame of damage on this surface: where its pixels changed,
    /// in its own surface's coordinates. One generation however many
    /// rectangles there are, and none for none.
    pub(crate) fn record_damage(&mut self, id: SurfaceId, rects: Vec<Rect>) {
        if rects.is_empty() {
            return;
        }
        let history = self.damage.entry(id).or_default();
        let generation = history.last().map_or(0, |(g, _)| *g) + 1;
        history.extend(rects.into_iter().map(|rect| (generation, rect)));
        if history.len() > DAMAGE_HISTORY {
            history.drain(..history.len() - DAMAGE_HISTORY);
        }
    }

    /// The window or panel a menu hangs from, if it is one the index is told
    /// about.
    pub(crate) fn hung_from(&self, hung: &popup::Hung) -> Option<SurfaceId> {
        self.window_for(&hung.from)
            .as_ref()
            .and_then(shell::id_of)
            .or_else(|| self.layer_id(&hung.from))
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

    /// A window or panel committed with its picture taken away. An xdg
    /// window or a layer surface that had drawn has been unmapped by its
    /// client, and is withdrawn; anything else just shows nothing now, which
    /// the commit records as it does any picture.
    ///
    /// An X11 window's surface is Xwayland's to empty, and the X server says
    /// when the window is withdrawn. One that never drew was never mapped,
    /// and has nothing to unmap: smithay counts it so too. Whether it was
    /// withdrawn.
    fn unmapped(&mut self, surface: &WlSurface) -> bool {
        if let Some(id) = self.layer_id(surface) {
            let presented = self.presented.remove(&id);
            if presented {
                self.withdraw_layer(surface, id);
            }
            return presented;
        }
        let Some(window) = self
            .window_for(surface)
            .filter(|window| window.toplevel().is_some())
        else {
            return false;
        };
        let Some(id) = shell::id_of(&window) else {
            return false;
        };
        let presented = self.presented.remove(&id);
        if presented {
            self.withdraw(&window, id);
        }
        presented
    }

    /// The window carrying this id, if it is still mapped.
    ///
    /// The id lives in the window's user data, put there by `new_toplevel`, so
    /// this is the inverse of the only place ids are ever handed out.
    pub(crate) fn window_for_id(&self, id: SurfaceId) -> Option<Framed> {
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

    /// Which window or layer surface the keys would go to right now.
    ///
    /// Wider than [`Self::focused_surface`], which answers only for a
    /// window's own surface and is what window management asks. A menu open
    /// on a window takes the keyboard itself, and its keys are that window's;
    /// a layer surface holding the keyboard (a launcher, the shell's menu) is
    /// its own. This is the question an agent's receipts answer; typing asks
    /// the narrower [`Self::keyboard_holder`]. `None` for a lock surface, or
    /// one that is gone.
    pub(crate) fn keyboard_owner(&self) -> Option<SurfaceId> {
        let root = self.keyboard_root()?;
        self.window_for(&root)
            .as_ref()
            .and_then(shell::id_of)
            .or_else(|| self.layer_owning(&root))
    }

    /// The surface the keys go to right now, exactly: a menu holding the
    /// keyboard is itself, where [`Self::keyboard_owner`] answers with its
    /// window. What an agent's typing asks (#112), for keys typed at a field
    /// of a window whose menu holds the keyboard land in the menu.
    pub(crate) fn keyboard_holder(&self) -> Option<SurfaceId> {
        let focus = self.keyboard_focus()?;
        self.popup_id(&focus).or_else(|| self.keyboard_owner())
    }

    /// The window the keys go to right now, through any menu it opened: the
    /// one that looks in use, its frame lit and drawn undimmed, the taskbar's
    /// active task, and over the panels if it is fullscreen.
    ///
    /// Wider than [`Self::focused_surface`], which a window answers `None`
    /// for while its own menu holds the keyboard. Narrower than
    /// [`Self::keyboard_owner`]: a layer surface holding the keyboard (a
    /// launcher, the shell's menu) leaves no window in use. And never asked
    /// of the layer maps, so it can be asked while one is held, as drawing a
    /// picture holds it.
    pub(crate) fn window_in_use(&self) -> Option<SurfaceId> {
        let root = self.keyboard_root()?;
        self.window_for(&root).as_ref().and_then(shell::id_of)
    }

    /// The surface holding the keyboard, or the one a menu holding it was
    /// opened from.
    fn keyboard_root(&self) -> Option<WlSurface> {
        Some(self.menu_root(self.keyboard_focus()?))
    }

    /// The surface a menu was opened from, through any menus it hangs from,
    /// or `surface` itself when it is no menu.
    ///
    /// Takes the state locks of menus' surfaces, never the keyboard's, so it
    /// may be asked while smithay holds the keyboard to report a change of
    /// focus. Never from inside `with_states`.
    fn menu_root(&self, surface: WlSurface) -> WlSurface {
        self.popups
            .find_popup(&surface)
            .and_then(|popup| find_popup_root_surface(&popup).ok())
            .unwrap_or(surface)
    }

    /// When this compositor started. The base of every event timestamp it
    /// sends, so synthetic input is stamped on the same clock as frame
    /// callbacks rather than on a second one that could disagree.
    pub(crate) fn started_at(&self) -> Instant {
        self.started
    }

    /// Milliseconds since it started: the clock an animation is timed by.
    pub(crate) fn clock_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// The window whose toplevel is this surface, mapped or parked.
    pub(crate) fn window_for(&self, surface: &WlSurface) -> Option<Framed> {
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

    /// A client has finished describing a new state for a surface, and for
    /// the subsurfaces under it that were waiting on it.
    ///
    /// What it drew is recorded against the window or layer at the top of
    /// its tree, wherever in the tree it drew it, and a commit that brought
    /// no new buffer and named no damage drew nothing: see [`damage`]. A
    /// menu's tree tops out at the menu, so what it draws is recorded against
    /// the window or layer it hangs from, where it is drawn: see [`popup`]. A
    /// synchronized subsurface's state is applied just before its parent's,
    /// as part of the parent's commit, and is read there, so its own call
    /// is passed over.
    ///
    /// Headless, buffers are released immediately, which a rendering
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
        if is_sync_subsurface(surface) {
            return;
        }
        let renders = self.backend.keeps_buffers();
        let (root, at) = damage::root_of(surface);
        let is_root = root == *surface;
        // A menu's tree ends at the menu, and what it draws is the window's
        // or panel's it hangs from, where it is drawn on them, as well as
        // the menu's own.
        let popup = self.popups.find_popup(&root);
        let hung = popup.as_ref().and_then(popup::hung);
        let shift = hung.as_ref().map_or(Vec2::ZERO, |hung| hung.at);
        // Read before the renderer drains what it uses, and released here
        // only when there is no renderer to want it. The whole of the window
        // -- or of the menu -- is what it was before this commit: what a
        // picture taken away uncovered.
        let whole = geometry::window_geometry(&root).map(|whole| damage::to_rect(whole) + shift);
        let taken = damage::take(surface, at + shift, whole, !renders);
        #[cfg(any(feature = "seat", feature = "capture"))]
        if renders {
            smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
            // What the renderer did not take -- damage committed without a new
            // buffer -- is cleared here, as the headless path clears it, over
            // the whole tree the commit applied: a synchronized subsurface's
            // damage is the renderer's to read first, in this call and not
            // in its own.
            damage::clear(surface);
        }
        // A window or panel whose client took its picture away has been
        // unmapped: gone to everyone but its client, which may ask for it
        // again (see `shell::withdrawn` and `layers`). Before it is measured,
        // so a tab coming forward in its place takes the size it had, and
        // before a panel's commit is taken in, so a launcher's takes no keys.
        let withdrew = is_root
            && popup.is_none()
            && matches!(taken.own, damage::Buffer::Removed)
            && self.unmapped(&root);
        // How far the tree reaches now, which is the window's geometry if
        // its client declared none. Measured whatever the backend, from
        // what `take` just recorded.
        geometry::measure(&root);

        self.popups.commit(surface);
        // A window its client unmapped, asked for again: answered as a new
        // window is, and on screen again from its first picture (see
        // `shell::withdrawn`). A panel is back from the asking commit, as a
        // new one is, which this commit then configures (see `layers`).
        // Never in the commit that unmapped it.
        if is_root && popup.is_none() && !withdrew {
            self.map_again(&root, taken.own);
            self.map_layer_again(&root);
        }
        // A window or a layer surface: both are described to the index, so
        // both keep the same presentation and damage records. A subsurface's
        // are its root's, and a menu's are its window's or panel's.
        let id = match (&popup, &hung) {
            (Some(_popup), Some(hung)) => {
                let id = self.hung_from(hung);
                #[cfg(any(feature = "seat", feature = "capture"))]
                if renders && id.is_some() {
                    let frame = self
                        .window_for(&hung.from)
                        .map_or_else(smithay::utils::Point::default, |window| {
                            window.geometry().loc
                        });
                    popup::check_hung_as_drawn(frame, _popup, hung);
                }
                id
            }
            // A menu with nothing to hang from yet draws on nothing anyone
            // is told about.
            (Some(_), None) => None,
            (None, _) => match self.window_for(&root) {
                Some(window) => {
                    // The window's bounding box takes in its subsurfaces, so
                    // it follows theirs too. A resize is settled only by the
                    // toplevel's own commit, the one that answers it.
                    window.on_commit();
                    #[cfg(any(feature = "seat", feature = "capture"))]
                    if renders {
                        geometry::check_drawn_as_measured(&window);
                    }
                    if is_root {
                        self.settle_resize(&window);
                    }
                    // A Wayland window's first picture is the first time its
                    // size is known, and the first time it can be known by its
                    // program: it goes where that program's last window closed,
                    // if it did, then is put wholly on screen, once. An X11
                    // one was as it mapped.
                    if is_root
                        && window.toplevel().is_some()
                        && matches!(taken.own, damage::Buffer::New(_))
                    {
                        let on = self.place_remembered(&window);
                        self.settle_new(&window, on);
                    }
                    shell::id_of(&window)
                }
                None if is_root => self.layer_committed(surface),
                None => self.layer_id(&root),
            },
        };
        #[cfg(feature = "xwayland")]
        if id.is_none() && is_root && popup.is_none() && matches!(taken.own, damage::Buffer::New(_))
        {
            smithay::wayland::compositor::with_states(surface, |states| {
                states
                    .data_map
                    .insert_if_missing_threadsafe(|| crate::xwayland::PresentedUnclaimed);
            });
        }
        // A menu's frames are its own as well, in its own coordinates, so a
        // receipt for one of its items reads them there. The window keeps
        // its copy, where the menu is drawn on it (#42).
        if let Some(own) = popup
            .as_ref()
            .and_then(|popup| self.popup_id(popup.wl_surface()))
        {
            self.record_damage(own, taken.rects.iter().map(|rect| *rect - shift).collect());
            if geometry::shown(&root).is_some() {
                self.popups_shown.entry(own).or_insert_with(Instant::now);
            } else {
                self.popups_shown.remove(&own);
            }
        }
        if let Some(id) = id {
            // Whether there is anything to look at is the root's own picture,
            // and a commit that brought none leaves it as it was. A menu's
            // picture is the menu's: its window is on screen without it.
            if is_root && popup.is_none() {
                match taken.own {
                    damage::Buffer::New(_) => {
                        self.presented.insert(id);
                    }
                    damage::Buffer::Removed => {
                        self.presented.remove(&id);
                    }
                    damage::Buffer::Unchanged => {}
                }
            }
            // One frame for the whole commit, however many surfaces of the
            // tree it changed.
            self.record_damage(id, taken.rects);
        }
        self.space.refresh();
        self.backend.redraw();
        self.publish_facts();
        // Something changed, so a screenshot tool waiting for a change gets
        // its frame -- once this commit has been taken in, not halfway.
        #[cfg(feature = "capture")]
        if !self.screencopy.waiting.is_empty() {
            self.loop_handle
                .insert_idle(|state: &mut Self| state.flush_screencopy());
        }
    }
}

impl XdgShellHandler for Compositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell
    }

    /// A window asked for, which opens with the keys, or one the person did
    /// not ask for, which opens without them (#259), decided now: what it is
    /// first told says whether it is active.
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let asked = self.asked_for(surface.wl_surface());
        self.configure_new(&surface, asked);
        self.open_toplevel(surface, asked);
        self.publish_facts();
    }

    /// Titles are not double-buffered, so a new one arrives without a
    /// commit: published now, and the titlebar drawn again now, or the facts,
    /// every taskbar and the titlebar would keep the old title until the
    /// window next drew. A shell that sets the title from its prompt does so
    /// milliseconds after the program before it did, with nothing drawn in
    /// between, and the titlebar went on showing the program's title.
    fn title_changed(&mut self, surface: ToplevelSurface) {
        tracing::debug!(
            surface = self
                .window_for(surface.wl_surface())
                .as_ref()
                .and_then(shell::id_of)
                .map(|id| id.0),
            title = crate::facts::title(surface.wl_surface()),
            "title changed"
        );
        self.backend.redraw();
        self.publish_facts();
    }

    /// A window given a parent, or another, is kept on top with it or let
    /// go (`shell::stacking`). It may be given one after it shows, with
    /// nothing drawn after that would publish it.
    fn parent_changed(&mut self, _surface: ToplevelSurface) {
        self.backend.redraw();
        self.publish_facts();
    }

    fn app_id_changed(&mut self, _surface: ToplevelSurface) {
        // An application that names itself after it maps comes under its
        // rule in `[opacity]` now.
        self.backend.redraw();
        self.publish_facts();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // One its client unmapped first was forgotten then.
        self.withdrawn
            .retain(|withdrawn| withdrawn.toplevel != surface);
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
        // A popup needs its window on screen: one opened from a window its
        // client unmapped is dismissed at once, rather than kept to come back
        // with it. So is a submenu of a panel's menu, the panel unmapped; a
        // panel's own menu is given to it later (see `layers`). And so is one
        // from a window rolled up to its titlebar, which keeps the keyboard,
        // so that a key can ask for one: nothing of it is drawn to hang it
        // from.
        if find_popup_root_surface(&PopupKind::Xdg(surface.clone())).is_ok_and(|root| {
            self.is_withdrawn(&root)
                || self.is_withdrawn_layer(&root)
                || self
                    .window_for(&root)
                    .is_some_and(|window| shell::is_shaded(&window))
        }) {
            surface.send_popup_done();
            return;
        }
        if self.backend.has_person() {
            self.constrain(&surface);
        }
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface.clone())) {
            tracing::warn!(%error, "could not track popup");
        }
        let id = self.mint_surface_id();
        self.popup_ids.insert(surface.wl_surface().id(), id);
        if let Err(error) = surface.send_configure() {
            tracing::warn!(%error, "could not configure popup");
        }
    }

    /// A menu has gone. If it held the keyboard, its window or the menu
    /// below has it back. And what it covered shows again where it hung: its
    /// window's or panel's damage, as a menu's commit is. Nothing for a menu
    /// that showed nothing by then, one that took its picture away having
    /// been counted when it did.
    ///
    /// Choosing an item that changes nothing else -- Copy -- closes the menu
    /// and nothing more, and without this its receipt would read `quiet`.
    fn popup_destroyed(&mut self, surface: PopupSurface) {
        // First, and whatever it showed: a menu with nothing in it may hold
        // the keyboard too.
        self.hand_back_keyboard(&surface);
        // Its id goes with it, and what was kept under it: a surface given a
        // popup again gets a new one (`new_popup`).
        if let Some(id) = self.popup_ids.remove(&surface.wl_surface().id()) {
            self.damage.remove(&id);
            self.focused_at.remove(&id);
            self.popups_shown.remove(&id);
        }
        let popup = PopupKind::Xdg(surface);
        if geometry::shown(popup.wl_surface()).is_some()
            && let Some(hung) = popup::hung(&popup)
            && let Some(id) = self.hung_from(&hung)
        {
            let whole = geometry::window_geometry(popup.wl_surface())
                .map(|whole| damage::to_rect(whole) + hung.at);
            self.record_damage(id, whole.into_iter().collect());
        }
        self.backend.redraw();
        // Always: it was a surface of the facts, covering what is under it,
        // whatever it showed and wherever it hung, and smithay has already
        // stopped drawing it.
        self.publish_facts();
    }

    /// A menu wants the keyboard and pointer until it is dismissed. Only a
    /// person makes that request meaningful: headless, the act path addresses
    /// surfaces directly and a grab would only get in its way.
    ///
    /// Never while the session is locked, when the lock screen holds both: a
    /// menu that took them would take the password typed next. Refused as
    /// xdg-shell says, by dismissing the menu.
    ///
    /// Nor for a client without the keys (#187): the serial it names is no
    /// proof that anything was clicked or pressed in it (see
    /// [`Compositor::keys_are_with`]), and its window would keep the keys as
    /// the menu closed. A right-click still opens one, since the click gives
    /// the window clicked the keys before it reaches it.
    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        if self.lock.is_some() {
            surface.send_popup_done();
            return;
        }
        if !self.backend.has_person() {
            return;
        }
        if !surface
            .wl_surface()
            .client()
            .is_some_and(|client| self.keys_are_with(&client.id()))
        {
            tracing::debug!("menu grab refused: its client does not have the keys");
            surface.send_popup_done();
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
        self.fill_request(&surface, Fill::Maximized, None);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.unfill_request(&surface, Fill::Maximized);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        self.fill_request(&surface, Fill::Fullscreen, output.as_ref());
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.unfill_request(&surface, Fill::Fullscreen);
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
    /// Tell a toplevel how to look before it draws anything: what a new
    /// window is told at `get_toplevel`, and one its client unmapped when it
    /// is asked for again (see `shell::withdrawn`).
    ///
    /// Activated from the start if it is `active`, as every window is with
    /// nobody at the seat: a toolkit that believes it is unfocused renders
    /// differently, and M2 is about reading what is on screen. Not one that
    /// opens without the keys (#259). With a person at the seat, told the
    /// room it has ([`Self::bounds_for`]), so a toolkit that reads it opens
    /// at a size that fits.
    pub(crate) fn configure_new(&self, toplevel: &ToplevelSurface, active: bool) {
        let bounds = self.bounds_for(toplevel);
        toplevel.with_pending_state(|state| {
            if active {
                state.states.set(xdg_toplevel::State::Activated);
            }
            state.bounds = bounds;
        });
        toplevel.send_configure();
    }

    /// Put a toplevel on screen as a new window: with an id nothing has
    /// seen, placed, on the workspace its monitor shows, and with the
    /// keyboard if the person `asked` for it, beneath the window in use
    /// without it if not (#259). What a new window gets, and one its client
    /// unmapped when it maps it again.
    pub(crate) fn open_toplevel(&mut self, toplevel: ToplevelSurface, asked: bool) -> Framed {
        // Taken before the toplevel is moved into the window.
        let wl_surface = toplevel.wl_surface().clone();
        let window = Framed::from(Window::new_wayland_window(toplevel));
        let id = self.mint_surface_id();
        window.user_data().insert_if_missing(|| id);

        // Given back if it opens where its program's last window closed. And
        // when it opened, for a seat to name it if it never draws.
        shell::placement(&window, |placement| {
            placement.cascaded = Some(self.placed);
            placement.opened = Some(Instant::now());
        });
        let at = self.place_new();
        // Not activated by the map: smithay would unset every other window's
        // state unsaid, and the window in use would never be told it no
        // longer is. It is already active (`configure_new`), and on a seat
        // focusing it tells the others (`show_active`); with nobody at the
        // seat they all stay active.
        self.space.map_element(window.clone(), at, false);
        // A frame already settled is fitted now: a window mapped again keeps
        // who draws its frame, and no change of that is coming to fit it. A
        // new window has none yet, and is fitted when it gets one.
        self.fit_frame(&window);
        self.adopt(&window);
        tracing::info!(surface = id.0, at = ?at, "toplevel mapped");

        // Focus what just appeared, if the person asked for it. It does double
        // duty: it is the host half of the focus correlation, and it makes
        // the toolkit's own idea of which window is active agree with ours --
        // which is the pair of observations the join weighs. One that opens
        // without the keys is joined once it has them, if its title alone
        // cannot tell it from its program's other windows.
        if asked {
            self.focus_surface(wl_surface, id);
        } else {
            self.open_without_keys(&window);
        }
        window
    }

    /// A toplevel asking to fill its monitor: only with a person at the
    /// seat. Otherwise, or before it is a window, it is answered with the
    /// plain configure it would have had anyway. One its client unmapped is
    /// filled once it is on screen again, and answered by the configure
    /// that maps it.
    fn fill_request(&mut self, surface: &ToplevelSurface, fill: Fill, on: Option<&WlOutput>) {
        if let Some(withdrawn) = self.withdrawn_mut(surface.wl_surface()) {
            withdrawn.wants = Some(fill);
            return;
        }
        match self.window_for(surface.wl_surface()) {
            Some(window) if self.backend.has_person() => self.fill(&window, fill, on),
            _ => {
                surface.send_configure();
            }
        }
    }

    /// A toplevel asking to stop filling its monitor: only with a person at
    /// the seat, and only one that fills it, whose configure then carries the
    /// change. Otherwise -- nobody at the seat, a window that does not fill
    /// it, one snapped to a half, or before it is a window -- it is answered
    /// with the plain configure it would have had anyway: xdg-shell says
    /// every such request is answered.
    fn unfill_request(&mut self, surface: &ToplevelSurface, fill: Fill) {
        if let Some(withdrawn) = self.withdrawn_mut(surface.wl_surface()) {
            if withdrawn.wants == Some(fill) {
                withdrawn.wants = None;
            }
            return;
        }
        let filling = surface.with_pending_state(|pending| pending.states.contains(fill.state()));
        match self.window_for(surface.wl_surface()) {
            Some(window) if filling && self.backend.has_person() => {
                self.unfill(&window, fill, None);
            }
            _ => {
                surface.send_configure();
            }
        }
    }

    /// The window and grab start for an interactive move or resize, if the
    /// request is one to honour: a person at the seat, a window we know, and a
    /// serial that is the press currently holding the pointer.
    fn interactive(
        &self,
        surface: &ToplevelSurface,
        seat: &WlSeat,
        serial: Serial,
    ) -> Option<(Framed, GrabStartData<Self>)> {
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

    /// On a seat, the window in use -- the one holding the keyboard, or
    /// whose own menu holds it -- is the one that looks active, and no other
    /// (see `show_active`). Headless leaves every toplevel activated from the
    /// start (see `new_toplevel`), because the toolkits it hosts for reading
    /// render differently when they believe they are in the background.
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&FocusTarget>) {
        // X clients read which window has the keyboard off the root.
        #[cfg(feature = "xwayland")]
        self.tell_active_x11(match focused {
            Some(FocusTarget::X11(x11)) => Some(x11),
            _ => None,
        });
        let focused = focused
            .and_then(WaylandFocus::wl_surface)
            .map(std::borrow::Cow::into_owned);
        let focused = focused.as_ref();
        tracing::debug!(surface = ?focused.map(|surface| surface.id()), "keyboard focus changed");
        // The clipboard and the primary selection belong to whoever has the
        // keyboard: a client may only read a selection while focused. On both
        // backends, because an agent pasting is as real as a person pasting.
        let client = focused.and_then(|surface| self.display.get_client(surface.id()).ok());
        // Gone to another client, the keys take the person's attention with
        // them, and a token made before is spent (#257). Unless they leave a
        // launcher, which is asked now, as it takes them: one that has gone
        // is no launcher by the time it hands them on. A launcher's menu
        // counts as the launcher. The layer maps' locks are free here, as
        // `may_take_keyboard` found them just before.
        let launcher =
            focused.is_some_and(|surface| self.is_launcher(&self.menu_root(surface.clone())));
        self.attention
            .keys_to(client.as_ref().map(Client::id), launcher);
        set_data_device_focus(&self.display, seat, client.clone());
        set_primary_focus(&self.display, seat, client);
        // A chord is captured only for the process that has the keyboard.
        self.capture_follows_keyboard(focused);
        // The window in use: the one with the keyboard, or the one whose own
        // menu has it, since a menu's keys are its window's. Found from the
        // surface smithay names, as the keyboard cannot be asked here.
        let window = focused
            .and_then(|surface| self.window_for(&self.menu_root(surface.clone())))
            .and_then(|window| shell::id_of(&window));
        // A window that opened without the keys has them now (#259).
        if let Some(id) = window {
            self.heeded(id);
        }
        // A fullscreen window covers the panels only while it is in use.
        self.restack(window);
        // The window in use is typed in its own layout, when each has one.
        // Not here: smithay calls this from inside `set_focus`, holding the
        // keyboard, and reading or switching its layout now would wait on
        // that lock for ever. Once the loop is idle the keyboard is free.
        if self.layout_memory.switching() == perspicax_policy::Switching::Window {
            self.loop_handle
                .insert_idle(move |state| state.follow_layout(window));
        }
        // Which window is lit, and which are dimmed, follows the keyboard:
        // the next frame is drawn knowing. Only asked for here, which takes
        // no lock; what is in use is read when the frame is drawn.
        self.backend.redraw();
        // So does which window may hold the pointer: a game left lets go.
        self.settle_hold_later();

        self.show_active(window);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor = image;
        self.backend.redraw();
        #[cfg(feature = "capture")]
        self.flush_screencopy_for_pointer();
    }

    /// Lock keys light their LEDs, on every keyboard at once: one xkb state
    /// serves them all, so Caps Lock pressed on a laptop's keyboard lights
    /// a plugged-in one's too.
    fn led_state_changed(&mut self, _seat: &Seat<Self>, leds: LedState) {
        self.backend.light(leds);
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

impl OutputHandler for Compositor {
    /// A client bound a monitor: tell its taskbars and pagers, on that
    /// object, what they were last told is on the monitor. Both are fed by
    /// name, so a monitor bound after it was named to them -- one just
    /// plugged in, or lit again under the same name -- would otherwise never
    /// be named on the object the client now holds for it.
    fn output_bound(&mut self, output: smithay::output::Output, wl_output: WlOutput) {
        let name = output.name();
        self.enter_toplevels(&name, &wl_output);
        self.enter_workspaces(&name, &wl_output);
    }
}

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

    /// A client making a token, to hand to what it starts or wakes. Kept
    /// only if it names input and the client making it has the keys (see
    /// [`Compositor::keys_are_with`]), or names the person's latest press,
    /// under ten seconds old, which it was sent: a notification's action or
    /// a dock's icon clicked, which never has the keys (#257). Any other is
    /// forgotten as it is made,
    /// so spending it brings nothing forward, as a token naming no input
    /// never did. Its client is told the token all the same, as smithay
    /// tells every one, and cannot tell the difference.
    ///
    /// A kept token is stamped with the turn the person's attention is on,
    /// and is good only on that turn (#257). Every token that can no longer
    /// be granted is forgotten here, before smithay keeps the new one, and
    /// no client keeps more than [`TOKENS_PER_CLIENT`]: one made in a loop
    /// has nowhere to grow.
    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        let turn = self.attention.turn();
        self.activation
            .retain_tokens(|_, made| still_good(made, turn));
        let Some(((serial, _), client)) = data.serial.as_ref().zip(data.client_id.as_ref()) else {
            tracing::debug!("activation token refused: it names no input");
            return false;
        };
        let kept = self
            .activation
            .tokens()
            .filter(|(_, made)| made.client_id.as_ref() == Some(client))
            .count();
        if kept >= TOKENS_PER_CLIENT {
            tracing::debug!("activation token refused: its client keeps enough already");
            return false;
        }
        if !(self.keys_are_with(client) || self.attention.vouches(serial, client, Instant::now())) {
            tracing::debug!(
                "activation token refused: its client neither has the keys nor names the \
                 person's latest press, which it was sent"
            );
            return false;
        }
        data.user_data.insert_if_missing(|| turn);
        true
    }

    /// A client asking for one of its windows to be brought forward: an
    /// application opened from a launcher, a link opened in a browser that
    /// was already running. Every token known was made in answer to the
    /// person's own input, by the client with the keys or the one their
    /// latest press was sent to (`token_created`), so it is granted while it
    /// is fresh and the person has not moved on (see
    /// `perspicax_policy::grants_activation`), and a window cannot pull
    /// focus to itself while someone is typing elsewhere. One refused wants
    /// the person's attention, and its taskbar says so (#259).
    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        self.activation.remove_token(&token);
        // A client may name any surface it owns, and the window a subsurface
        // belongs to is its tree's root: that is what is raised and focused.
        let (surface, _) = damage::root_of(&surface);
        let Some(window) = self.window_for(&surface) else {
            return;
        };
        if !still_good(&data, self.attention.turn()) {
            tracing::debug!("activation refused: the token is stale, or the person moved on");
            self.refused_keys(&window);
            self.publish_facts();
            return;
        }
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

#[cfg(feature = "xwayland")]
impl Compositor {
    /// Spend the token `named` outside the protocol, as an X11 window's
    /// `_NET_STARTUP_ID` names the one its program was handed (#259):
    /// whether it was one still good, as `request_activation` asks.
    pub(crate) fn spend_token(&mut self, named: String) -> bool {
        let token = XdgActivationToken::from(named);
        let good = self
            .activation
            .data_for_token(&token)
            .is_some_and(|made| still_good(made, self.attention.turn()));
        self.activation.remove_token(&token);
        good
    }
}

/// How many activation tokens one client may keep at once (#257). A token
/// that brings a window forward with the keys turns the person's attention,
/// and so spends every other there is: a program needs one or two at a
/// time, a launcher one.
const TOKENS_PER_CLIENT: usize = 8;

/// Whether a token kept as `made` may still be granted, on `turn`: fresh, and
/// stamped with that turn, so made since the person last pressed anything or
/// the keys last moved on (#257).
fn still_good(made: &XdgActivationTokenData, turn: perspicax_policy::Turn) -> bool {
    perspicax_policy::grants_activation(
        made.timestamp.elapsed(),
        made.user_data.get::<perspicax_policy::Turn>() == Some(&turn),
    )
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
    /// A pidfd on the origin's process, when one could be vouched for: what
    /// a grant to it is held by. See [`crate::origin::attest`].
    pidfd: OnceLock<OwnedFd>,
    /// The origin's process, with when it started: what a press that reached
    /// this client starts programs under (#259).
    process: OnceLock<perspicax_policy::Process>,
    /// The person's press that started this client's program, if one did:
    /// its windows open with the keys while that press is their latest
    /// (#259). See [`Compositor::stamp_started`].
    started_on: OnceLock<perspicax_policy::Turn>,
    /// Whether it has stopped reading what is sent to it.
    stall: Stall,
    /// What of the pointer it is owed while it has.
    owed: Owed<FocusTarget>,
    /// Whether it has gone, so that it is flushed no more.
    gone: AtomicBool,
}

impl ClientState {
    /// Record who this client is. Once, and only the first time: an identity
    /// that could be overwritten is not an attestation.
    fn attribute(&self, origin: Origin, pidfd: Option<OwnedFd>) {
        let _ = self.origin.set(origin);
        if let Some(pidfd) = pidfd {
            let _ = self.pidfd.set(pidfd);
        }
    }

    /// Who this client is, or `Unattributed` if it was never established.
    pub(crate) fn origin(&self) -> Origin {
        self.origin.get().cloned().unwrap_or_default()
    }

    /// A pidfd of its own on this client's process, if one was vouched for.
    pub(crate) fn pidfd(&self) -> Option<OwnedFd> {
        self.pidfd.get()?.try_clone().ok()
    }

    /// This client's process, with when it started, if it could be read.
    pub(crate) fn process(&self) -> Option<perspicax_policy::Process> {
        self.process.get().copied()
    }

    /// The person's press that started this client's program, if one did.
    pub(crate) fn started_on(&self) -> Option<perspicax_policy::Turn> {
        self.started_on.get().copied()
    }

    /// What to send this client of a pointer `event` for `target`, and what
    /// owed to send before it (see [`crate::owed`]).
    pub(crate) fn plan(&self, target: &FocusTarget, event: Event<'_>) -> Plan {
        self.owed.plan(target, self.stall.stalled(), event)
    }
}

impl ClientData for ClientState {
    fn initialized(&self, _id: ClientId) {}

    /// Say who went, and how, beside the line that said they came.
    ///
    /// A client that hung up and one wayland-backend cut off arrive alike, as
    /// `ConnectionClosed`. wayland-backend cuts off a client whose outgoing
    /// buffer is full, which is what input did to a client that had stopped
    /// reading -- an application frozen for 25 s while an AT asked it where
    /// something is, as #80's Flutter applications were, or the shell when
    /// something kept it from reading (#101). The pointer's motion no longer
    /// does, being owed instead (#169), but keys and configures still can,
    /// and so can whatever a client asked for. Nothing here can tell the two
    /// apart, but a client that went while it read nothing is said to have,
    /// and for how long: cut off for it, or ended frozen. The origin's pid is
    /// what ties this line to the application's own last words, and to the
    /// keeper's "forgotten".
    ///
    /// Called with wayland-backend's state locked, so it touches only its
    /// own.
    fn disconnected(&self, id: ClientId, reason: DisconnectReason) {
        self.gone.store(true, Ordering::Relaxed);
        let origin = self.origin();
        match reason {
            DisconnectReason::ConnectionClosed => match self.stall.behind(Instant::now()) {
                Some(behind) => tracing::warn!(
                    ?id,
                    ?origin,
                    ?behind,
                    "client disconnected while it read nothing: cut off for it, or ended frozen"
                ),
                None => tracing::info!(?id, ?origin, "client disconnected"),
            },
            DisconnectReason::ProtocolError(error) => {
                tracing::warn!(?id, ?origin, %error, "client disconnected for a protocol error");
            }
        }
    }
}

delegate_data_device!(Compositor);
delegate_output!(Compositor);
delegate_seat!(Compositor);
delegate_primary_selection!(Compositor);
delegate_xdg_activation!(Compositor);
delegate_cursor_shape!(Compositor);
delegate_relative_pointer!(Compositor);

/// No tablets, so nothing to say about a tablet tool's pointer: required of
/// any compositor offering `cursor-shape-v1`, which names a tool's too.
impl TabletSeatHandler for Compositor {}
