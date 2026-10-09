//! What every surface the shell draws needs: layer-shell surfaces to draw
//! on, and shared memory to draw into.
//!
//! The desktops, the panels and the menus each keep their own surfaces, and
//! come here to make one and to show a picture on it. A picture is painted in place in
//! the buffer the compositor will read, as premultiplied RGBA, then put in
//! the byte order Wayland reads.

#[cfg(feature = "menus")]
use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity};
#[cfg(feature = "pie")]
use smithay_client_toolkit::shm::slot::Buffer;
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

    #[cfg(any(feature = "menus", feature = "panel"))]
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

    /// A clear surface over the whole of `output`, over everything, that
    /// takes the keyboard while it is up: what a menu or a pie is drawn on.
    /// Committed, for the compositor to size.
    #[cfg(feature = "menus")]
    pub(super) fn overlay(
        &self,
        qh: &QueueHandle<App>,
        namespace: &str,
        output: &wl_output::WlOutput,
    ) -> LayerSurface {
        let layer = self.layer(qh, Layer::Overlay, namespace, output);
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();
        layer
    }

    /// Have `layer`, `size` big, take clicks everywhere but `cut`: where a
    /// panel of the shell's is, which takes its own. From its next picture.
    #[cfg(feature = "menus")]
    pub(super) fn take_clicks(
        &self,
        layer: &LayerSurface,
        (width, height): (u32, u32),
        cut: Option<Area>,
    ) {
        let surface = layer.wl_surface();
        let Some((x, y, w, h)) = cut else {
            surface.set_input_region(None);
            return;
        };
        if let Ok(region) = Region::new(&self.compositor) {
            region.add(0, 0, width as i32, height as i32);
            region.subtract(x, y, w, h);
            surface.set_input_region(Some(region.wl_region()));
        }
    }

    /// Have `layer` take clicks only in `area`: a panel's bar, so that a
    /// click on the clear rest of its strip reaches what is under it. From
    /// its next picture.
    #[cfg(feature = "panel")]
    pub(super) fn take_clicks_in(&self, layer: &LayerSurface, (x, y, w, h): Area) {
        if let Ok(region) = Region::new(&self.compositor) {
            region.add(x, y, w, h);
            layer
                .wl_surface()
                .set_input_region(Some(region.wl_region()));
        }
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

    /// Show on `layer` a picture `size` logical pixels at `scale`, of which
    /// `draw` paints only `painted`, and the rest is clear; opaque nowhere:
    /// a pie, round and see-through, drawn again on every move of the
    /// pointer. What is under it shows through, and an agent is told it
    /// covers nothing.
    ///
    /// The picture goes in one of `frames`' buffers that the compositor
    /// has let go of, or a new one, cleared once when it is made: only
    /// `painted` is cleared, painted, turned and damaged after that, so a
    /// picture costs its square and not the monitor, and the compositor
    /// takes in only the square. `false` when every buffer is still the
    /// compositor's, and nothing was shown.
    #[cfg(feature = "pie")]
    pub(super) fn show_part(
        &mut self,
        layer: &LayerSurface,
        frames: &mut Frames,
        (width, height): (u32, u32),
        scale: u32,
        painted: Area,
        draw: impl FnOnce(&mut PixmapMut<'_>),
    ) -> bool {
        let (pixels_wide, pixels_high) = (width * scale, height * scale);
        let (Ok(wide), Ok(high)) = (i32::try_from(pixels_wide), i32::try_from(pixels_high)) else {
            return false;
        };
        let px = |n: i32| u32::try_from(n.max(0)).unwrap_or(0) * scale;
        let (x, y, w, h) = painted;
        let area = (px(x), px(y), px(w), px(h));
        // A buffer of another size, or with another square painted in it,
        // is of no use: what is outside this square must be clear.
        if frames.painted != Some(((pixels_wide, pixels_high), area)) {
            frames.buffers.clear();
            frames.painted = Some(((pixels_wide, pixels_high), area));
        }
        let pool = &mut self.pool;
        let free = frames
            .buffers
            .iter()
            .position(|buffer| buffer.canvas(pool).is_some());
        let at = match free {
            Some(at) => at,
            None if frames.buffers.len() < Frames::MOST => {
                match pool.create_buffer(wide, high, wide * 4, wl_shm::Format::Argb8888) {
                    Ok((buffer, canvas)) => {
                        canvas.fill(0);
                        frames.buffers.push(buffer);
                        frames.buffers.len() - 1
                    }
                    Err(error) => {
                        tracing::warn!("no memory for a {wide}x{high} picture: {error}");
                        return false;
                    }
                }
            }
            None => return false,
        };
        let buffer = &frames.buffers[at];
        let Some(canvas) = buffer.canvas(pool) else {
            return false;
        };
        let (left, top, across, down) = area;
        let right = (left + across).min(pixels_wide) as usize;
        for row in top.min(pixels_high)..(top + down).min(pixels_high) {
            let start = (row * pixels_wide) as usize;
            canvas[(start + left as usize) * 4..(start + right) * 4].fill(0);
        }
        let Some(mut picture) = PixmapMut::from_bytes(canvas, pixels_wide, pixels_high) else {
            return false;
        };
        draw(&mut picture);
        paint::argb(canvas, pixels_wide, &[area]);
        let surface = layer.wl_surface();
        surface.set_buffer_scale(scale as i32);
        surface.set_opaque_region(None);
        if let Err(error) = buffer.attach_to(surface) {
            tracing::warn!("could not show a picture: {error}");
            return false;
        }
        let damage = |n: u32| i32::try_from(n).unwrap_or(i32::MAX);
        surface.damage_buffer(damage(left), damage(top), damage(across), damage(down));
        layer.commit();
        true
    }
}

/// The buffers a surface drawn again and again is shown in, in turn.
#[cfg(feature = "pie")]
#[derive(Default)]
pub(super) struct Frames {
    buffers: Vec<Buffer>,
    /// The size the buffers are, and the square of each that is painted,
    /// in pixels.
    painted: Option<((u32, u32), Pixels)>,
}

/// A rectangle of a buffer, in its pixels: x, y, width, height.
#[cfg(feature = "pie")]
type Pixels = (u32, u32, u32, u32);

#[cfg(feature = "pie")]
impl Frames {
    /// Enough for one on screen, one waiting, and one being drawn.
    const MOST: usize = 3;
}

/// A scale as a whole number of pixels, never less than one.
pub(super) fn whole(scale: i32) -> u32 {
    u32::try_from(scale).unwrap_or(1).max(1)
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        #[cfg(feature = "wallpaper")]
        self.desktops.closed(layer);
        #[cfg(feature = "panel")]
        self.panels.closed(layer);
        #[cfg(feature = "menus")]
        self.menus.closed(&mut self.kit.fonts, layer);
        #[cfg(feature = "pie")]
        self.pies.closed(layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        #[cfg(not(feature = "pie"))]
        let _ = qh;
        #[cfg(feature = "wallpaper")]
        self.desktops
            .configure(&mut self.canvas, &mut self.kit, layer, &configure);
        #[cfg(feature = "panel")]
        self.panels.configure(
            &mut self.canvas,
            &mut self.kit,
            &self.workspaces.model,
            layer,
            &configure,
        );
        #[cfg(feature = "menus")]
        self.menus
            .configure(&mut self.canvas, &mut self.kit, layer, &configure);
        #[cfg(feature = "pie")]
        self.pies
            .configure(&mut self.canvas, &mut self.kit, qh, layer, &configure);
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

    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        #[cfg(feature = "pie")]
        self.pies
            .frame(&mut self.canvas, &mut self.kit, qh, surface);
        #[cfg(not(feature = "pie"))]
        let _ = (qh, surface);
    }

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
