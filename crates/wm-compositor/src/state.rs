//! The compositor's state, and the protocol handlers that mutate it.
//!
//! Smithay's shape is one state struct plus a handler trait per protocol, wired
//! together by `delegate_*!`. Nothing here is unusual for a Wayland compositor;
//! the two decisions worth reading are in [`Compositor::commit`] and
//! [`Compositor::send_frames`], and both follow from this compositor not
//! drawing anything.

use std::{
    os::unix::net::UnixStream,
    sync::{Arc, OnceLock},
    time::Instant,
};

use wm_node::{Origin, SurfaceId};

use crate::{facts::Facts, origin};

use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{Space, Window},
    input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_surface::WlSurface},
        },
    },
    utils::{Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
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
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

/// How far each new toplevel is offset from the last, so two windows opened in
/// a row are not stacked exactly on top of each other and invisible to a test
/// about overlap. A placement policy, and a placeholder for one: the arc's
/// occlusion demo needs to *choose* where windows go, and will replace this.
const CASCADE: i32 = 32;

/// Everything this compositor knows.
pub struct Compositor {
    /// Kept so a new client can be inserted from an event callback.
    pub(crate) display: DisplayHandle,
    pub(crate) compositor: CompositorState,
    pub(crate) xdg_shell: XdgShellState,
    pub(crate) shm: ShmState,
    /// Held, not read. These three are RAII handles for protocol globals:
    /// dropping the `Output` withdraws `wl_output`, dropping the
    /// `OutputManagerState` withdraws `xdg_output`, and dropping the `Seat`
    /// withdraws `wl_seat` -- so a client that connected a moment earlier would
    /// watch the desktop lose features it had already bound. `expect` rather
    /// than `allow`: the next slice dispatches focus through the seat, and this
    /// should start complaining the moment that makes it live.
    #[expect(dead_code, reason = "RAII handle for the xdg_output global")]
    pub(crate) output_manager: OutputManagerState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) data_device: DataDeviceState,
    #[expect(dead_code, reason = "RAII handle for the wl_seat global")]
    pub(crate) seat: Seat<Self>,
    /// Windows and their z-order. `Space::elements()` iterates back to front,
    /// which is the order `wm_index::HostFacts` wants, so the two agree by
    /// construction rather than by a conversion someone has to keep right.
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
    #[expect(dead_code, reason = "RAII handle for the wl_output global")]
    pub(crate) output: Output,
    placed: i32,
    started: Instant,
}

impl Compositor {
    /// Bring up every global a stock GTK or Qt client expects to find, and one
    /// virtual output for them to sit on.
    pub(crate) fn new(display: &DisplayHandle, size: (i32, i32), facts: Facts) -> Self {
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(display, "wm-seat");

        let output = Output::new(
            "wm-headless".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "wm".to_owned(),
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
        output.create_global::<Self>(display);

        let mut space = Space::default();
        space.map_output(&output, (0, 0));

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
            output,
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

    /// The window whose toplevel owns this surface, if any.
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
    /// The buffer is released immediately, which a rendering compositor would
    /// not do. It holds a buffer until it has drawn from it, and Smithay
    /// therefore releases the *previous* buffer when a newer one arrives -- a
    /// scheme that quietly requires the client to own at least two. This
    /// compositor never reads a pixel, so it is finished with a buffer the
    /// instant it arrives, and saying so keeps a single-buffered client running
    /// instead of stalled against a compositor that had no use for its
    /// contents in the first place.
    fn commit(&mut self, surface: &WlSurface) {
        with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            if let Some(BufferAssignment::NewBuffer(buffer)) = attributes.current().buffer.take() {
                buffer.release();
            }
        });

        if let Some(window) = self.window_for(surface) {
            window.on_commit();
        }
        self.space.refresh();
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

        let window = Window::new_wayland_window(surface);
        let id = self.mint_surface_id();
        window.user_data().insert_if_missing(|| id);

        let at = (self.placed * CASCADE, self.placed * CASCADE);
        self.placed += 1;
        self.space.map_element(window, at, true);
        tracing::info!(surface = id.0, at = ?at, "toplevel mapped");
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
