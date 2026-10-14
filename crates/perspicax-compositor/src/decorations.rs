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
//!
//! Where a person has to agree to what an agent acts on, a window it may act
//! on is marked out ([`Compositor::marked`]): its titlebar in the agent's
//! colours, and a line round it, outside its frame, whoever draws the frame.
//! The line is drawn wherever the window is, in a picture too, and goes out
//! with the frame's facts, for it covers what is under it as a titlebar does.

use std::cell::Cell;

use perspicax_node::Rect as NodeRect;
use perspicax_policy::{Decorations, Insets, Look, fit, frame_rects, outline_rects, outset};
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
    /// `filling`: a window asked to maximize is framed as maximized from
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

    /// Whether `window` is marked out as one an agent may act on: where
    /// consent is by grant, and a grant covers the process that drew it.
    pub(crate) fn marked(&self, window: &Framed) -> bool {
        self.grants.marks(&self.window_origin(window))
    }

    /// How wide the line round `window` is: `[decorations.agent] outline`
    /// for a window [`marked`](Self::marked), and none for any other, or for
    /// one that is fullscreen, which nothing is drawn round.
    ///
    /// What the line is drawn from and what its facts say both ask here,
    /// so a window covers no more and no less than it is drawn with.
    pub(crate) fn outline_of(&self, window: &Framed) -> i32 {
        if Self::look(window) == Look::Fullscreen || !self.marked(window) {
            0
        } else {
            self.backend.decorations().agent.outline
        }
    }

    /// Tell every window whether it is marked out as one an agent may act
    /// on, for drawing it: on a monitor, by [`Self::dress_frames`], and in a
    /// picture, which headless is the only drawing there is.
    #[cfg(any(feature = "seat", feature = "capture"))]
    pub(crate) fn dress_marks(&self) {
        let colour = self.backend.decorations().agent.outline_colour;
        for window in self.space.elements() {
            window.mark(self.insets(window), self.outline_of(window), colour);
        }
    }

    /// Tell every window on screen how it and its frame look, for drawing
    /// them: how far the frame reaches, whether it is the window in use,
    /// whether an agent may act on it, and how opaque the window is.
    ///
    /// In use is where the keys go, through any menu the window opened: a
    /// window whose own menu is open still looks in use, lit and undimmed.
    #[cfg(feature = "seat")]
    pub(crate) fn dress_frames(&self) {
        let decorations = self.backend.decorations();
        let in_use = self.window_in_use();
        for window in self.space.elements() {
            let (colour, ink) = perspicax_policy::frame_colours(
                &decorations,
                in_use.is_some() && crate::shell::id_of(window) == in_use,
                self.marked(window),
            );
            let insets = self.insets(window);
            let size = crate::shell::extent_size(window);
            let client = perspicax_policy::Rect::new(0, 0, size.w, size.h);
            let title_at = perspicax_policy::titlebar(client, insets, &decorations);
            let grip = if Self::look(window) == Look::Normal {
                perspicax_policy::GRIP
            } else {
                0
            };
            window.wear(insets, grip, colour, ink, title_at);
            window.fade(crate::shell::alpha(self.opacity_of(window, in_use)));
        }
        self.dress_marks();
    }

    /// What part of `window`'s frame the pointer at `at` is on, if any.
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
            crate::shell::id_of(window)
                .and_then(|id| self.tabs.tabs(id))
                .map_or(1, <[_]>::len),
        )
    }

    /// What this compositor draws round `window`, in global space, for the
    /// facts: its frame's strips, if it frames it, and the line round it, if
    /// an agent may act on it.
    pub(crate) fn frame_facts(
        &self,
        window: &Framed,
        client: Rectangle<i32, Logical>,
    ) -> Vec<NodeRect> {
        let (client, insets) = (rect(client), self.insets(window));
        frame_rects(client, insets)
            .into_iter()
            .chain(outline_rects(
                outset(client, insets),
                self.outline_of(window),
            ))
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
    ///
    /// Not a window that fills its monitor: a fullscreen one is meant to be
    /// over the panels, and fitting it would push it below them.
    pub(crate) fn fit_frame(&mut self, window: &Framed) {
        if Self::filling(window).is_some() {
            return;
        }
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

    /// The client changed who draws its frame: refit its window to the new
    /// frame.
    fn decoration_changed(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for(surface) else {
            return;
        };
        self.refit(&window);
        self.backend.redraw();
        self.publish_facts();
    }

    /// The `[decorations]` settings changed, or the room panels leave did:
    /// refit every window on screen to the frame and the room it now has. A
    /// window that is off screen is fitted when it comes back.
    pub(crate) fn refit_frames(&mut self) {
        let windows: Vec<Framed> = self.space.elements().cloned().collect();
        for window in &windows {
            self.refit(window);
        }
        self.backend.redraw();
        self.publish_facts();
    }

    /// Give a window that fills a monitor or a zone the size that leaves
    /// room for its frame and the panels as they are now, and keep any other
    /// one's titlebar on screen. Only for a window on screen: a parked one
    /// is refitted when it comes back.
    pub(crate) fn refit(&mut self, window: &Framed) {
        if self.space.element_location(window).is_none() {
            return;
        }
        let window = window.clone();
        if let Some(fill) = Self::filling(&window) {
            self.fill(&window, fill, None);
        } else if let Some(zone) = placement(&window, |placement| placement.snapped) {
            self.snap(&window, zone, None);
        } else {
            self.fit_frame(&window);
        }
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
