//! What a binding does, carried out: the same code whichever backend is
//! running and whoever asked, a person's keys or mouse on a seat or a
//! [`crate::Command::Perform`] from a test.
//!
//! Two actions need the seat, and say so in the log rather than pretending:
//! starting a program for the person, and re-reading the person's config.
//! Headless has neither a person nor a config.

use perspicax_node::SurfaceId;
use perspicax_policy::{Action, Change, Decision, cycle};
use smithay::{
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel, utils::SERIAL_COUNTER,
};

use super::{id_of, surface_of};
use crate::{framed::Framed, state::Compositor};

impl Compositor {
    /// Carry out a key binding, on the monitor the person is working on.
    pub(crate) fn perform(&mut self, action: &Action) {
        self.perform_on(action, self.working_monitor());
    }

    /// Carry out a binding, switching workspaces on `monitor`. A key binding
    /// acts where the person is working, a mouse binding where the hand is:
    /// a click on the desktop of one monitor switches that one, wherever the
    /// focused window is.
    pub(crate) fn perform_on(&mut self, action: &Action, monitor: Option<String>) {
        let focused = self.focused_surface().and_then(|id| self.window_for_id(id));
        match action {
            Action::Close => {
                if let Some(window) = focused {
                    Self::close(&window);
                }
            }
            Action::ToggleMaximize => {
                if let Some(window) = focused {
                    self.toggle_maximize(&window);
                }
            }
            Action::CycleTab { forward } => self.cycle_tab(*forward),
            Action::TabWithPrevious => self.tab_with_previous(),
            Action::DetachTab => self.detach_tab(),
            Action::Minimize => {
                if let Some(window) = focused {
                    self.minimize(&window);
                }
            }
            Action::Spawn(command) => self.spawn_for_person(command),
            Action::MoveToOutput(towards) => {
                if let Some(window) = focused {
                    self.move_to_output(&window, *towards);
                }
            }
            Action::Workspace(direction) => self.switch_workspace(monitor, *direction),
            Action::GoToWorkspace(number) => self.go_to_workspace(monitor, *number),
            Action::CycleWorkspace { forward } => {
                if let Some(monitor) = monitor {
                    self.scroll_workspace(&monitor, *forward);
                }
            }
            Action::SendToWorkspace(direction) => self.send_to_workspace(*direction, false),
            Action::CarryToWorkspace(direction) => self.send_to_workspace(*direction, true),
            Action::ToggleSticky => self.toggle_sticky(),
            Action::Snap(direction) => self.snap_focused(*direction),
            Action::Reload => self.reload(),
            Action::StartMenu => self.ask_shell(crate::shell_protocol::Menu::Start),
            Action::RootMenu => self.ask_shell(crate::shell_protocol::Menu::Root),
            Action::Pie(name) => self.ask_shell(crate::shell_protocol::Menu::Pie(name.clone())),
            Action::CycleLayout { forward } => self.cycle_layout(*forward),
            Action::Layout(number) => self.go_to_layout(*number),
            Action::CycleFocus => {
                // Minimized windows on this workspace count as below the
                // bottom of the stack, so cycling reaches them first and
                // brings them back: without a taskbar (W4), this is how a
                // minimized window returns. Windows on other workspaces are
                // not in the cycle, as they are not in Plasma's.
                let stack: Vec<SurfaceId> = self
                    .parked
                    .iter()
                    .filter(|window| {
                        Self::is_minimized(window) && self.on_showing_workspace(window)
                    })
                    .chain(self.space.elements())
                    .filter_map(id_of)
                    .collect();
                if let Some(next) = cycle(&stack, self.focused_surface()) {
                    self.apply_focus(Decision {
                        focus: Change::To(next),
                        raise: Some(next),
                    });
                }
            }
        }
    }

    /// Ask a window to close. A request the client may decline, never a
    /// kill.
    pub(crate) fn close(window: &Framed) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.send_close();
        }
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            let _ = x11.close();
        }
    }

    /// Maximize a window, or put a maximized one back where it was. An X11
    /// window has no xdg state to carry it, so it is snapped to the whole
    /// monitor instead, which is the same rect by the same path.
    pub(crate) fn toggle_maximize(&mut self, window: &Framed) {
        if let Some(toplevel) = window.toplevel().cloned() {
            let maximized = toplevel.with_pending_state(|pending| {
                pending.states.contains(xdg_toplevel::State::Maximized)
            });
            if maximized {
                self.unfill(window, super::Fill::Maximized, None);
            } else {
                self.fill(window, super::Fill::Maximized, None);
            }
        } else if Self::is_snapped(window) {
            self.unsnap(window, None);
        } else {
            self.snap(window, perspicax_policy::Zone::Top, None);
        }
        self.backend.redraw();
        self.publish_facts();
    }

    /// Carry out a focus decision. A parked window being focused or raised
    /// is restored first, which shows its workspace if that is not showing.
    pub(crate) fn apply_focus(&mut self, decision: Decision<SurfaceId>) {
        if decision == Decision::none() {
            return;
        }
        let targets = [
            match decision.focus {
                Change::To(id) => Some(id),
                _ => None,
            },
            decision.raise,
        ];
        let parked: Vec<_> = targets
            .into_iter()
            .flatten()
            .filter_map(|id| self.parked_with(id))
            .collect();
        for window in parked {
            self.restore(&window);
        }
        match decision.focus {
            Change::Keep => {}
            Change::To(id) => {
                let surface = self
                    .window_for_id(id)
                    .and_then(|window| surface_of(&window));
                if let Some(surface) = surface {
                    self.focus_surface(surface, id);
                }
            }
            Change::Clear => {
                if let Some(keyboard) = self.keyboard.clone() {
                    keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
                }
            }
        }
        if let Some(window) = decision.raise.and_then(|id| self.window_for_id(id)) {
            self.space.raise_element(&window, false);
            self.backend.redraw();
        }
        // Focus and stacking are both facts the index judges against.
        self.publish_facts();
    }

    fn spawn_for_person(&mut self, command: &[String]) {
        #[cfg(feature = "seat")]
        if let crate::backend::Running::Seat(session) = &mut self.backend {
            let launch = self.launch.clone();
            session.spawn(launch.as_ref(), command);
            return;
        }
        tracing::warn!(
            ?command,
            "a binding's spawn is for a person's session; headless has none"
        );
    }

    fn reload(&mut self) {
        #[cfg(feature = "seat")]
        if matches!(self.backend, crate::backend::Running::Seat(_)) {
            crate::backend::seat::reload(self);
            return;
        }
        tracing::warn!("headless reads no config, so there is nothing to reload");
    }
}
