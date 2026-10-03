//! What every surface the shell draws needs: layer-shell surfaces to draw
//! on, and shared memory to draw into.
//!
//! The desktops and the menus each keep their own surfaces, and come here
//! to make one and to show a picture on it. A picture is painted in place in
//! the buffer the compositor will read, as premultiplied RGBA, then put in
//! the byte order Wayland reads.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_shm,
    shell::{
        WaylandSurface,
        wlr_layer::{Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use tiny_skia::PixmapMut;
use wayland_client::{
    Connection, QueueHandle,
    globals::GlobalList,
    protocol::{wl_output, wl_shm, wl_surface},
};

use super::App;
use crate::{Error, paint};

/// A rectangle of a surface, in its logical pixels: x, y, width, height.
pub(super) type Area = (i32, i32, i32, i32);

/// The compositor's globals the shell draws with, and the memory it draws
/// in.
pub(super) struct Canvas {
    pub(super) compositor: CompositorState,
    layers: LayerShell,
    shm: Shm,
    pool: SlotPool,
}

impl Canvas {
    pub(super) fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Result<Self, Error> {
        let compositor =
            CompositorState::bind(globals, qh).map_err(|_| Error::Missing("wl_compositor"))?;
        let layers =
            LayerShell::bind(globals, qh).map_err(|_| Error::Missing("zwlr_layer_shell_v1"))?;
        let shm = Shm::bind(globals, qh).map_err(|_| Error::Missing("wl_shm"))?;
        // Room for one 1080p monitor to start with; it grows for more.
        let pool = SlotPool::new(1920 * 1080 * 4, &shm)
            .map_err(|error| Error::Wayland(error.to_string()))?;
        Ok(Self {
            compositor,
            layers,
            shm,
            pool,
        })
    }

    #[cfg(feature = "menus")]
    pub(super) fn shm(&self) -> &Shm {
        &self.shm
    }

    /// A new surface on `layer` of `output`, named `namespace`, not yet
    /// committed.
    pub(super) fn layer(
        &self,
        qh: &QueueHandle<App>,
        layer: Layer,
        namespace: &str,
        output: &wl_output::WlOutput,
    ) -> LayerSurface {
        let surface = self.compositor.create_surface(qh);
        self.layers.create_layer_surface(
            qh,
            surface,
            layer,
            Some(namespace.to_owned()),
            Some(output),
        )
    }

    /// Show on `layer` a picture `size` logical pixels at `scale`, painted
    /// by `draw`: opaque in `opaque`, and clear everywhere else.
    pub(super) fn show(
        &mut self,
        layer: &LayerSurface,
        (width, height): (u32, u32),
        scale: u32,
        opaque: &[Area],
        draw: impl FnOnce(&mut PixmapMut<'_>),
    ) {
        let (pixels_wide, pixels_high) = (width * scale, height * scale);
        let (Ok(wide), Ok(high)) = (i32::try_from(pixels_wide), i32::try_from(pixels_high)) else {
            return;
        };
        let (buffer, canvas) =
            match self
                .pool
                .create_buffer(wide, high, wide * 4, wl_shm::Format::Argb8888)
            {
                Ok(made) => made,
                Err(error) => {
                    tracing::warn!("no memory for a {wide}x{high} picture: {error}");
                    return;
                }
            };
        let Some(mut picture) = PixmapMut::from_bytes(canvas, pixels_wide, pixels_high) else {
            return;
        };
        draw(&mut picture);
        let pixels: Vec<_> = opaque
            .iter()
            .filter_map(|&(x, y, w, h)| {
                let px = |n: i32| u32::try_from(n).ok().map(|n| n * scale);
                Some((px(x)?, px(y)?, px(w)?, px(h)?))
            })
            .collect();
        paint::argb(canvas, pixels_wide, &pixels);

        let surface = layer.wl_surface();
        surface.set_buffer_scale(scale as i32);
        // What is opaque lets the compositor skip drawing what is under it,
        // and tells an agent what covers what.
        if let Ok(region) = Region::new(&self.compositor) {
            for &(x, y, w, h) in opaque {
                region.add(x, y, w, h);
            }
            surface.set_opaque_region(Some(region.wl_region()));
        }
        if let Err(error) = buffer.attach_to(surface) {
            tracing::warn!("could not show a picture: {error}");
            return;
        }
        surface.damage_buffer(0, 0, wide, high);
        layer.commit();
    }
}

/// A scale as a whole number of pixels, never less than one.
pub(super) fn whole(scale: i32) -> u32 {
    u32::try_from(scale).unwrap_or(1).max(1)
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        #[cfg(feature = "wallpaper")]
        self.desktops.closed(layer);
        #[cfg(feature = "menus")]
        self.menus.closed(layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        #[cfg(feature = "wallpaper")]
        self.desktops.configure(&mut self.canvas, layer, &configure);
        #[cfg(feature = "menus")]
        self.menus.configure(&mut self.canvas, layer, &configure);
    }
}

impl CompositorHandler for App {
    // Each surface is on one monitor, and drawn at that monitor's scale, so
    // the scale a surface is told it entered at is already known.
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.canvas.shm
    }
}

delegate_compositor!(App);
delegate_layer!(App);
delegate_shm!(App);
