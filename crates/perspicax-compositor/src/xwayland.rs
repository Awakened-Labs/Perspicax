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
//! # Started eagerly, not lazily
//!
//! Smithay 0.7 creates the X11 sockets and starts the server in one call, with
//! no way to hold the sockets and start the server on first connection. So
//! Xwayland starts with the seat, costing one idle process, and `xwayland =
//! false` in the config turns it off.

use std::{cell::RefCell, os::fd::OwnedFd, process::Stdio, sync::mpsc};

use perspicax_node::{Origin, X11Basis, X11Origin};
use smithay::{
    desktop::Window,
    reexports::{
        calloop::{LoopHandle, channel},
        wayland_server::{Client, Resource as _},
    },
    utils::{Logical, Rectangle, SERIAL_COUNTER},
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

use crate::{Error, framed::Framed, shell, state::Compositor};

/// Everything about the running Xwayland.
#[derive(Default)]
pub(crate) struct Xwayland {
    pub(crate) wm: Option<X11Wm>,
    /// Xwayland's own pid, from its Wayland credentials.
    server: Option<u32>,
    client: Option<Client>,
    /// Window ids to ask the XRes thread about.
    ask: Option<mpsc::Sender<u32>>,
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
    /// arrived before the association would be judged empty forever.
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
    /// placed, tell it where it is, and focus it.
    fn map_window_request(&mut self, _xwm: XwmId, x11: X11Surface) {
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
        if let Some(wm) = self.xwayland.wm.as_mut() {
            let _ = wm.raise_window(&x11);
        }
        if let Some(surface) = shell::surface_of(&window) {
            self.focus_surface(surface, id);
        }
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

    fn unmapped_window(&mut self, _xwm: XwmId, x11: X11Surface) {
        self.forget_x11(&x11);
        if !x11.is_override_redirect() {
            let _ = x11.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, x11: X11Surface) {
        self.forget_x11(&x11);
    }

    /// An X11 window asking for a size. Granted: the size is the client's to
    /// choose. The position is not: that is the window manager's.
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
    }
}

impl Compositor {
    fn forget_x11(&mut self, x11: &X11Surface) {
        if let Some(window) = self.x11_window(x11) {
            self.forget_window(&window);
            self.refocus_after_close(x11.wl_surface().as_ref());
            self.backend.redraw();
            self.publish_facts();
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
