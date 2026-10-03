//! The desktop: one surface on the `background` layer of every monitor,
//! with the wallpaper painted on it.
//!
//! Each is anchored to all four edges with an exclusive zone of -1, so it
//! covers the whole monitor and ignores the room a panel reserves. Its
//! namespace is `perspicax-desktop-<connector>`, which is how an agent tells
//! one monitor's desktop from another's and from a window.

use std::path::Path;

use perspicax_config::{Shell, Wallpaper};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_shm,
    output::OutputInfo,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use tiny_skia::{Pixmap, PixmapMut};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::GlobalList,
    protocol::{wl_output, wl_shm, wl_surface},
};

use super::App;
use crate::{Error, model::wallpaper, paint};

/// Every monitor's desktop, and what they are painted from.
pub(super) struct Desktops {
    compositor: CompositorState,
    layers: LayerShell,
    shm: Shm,
    pool: SlotPool,
    /// `None` is no wallpaper, and then no desktop either.
    wallpaper: Option<Wallpaper>,
    /// The wallpaper's image, read once.
    image: Option<Pixmap>,
    each: Vec<Desktop>,
}

/// One monitor's desktop.
struct Desktop {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    /// The monitor's whole scale: the buffer is this many pixels to each of
    /// the surface's.
    scale: u32,
    /// In the surface's own units, once the compositor has said.
    size: Option<(u32, u32)>,
}

impl Desktops {
    pub(super) fn new(
        globals: &GlobalList,
        qh: &QueueHandle<App>,
        shell: &Shell,
        config: Option<&Path>,
    ) -> Result<Self, Error> {
        let compositor =
            CompositorState::bind(globals, qh).map_err(|_| Error::Missing("wl_compositor"))?;
        let layers =
            LayerShell::bind(globals, qh).map_err(|_| Error::Missing("zwlr_layer_shell_v1"))?;
        let shm = Shm::bind(globals, qh).map_err(|_| Error::Missing("wl_shm"))?;
        // Room for one 1080p monitor to start with; it grows for more.
        let pool = SlotPool::new(1920 * 1080 * 4, &shm)
            .map_err(|error| Error::Wayland(error.to_string()))?;
        let wallpaper = shell.wallpaper.clone();
        let image = wallpaper
            .as_ref()
            .and_then(|wallpaper| wallpaper.image.as_deref())
            .and_then(|written| read(written, config));
        Ok(Self {
            compositor,
            layers,
            shm,
            pool,
            wallpaper,
            image,
            each: Vec::new(),
        })
    }

    /// Put a desktop on a monitor the compositor just announced.
    pub(super) fn add(
        &mut self,
        qh: &QueueHandle<App>,
        output: wl_output::WlOutput,
        info: &OutputInfo,
    ) {
        if self.wallpaper.is_none() {
            return;
        }
        let name = info
            .name
            .clone()
            .unwrap_or_else(|| output.id().protocol_id().to_string());
        let surface = self.compositor.create_surface(qh);
        let layer = self.layers.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some(format!("perspicax-desktop-{name}")),
            Some(&output),
        );
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        layer.commit();
        self.each.push(Desktop {
            output,
            layer,
            scale: whole(info.scale_factor),
            size: None,
        });
    }

    /// Paint a monitor's desktop again if its scale changed.
    pub(super) fn rescale(&mut self, output: &wl_output::WlOutput, scale: i32) {
        let scale = whole(scale);
        if let Some(at) = self
            .each
            .iter()
            .position(|desktop| desktop.output == *output && desktop.scale != scale)
        {
            self.each[at].scale = scale;
            self.draw(at);
        }
    }

    /// Take a monitor's desktop away with it.
    pub(super) fn remove(&mut self, output: &wl_output::WlOutput) {
        self.each.retain(|desktop| desktop.output != *output);
    }

    fn configure(&mut self, layer: &LayerSurface, configure: &LayerSurfaceConfigure) {
        let Some(at) = self.each.iter().position(|desktop| desktop.layer == *layer) else {
            return;
        };
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        self.each[at].size = Some((width, height));
        self.draw(at);
    }

    /// Paint the wallpaper on desktop `at`, at its monitor's scale.
    fn draw(&mut self, at: usize) {
        let desktop = &self.each[at];
        let (Some(wallpaper), Some((width, height))) = (&self.wallpaper, desktop.size) else {
            return;
        };
        let (pixels_wide, pixels_high) = (width * desktop.scale, height * desktop.scale);
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
                    tracing::warn!("no memory for a {wide}x{high} wallpaper: {error}");
                    return;
                }
            };
        let Some(mut picture) = PixmapMut::from_bytes(canvas, pixels_wide, pixels_high) else {
            return;
        };
        paint::wallpaper::paint(wallpaper, self.image.as_ref(), &mut picture);
        paint::argb(canvas);

        let surface = desktop.layer.wl_surface();
        surface.set_buffer_scale(desktop.scale as i32);
        // Opaque, which lets the compositor skip whatever is under it.
        if let Ok(region) = Region::new(&self.compositor) {
            region.add(0, 0, width as i32, height as i32);
            surface.set_opaque_region(Some(region.wl_region()));
        }
        if let Err(error) = buffer.attach_to(surface) {
            tracing::warn!("could not show the wallpaper: {error}");
            return;
        }
        surface.damage_buffer(0, 0, wide, high);
        desktop.layer.commit();
    }
}

/// Read the image `written` in the config, or say why not and go without.
fn read(written: &Path, config: Option<&Path>) -> Option<Pixmap> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let path = wallpaper::locate(written, config, home.as_deref());
    match wallpaper::load(&path) {
        Ok(image) => Some(image),
        Err(error) => {
            tracing::warn!(
                "the wallpaper {} could not be read: {error}; showing its colour instead",
                path.display()
            );
            None
        }
    }
}

/// A scale as a whole number of pixels, never less than one.
fn whole(scale: i32) -> u32 {
    u32::try_from(scale).unwrap_or(1).max(1)
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.desktops.each.retain(|desktop| desktop.layer != *layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.desktops.configure(layer, &configure);
    }
}

impl CompositorHandler for App {
    // Each desktop is on one monitor, and drawn at that monitor's scale
    // (see `rescale`), so the scale a surface is told it entered at is
    // already known.
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
        &mut self.desktops.shm
    }
}

delegate_compositor!(App);
delegate_layer!(App);
delegate_shm!(App);
