//! `perspicax-shell-v1`: the channel to the desktop shell.
//!
//! What passes between the compositor and `perspicax-shell` that no standard
//! protocol carries. A binding asked for the start menu or the root menu,
//! and the shell is told so, with the monitor under the pointer and, for the
//! root menu, where on it the pointer is: a layer-shell client sees the
//! pointer only while it is over the client's own surfaces. The other way,
//! the shell may end the session when the person chooses to log out.
//!
//! From version 3 a binding may ask for a pie by name, and the shell is told
//! that name and where the pointer is, as for the root menu. A shell bound
//! at an earlier version is not told: an event it does not know would break
//! it, and wayland-backend sends whatever it is asked to.
//!
//! From version 4 a volume key, bound to change the sound, is told to the
//! shell, which holds the sound server's ear: perspicax has no volume of
//! its own. A shell bound earlier is not told, as for a pie.
//!
//! From version 5 the shell may make a window sticky, keep it on top, roll
//! it up or send it to a workspace: what the person chose in its menu. It
//! names the window by the handle its taskbar holds, and the workspace by
//! its pager's, so each names one only while the shell is still admitted to
//! that protocol too. A request says what the window is to be, and a window
//! that is so already is left alone; each goes through what the person's
//! key would, on the tab in front for a tab behind it.
//!
//! And a shell of version 5 is told, of each window it holds a taskbar's
//! handle on, what the person made of it (sticky, on top, rolled up), what
//! can be made of it, and its workspace by the shell's pager's handle, as
//! that changes: so a menu shows the window as it is. Like the layouts it is
//! a difference from what each shell was last told, worked out once a turn
//! of the loop, but only after something it depends on may have changed: a
//! publication of the facts, which follows every change to a window, or a
//! shell, taskbar or pager bound, or the rules changed. What a shell was told
//! is kept with its binding, and goes with it.
//!
//! From version 6 a shell is told what the person asked for that was
//! refused, so it can say why in words: a window's grant to an agent, by
//! the key, with which of the four reasons refused it; a save of the config
//! it cannot use, with why in a line; or a keymap xkb could not compile, by
//! its layout as written. The compositor sends the kind and what the words
//! need of it, never prose of its own: the words are the shell's. What it
//! sends is only ever the person's own words or the config's, never a
//! window's title or a program's name for itself, which clients choose. A
//! shell bound earlier is not told, as for a pie, and the log still says
//! why.
//!
//! From version 7 a shell is told, with the rest of where a window stands,
//! whether it wants the person's attention: it opened without the keys, or
//! its program asked to bring it forward and was refused (#259), and the
//! person has not given it the keys since. A taskbar marks it. A shell
//! bound earlier is never told the bit, as for a pie.
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

use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

use perspicax_node::SurfaceId;
use perspicax_policy::{Home, Protocol, VolumeKey};
use perspicax_protocols::shell::v1::server::perspicax_shell_v1::{self, PerspicaxShellV1};
use smithay::{
    reexports::{
        wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::ClientId,
        },
    },
    utils::Point,
};

use crate::{
    Keymap,
    access::{Filtered, Gate},
    framed::Framed,
    shell::{Fill, id_of, is_on_top, is_shaded, wants_attention},
    state::Compositor,
};

/// The version a shell is told from whether a window wants attention: a
/// bit of `window_state`, for which wayland-scanner makes no constant.
const WANTS_ATTENTION_SINCE: u32 = 7;

/// Which menu a binding asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Menu {
    Start,
    Root,
    /// A pie, by its name in `[shell.pie.menus]`.
    Pie(String),
}

/// Something the person asked for, refused: what a shell is told, so it can
/// say why. Each carries only what its words need, and only words of the
/// person's own or the config's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refused {
    /// A window's grant to an agent: consent for X11 is Xwayland's.
    GrantX11,
    /// A window's grant to an agent: nobody knows which process drew it.
    GrantUnattributed,
    /// A window's grant to an agent: its process could not be held, so its
    /// grant could not end with it.
    GrantUnheld,
    /// A window's grant to an agent: its program is one no agent is ever
    /// given, as `[agent] never` writes it.
    GrantNever(String),
    /// A save of the config, refused whole, the one in use kept: why, in
    /// a line, as the config says it.
    #[cfg_attr(not(feature = "seat"), expect(dead_code, reason = "a seat's reload"))]
    Config(String),
    /// A keymap xkb could not compile, by a save or, headless, a command:
    /// its layout, as written. The keyboard keeps its old one.
    Keymap(String),
}

/// Every shell bound to the channel.
#[derive(Default)]
pub(crate) struct Shells {
    bound: Vec<PerspicaxShellV1>,
    /// The keymap and the layout in use, as the shells were last told them.
    /// `None`: tell them everything.
    told: Option<(Keymap, u32)>,
    /// Something the shells are told of the windows may have changed since
    /// they were: work out what did at the next turn of the loop.
    stale: bool,
}

/// What one shell was last told of each window, by the window's id: kept
/// with its binding, so it goes with it.
#[derive(Default)]
pub(crate) struct WindowsTold(Mutex<BTreeMap<SurfaceId, Standing>>);

/// What a shell is told of a window: by the handle the shell holds on it,
/// what the person made of it, what can be made of it, and the handle the
/// shell holds on its workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Standing {
    toplevel: ZwlrForeignToplevelHandleV1,
    state: perspicax_shell_v1::WindowState,
    capabilities: perspicax_shell_v1::WindowCapabilities,
    workspace: Option<ExtWorkspaceHandleV1>,
}

impl Shells {
    /// Advertise the global, filtered by `gate`.
    pub(crate) fn new(display: &DisplayHandle, gate: &Gate) -> Self {
        display.create_global::<Compositor, PerspicaxShellV1, _>(
            7,
            Filtered {
                gate: gate.clone(),
                protocol: Protocol::Shell,
            },
        );
        Self::default()
    }

    /// Something the shells are told of the windows may have changed: a
    /// window, the handles a taskbar or a pager holds, the rules.
    pub(crate) fn windows_changed(&mut self) {
        self.stale = true;
    }
}

impl Compositor {
    /// A binding pressed a volume key: tell every shell that knows them.
    /// Nothing at the lock screen.
    pub(crate) fn tell_shell_volume(&mut self, key: VolumeKey) {
        if self.lock.is_some() {
            return;
        }
        if self.shells.bound.is_empty() {
            tracing::info!(?key, "a volume key was pressed, and no shell is listening");
            return;
        }
        let key = match key {
            VolumeKey::Up => perspicax_shell_v1::VolumeKey::Up,
            VolumeKey::Down => perspicax_shell_v1::VolumeKey::Down,
            VolumeKey::Mute => perspicax_shell_v1::VolumeKey::Mute,
            VolumeKey::MicMute => perspicax_shell_v1::VolumeKey::MicMute,
        };
        for shell in &self.shells.bound {
            if shell.version() >= perspicax_shell_v1::EVT_VOLUME_SINCE {
                shell.volume(key);
            } else {
                tracing::info!(
                    version = shell.version(),
                    "a volume key was pressed, and this shell is too old to hear of it"
                );
            }
        }
    }

    /// Something the person asked for was refused: tell every shell that
    /// knows refusals, so it can say why. Nothing at the lock screen; the
    /// log, which the caller has written, still says why.
    pub(crate) fn tell_shell_refused(&mut self, refused: Refused) {
        use perspicax_shell_v1::Refusal;
        if self.lock.is_some() {
            return;
        }
        if self.shells.bound.is_empty() {
            tracing::info!(?refused, "refused, and no shell is listening to say so");
            return;
        }
        let (kind, subject) = match refused {
            Refused::GrantX11 => (Refusal::GrantX11, String::new()),
            Refused::GrantUnattributed => (Refusal::GrantUnattributed, String::new()),
            Refused::GrantUnheld => (Refusal::GrantUnheld, String::new()),
            Refused::GrantNever(program) => (Refusal::GrantNever, program),
            Refused::Config(said) => (Refusal::Config, said),
            Refused::Keymap(layout) => (Refusal::Keymap, layout),
        };
        for shell in &self.shells.bound {
            if shell.version() >= perspicax_shell_v1::EVT_REFUSED_SINCE {
                shell.refused(kind, subject.clone());
            } else {
                tracing::info!(
                    version = shell.version(),
                    ?kind,
                    "refused, and this shell is too old to say so"
                );
            }
        }
    }

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
            match &menu {
                Menu::Start => shell.start_menu(wl_output.as_ref()),
                Menu::Root => shell.root_menu(wl_output.as_ref(), local.x, local.y),
                Menu::Pie(name) if shell.version() >= perspicax_shell_v1::EVT_PIE_MENU_SINCE => {
                    shell.pie_menu(wl_output.as_ref(), local.x, local.y, name.clone());
                }
                Menu::Pie(name) => tracing::info!(
                    name,
                    version = shell.version(),
                    "a pie was asked for, and this shell is too old to show one"
                ),
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

    /// Tell every shell of version 5 what changed, of the windows it holds a
    /// taskbar's handle on, since it was last told: what each is made, what
    /// can be made of it and the workspace it is on. Called each turn of the
    /// loop, and does anything only once [`Shells::windows_changed`] said
    /// something may have; nothing while the session is locked, and what
    /// changed meanwhile is told once it is unlocked.
    pub(crate) fn announce_windows(&mut self) {
        if !self.shells.stale || self.lock.is_some() {
            return;
        }
        self.shells.stale = false;
        for shell in &self.shells.bound {
            if shell.version() < perspicax_shell_v1::EVT_TOPLEVEL_STATE_SINCE {
                continue;
            }
            let (Ok(client), Some(told)) = (
                self.display.get_client(shell.id()),
                shell.data::<WindowsTold>(),
            ) else {
                continue;
            };
            // Only objects of the shell's own: wayland-backend refuses, by
            // a panic, to send another client's.
            let now: BTreeMap<SurfaceId, Standing> = self
                .toplevels
                .handles_of(&client)
                .into_iter()
                .filter_map(|(id, toplevel)| {
                    let window = self.any_window(id)?;
                    Some((
                        id,
                        self.standing(&client, shell.version(), toplevel, &window),
                    ))
                })
                .collect();
            let mut told = told.0.lock().unwrap_or_else(PoisonError::into_inner);
            for (id, standing) in &now {
                if told.get(id) != Some(standing) {
                    shell.toplevel_state(
                        &standing.toplevel,
                        standing.state,
                        standing.capabilities,
                        standing.workspace.as_ref(),
                    );
                }
            }
            *told = now;
        }
    }

    /// What `client`, bound at `version` and holding the handle `toplevel`
    /// on `window`, is told of it: its group's, as the tab in front holds
    /// it.
    fn standing(
        &self,
        client: &Client,
        version: u32,
        toplevel: ZwlrForeignToplevelHandleV1,
        window: &Framed,
    ) -> Standing {
        use perspicax_shell_v1::{WindowCapabilities, WindowState};
        let front = self.front_of(window);
        let home = id_of(&front).and_then(|id| self.workspaces.home(id));
        let mut state = WindowState::empty();
        state.set(WindowState::Sticky, home == Some(Home::Sticky));
        state.set(WindowState::OnTop, is_on_top(&front));
        state.set(WindowState::Shaded, is_shaded(&front));
        state.set(
            WindowState::WantsAttention,
            version >= WANTS_ATTENTION_SINCE && wants_attention(&front),
        );
        let mut capabilities = WindowCapabilities::empty();
        capabilities.set(WindowCapabilities::Workspace, home.is_some());
        capabilities.set(
            WindowCapabilities::Shade,
            Self::is_server_decorated(&front) && Self::filling(&front) != Some(Fill::Fullscreen),
        );
        let workspace = match home {
            Some(Home::On(cell)) => self.workspace_held_by(client, &front, cell),
            _ => None,
        };
        Standing {
            toplevel,
            state,
            capabilities,
            workspace,
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

    /// The window a client names by its handle on it from the taskbar's
    /// protocol, while the client is admitted to that protocol: the tab in
    /// front, for a tab behind it. None once the window has closed.
    fn named_window(
        &self,
        client: &Client,
        toplevel: &ZwlrForeignToplevelHandleV1,
    ) -> Option<Framed> {
        if !self
            .gate
            .admits(Protocol::ForeignToplevelManagement, client)
        {
            return None;
        }
        let window = self.any_window(*toplevel.data::<SurfaceId>()?)?;
        Some(self.front_of(&window))
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
        let shell = data_init.init(resource, WindowsTold::default());
        state.shells.bound.push(shell);
        // Every shell is told the layouts again, the new one among them: a
        // list replaces the one before, and a shell binds seldom.
        state.shells.told = None;
        // And the new one the windows it already holds handles on.
        state.shells.windows_changed();
    }

    fn can_view(client: Client, global: &Filtered) -> bool {
        global.admits(&client)
    }
}

impl Dispatch<PerspicaxShellV1, WindowsTold> for Compositor {
    fn request(
        state: &mut Self,
        client: &Client,
        _shell: &PerspicaxShellV1,
        request: perspicax_shell_v1::Request,
        _data: &WindowsTold,
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
            perspicax_shell_v1::Request::SetSticky { toplevel, sticky } if may => {
                if let Some(window) = state.named_window(client, &toplevel) {
                    state.set_sticky(&window, sticky != 0);
                }
            }
            perspicax_shell_v1::Request::SetOnTop { toplevel, on_top } if may => {
                if let Some(window) = state.named_window(client, &toplevel) {
                    state.set_on_top(&window, on_top != 0);
                }
            }
            perspicax_shell_v1::Request::SetShaded { toplevel, shaded } if may => {
                if let Some(window) = state.named_window(client, &toplevel) {
                    state.roll(&window, shaded != 0);
                }
            }
            perspicax_shell_v1::Request::SendToWorkspace {
                toplevel,
                workspace,
            } if may => {
                if let Some(window) = state.named_window(client, &toplevel)
                    && let Some(cell) = state.cell_for(client, &workspace, &window)
                {
                    state.send_window_to(&window, cell);
                }
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        shell: &PerspicaxShellV1,
        _data: &WindowsTold,
    ) {
        state.shells.bound.retain(|held| held != shell);
    }
}
