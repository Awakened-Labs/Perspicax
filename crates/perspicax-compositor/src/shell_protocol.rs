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
//! From version 2 a shell is also told the keyboard's layouts and which is
//! in use, for a panel's indicator, and may switch it. What it was last told
//! is kept, and each turn of the loop it is told only what changed since: a
//! switch from any source, a key, a binding, a window taking the keyboard
//! or a new keymap, reaches it without each having to say so. An agent
//! typing in another layout switches and switches back within one turn, so
//! the indicator never shows it.
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
    Keymap,
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
    /// The keymap and the layout in use, as the shells were last told them.
    /// `None`: tell them everything.
    told: Option<(Keymap, u32)>,
}

impl Shells {
    /// Advertise the global, filtered by `gate`.
    pub(crate) fn new(display: &DisplayHandle, gate: &Gate) -> Self {
        display.create_global::<Compositor, PerspicaxShellV1, _>(
            2,
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

    /// The config's `[shell]` table changed, or its `[protocols]` rules did:
    /// tell every shell to read the file again, and to take back any
    /// protocol the rules give back to it.
    pub(crate) fn reconfigure_shells(&self) {
        for shell in &self.shells.bound {
            shell.reconfigure();
        }
    }

    /// Tell every shell of version 2 what changed of the keyboard's layouts
    /// since it was last told: all of them when the keymap changed, then the
    /// one in use. Called each turn of the loop; nothing while the session
    /// is locked, and what changed meanwhile is told once it is unlocked.
    pub(crate) fn announce_layout(&mut self) {
        let listening = |shell: &&PerspicaxShellV1| {
            shell.version() >= perspicax_shell_v1::EVT_ACTIVE_LAYOUT_SINCE
        };
        if self.lock.is_some() || !self.shells.bound.iter().any(|shell| listening(&shell)) {
            return;
        }
        let Some((active, _)) = self.layouts() else {
            return;
        };
        let keymap_told = self
            .shells
            .told
            .as_ref()
            .is_some_and(|(keymap, _)| *keymap == self.keymap);
        if keymap_told && self.shells.told.as_ref().map(|&(_, told)| told) == Some(active) {
            return;
        }
        let layouts = if keymap_told {
            Vec::new()
        } else {
            self.layout_labels()
        };
        for shell in self.shells.bound.iter().filter(listening) {
            if !keymap_told {
                for (index, (name, short)) in (0..).zip(&layouts) {
                    shell.layout(index, name.clone(), short.clone());
                }
                shell.layouts_done();
            }
            shell.active_layout(active);
        }
        self.shells.told = Some((self.keymap.clone(), active));
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
        // Every shell is told the layouts again, the new one among them: a
        // list replaces the one before, and a shell binds seldom.
        state.shells.told = None;
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
        // What destroy ends is `destroyed`'s to forget.
        let may = state.lock.is_none() && state.gate.admits(Protocol::Shell, client);
        match request {
            perspicax_shell_v1::Request::ExitSession if may => {
                tracing::info!("the shell asked to end the session");
                state.exit_asked = true;
            }
            perspicax_shell_v1::Request::ExitSession => {
                tracing::warn!("the shell asked to end the session, and may not now");
            }
            perspicax_shell_v1::Request::SetLayout { index } if may => {
                if state.layouts().is_some_and(|(_, count)| index < count) {
                    state.lock_layout(index);
                }
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, shell: &PerspicaxShellV1, _data: &()) {
        state.shells.bound.retain(|held| held != shell);
    }
}
