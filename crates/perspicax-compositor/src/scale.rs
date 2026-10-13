//! Drawing at a scale that is not a whole number.
//!
//! A monitor at a scale of 0.9 or 1.25 is, to a client, a logical size
//! that is not its mode's: `xdg_output` says so. A client that wants sharp
//! pixels there draws its buffer that many times its window's size, and
//! says what size the buffer is shown at, with `wp_viewporter`. One that
//! cannot say so is drawn one logical pixel to each of its buffer's, which
//! shrinks it by the scale a second time. Firefox takes its scale from the
//! logical size whether or not it can say so, and on a monitor at 0.9 it
//! drew at 0.81 of its window, with its clicks landing where it laid things
//! out rather than where it drew them (issue #135).
//!
//! What size a surface is shown at is read where it is measured
//! ([`geometry`](crate::geometry)), as smithay's renderer reads it to draw
//! the surface and to find what is under the pointer.
//!
//! # Which scale
//!
//! Firefox guesses its scale from the monitor's logical size. GTK 4, Qt 6
//! and Chromium do not guess: they draw at a scale that is not a whole
//! number only when told it with `wp_fractional_scale_v1`, and otherwise
//! draw at 1 and are scaled down, the right size but soft. Each surface is
//! told the scale of the monitor its window is mostly on, a panel or a lock
//! screen that of its own monitor, and told again whenever that changes:
//! the window moved, or the monitor was given another scale. Asked of
//! every surface at each frame, as smithay is asked which monitors each is
//! on, so it follows whatever moved them; smithay sends a scale only when
//! it changed.
//!
//! # Not Xwayland
//!
//! Xwayland shows an X game that changed the screen's resolution through a
//! viewport: the game's window, at the size it asked for, scaled to the
//! monitor. Where an X window is and how big comes from the X server, here
//! and in smithay's search for what is under the pointer, so neither would
//! follow it. Xwayland is not offered `wp_viewporter`, and X games keep
//! the resolution they have.

use smithay::{
    delegate_fractional_scale,
    desktop::{layer_map_for_output, utils::with_surfaces_surface_tree},
    reexports::{
        wayland_protocols::wp::viewporter::server::{
            wp_viewport::WpViewport, wp_viewporter::WpViewporter,
        },
        wayland_server::{
            Client, DataInit, DisplayHandle, GlobalDispatch, New, backend::GlobalId,
            delegate_dispatch, protocol::wl_surface::WlSurface,
        },
    },
    wayland::{
        compositor::{SurfaceData, with_states},
        fractional_scale::{FractionalScaleHandler, with_fractional_scale},
        viewporter::{ViewportState, ViewporterState},
    },
};

use crate::state::Compositor;

/// Offer `wp_viewporter`, to every client but Xwayland.
///
/// Smithay's own `ViewporterState::new` offers it to every client, so the
/// global is made here, and smithay answers what is asked of it.
pub(crate) fn offer_viewporter(display: &DisplayHandle) -> GlobalId {
    display.create_global::<Compositor, WpViewporter, ()>(1, ())
}

impl GlobalDispatch<WpViewporter, ()> for Compositor {
    fn bind(
        _state: &mut Self,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<WpViewporter>,
        _global: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _global: &()) -> bool {
        !is_xwayland(&client)
    }
}

delegate_dispatch!(Compositor: [WpViewporter: ()] => ViewporterState);
delegate_dispatch!(Compositor: [WpViewport: ViewportState] => ViewportState);

impl Compositor {
    /// Tell every surface the scale of the monitor it is on: a window's,
    /// with its subsurfaces and menus, that of the monitor it is mostly on;
    /// a panel's and a lock screen's, that of their own.
    pub(crate) fn settle_scales(&self) {
        for window in self.space.elements() {
            if let Some(output) = self.output_of(window) {
                let scale = output.current_scale().fractional_scale();
                window.with_surfaces(|_, states| ask(states, scale));
            }
        }
        // Gathered first, so no layer map is held while a surface is.
        let layers: Vec<_> = self
            .space
            .outputs()
            .flat_map(|output| {
                let scale = output.current_scale().fractional_scale();
                layer_map_for_output(output)
                    .layers()
                    .map(|layer| (layer.clone(), scale))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (layer, scale) in layers {
            layer.with_surfaces(|_, states| ask(states, scale));
        }
        for cover in self.lock.iter().flat_map(|locked| &locked.surfaces) {
            let scale = cover.output.current_scale().fractional_scale();
            with_surfaces_surface_tree(cover.surface.wl_surface(), |_, states| {
                ask(states, scale);
            });
        }
    }
}

impl FractionalScaleHandler for Compositor {
    /// A surface on a monitor already is told its scale as it asks, by
    /// smithay. One on none yet -- a window asking before it has drawn, as
    /// toolkits do -- is told the scale of the monitor a new window opens
    /// on, which is the pointer's.
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        self.settle_scales();
        let told = with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.preferred_scale().is_some())
        });
        if !told && let Some(output) = self.output_under_pointer() {
            let scale = output.current_scale().fractional_scale();
            with_states(&surface, |states| ask(states, scale));
        }
    }
}

delegate_fractional_scale!(Compositor);

/// Tell the surface whose states these are to draw at `scale`. Said only if
/// it changed, and kept for an object the client has yet to ask for.
fn ask(states: &SurfaceData, scale: f64) {
    with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
}

/// Whether `client` is the Xwayland this compositor started: the one client
/// carrying smithay's data rather than ours.
fn is_xwayland(client: &Client) -> bool {
    #[cfg(feature = "xwayland")]
    return client
        .get_data::<smithay::xwayland::XWaylandClientData>()
        .is_some();
    #[cfg(not(feature = "xwayland"))]
    {
        let _ = client;
        false
    }
}
