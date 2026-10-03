//! `perspicax-shell-v1`: the channel to the desktop shell.
//!
//! What passes between the compositor and `perspicax-shell` that no standard
//! protocol carries. A binding asked for the start menu or the root menu,
//! and the shell is told so, with the monitor under the pointer and, for the
//! root menu, where on it the pointer is: a layer-shell client sees the
//! pointer only while it is over the client's own surfaces. The other way,
//! the shell may end the session when the person chooses to log out.
//!
//! When a save changes the config's `[shell]` table, every shell is told to
//! read it again, and applies it in place.
//!
//! Gated like the taskbar protocols: offered only to the programs the
//! `[protocols] shell` rule admits, withdrawn with `finished` from a client
//! a reload no longer admits, and silent while the session is locked. A
//! request from a client no longer admitted, or made while locked, is
//! ignored.

use perspicax_policy::Protocol;
use perspicax_protocols::shell::v1::server::perspicax_shell_v1::{self, PerspicaxShellV1};
use smithay::{
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::ClientId,
    },
    utils::Point,
};

use crate::{
    access::{Filtered, Gate},
    state::Compositor,
};

/// Which menu a binding asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Menu {
    Start,
    Root,
}

/// Every shell bound to the channel.
#[derive(Default)]
pub(crate) struct Shells {
    bound: Vec<PerspicaxShellV1>,
}

impl Shells {
    /// Advertise the global, filtered by `gate`.
    pub(crate) fn new(display: &DisplayHandle, gate: &Gate) -> Self {
        display.create_global::<Compositor, PerspicaxShellV1, _>(
            1,
            Filtered {
                gate: gate.clone(),
                protocol: Protocol::Shell,
            },
        );
        Self::default()
    }
}

impl Compositor {
    /// A binding asked for a menu: tell every shell, on the monitor under
    /// the pointer. Nothing while the session is locked.
    pub(crate) fn ask_shell(&mut self, menu: Menu) {
        if self.lock.is_some() {
            return;
        }
        if self.shells.bound.is_empty() {
            tracing::info!(?menu, "a menu was asked for, and no shell is listening");
            return;
        }
        let at = self
            .pointer
            .as_ref()
            .map_or_else(Point::default, |pointer| pointer.current_location());
        let output = self.output_under_pointer();
        let local = output
            .as_ref()
            .and_then(|output| self.space.output_geometry(output))
            .map_or_else(Point::default, |geometry| {
                (at - geometry.loc.to_f64()).to_i32_round()
            });
        for shell in &self.shells.bound {
            let Ok(client) = self.display.get_client(shell.id()) else {
                continue;
            };
            let wl_output = output
                .as_ref()
                .and_then(|output| output.client_outputs(&client).next());
            match menu {
                Menu::Start => shell.start_menu(wl_output.as_ref()),
                Menu::Root => shell.root_menu(wl_output.as_ref(), local.x, local.y),
            }
        }
    }

    /// The config's `[shell]` table changed: tell every shell to read the
    /// file again.
    pub(crate) fn reconfigure_shells(&self) {
        for shell in &self.shells.bound {
            shell.reconfigure();
        }
    }

    /// Whether process `pid` holds the channel, and so hears a
    /// `reconfigure`.
    #[cfg_attr(not(feature = "seat"), expect(dead_code, reason = "the seat's"))]
    pub(crate) fn shell_listening(&self, pid: u32) -> bool {
        self.shells.bound.iter().any(|shell| {
            self.display
                .get_client(shell.id())
                .and_then(|client| client.get_credentials(&self.display))
                .is_ok_and(|credentials| u32::try_from(credentials.pid) == Ok(pid))
        })
    }

    /// Withdraw the channel from every shell the rules no longer admit.
    pub(crate) fn revoke_shell(&mut self) {
        let gate = self.gate.clone();
        let display = self.display.clone();
        self.shells.bound.retain(|shell| {
            let admitted = display
                .get_client(shell.id())
                .is_ok_and(|client| gate.admits(Protocol::Shell, &client));
            if !admitted {
                shell.finished();
                tracing::info!(
                    "perspicax-shell-v1 withdrawn from a client [protocols] no longer admits"
                );
            }
            admitted
        });
    }
}

impl GlobalDispatch<PerspicaxShellV1, Filtered> for Compositor {
    fn bind(
        state: &mut Self,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<PerspicaxShellV1>,
        _global: &Filtered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let shell = data_init.init(resource, ());
        state.shells.bound.push(shell);
    }

    fn can_view(client: Client, global: &Filtered) -> bool {
        global.admits(&client)
    }
}

impl Dispatch<PerspicaxShellV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        client: &Client,
        _shell: &PerspicaxShellV1,
        request: perspicax_shell_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Destroy is the only other request, and what it ends is
        // `destroyed`'s to forget.
        if let perspicax_shell_v1::Request::ExitSession = request {
            if state.lock.is_some() || !state.gate.admits(Protocol::Shell, client) {
                tracing::warn!("the shell asked to end the session, and may not now");
                return;
            }
            tracing::info!("the shell asked to end the session");
            state.exit_asked = true;
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, shell: &PerspicaxShellV1, _data: &()) {
        state.shells.bound.retain(|held| held != shell);
    }
}
