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
//! # Not Xwayland
//!
//! Xwayland shows an X game that changed the screen's resolution through a
//! viewport: the game's window, at the size it asked for, scaled to the
//! monitor. Where an X window is and how big comes from the X server, here
//! and in smithay's search for what is under the pointer, so neither would
//! follow it. Xwayland is not offered `wp_viewporter`, and X games keep
//! the resolution they have.

use smithay::{
    reexports::{
        wayland_protocols::wp::viewporter::server::{
            wp_viewport::WpViewport, wp_viewporter::WpViewporter,
        },
        wayland_server::{
            Client, DataInit, DisplayHandle, GlobalDispatch, New, backend::GlobalId,
            delegate_dispatch,
        },
    },
    wayland::viewporter::{ViewportState, ViewporterState},
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
