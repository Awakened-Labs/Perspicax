//! The sizes a client sends, checked before smithay 0.7 reads them.
//!
//! Smithay's `Size` asserts in a debug build that neither of its sides is
//! negative, and smithay 0.7.0 builds one straight from what a client sent
//! in several requests. One such request, from any client -- a test's, a
//! real one with a bug, or a hostile one -- panicked a debug build, which
//! every `cargo run` and every test run is, and ended the session (issue
//! #106). A release build took the values without complaint, as rectangles
//! turned inside out.
//!
//! So those requests are asked here first, and smithay answers only what
//! passes. What doesn't is refused as the protocol says where it says, and
//! as smithay's own master branch does where it does not:
//!
//! | Request | Refused when | As |
//! |---|---|---|
//! | `wl_surface.damage` | a side is negative | dropped, as 0.7.0 drops `damage_buffer`'s |
//! | `wl_region.add`, `subtract` | a side is not above 0 | dropped, as master drops it |
//! | `xdg_surface.set_window_geometry` | a side is not above 0 | xdg-shell's `invalid_size` |
//! | `xdg_toplevel.set_min_size`, `set_max_size` | a side is negative | xdg-shell's `invalid_size` |
//!
//! A refusal with a protocol error ends the client, which is what the
//! protocol asks; a geometry without area, accepted until #106, is refused
//! in a release build too.
//!
//! # When to delete this
//!
//! Each check goes once a smithay release carries its own, and the
//! `delegate_*!` macro its section stands in for comes back in its place.
//! Smithay master has the region, geometry and min/max checks; the damage
//! check is in a pull request (Smithay/smithay#2200).

use smithay::{
    reexports::{
        wayland_protocols::xdg::shell::server::{
            xdg_popup::XdgPopup,
            xdg_positioner::XdgPositioner,
            xdg_surface::{self, XdgSurface},
            xdg_toplevel::{self, XdgToplevel},
            xdg_wm_base::XdgWmBase,
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, Resource,
            backend::ClientId,
            delegate_dispatch, delegate_global_dispatch,
            protocol::{
                wl_callback::WlCallback,
                wl_compositor::WlCompositor,
                wl_region::{self, WlRegion},
                wl_subcompositor::WlSubcompositor,
                wl_subsurface::WlSubsurface,
                wl_surface::{self, WlSurface},
            },
        },
    },
    wayland::{
        compositor::{CompositorState, RegionUserData, SubsurfaceUserData, SurfaceUserData},
        shell::xdg::{
            XdgPositionerUserData, XdgShellState, XdgShellSurfaceUserData, XdgSurfaceUserData,
            XdgWmBaseUserData,
        },
    },
};

use crate::state::Compositor;

/// `delegate_dispatch!`, with a check first: the compositor's
/// `Dispatch<$interface, $data>`, answered by smithay's `$smithay` unless
/// `$refused` refuses the request. A check that refuses has already posted
/// the protocol's error, where the protocol names one.
macro_rules! checked {
    ($interface:ty: $data:ty => $smithay:ty, $refused:path) => {
        impl Dispatch<$interface, $data> for Compositor {
            fn request(
                state: &mut Self,
                client: &Client,
                resource: &$interface,
                request: <$interface as Resource>::Request,
                data: &$data,
                display: &DisplayHandle,
                data_init: &mut DataInit<'_, Self>,
            ) {
                if !$refused(resource, &request) {
                    <$smithay as Dispatch<$interface, $data, Self>>::request(
                        state, client, resource, request, data, display, data_init,
                    );
                }
            }

            fn destroyed(state: &mut Self, client: ClientId, resource: &$interface, data: &$data) {
                <$smithay as Dispatch<$interface, $data, Self>>::destroyed(
                    state, client, resource, data,
                );
            }
        }
    };
}

/// Whether a rectangle this wide and high covers anything.
fn has_area(width: i32, height: i32) -> bool {
    width > 0 && height > 0
}

// `delegate_compositor!`, but for `wl_surface` and `wl_region`.

delegate_global_dispatch!(Compositor: [WlCompositor: ()] => CompositorState);
delegate_global_dispatch!(Compositor: [WlSubcompositor: ()] => CompositorState);
delegate_dispatch!(Compositor: [WlCompositor: ()] => CompositorState);
checked!(WlSurface: SurfaceUserData => CompositorState, damage);
checked!(WlRegion: RegionUserData => CompositorState, region);
delegate_dispatch!(Compositor: [WlCallback: ()] => CompositorState);
delegate_dispatch!(Compositor: [WlSubcompositor: ()] => CompositorState);
delegate_dispatch!(Compositor: [WlSubsurface: SubsurfaceUserData] => CompositorState);

/// Damage turned inside out names nothing, and is dropped, as smithay
/// already drops it from `damage_buffer`.
fn damage(_: &WlSurface, request: &wl_surface::Request) -> bool {
    matches!(
        *request,
        wl_surface::Request::Damage { width, height, .. } if width < 0 || height < 0
    )
}

/// A rectangle without area adds nothing to a region and takes nothing from
/// it, and is dropped.
fn region(_: &WlRegion, request: &wl_region::Request) -> bool {
    match *request {
        wl_region::Request::Add { width, height, .. }
        | wl_region::Request::Subtract { width, height, .. } => !has_area(width, height),
        _ => false,
    }
}

// `delegate_xdg_shell!`, but for `xdg_surface` and `xdg_toplevel`.

delegate_global_dispatch!(Compositor: [XdgWmBase: ()] => XdgShellState);
delegate_dispatch!(Compositor: [XdgWmBase: XdgWmBaseUserData] => XdgShellState);
delegate_dispatch!(Compositor: [XdgPositioner: XdgPositionerUserData] => XdgShellState);
delegate_dispatch!(Compositor: [XdgPopup: XdgShellSurfaceUserData] => XdgShellState);
checked!(XdgSurface: XdgSurfaceUserData => XdgShellState, window_geometry);
checked!(XdgToplevel: XdgShellSurfaceUserData => XdgShellState, limits);

/// xdg-shell: "The width and height of the effective window geometry must
/// be greater than zero. Setting an invalid size will raise an invalid_size
/// error."
fn window_geometry(xdg_surface: &XdgSurface, request: &xdg_surface::Request) -> bool {
    let xdg_surface::Request::SetWindowGeometry { width, height, .. } = *request else {
        return false;
    };
    if has_area(width, height) {
        return false;
    }
    xdg_surface.post_error(
        xdg_surface::Error::InvalidSize,
        format!("a window geometry of {width}x{height}: both sides must be above 0"),
    );
    true
}

/// xdg-shell, of a minimum or maximum size: "Using strictly negative values
/// for width or height will result in a invalid_size error." 0 is no limit.
fn limits(toplevel: &XdgToplevel, request: &xdg_toplevel::Request) -> bool {
    let (xdg_toplevel::Request::SetMinSize { width, height }
    | xdg_toplevel::Request::SetMaxSize { width, height }) = *request
    else {
        return false;
    };
    if width >= 0 && height >= 0 {
        return false;
    }
    toplevel.post_error(
        xdg_toplevel::Error::InvalidSize,
        format!("a size limit of {width}x{height}: neither side may be negative"),
    );
    true
}
