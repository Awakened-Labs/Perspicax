//! The sizes a client sends, checked before smithay 0.7 reads them.
//!
//! Smithay's `Size` asserts in a debug build that neither of its sides is
//! negative, and smithay 0.7.0 builds one straight from what a client sent
//! in several requests. One such request, from any client -- a test's, a
//! real one with a bug, or a hostile one -- panicked a debug build, which
//! every `cargo run` and every test run is, and ended the session (issue
//! #106). A release build took the values without complaint, as rectangles
//! turned inside out. A layer's size, a `u32` on the wire, is cast to an
//! `i32`, and one past `i32::MAX` comes out negative. One more panicked a
//! release build too: smithay posts
//! `invalid_fd` for `wl_shm_pool.resize(0)`, then goes on to unwrap a
//! `NonZeroUsize` of the 0 it refused (#153).
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
//! | `xdg_positioner.set_parent_size` | a side is negative | xdg-shell's `invalid_input` |
//! | `zwlr_layer_surface_v1.set_size` | a side is past `i32::MAX` | layer-shell's `invalid_size` |
//! | `wl_shm_pool.resize` | the size is not above 0 | `invalid_fd`, as smithay posts it |
//! | `zwp_linux_buffer_params_v1.create`, `create_immed` | a side is not above 0 | `invalid_dimensions`, as smithay posts it |
//!
//! A refusal with a protocol error ends the client, which is what the
//! protocol asks; a geometry without area, accepted until #106, is refused
//! in a release build too.
//!
//! # Numbers past the world
//!
//! Smithay 0.7.0 also adds some of a client's numbers together in `i32`,
//! each fine on its own: where a popup hangs from its anchor, and how much
//! room the panels on a monitor leave. Two near `i32::MAX` panicked a debug
//! build at the overflow, and wrapped round in a release one (#154). Their
//! protocols allow any such number, so none is refused. A number further
//! from 0 than [`FAR`], past anything a monitor shows, is taken back to it
//! and the request answered as if the client had asked for that:
//!
//! | Request | Taken into the world |
//! |---|---|
//! | `xdg_positioner.set_size` | each side, to `FAR` at most |
//! | `xdg_positioner.set_anchor_rect` | its corner, to `FAR` from 0 either way; each side, to `FAR` at most |
//! | `xdg_positioner.set_offset` | each way, to `FAR` from 0 |
//! | `zwlr_layer_surface_v1.set_margin` | each side, to `FAR` from 0 either way |
//! | `zwlr_layer_surface_v1.set_exclusive_zone` | to `FAR` at most |
//!
//! A side below 1 is left as sent, for smithay to refuse as it does.
//!
//! That bounds what each layer adds, not what they add together: smithay
//! carries the room one exclusive layer leaves on to the next, so some
//! 11 000 of them on one monitor would still overflow it. Smithay master adds
//! a layer's numbers saturating (Smithay/smithay#1759), which ends that.
//!
//! # When to delete this
//!
//! Each check goes once a smithay release carries its own, and the
//! `delegate_*!` macro its section stands in for comes back in its place
//! (#155).
//! Smithay master has the region, geometry and min/max checks. The damage
//! and parent size checks are in a pull request (Smithay/smithay#2200), and
//! so are the pool's and the dmabuf's (#2197); the layer's is in none.
//! The layer's numbers are held until a release carries #1759. Nothing
//! upstream holds a positioner's yet: #2214, which would have, was closed
//! unmerged.

use smithay::{
    reexports::{
        wayland_protocols::xdg::shell::server::{
            xdg_popup::XdgPopup,
            xdg_positioner::{self, XdgPositioner},
            xdg_surface::{self, XdgSurface},
            xdg_toplevel::{self, XdgToplevel},
            xdg_wm_base::XdgWmBase,
        },
        wayland_protocols_wlr::layer_shell::v1::server::{
            zwlr_layer_shell_v1::ZwlrLayerShellV1,
            zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
        },
        wayland_server::{
            Resource, delegate_dispatch, delegate_global_dispatch,
            protocol::{
                wl_buffer::WlBuffer,
                wl_callback::WlCallback,
                wl_compositor::WlCompositor,
                wl_region::{self, WlRegion},
                wl_shm::{self, WlShm},
                wl_shm_pool::{self, WlShmPool},
                wl_subcompositor::WlSubcompositor,
                wl_subsurface::WlSubsurface,
                wl_surface::{self, WlSurface},
            },
        },
    },
    wayland::{
        compositor::{CompositorState, RegionUserData, SubsurfaceUserData, SurfaceUserData},
        shell::{
            wlr_layer::{WlrLayerShellGlobalData, WlrLayerShellState, WlrLayerSurfaceUserData},
            xdg::{
                XdgPositionerUserData, XdgShellState, XdgShellSurfaceUserData, XdgSurfaceUserData,
                XdgWmBaseUserData,
            },
        },
        shm::{ShmBufferUserData, ShmPoolUserData, ShmState},
    },
};

use perspicax_policy::FAR;

use crate::state::Compositor;

/// `delegate_dispatch!`, with a check first: the compositor's
/// `Dispatch<$interface, $data>`, answered by smithay's `$smithay` unless
/// `$refused` refuses the request. A check that refuses has already posted
/// the protocol's error, where the protocol names one; one that does not may
/// have taken a number in the request into the world first.
macro_rules! checked {
    ($interface:ty: $data:ty => $smithay:ty, $refused:path) => {
        const _: () = {
            use smithay::reexports::wayland_server::{
                Client, DataInit, Dispatch, DisplayHandle, Resource, backend::ClientId,
            };
            use $crate::state::Compositor;

            impl Dispatch<$interface, $data> for Compositor {
                fn request(
                    state: &mut Self,
                    client: &Client,
                    resource: &$interface,
                    mut request: <$interface as Resource>::Request,
                    data: &$data,
                    display: &DisplayHandle,
                    data_init: &mut DataInit<'_, Self>,
                ) {
                    if !$refused(resource, &mut request) {
                        <$smithay as Dispatch<$interface, $data, Self>>::request(
                            state, client, resource, request, data, display, data_init,
                        );
                    }
                }

                fn destroyed(
                    state: &mut Self,
                    client: ClientId,
                    resource: &$interface,
                    data: &$data,
                ) {
                    <$smithay as Dispatch<$interface, $data, Self>>::destroyed(
                        state, client, resource, data,
                    );
                }
            }
        };
    };
}

/// Whether a rectangle this wide and high covers anything.
fn has_area(width: i32, height: i32) -> bool {
    width > 0 && height > 0
}

/// Each of `values`, a coordinate, brought back to the world's edge if it
/// is past it, either way.
fn held<const N: usize>(values: [&mut i32; N]) {
    for value in values {
        *value = (*value).clamp(-FAR, FAR);
    }
}

/// Each of `sides`, a size, brought back to the world's width if it is
/// wider. One below 1 is left as it was sent, for smithay to refuse.
fn sides_held<const N: usize>(sides: [&mut i32; N]) {
    for side in sides {
        *side = (*side).min(FAR);
    }
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
fn damage(_: &WlSurface, request: &mut wl_surface::Request) -> bool {
    matches!(
        *request,
        wl_surface::Request::Damage { width, height, .. } if width < 0 || height < 0
    )
}

/// A rectangle without area adds nothing to a region and takes nothing from
/// it, and is dropped.
fn region(_: &WlRegion, request: &mut wl_region::Request) -> bool {
    match *request {
        wl_region::Request::Add { width, height, .. }
        | wl_region::Request::Subtract { width, height, .. } => !has_area(width, height),
        _ => false,
    }
}

// `delegate_xdg_shell!`, but for `xdg_positioner`, `xdg_surface` and
// `xdg_toplevel`.

delegate_global_dispatch!(Compositor: [XdgWmBase: ()] => XdgShellState);
delegate_dispatch!(Compositor: [XdgWmBase: XdgWmBaseUserData] => XdgShellState);
checked!(XdgPositioner: XdgPositionerUserData => XdgShellState, positioner);
delegate_dispatch!(Compositor: [XdgPopup: XdgShellSurfaceUserData] => XdgShellState);
checked!(XdgSurface: XdgSurfaceUserData => XdgShellState, window_geometry);
checked!(XdgToplevel: XdgShellSurfaceUserData => XdgShellState, limits);

/// A positioner's corner, offset and sizes, held to the world, and its parent
/// size checked.
fn positioner(positioner: &XdgPositioner, request: &mut xdg_positioner::Request) -> bool {
    match request {
        xdg_positioner::Request::SetSize { width, height } => {
            sides_held([width, height]);
        }
        xdg_positioner::Request::SetAnchorRect {
            x,
            y,
            width,
            height,
        } => {
            held([x, y]);
            sides_held([width, height]);
        }
        xdg_positioner::Request::SetOffset { x, y } => held([x, y]),
        _ => {}
    }
    parent_size(positioner, request)
}

/// A parent size turned inside out describes no parent. xdg-shell names no
/// error for it, so the one the positioner's other sizes get, as smithay's
/// own pull request for it posts (Smithay/smithay#2200).
fn parent_size(positioner: &XdgPositioner, request: &xdg_positioner::Request) -> bool {
    let xdg_positioner::Request::SetParentSize {
        parent_width,
        parent_height,
    } = *request
    else {
        return false;
    };
    if parent_width >= 0 && parent_height >= 0 {
        return false;
    }
    positioner.post_error(
        xdg_positioner::Error::InvalidInput,
        format!("a parent size of {parent_width}x{parent_height}: neither side may be negative"),
    );
    true
}

/// xdg-shell: "The width and height of the effective window geometry must
/// be greater than zero. Setting an invalid size will raise an invalid_size
/// error."
fn window_geometry(xdg_surface: &XdgSurface, request: &mut xdg_surface::Request) -> bool {
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
fn limits(toplevel: &XdgToplevel, request: &mut xdg_toplevel::Request) -> bool {
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

// `delegate_layer_shell!`, but for `zwlr_layer_surface_v1`.

delegate_global_dispatch!(Compositor: [ZwlrLayerShellV1: WlrLayerShellGlobalData] => WlrLayerShellState);
delegate_dispatch!(Compositor: [ZwlrLayerShellV1: ()] => WlrLayerShellState);
checked!(ZwlrLayerSurfaceV1: WlrLayerSurfaceUserData => WlrLayerShellState, layer);

/// A layer's margins and the room it keeps from windows, held to the world,
/// and its size checked.
fn layer(layer: &ZwlrLayerSurfaceV1, request: &mut zwlr_layer_surface_v1::Request) -> bool {
    match request {
        zwlr_layer_surface_v1::Request::SetMargin {
            top,
            right,
            bottom,
            left,
        } => held([top, right, bottom, left]),
        // Below 0 is not room: it asks to be put over other layers' rooms.
        zwlr_layer_surface_v1::Request::SetExclusiveZone { zone } => sides_held([zone]),
        _ => {}
    }
    layer_size(layer, request)
}

/// A layer's size is a `u32` on the wire and an `i32` in smithay, which
/// casts one to the other: past `i32::MAX`, a side comes out negative. No
/// monitor is that big, so layer-shell's `invalid_size`.
fn layer_size(layer: &ZwlrLayerSurfaceV1, request: &zwlr_layer_surface_v1::Request) -> bool {
    let zwlr_layer_surface_v1::Request::SetSize { width, height } = *request else {
        return false;
    };
    if i32::try_from(width).is_ok() && i32::try_from(height).is_ok() {
        return false;
    }
    layer.post_error(
        zwlr_layer_surface_v1::Error::InvalidSize,
        format!("a layer size of {width}x{height}: past the largest a size can be"),
    );
    true
}

// `delegate_shm!`, but for `wl_shm_pool`.

delegate_global_dispatch!(Compositor: [WlShm: ()] => ShmState);
delegate_dispatch!(Compositor: [WlShm: ()] => ShmState);
checked!(WlShmPool: ShmPoolUserData => ShmState, pool_size);
delegate_dispatch!(Compositor: [WlBuffer: ShmBufferUserData] => ShmState);

/// A pool only grows, and has a size above 0. Smithay posts the error
/// itself, but then resizes the pool anyway, unwrapping a `NonZeroUsize` of
/// the size: at 0, a panic in any build (#153).
fn pool_size(pool: &WlShmPool, request: &mut wl_shm_pool::Request) -> bool {
    let wl_shm_pool::Request::Resize { size } = *request else {
        return false;
    };
    if size > 0 {
        return false;
    }
    pool.post_error(
        wl_shm::Error::InvalidFd,
        format!("a pool resized to {size}: its size must be above 0"),
    );
    true
}

/// `delegate_dmabuf!`, but for `zwp_linux_buffer_params_v1`: on a seat only,
/// the one backend that takes buffers from the GPU.
#[cfg(feature = "seat")]
mod dmabuf {
    use smithay::{
        backend::allocator::dmabuf::Dmabuf,
        reexports::{
            wayland_protocols::wp::linux_dmabuf::zv1::server::{
                zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
                zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
                zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
            },
            wayland_server::{
                Resource, delegate_dispatch, delegate_global_dispatch,
                protocol::wl_buffer::WlBuffer,
            },
        },
        wayland::dmabuf::{
            DmabufData, DmabufFeedbackData, DmabufGlobalData, DmabufParamsData, DmabufState,
        },
    };

    use super::has_area;
    use crate::state::Compositor;

    delegate_global_dispatch!(Compositor: [ZwpLinuxDmabufV1: DmabufGlobalData] => DmabufState);
    delegate_dispatch!(Compositor: [ZwpLinuxDmabufV1: DmabufData] => DmabufState);
    checked!(ZwpLinuxBufferParamsV1: DmabufParamsData => DmabufState, buffer_size);
    delegate_dispatch!(Compositor: [WlBuffer: Dmabuf] => DmabufState);
    delegate_dispatch!(Compositor: [ZwpLinuxDmabufFeedbackV1: DmabufFeedbackData] => DmabufState);

    /// A buffer has a size above 0. Smithay posts the error itself, but then
    /// builds the buffer anyway, with a `Size` of the width and height it
    /// refused.
    fn buffer_size(
        params: &ZwpLinuxBufferParamsV1,
        request: &mut zwp_linux_buffer_params_v1::Request,
    ) -> bool {
        let (zwp_linux_buffer_params_v1::Request::Create { width, height, .. }
        | zwp_linux_buffer_params_v1::Request::CreateImmed { width, height, .. }) = *request
        else {
            return false;
        };
        if has_area(width, height) {
            return false;
        }
        params.post_error(
            zwp_linux_buffer_params_v1::Error::InvalidDimensions,
            format!("a buffer of {width}x{height}: both sides must be above 0"),
        );
        true
    }
}
