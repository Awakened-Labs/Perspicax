//! X11 applications, through an Xwayland this compositor starts.
//!
//! One display-server path: perspicax is always a Wayland compositor, and X11
//! enters as one more Wayland client (Xwayland), with this module as its
//! window manager.
//!
//! # Provenance
//!
//! The compositor's Wayland credentials for every X11 window name one
//! process: Xwayland. Which X client drew a given window is a second question,
//! answered in two ways of different strength:
//!
//! - **XRes** (`QueryClientIds`): the X server's own record of the peer on the
//!   connection that created the window. Trusted because the compositor
//!   started that server itself and nothing else could have.
//! - **`_NET_WM_PID`**: a property the client sets about itself. A hint.
//!
//! A window is described with the claimed pid the moment it maps, and upgraded
//! when the XRes answer arrives. Every X11 origin is graded weaker than a
//! Wayland-native one, and consent treats the whole Xwayland as one trust
//! domain (see `perspicax_index::Consent::permits`).
//!
//! The XRes query runs on its own thread with its own X connection. Asking on
//! the compositor thread would block it on Xwayland, and Xwayland may at that
//! moment be blocked on a Wayland round trip to us.
//!
//! # Telling a window its state
//!
//! An X client asks for a state -- minimized, here -- and then waits to be
//! told it has it: Wine changes nothing more about a window until the window
//! manager answers, and restores a minimized window only when its `WM_STATE`
//! goes from iconic back to normal (issue #90). Smithay writes `WM_STATE`
//! only by mapping or unmapping a window's frame, and unmapping it would cost
//! the window the surface Xwayland gave it. So a minimized window stays
//! mapped, as far as X is concerned, and a second connection of the
//! compositor's own, [`Side`], tells it `WM_STATE` instead; Smithay's
//! `_NET_WM_STATE_HIDDEN` goes beside it. Every request is answered, the ones
//! declined included. A window parked with a workspace that is not showing
//! is not minimized, and is not told it is.
//!
//! A window may also ask before it maps, by writing `_NET_WM_STATE` itself,
//! as EWMH lets a withdrawn window and as Wine does for a game that starts
//! fullscreen. Smithay never reads that, and replaces it with its own set the
//! first time it writes one, when the window is first activated. So [`Side`]
//! reads it as the window asks to be mapped, before anything is written. That
//! read is the one X round trip this module makes on the compositor's thread,
//! against the rule the XRes thread keeps: its answer is needed before the
//! window is activated, and it is asked beside Smithay's own reads of the
//! same window, at a moment Xwayland has just sent the request and is not
//! waiting on us.
//!
//! # The active window
//!
//! EWMH has a window manager name the window with the keyboard in the
//! root's `_NET_ACTIVE_WINDOW`, and clients go by it: Wine takes it for the
//! window in front, and holds back its own idea of which that is until the
//! name changes (issue #91). Smithay names the root itself there, a moment
//! after every change of focus: it writes the window its focus events
//! arrive at, and only the root hears them. So [`Side`] names the window as
//! the keyboard moves, and listens on a thread of its own for the root's
//! property changing; whenever it no longer names that window, it is named
//! again. A client that reads between the two writes still reads the root.
//!
//! # Started eagerly, not lazily
//!
//! Smithay 0.7 creates the X11 sockets and starts the server in one call, with
//! no way to hold the sockets and start the server on first connection. So
//! Xwayland starts with the seat, costing one idle process, and `xwayland =
//! false` in the config turns it off.

use std::{
    cell::RefCell,
    os::fd::OwnedFd,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
};

use perspicax_node::{Origin, SurfaceId, X11Basis, X11Origin};
use perspicax_policy::Zone;
use smithay::{
    desktop::Window,
    output::Output,
    reexports::{
        calloop::{LoopHandle, channel},
        wayland_server::{Client, Resource as _, protocol::wl_output::WlOutput},
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::{
        selection::{
            SelectionTarget,
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge, WmWindowProperty, XwmId},
    },
};

use x11rb::{
    connection::Connection as _,
    protocol::{
        Event,
        xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, PropMode},
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::{
    Error,
    framed::Framed,
    shell::{self, Fill, placement},
    state::Compositor,
};

/// Everything about the running Xwayland.
#[derive(Default)]
pub(crate) struct Xwayland {
    pub(crate) wm: Option<X11Wm>,
    /// Xwayland's own pid, from its Wayland credentials.
    server: Option<u32>,
    client: Option<Client>,
    /// Window ids to ask the XRes thread about.
    ask: Option<mpsc::Sender<u32>>,
    /// The window that mapped last, while it waits for the surface it takes
    /// the keyboard with. See `map_window_request`.
    awaiting_focus: Option<SurfaceId>,
    /// The connection that tells windows what Smithay's window manager does
    /// not. See [`Side`].
    side: Option<Side>,
}

/// Start Xwayland, and become its window manager once it is ready. `ready`
/// runs then, with the display number, so whatever needs `DISPLAY` -- the
/// agent's `--spawn` programs and the autostart list -- waits for it.
pub(crate) fn start(
    state: &mut Compositor,
    handle: &LoopHandle<'static, Compositor>,
    ready: impl FnOnce(&mut Compositor) + 'static,
) -> Result<(), Error> {
    let (xwayland, client) = XWayland::spawn(
        &state.display,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_| (),
    )
    .map_err(|error| Error::Seat(format!("could not start Xwayland: {error}")))?;

    let wm_handle = handle.clone();
    let mut ready = Some(ready);
    handle
        .insert_source(xwayland, move |event, (), state| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => {
                match X11Wm::start_wm(wm_handle.clone(), x11_socket, client.clone()) {
                    Ok(wm) => state.xwayland.wm = Some(wm),
                    Err(error) => {
                        tracing::error!(%error, "Xwayland is up but will not take a window manager");
                        return;
                    }
                }
                state.xwayland.server = server_pid(display_number);
                state.xwayland.client = Some(client.clone());
                state.xwayland.ask = provenance(&wm_handle, display_number);
                state.xwayland.side = Side::open(display_number);
                if let Some(launch) = state.launch.as_mut() {
                    launch.x11_display = Some(display_number);
                }
                state.facts.publish_x11_display(Some(display_number));
                tracing::info!(display = display_number, "Xwayland ready");
                if let Some(ready) = ready.take() {
                    ready(state);
                }
            }
            XWaylandEvent::Error => {
                tracing::error!("Xwayland exited while starting; X11 applications will not run");
                if let Some(ready) = ready.take() {
                    ready(state);
                }
            }
        })
        .map_err(|error| Error::EventLoop(error.to_string()))?;
    state.xwayland_shell = Some(XWaylandShellState::new::<Compositor>(&state.display));
    Ok(())
}

/// Xwayland's pid, from the kernel's process tree.
///
/// Neither socket can say. Smithay connects Xwayland over a socket pair this
/// process created, and creates the X11 listening sockets here too before
/// handing them over; peer credentials on either name whoever created or
/// listened -- this compositor. What the kernel does attest is parentage: the
/// server is the child of this process named `Xwayland` that was told to serve
/// this display.
fn server_pid(display: u32) -> Option<u32> {
    let me = std::process::id();
    let wanted = format!(":{display}");
    std::fs::read_dir("/proc")
        .ok()?
        .flatten()
        .find_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            // `pid (comm) state ppid ...`, where comm may itself hold spaces or
            // parentheses, so split at the last `)`.
            let (head, tail) = stat.rsplit_once(')')?;
            let comm = head.split_once('(')?.1;
            let parent: u32 = tail.split_whitespace().nth(1)?.parse().ok()?;
            if parent != me || comm != "Xwayland" {
                return None;
            }
            let cmdline = std::fs::read(entry.path().join("cmdline")).ok()?;
            cmdline
                .split(|&byte| byte == 0)
                .any(|argument| argument == wanted.as_bytes())
                .then_some(pid)
        })
}

/// A surface that presented a buffer before any window claimed it. Xwayland's
/// first commit can arrive before its surface is associated with an X
/// window; kept on the surface itself, so it lives and dies with it.
pub(crate) struct PresentedUnclaimed;

/// The XRes thread: its own X connection, asked about one window at a time,
/// answering into the compositor's loop.
fn provenance(handle: &LoopHandle<'static, Compositor>, display: u32) -> Option<mpsc::Sender<u32>> {
    let (ask, asked) = mpsc::channel::<u32>();
    let (answer, answers) = channel::channel::<(u32, Option<u32>)>();
    if let Err(error) = handle.insert_source(answers, |event, (), state| {
        if let channel::Event::Msg((window, pid)) = event {
            state.attribute_x11(window, pid);
        }
    }) {
        tracing::warn!(%error, "no XRes answers: X11 windows keep their claimed pids");
        return None;
    }
    std::thread::Builder::new()
        .name("perspicax-xres".to_owned())
        .spawn(move || {
            use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _};
            let Ok((connection, _)) = x11rb::connect(Some(&format!(":{display}"))) else {
                tracing::warn!("could not open an X connection for XRes");
                return;
            };
            for window in asked {
                let pid = connection
                    .res_query_client_ids(&[ClientIdSpec {
                        client: window,
                        mask: ClientIdMask::LOCAL_CLIENT_PID,
                    }])
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                    .and_then(|reply| reply.ids.first().and_then(|id| id.value.first().copied()));
                if answer.send((window, pid)).is_err() {
                    return;
                }
            }
        })
        .map_err(|error| tracing::warn!(%error, "no XRes thread"))
        .ok()?;
    Some(ask)
}

x11rb::atom_manager! {
    /// The atoms the side connection reads and writes.
    SideAtoms: SideAtomsCookie {
        WM_STATE,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_ACTIVE_WINDOW,
    }
}

/// What a window asked to be by the `_NET_WM_STATE` it set on itself before
/// it mapped.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Wants {
    fullscreen: bool,
    /// Both ways at once: Smithay counts nothing less as maximized.
    maximized: bool,
}

impl Wants {
    fn read(state: &[u32], atoms: &SideAtoms) -> Self {
        Self {
            fullscreen: state.contains(&atoms._NET_WM_STATE_FULLSCREEN),
            maximized: state.contains(&atoms._NET_WM_STATE_MAXIMIZED_HORZ)
                && state.contains(&atoms._NET_WM_STATE_MAXIMIZED_VERT),
        }
    }
}

/// ICCCM's `WM_STATE`: what the window manager tells a window it has done
/// with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WmState {
    Withdrawn,
    Normal,
    Iconic,
}

impl WmState {
    /// As ICCCM numbers it.
    fn value(self) -> u32 {
        match self {
            Self::Withdrawn => 0,
            Self::Normal => 1,
            Self::Iconic => 3,
        }
    }
}

/// Perspicax's own X connection, beside the window manager's, for what
/// Smithay's window manager does not write: `WM_STATE` as a window is
/// minimized and brought back, and the root's `_NET_ACTIVE_WINDOW`. Smithay
/// writes the first only by mapping or unmapping a window's frame, and
/// unmapping the frame would cost the window the surface Xwayland gave it;
/// the second it writes wrong. Writes from the compositor's thread, so
/// nothing there waits on Xwayland, and listens on a thread of its own: see
/// [`Side::listen`].
#[derive(Clone)]
pub(crate) struct Side {
    connection: Arc<RustConnection>,
    atoms: SideAtoms,
    root: u32,
    /// The X11 window with the keyboard, or `NONE`: what the root's
    /// `_NET_ACTIVE_WINDOW` is kept saying.
    active: Arc<AtomicU32>,
}

impl Side {
    /// Connect to `display`, learn the atoms, and listen. Without it, X11
    /// windows are minimized as before, untold, and the root names none as
    /// active.
    fn open(display: u32) -> Option<Self> {
        let opened = x11rb::connect(Some(&format!(":{display}")))
            .map_err(|error| error.to_string())
            .and_then(|(connection, screen)| {
                let root = connection.setup().roots[screen].root;
                let atoms = SideAtoms::new(&connection)
                    .map_err(|error| error.to_string())?
                    .reply()
                    .map_err(|error| error.to_string())?;
                Ok(Self {
                    connection: Arc::new(connection),
                    atoms,
                    root,
                    active: Arc::default(),
                })
            });
        let side = opened
            .map_err(|error| {
                tracing::warn!(%error, "no side X connection: X11 windows are not told they are minimized");
            })
            .ok()?;
        side.listen();
        Some(side)
    }

    /// Hear what Smithay's window manager drops, on a thread of its own: the
    /// root's property changes, so the root goes on naming the window with
    /// the keyboard when Smithay writes otherwise there. A thread, as for
    /// XRes, because that is answered by reading the root back, and a read
    /// on the compositor's thread would wait on Xwayland. Everything else it
    /// hears it drops, the errors that come back for writes to a window
    /// already gone included. It ends when Xwayland does, its connection
    /// with it.
    fn listen(&self) {
        let selected = self
            .connection
            .change_window_attributes(
                self.root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(|error| error.to_string())
            .and_then(|cookie| cookie.check().map_err(|error| error.to_string()));
        if let Err(error) = selected {
            tracing::warn!(%error, "the side X connection hears nothing: the root may name the wrong window as active");
            return;
        }
        let side = self.clone();
        let spawned = std::thread::Builder::new()
            .name("perspicax-x11-side".to_owned())
            .spawn(move || {
                while let Ok(event) = side.connection.wait_for_event() {
                    if let Event::PropertyNotify(notify) = event
                        && notify.window == side.root
                        && notify.atom == side.atoms._NET_ACTIVE_WINDOW
                    {
                        side.keep_active();
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "no side X thread: the root may name the wrong window as active");
        }
    }

    /// Name `window` as the one with the keyboard, or `NONE`, in the root's
    /// `_NET_ACTIVE_WINDOW`. Written whatever it was before: a client that
    /// asked to be active waits for the write, even when nothing changed.
    pub(crate) fn set_active(&self, window: u32) {
        self.active.store(window, Ordering::Relaxed);
        self.write_active(window);
    }

    /// A window that is gone: if the root names it, name none instead.
    fn forget_active(&self, window: u32) {
        let named =
            self.active
                .compare_exchange(window, x11rb::NONE, Ordering::Relaxed, Ordering::Relaxed);
        if named.is_ok() {
            self.write_active(x11rb::NONE);
        }
    }

    /// The root's `_NET_ACTIVE_WINDOW` changed: if it no longer names the
    /// window with the keyboard -- Smithay writes the root there a moment
    /// after every change of focus -- name that window again. Smithay does
    /// not answer the root's property changes, so this cannot go back and
    /// forth with it.
    fn keep_active(&self) {
        let says = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().and_then(|mut value| value.next()));
        // Read again rather than kept from before the round trip: focus may
        // have moved meanwhile, and the write must be the latest word.
        let active = self.active.load(Ordering::Relaxed);
        if says != Some(active) {
            self.write_active(active);
        }
    }

    fn write_active(&self, window: u32) {
        let _ = self.connection.change_property32(
            PropMode::REPLACE,
            self.root,
            self.atoms._NET_ACTIVE_WINDOW,
            AtomEnum::WINDOW,
            &[window],
        );
        let _ = self.connection.flush();
    }

    /// What `window` asked to be, by the `_NET_WM_STATE` it set before it
    /// mapped. The one read this connection makes, and so its one round trip
    /// on the compositor's thread: see the module docs.
    fn wants(&self, window: u32) -> Wants {
        let state: Vec<u32> = self
            .connection
            .get_property(
                false,
                window,
                self.atoms._NET_WM_STATE,
                AtomEnum::ATOM,
                0,
                32,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(Iterator::collect))
            .unwrap_or_default();
        Wants::read(&state, &self.atoms)
    }

    /// Tell `window` it is in `state`. Written whatever it was before: the
    /// write is the answer a client waits for, even when nothing changed.
    fn set_wm_state(&self, window: u32, state: WmState) {
        let _ = self.connection.change_property32(
            PropMode::REPLACE,
            window,
            self.atoms.WM_STATE,
            self.atoms.WM_STATE,
            &[state.value(), x11rb::NONE],
        );
        let _ = self.connection.flush();
    }

    /// Answer a request about `_NET_WM_STATE` without changing it: appending
    /// nothing still tells the window its property changed, which is what a
    /// client that asked waits for.
    fn answer(&self, window: u32) {
        let _ = self.connection.change_property32(
            PropMode::APPEND,
            window,
            self.atoms._NET_WM_STATE,
            AtomEnum::ATOM,
            &[],
        );
        let _ = self.connection.flush();
    }
}

/// Where an X11 window's origin is kept, updated when XRes answers.
type Attribution = RefCell<Option<X11Origin>>;

impl Compositor {
    /// The origin of an X11 window, as far as it is known.
    pub(crate) fn x11_origin(window: &Framed) -> Origin {
        window
            .user_data()
            .get::<Attribution>()
            .and_then(|cell| cell.borrow().clone())
            .map_or(Origin::Unattributed, |origin| Origin::X11(Box::new(origin)))
    }

    /// XRes answered for `window_id`: upgrade the origin to the server's own
    /// word, or leave the claim standing if it had nothing to say.
    fn attribute_x11(&mut self, window_id: u32, pid: Option<u32>) {
        let Some(pid) = pid else {
            return;
        };
        let window = self
            .space
            .elements()
            .chain(&self.parked)
            .find(|window| {
                window
                    .x11_surface()
                    .is_some_and(|x11| x11.window_id() == window_id)
            })
            .cloned();
        let (Some(window), Some(server)) = (window, self.xwayland.server) else {
            return;
        };
        let client = match crate::origin::of_pid(i32::try_from(pid).unwrap_or(-1)) {
            Origin::Process(process) => Some(*process),
            _ => None,
        };
        window
            .user_data()
            .insert_if_missing(|| Attribution::new(None));
        if let Some(cell) = window.user_data().get::<Attribution>() {
            *cell.borrow_mut() = Some(X11Origin {
                server,
                client,
                basis: X11Basis::XRes,
            });
        }
        self.publish_facts();
    }

    /// Record what is known about an X11 window's origin at map time -- its
    /// claimed pid -- and ask XRes for the real one.
    fn claim_x11(&mut self, window: &Framed, x11: &X11Surface) {
        let Some(server) = self.xwayland.server else {
            return;
        };
        let client =
            x11.pid().and_then(
                |pid| match crate::origin::of_pid(i32::try_from(pid).ok()?) {
                    Origin::Process(process) => Some(*process),
                    _ => None,
                },
            );
        let basis = if client.is_some() {
            X11Basis::ClaimedPid
        } else {
            X11Basis::ServerOnly
        };
        window.user_data().insert_if_missing(|| {
            Attribution::new(Some(X11Origin {
                server,
                client,
                basis,
            }))
        });
        if let Some(ask) = &self.xwayland.ask {
            let _ = ask.send(x11.window_id());
        }
    }

    /// The mapped or parked window wrapping this X11 surface.
    fn x11_window(&self, surface: &X11Surface) -> Option<Framed> {
        self.space
            .elements()
            .chain(&self.parked)
            .find(|window| window.x11_surface() == Some(surface))
            .cloned()
    }

    /// Whether the keyboard is with an X11 window, which is when X clients
    /// may read the clipboard.
    fn x11_has_focus(&self) -> bool {
        let focused_client = self
            .keyboard_focus()
            .and_then(|surface| self.display.get_client(surface.id()).ok());
        focused_client.is_some() && focused_client == self.xwayland.client
    }
}

impl XWaylandShellHandler for Compositor {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        self.xwayland_shell
            .as_mut()
            .expect("the xwayland-shell global exists before Xwayland can use it")
    }

    /// An X window now has its surface. If that surface already presented
    /// something, the window has: without this, a window whose only commit
    /// arrived before the association would be judged empty forever. And if
    /// it is the window that mapped last, it takes the keyboard it was
    /// waiting for, unless it has gone off screen meanwhile.
    fn surface_associated(
        &mut self,
        _xwm: XwmId,
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        x11: X11Surface,
    ) {
        let presented = smithay::wayland::compositor::with_states(&surface, |states| {
            states.data_map.get::<PresentedUnclaimed>().is_some()
        });
        if let Some(id) = self.x11_window(&x11).as_ref().and_then(shell::id_of) {
            if presented {
                self.mark_presented(id);
            }
            let awaited = self
                .xwayland
                .awaiting_focus
                .take_if(|awaited| *awaited == id);
            if awaited.is_some() && self.window_for_id(id).is_some() {
                self.focus_surface(surface, id);
            }
            self.backend.redraw();
            self.publish_facts();
        }
    }
}

impl XwmHandler for Compositor {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwayland
            .wm
            .as_mut()
            .expect("only a running window manager sends events")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    /// A title or class changed: published now, as a Wayland window's is,
    /// rather than whenever the window next draws.
    fn property_notify(&mut self, _xwm: XwmId, x11: X11Surface, property: WmWindowProperty) {
        if matches!(property, WmWindowProperty::Title | WmWindowProperty::Class)
            && self.x11_window(&x11).is_some()
        {
            self.backend.redraw();
            self.publish_facts();
        }
    }
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    /// A managed X11 window wants to be seen: place it as any new window is
    /// placed, tell it where it is, and give it the keyboard, as a new
    /// Wayland window takes it.
    fn map_window_request(&mut self, _xwm: XwmId, x11: X11Surface) {
        // What it asked to be before it mapped -- a game starting
        // fullscreen, say -- read before anything writes `_NET_WM_STATE`,
        // which Smithay replaces with its own set. Only with a person at the
        // seat, as for a request.
        let wants = match &self.xwayland.side {
            Some(side) if self.backend.has_person() => side.wants(x11.window_id()),
            _ => Wants::default(),
        };
        if let Err(error) = x11.set_mapped(true) {
            tracing::warn!(%error, "could not map X11 window");
            return;
        }
        let window = Framed::from(Window::new_x11_window(x11.clone()));
        let id = self.mint_surface_id();
        window.user_data().insert_if_missing(|| id);
        self.claim_x11(&window, &x11);
        let at = self.place_new();
        let size = x11.geometry().size;
        let _ = x11.configure(Rectangle::new(at, size));
        self.space.map_element(window.clone(), at, false);
        // Its Motif hints are in by now, so whether it is framed is known,
        // and the titlebar has to start on screen.
        self.fit_frame(&window);
        self.adopt(&window);
        // Maximized first, so that leaving fullscreen lands on it; both
        // before it takes the keyboard, whose activation is the first write.
        if wants.maximized {
            self.fill(&window, Fill::Maximized, None);
        }
        if wants.fullscreen {
            self.fill(&window, Fill::Fullscreen, None);
        }
        if let Some(wm) = self.xwayland.wm.as_mut() {
            let _ = wm.raise_window(&x11);
        }
        // The keyboard goes to a surface, and a window maps before Xwayland
        // has given it one, as a rule: then it waits for `surface_associated`.
        // Dropping it instead left the keyboard with the window before, which
        // got whatever was typed next.
        self.xwayland.awaiting_focus = match shell::surface_of(&window) {
            Some(surface) => {
                self.focus_surface(surface, id);
                None
            }
            None => Some(id),
        };
        tracing::info!(surface = id.0, title = x11.title(), "X11 window mapped");
        self.backend.redraw();
        self.publish_facts();
    }

    /// Menus and tooltips place themselves; the compositor only records
    /// where, so they render, hit-test and occlude like anything else.
    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, x11: X11Surface) {
        let at = x11.geometry().loc;
        let window = Framed::from(Window::new_x11_window(x11.clone()));
        let id = self.mint_surface_id();
        window.user_data().insert_if_missing(|| id);
        self.claim_x11(&window, &x11);
        self.space.map_element(window, at, false);
        self.backend.redraw();
        self.publish_facts();
    }

    /// A window its client withdrew. Told so, before the keyboard moves on
    /// from it: see `withdraw_x11`.
    fn unmapped_window(&mut self, _xwm: XwmId, x11: X11Surface) {
        if !x11.is_override_redirect() {
            self.withdraw_x11(&x11);
        }
        self.forget_x11(&x11);
    }

    fn destroyed_window(&mut self, _xwm: XwmId, x11: X11Surface) {
        self.forget_x11(&x11);
    }

    /// An X11 window asking for a size. Granted: the size is the client's to
    /// choose. The position is not: that is the window manager's. Nor is the
    /// size of a window that fills its monitor or a zone: it keeps it, and is
    /// told the rect it has, as ICCCM answers a request not granted. A game
    /// going fullscreen asks for sizes as it sets its mode.
    fn configure_request(
        &mut self,
        _xwm: XwmId,
        x11: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let held = self
            .x11_window(&x11)
            .is_some_and(|window| Self::filling(&window).is_some() || Self::is_snapped(&window));
        if held {
            let _ = x11.configure(None);
            return;
        }
        let mut geometry = x11.geometry();
        if let Some(w) = w.and_then(|w| i32::try_from(w).ok()) {
            geometry.size.w = w;
        }
        if let Some(h) = h.and_then(|h| i32::try_from(h).ok()) {
            geometry.size.h = h;
        }
        let _ = x11.configure(geometry);
    }

    /// An override-redirect window moved itself.
    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        x11: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        if !x11.is_override_redirect() {
            return;
        }
        if let Some(window) = self.x11_window(&x11) {
            self.space.map_element(window, geometry.loc, false);
            self.backend.redraw();
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, x11: X11Surface, _button: u32, edge: ResizeEdge) {
        let Some((window, start)) = self.x11_interactive(&x11) else {
            return;
        };
        let edges = perspicax_policy::Edges {
            top: matches!(
                edge,
                ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
            ),
            bottom: matches!(
                edge,
                ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
            ),
            left: matches!(
                edge,
                ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
            ),
            right: matches!(
                edge,
                ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
            ),
        };
        self.start_resize(&window, edges, start, SERIAL_COUNTER.next_serial());
    }

    fn move_request(&mut self, _xwm: XwmId, x11: X11Surface, _button: u32) {
        let Some((window, start)) = self.x11_interactive(&x11) else {
            return;
        };
        self.start_move(&window, start, SERIAL_COUNTER.next_serial());
    }

    fn allow_selection_access(&mut self, _xwm: XwmId, _selection: SelectionTarget) -> bool {
        self.x11_has_focus()
    }

    /// A Wayland client is pasting what an X client copied.
    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let result = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.seat, mime_type, fd)
                    .map_err(|e| e.to_string())
            }
            SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime_type, fd)
                .map_err(|e| e.to_string()),
        };
        if let Err(error) = result {
            tracing::warn!(%error, ?selection, "could not hand an X11 selection over");
        }
    }

    /// An X client copied something: offer it to Wayland clients.
    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display, &self.seat, mime_types, ());
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display, &self.seat, mime_types, ());
            }
        }
    }

    /// The X client's selection went away. Cleared only if it is still the
    /// one on offer: a Wayland client may have copied something since.
    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display, &self.seat);
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display, &self.seat);
                }
            }
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        tracing::warn!("Xwayland's window manager connection closed");
        self.xwayland.wm = None;
        self.xwayland.side = None;
    }

    /// An X11 window asking to be minimized: ICCCM's `WM_CHANGE_STATE` to
    /// iconic, which is how Wine minimizes one. Only with a person at the
    /// seat, as for a Wayland window. Either way it is answered with the
    /// state it is now in: Wine changes nothing more about a window while a
    /// request of its is waiting.
    fn minimize_request(&mut self, _xwm: XwmId, x11: X11Surface) {
        let Some(window) = self.x11_window(&x11) else {
            return;
        };
        if self.backend.has_person() && !Self::is_minimized(&window) {
            self.minimize(&window);
        } else {
            self.answer_wm_state(&window, &x11);
        }
    }

    /// An X11 window asking to go fullscreen: EWMH's `_NET_WM_STATE`, which is
    /// how a Wine game does. Only with a person at the seat, as for a Wayland
    /// window, and answered either way.
    fn fullscreen_request(&mut self, _xwm: XwmId, x11: X11Surface) {
        if let Some(window) = self.honoured(&x11).filter(|_| !x11.is_fullscreen()) {
            self.fill(&window, Fill::Fullscreen, None);
        }
        self.answer_net_wm_state(&x11);
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, x11: X11Surface) {
        if let Some(window) = self.honoured(&x11).filter(|_| x11.is_fullscreen()) {
            self.unfill(&window, Fill::Fullscreen, None);
        }
        self.answer_net_wm_state(&x11);
    }

    /// Maximized both ways at once; Smithay passes on nothing less. Under
    /// fullscreen it is only noted, and shows once fullscreen ends.
    fn maximize_request(&mut self, _xwm: XwmId, x11: X11Surface) {
        if let Some(window) = self.honoured(&x11).filter(|_| !x11.is_maximized()) {
            if x11.is_fullscreen() {
                let _ = x11.set_maximized(true);
            } else {
                self.fill(&window, Fill::Maximized, None);
            }
        }
        self.answer_net_wm_state(&x11);
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, x11: X11Surface) {
        if let Some(window) = self.honoured(&x11).filter(|_| x11.is_maximized()) {
            if x11.is_fullscreen() {
                let _ = x11.set_maximized(false);
            } else {
                self.unfill(&window, Fill::Maximized, None);
            }
        }
        self.answer_net_wm_state(&x11);
    }

    /// An X11 window asking to be brought back from minimized. Restored and
    /// raised, but not given the keyboard: a window may not take that for
    /// itself, as with an xdg activation no input is behind.
    fn unminimize_request(&mut self, _xwm: XwmId, x11: X11Surface) {
        let Some(window) = self.x11_window(&x11) else {
            return;
        };
        if self.backend.has_person() && Self::is_minimized(&window) {
            self.restore(&window);
            self.space.raise_element(&window, false);
            if let Some(wm) = self.xwayland.wm.as_mut() {
                let _ = wm.raise_window(&x11);
            }
            self.backend.redraw();
            self.publish_facts();
        } else {
            self.answer_wm_state(&window, &x11);
        }
    }
}

impl Compositor {
    /// Tell an X11 window it was minimized, or brought back: ICCCM's
    /// `WM_STATE`, whose change from iconic to normal is what Wine restores a
    /// window on, and EWMH's `_NET_WM_STATE_HIDDEN`. Its frame stays mapped,
    /// so the surface Xwayland gave it stays its own.
    pub(crate) fn x11_minimized(&self, window: &Framed, minimized: bool) {
        let Some(x11) = window.x11_surface() else {
            return;
        };
        if x11.is_override_redirect() {
            return;
        }
        let _ = x11.set_suspended(minimized);
        let state = if minimized {
            WmState::Iconic
        } else {
            WmState::Normal
        };
        self.tell_wm_state(x11, state);
    }

    /// Answer a request to be minimized or brought back with the state the
    /// window is in.
    fn answer_wm_state(&self, window: &Framed, x11: &X11Surface) {
        let state = if Self::is_minimized(window) {
            WmState::Iconic
        } else {
            WmState::Normal
        };
        self.tell_wm_state(x11, state);
    }

    /// Tell a window its client withdrew that it is withdrawn, as ICCCM has
    /// a window manager do: Wine maps a window again only once it has seen
    /// that. Smithay unmaps the frame of its own accord and writes nothing.
    /// And take back the states this window manager gave it, as EWMH has it,
    /// so it maps again as a new window would and not, say, still focused or
    /// fullscreen.
    fn withdraw_x11(&self, x11: &X11Surface) {
        let _ = x11.set_activated(false);
        let _ = x11.set_fullscreen(false);
        let _ = x11.set_maximized(false);
        let _ = x11.set_suspended(false);
        self.tell_wm_state(x11, WmState::Withdrawn);
    }

    /// Name the X11 window with the keyboard, `focused`, or none, in the
    /// root's `_NET_ACTIVE_WINDOW`, as EWMH has a window manager do. With or
    /// without a person at the seat: it is a fact about the keyboard.
    pub(crate) fn tell_active_x11(&self, focused: Option<&X11Surface>) {
        if let Some(side) = &self.xwayland.side {
            side.set_active(focused.map_or(x11rb::NONE, X11Surface::window_id));
        }
    }

    fn tell_wm_state(&self, x11: &X11Surface, state: WmState) {
        if let Some(side) = &self.xwayland.side {
            side.set_wm_state(x11.window_id(), state);
        }
    }

    /// Answer a request about `_NET_WM_STATE`, whether it changed anything
    /// or not: a client waits for the answer either way.
    fn answer_net_wm_state(&self, x11: &X11Surface) {
        if let Some(side) = &self.xwayland.side {
            side.answer(x11.window_id());
        }
    }

    /// The window an X11 request to change its state is about, if the
    /// request is one to honour: a person at the seat, and a window we know.
    fn honoured(&self, x11: &X11Surface) -> Option<Framed> {
        if !self.backend.has_person() {
            return None;
        }
        self.x11_window(x11)
    }

    /// Make an X11 window fullscreen on `on`, or the monitor it is on: the
    /// whole of it, over the panels and with no frame, which follow
    /// `_NET_WM_STATE`. A window in a zone comes out of it, maximized
    /// included, and `restore` keeps where it was before the zone; leaving
    /// fullscreen goes back to maximized, if it was. A parked window is only
    /// told, as an xdg one is.
    pub(crate) fn fill_x11(&mut self, window: &Framed, on: Option<&WlOutput>) {
        let Some(x11) = window.x11_surface() else {
            return;
        };
        let _ = x11.set_fullscreen(true);
        placement(window, |placement| placement.snapped = None);
        let Some(current) = self.extent(window) else {
            Self::restore_from_parked(window);
            self.publish_facts();
            return;
        };
        let output = on
            .and_then(Output::from_resource)
            .or_else(|| self.output_of(window));
        let Some(area) = output.and_then(|output| self.space.output_geometry(&output)) else {
            return;
        };
        placement(window, |placement| {
            placement.restore.get_or_insert(current);
        });
        let _ = x11.configure(area);
        self.space.map_element(window.clone(), area.loc, false);
        self.window_moved(window);
        self.backend.redraw();
        self.publish_facts();
    }

    /// Take an X11 window out of fullscreen: back to maximized, if it was
    /// maximized under it, or else to where it was at the size it was -- or
    /// to `at`, for one dragged out of it.
    pub(crate) fn unfill_x11(&mut self, window: &Framed, at: Option<Point<i32, Logical>>) {
        let Some(x11) = window.x11_surface() else {
            return;
        };
        let _ = x11.set_fullscreen(false);
        let shown = self.space.element_location(window).is_some();
        if x11.is_maximized() {
            if shown {
                self.snap(window, Zone::Top, None);
            } else {
                placement(window, |placement| placement.snapped = Some(Zone::Top));
                self.publish_facts();
            }
            return;
        }
        if let Some(restore) = placement(window, |placement| placement.restore.take()) {
            let to = at.unwrap_or(restore.loc);
            let _ = x11.configure(Rectangle::new(to, restore.size));
            if shown {
                self.space.map_element(window.clone(), to, false);
                self.window_moved(window);
            } else {
                placement(window, |placement| placement.parked = Some(to));
            }
        }
        self.backend.redraw();
        self.publish_facts();
    }

    /// A window gone. If the keyboard has not moved on from it -- with
    /// nobody at the seat it does not -- the root names none as active,
    /// rather than a window that is not there.
    fn forget_x11(&mut self, x11: &X11Surface) {
        if let Some(window) = self.x11_window(x11) {
            self.forget_window(&window);
            self.refocus_after_close(x11.wl_surface().as_ref());
            self.backend.redraw();
            self.publish_facts();
        }
        if let Some(side) = &self.xwayland.side {
            side.forget_active(x11.window_id());
        }
    }

    /// The window and grab start for an X11 move or resize: a person at the
    /// seat and a button still held. X11 has no serial to check, so the held
    /// button is the evidence that the drag is real.
    fn x11_interactive(
        &self,
        x11: &X11Surface,
    ) -> Option<(Framed, smithay::input::pointer::GrabStartData<Self>)> {
        if !self.backend.has_person() {
            return None;
        }
        let pointer = self.pointer.as_ref()?;
        let start = pointer.grab_start_data()?;
        Some((self.x11_window(x11)?, start))
    }
}

smithay::delegate_xwayland_shell!(Compositor);

#[cfg(test)]
mod tests {
    use super::*;

    fn atoms() -> SideAtoms {
        SideAtoms {
            WM_STATE: 1,
            _NET_WM_STATE: 2,
            _NET_WM_STATE_FULLSCREEN: 3,
            _NET_WM_STATE_MAXIMIZED_HORZ: 4,
            _NET_WM_STATE_MAXIMIZED_VERT: 5,
            _NET_ACTIVE_WINDOW: 6,
        }
    }

    #[test]
    fn a_window_maximized_one_way_is_not_asking_to_be_maximized() {
        let atoms = atoms();
        assert_eq!(Wants::read(&[5], &atoms), Wants::default());
        assert_eq!(Wants::read(&[4], &atoms), Wants::default());
        assert_eq!(
            Wants::read(&[5, 4], &atoms),
            Wants {
                fullscreen: false,
                maximized: true,
            }
        );
    }

    #[test]
    fn fullscreen_is_found_anywhere_in_the_list() {
        let atoms = atoms();
        assert!(Wants::read(&[7, 9, 3], &atoms).fullscreen);
        assert!(Wants::read(&[3], &atoms).fullscreen);
        assert!(!Wants::read(&[], &atoms).fullscreen);
    }
}
