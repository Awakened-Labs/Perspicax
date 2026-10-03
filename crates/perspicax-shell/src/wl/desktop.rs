//! The desktop: one surface on the `background` layer of every monitor,
//! with the wallpaper painted on it.
//!
//! Each is anchored to all four edges with an exclusive zone of -1, so it
//! covers the whole monitor and ignores the room a panel reserves. Its
//! namespace is `perspicax-desktop-<connector>`, which is how an agent tells
//! one monitor's desktop from another's and from a window; its accessibility
//! window carries the same name, which is how perspicax joins the two.
//!
//! A changed config repaints the same surfaces, so an agent holding a
//! desktop's id still holds it afterwards. Only turning the wallpaper off
//! takes them away, and turning it on puts them back.

use std::path::Path;

use perspicax_config::{Shell, Wallpaper};
use smithay_client_toolkit::{
    output::{OutputInfo, OutputState},
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceConfigure},
    },
};
use tiny_skia::Pixmap;
#[cfg(feature = "menus")]
use wayland_client::protocol::wl_surface;
use wayland_client::{Proxy, QueueHandle, protocol::wl_output};

use super::{
    App,
    canvas::{Canvas, whole},
};
use crate::{
    a11y::{self, adapter::Served},
    model::image,
    paint,
};

/// What every desktop's namespace starts with, before its monitor's name.
const NAMESPACE: &str = "perspicax-desktop-";

/// Every monitor's desktop, and what they are painted from.
pub(super) struct Desktops {
    /// `None` is no wallpaper, and then no desktop either.
    wallpaper: Option<Wallpaper>,
    /// The wallpaper's image, read once, and again when the config names
    /// another.
    image: Option<Pixmap>,
    each: Vec<Desktop>,
}

/// One monitor's desktop.
struct Desktop {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    /// `perspicax-desktop-<connector>`: the surface's and its window's.
    namespace: String,
    /// Its tree, on the accessibility bus.
    a11y: Served,
    /// The monitor's whole scale: the buffer is this many pixels to each of
    /// the surface's.
    scale: u32,
    /// In the surface's own units, once the compositor has said.
    size: Option<(u32, u32)>,
}

impl Desktops {
    pub(super) fn new(shell: &Shell, config: Option<&Path>) -> Self {
        let wallpaper = shell.wallpaper.clone();
        let image = image_of(wallpaper.as_ref(), config);
        Self {
            wallpaper,
            image,
            each: Vec::new(),
        }
    }

    /// Show the wallpaper `shell` asks for now. Every desktop is painted
    /// again where it is, unless the wallpaper was turned off, which takes
    /// them all away, or on, which puts one on every monitor.
    pub(super) fn reconfigure(
        &mut self,
        canvas: &mut Canvas,
        qh: &QueueHandle<App>,
        shell: &Shell,
        config: Option<&Path>,
        outputs: &OutputState,
    ) {
        if shell.wallpaper == self.wallpaper {
            return;
        }
        if named(shell.wallpaper.as_ref()) != named(self.wallpaper.as_ref()) {
            self.image = image_of(shell.wallpaper.as_ref(), config);
        }
        let was_on = self.wallpaper.is_some();
        self.wallpaper = shell.wallpaper.clone();
        match (was_on, self.wallpaper.is_some()) {
            (true, true) => (0..self.each.len()).for_each(|at| self.draw(canvas, at)),
            (true, false) => self.each.clear(),
            (false, true) => {
                for output in outputs.outputs() {
                    if let Some(info) = outputs.info(&output) {
                        self.add(canvas, qh, output, &info);
                    }
                }
            }
            (false, false) => {}
        }
    }

    /// Put a desktop on a monitor the compositor just announced.
    pub(super) fn add(
        &mut self,
        canvas: &Canvas,
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
        let namespace = format!("{NAMESPACE}{name}");
        let layer = canvas.layer(qh, Layer::Background, &namespace, &output);
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        layer.commit();
        self.each.push(Desktop {
            output,
            layer,
            a11y: Served::new(a11y::desktop(&namespace, None)),
            namespace,
            scale: whole(info.scale_factor),
            size: None,
        });
    }

    /// Paint a monitor's desktop again if its scale changed.
    pub(super) fn rescale(
        &mut self,
        canvas: &mut Canvas,
        output: &wl_output::WlOutput,
        scale: i32,
    ) {
        let scale = whole(scale);
        if let Some(at) = self
            .each
            .iter()
            .position(|desktop| desktop.output == *output && desktop.scale != scale)
        {
            self.each[at].scale = scale;
            self.draw(canvas, at);
        }
    }

    /// Take a monitor's desktop away with it.
    pub(super) fn remove(&mut self, output: &wl_output::WlOutput) {
        self.each.retain(|desktop| desktop.output != *output);
    }

    /// The monitor and size of the desktop that is `surface`, if one is.
    #[cfg(feature = "menus")]
    pub(super) fn at(&self, surface: &wl_surface::WlSurface) -> Option<(&str, (u32, u32))> {
        self.each
            .iter()
            .find(|desktop| desktop.layer.wl_surface() == surface)
            .and_then(|desktop| {
                let name = desktop.namespace.strip_prefix(NAMESPACE)?;
                Some((name, desktop.size?))
            })
    }

    pub(super) fn closed(&mut self, layer: &LayerSurface) {
        self.each.retain(|desktop| desktop.layer != *layer);
    }

    pub(super) fn configure(
        &mut self,
        canvas: &mut Canvas,
        layer: &LayerSurface,
        configure: &LayerSurfaceConfigure,
    ) {
        let Some(at) = self.each.iter().position(|desktop| desktop.layer == *layer) else {
            return;
        };
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        let desktop = &mut self.each[at];
        desktop.size = Some((width, height));
        desktop
            .a11y
            .show(a11y::desktop(&desktop.namespace, desktop.size));
        self.draw(canvas, at);
    }

    /// Paint the wallpaper on desktop `at`, at its monitor's scale.
    fn draw(&self, canvas: &mut Canvas, at: usize) {
        let desktop = &self.each[at];
        let (Some(wallpaper), Some((width, height))) = (&self.wallpaper, desktop.size) else {
            return;
        };
        // Opaque, which lets the compositor skip whatever is under it.
        let whole = (0, 0, width as i32, height as i32);
        canvas.show(
            &desktop.layer,
            (width, height),
            desktop.scale,
            &[whole],
            |picture| {
                paint::wallpaper::paint(wallpaper, self.image.as_ref(), picture);
            },
        );
    }
}

/// The image `wallpaper` names, as the config writes it.
fn named(wallpaper: Option<&Wallpaper>) -> Option<&Path> {
    wallpaper?.image.as_deref()
}

/// The image `wallpaper` names, if it names one that can be read.
fn image_of(wallpaper: Option<&Wallpaper>, config: Option<&Path>) -> Option<Pixmap> {
    read(named(wallpaper)?, config)
}

/// Read the image `written` in the config, or say why not and go without.
fn read(written: &Path, config: Option<&Path>) -> Option<Pixmap> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let path = image::locate(written, config, home.as_deref());
    match image::load(&path) {
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
