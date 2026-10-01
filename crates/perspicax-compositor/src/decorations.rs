//! Who draws a window's frame, and where this compositor's frame is.
//!
//! Two protocols ask the question. `xdg-decoration` is the standard one and
//! what Qt, foot and most newer clients speak. KDE's `server-decoration` is
//! older and is what GTK 3 speaks: with it offered and defaulting to the
//! server, a GTK 3 application without a headerbar stops drawing its own
//! frame. GTK 4 speaks neither and always draws its own. A client that
//! speaks neither, which includes every test client here, gets no frame.
//!
//! The answer is kept in one place for both protocols, the surface's own data
//! (KDE names a surface, not a toplevel, and may do so before the surface has
//! a role), and [`Compositor::insets`] is the only thing that reads it.
//!
//! An X11 window is framed unless its Motif hints say it draws its own,
//! which is what `X11Surface::is_decorated` means. Override-redirect windows
//! (menus, tooltips) are never framed.
//!
//! Where a frame goes is pure policy (`perspicax_policy::frame`). Its pixels
//! are drawn by [`crate::framed::Framed`] on the seat. Its facts go out with
//! each window's, so the index can see that a titlebar covers what is under it.

use std::cell::Cell;

use perspicax_node::Rect as NodeRect;
use perspicax_policy::{Decorations, Insets, Look, fit, frame_rects};
use smithay::{
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgMode,
            shell::server::xdg_toplevel,
        },
        wayland_protocols_misc::server_decoration::server::{
            org_kde_kwin_server_decoration::{Mode as KdeMode, OrgKdeKwinServerDecoration},
            org_kde_kwin_server_decoration_manager::Mode as KdeDefault,
        },
        wayland_server::{WEnum, protocol::wl_surface::WlSurface},
    },
    utils::{Logical, Rectangle},
    wayland::{
        compositor::with_states,
        shell::{
            kde::decoration::{KdeDecorationHandler, KdeDecorationState},
            xdg::{
                ToplevelSurface,
                decoration::{XdgDecorationHandler, XdgDecorationState},
            },
        },
    },
};

use crate::{
    framed::Framed,
    shell::{placement, rect},
    state::Compositor,
};

/// This compositor draws the surface's frame. Absent means the client draws
/// its own, or none: the default, for a client that never asked.
struct ServerSide(Cell<bool>);

fn set_server_side(surface: &WlSurface, server: bool) {
    with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing(|| ServerSide(Cell::new(server)));
        if let Some(flag) = states.data_map.get::<ServerSide>() {
            flag.0.set(server);
        }
    });
}

fn is_server_side(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<ServerSide>()
            .is_some_and(|flag| flag.0.get())
    })
}

/// The protocols' globals, created once with the compositor.
pub(crate) struct DecorationStates {
    /// Held for the life of the compositor; its handler needs no access.
    _xdg: XdgDecorationState,
    pub(crate) kde: KdeDecorationState,
}

impl DecorationStates {
    pub(crate) fn new(
        display: &smithay::reexports::wayland_server::DisplayHandle,
        decorations: &Decorations,
    ) -> Self {
        // KDE announces its default once, when a client binds. A reload that
        // changes `mode` reaches xdg-decoration clients as they negotiate, and
        // KDE ones when they next start.
        let default = if decorations.server {
            KdeDefault::Server
        } else {
            KdeDefault::Client
        };
        Self {
            _xdg: XdgDecorationState::new::<Compositor>(display),
            kde: KdeDecorationState::new::<Compositor>(display, default),
        }
    }
}

impl Compositor {
    /// Whether this compositor draws `window`'s frame.
    pub(crate) fn is_server_decorated(window: &Framed) -> bool {
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            return !x11.is_override_redirect() && !x11.is_decorated();
        }
        window
            .toplevel()
            .is_some_and(|toplevel| is_server_side(toplevel.wl_surface()))
    }

    /// How far `window`'s frame reaches past its client geometry, as it is
    /// now.
    pub(crate) fn insets(&self, window: &Framed) -> Insets {
        self.insets_as(window, Self::look(window))
    }

    /// How far `window`'s frame would reach in `look`: what a window about to
    /// be maximized has to leave room for.
    ///
    /// A window that draws its own frame has none of ours, unless it is a
    /// tab: then it has a strip of tabs above it, as tall as a titlebar, so
    /// every tab of a group starts at the same height.
    pub(crate) fn insets_as(&self, window: &Framed, look: Look) -> Insets {
        let decorations = self.backend.decorations();
        if Self::is_server_decorated(window) {
            Insets::of(&decorations, look)
        } else if look != Look::Fullscreen && self.is_tabbed(window) {
            Insets {
                top: decorations.title,
                ..Insets::NONE
            }
        } else {
            Insets::NONE
        }
    }

    /// Which state `window` is in, as far as its frame cares. Pending, like
    /// `is_filling`: a window asked to maximize is framed as maximized from
    /// the moment it is asked, so the size it is asked for and the frame it
    /// gets agree.
    fn look(window: &Framed) -> Look {
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            return if x11.is_fullscreen() {
                Look::Fullscreen
            } else if x11.is_maximized() {
                Look::Maximized
            } else {
                Look::Normal
            };
        }
        window.toplevel().map_or(Look::Normal, |toplevel| {
            toplevel.with_pending_state(|pending| {
                if pending.states.contains(xdg_toplevel::State::Fullscreen) {
                    Look::Fullscreen
                } else if pending.states.contains(xdg_toplevel::State::Maximized) {
                    Look::Maximized
                } else {
                    Look::Normal
                }
            })
        })
    }

    /// The title a window's client set, if it set one.
    #[cfg(feature = "seat")]
    pub(crate) fn window_title(window: &Framed) -> Option<String> {
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            return Some(x11.title()).filter(|title| !title.is_empty());
        }
        crate::facts::title(window.toplevel()?.wl_surface())
    }

    /// Tell every window on screen how its frame looks, for drawing it:
    /// how far it reaches, and whether it is the one with the keyboard.
    #[cfg(feature = "seat")]
    pub(crate) fn dress_frames(&self) {
        let decorations = self.backend.decorations();
        let focused = self.focused_surface();
        for window in self.space.elements() {
            let colour = if focused.is_some() && crate::shell::id_of(window) == focused {
                decorations.focused
            } else {
                decorations.unfocused
            };
            let insets = self.insets(window);
            let size = crate::shell::extent_size(window);
            let client = perspicax_policy::Rect::new(0, 0, size.w, size.h);
            let title_at = perspicax_policy::titlebar(client, insets, &decorations);
            let grip = if Self::look(window) == Look::Normal {
                perspicax_policy::GRIP
            } else {
                0
            };
            window.wear(insets, grip, colour, title_at);
        }
    }

    /// What part of `window`'s frame the pointer at `at` is on, if any.
    #[cfg(feature = "seat")]
    pub(crate) fn frame_part(
        &self,
        window: &Framed,
        at: smithay::utils::Point<f64, Logical>,
    ) -> Option<perspicax_policy::Part> {
        let client = self.extent(window)?;
        perspicax_policy::part_at(
            (at.x, at.y),
            rect(client),
            self.insets(window),
            &self.backend.decorations(),
            Self::look(window) == Look::Normal,
        )
    }

    /// The frame's strips in global space, for the facts: nothing for a
    /// window that is not on screen, or not framed by us.
    pub(crate) fn frame_facts(
        &self,
        window: &Framed,
        client: Rectangle<i32, Logical>,
    ) -> Vec<NodeRect> {
        frame_rects(rect(client), self.insets(window))
            .into_iter()
            .map(|strip| {
                NodeRect::new(
                    f64::from(strip.x),
                    f64::from(strip.y),
                    f64::from(strip.x + strip.w),
                    f64::from(strip.y + strip.h),
                )
            })
            .collect()
    }

    /// Move `window` so the top and left of its frame are on its monitor's
    /// free area. A window is placed before its client says who draws the
    /// frame, so a frame that arrives afterwards may start above the screen,
    /// where its titlebar could not be grabbed.
    pub(crate) fn fit_frame(&mut self, window: &Framed) {
        let insets = self.insets(window);
        let (Some(location), Some(area)) = (
            self.space.element_location(window),
            self.output_of(window)
                .and_then(|output| self.usable_area(&output)),
        ) else {
            return;
        };
        let to = fit((location.x, location.y), insets, rect(area));
        if to == (location.x, location.y) {
            return;
        }
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            let _ = x11.configure(Rectangle::new(to.into(), x11.geometry().size));
        }
        self.space.map_element(window.clone(), to, false);
    }

    /// The client changed who draws its frame: give a window that fills a
    /// zone or a monitor the size that leaves room for the new frame, and
    /// keep any other one's titlebar on screen.
    fn decoration_changed(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for(surface) else {
            return;
        };
        let zone = placement(&window, |placement| placement.snapped);
        let toplevel = window.toplevel().cloned();
        let maximized = toplevel.as_ref().is_some_and(|toplevel| {
            toplevel.with_pending_state(|pending| {
                pending.states.contains(xdg_toplevel::State::Maximized)
            })
        });
        if let (Some(zone), true) = (zone, self.space.element_location(&window).is_some()) {
            self.snap(&window, zone, None);
        } else if let (Some(toplevel), true) = (toplevel, maximized) {
            self.fill(&toplevel, xdg_toplevel::State::Maximized, None);
        } else {
            self.fit_frame(&window);
        }
        self.backend.redraw();
        self.publish_facts();
    }

    /// The mode to answer a client with, given what it asked for: what it
    /// asked for, unless this compositor has been told never to draw frames.
    fn xdg_mode(&self, asked: Option<XdgMode>) -> XdgMode {
        match (self.backend.decorations().server, asked) {
            (false, _) | (true, Some(XdgMode::ClientSide)) => XdgMode::ClientSide,
            (true, _) => XdgMode::ServerSide,
        }
    }

    fn answer_xdg(&mut self, toplevel: &ToplevelSurface, asked: Option<XdgMode>) {
        let mode = self.xdg_mode(asked);
        toplevel.with_pending_state(|pending| pending.decoration_mode = Some(mode));
        set_server_side(toplevel.wl_surface(), mode == XdgMode::ServerSide);
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        self.decoration_changed(&toplevel.wl_surface().clone());
    }
}

impl XdgDecorationHandler for Compositor {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.answer_xdg(&toplevel, None);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: XdgMode) {
        self.answer_xdg(&toplevel, Some(mode));
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.answer_xdg(&toplevel, None);
    }
}

impl KdeDecorationHandler for Compositor {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.decorations.kde
    }

    fn new_decoration(&mut self, surface: &WlSurface, _decoration: &OrgKdeKwinServerDecoration) {
        set_server_side(surface, self.backend.decorations().server);
        self.decoration_changed(surface);
    }

    fn request_mode(
        &mut self,
        surface: &WlSurface,
        decoration: &OrgKdeKwinServerDecoration,
        mode: WEnum<KdeMode>,
    ) {
        let asked = match mode {
            WEnum::Value(KdeMode::Client | KdeMode::None) => Some(XdgMode::ClientSide),
            WEnum::Value(KdeMode::Server) => Some(XdgMode::ServerSide),
            _ => None,
        };
        let server = self.xdg_mode(asked) == XdgMode::ServerSide;
        decoration.mode(if server {
            KdeMode::Server
        } else {
            KdeMode::Client
        });
        set_server_side(surface, server);
        self.decoration_changed(surface);
    }

    fn release(&mut self, _decoration: &OrgKdeKwinServerDecoration, surface: &WlSurface) {
        set_server_side(surface, false);
        self.decoration_changed(surface);
    }
}

smithay::delegate_xdg_decoration!(Compositor);
smithay::delegate_kde_decoration!(Compositor);
