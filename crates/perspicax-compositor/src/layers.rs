//! Layer-shell: the surfaces that are part of the desktop rather than windows
//! on it -- a panel, a launcher, a wallpaper, a notification.
//!
//! Each belongs to one output and one of four layers. `background` and
//! `bottom` sit under every window; `top` and `overlay` sit over them. That
//! order is the whole of what the index needs to know about them: a panel on
//! `top` really does cover the bottom edge of a maximized window, and an
//! agent's click aimed there has to be refused as occluded by the panel. So
//! layer surfaces enter [`perspicax_index::HostFacts`] in their true place in
//! the stack, and are hit-tested in the same order the person sees them.
//!
//! Handled the same headless and on a seat. The slice's live test is exactly
//! this: a panel occluding a window, judged by a compositor with no screen.

use smithay::{
    desktop::{LayerSurface, PopupKind, WindowSurfaceType, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::with_states,
        shell::{
            wlr_layer::{
                KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData,
                WlrLayerShellHandler, WlrLayerShellState,
            },
            xdg::PopupSurface,
        },
    },
};

use perspicax_node::SurfaceId;

use crate::state::Compositor;

/// The layers under the windows, bottom first.
pub(crate) const BELOW: [Layer; 2] = [Layer::Background, Layer::Bottom];
/// The layers over the windows, bottom first.
pub(crate) const ABOVE: [Layer; 2] = [Layer::Top, Layer::Overlay];

impl WlrLayerShellHandler for Compositor {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell
    }

    /// A new layer surface, placed on the output it asked for, or the one the
    /// person is looking at. Its first configure waits for its first commit,
    /// when the layer map knows what size to offer it.
    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.output_under_pointer());
        let Some(output) = output else {
            // Nowhere to put it: a panel for a session with no screen.
            surface.send_close();
            return;
        };
        let layer = LayerSurface::new(surface, namespace);
        let id = self.mint_surface_id();
        layer.user_data().insert_if_missing(|| id);
        if let Err(error) = layer_map_for_output(&output).map_layer(&layer) {
            tracing::warn!(%error, "could not map layer surface");
            return;
        }
        tracing::info!(
            surface = id.0,
            namespace = layer.namespace(),
            output = output.name(),
            "layer surface mapped"
        );
    }

    /// A panel's menu. Tracked so it renders and hit-tests with its panel.
    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            tracing::warn!(%error, "could not track layer popup");
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let mut freed = false;
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let before = map.non_exclusive_zone();
            let gone = map
                .layers()
                .find(|layer| layer.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = gone {
                map.unmap_layer(&layer);
                map.arrange();
                freed |= map.non_exclusive_zone() != before;
            }
        }
        // A panel that went gives its room back to the windows that fill.
        if freed {
            self.refit_frames();
        }
        // A launcher that held the keyboard gives it back to the window
        // beneath.
        if self.keyboard_focus().as_ref() == Some(surface.wl_surface()) {
            self.focus_top_window();
        }
        self.backend.redraw();
        self.publish_facts();
    }
}

impl Compositor {
    /// A commit on a layer surface: rearrange its output (its size or
    /// exclusive zone may have changed), send its first configure if this is
    /// its first commit, and give it the keyboard if it asks for all of it.
    /// Returns its id, for the damage bookkeeping every surface gets.
    pub(crate) fn layer_committed(&mut self, surface: &WlSurface) -> Option<SurfaceId> {
        let output = self.space.outputs().find(|output| {
            layer_map_for_output(output)
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .is_some()
        })?;
        let output = output.clone();
        let (layer, reserved) = {
            let mut map = layer_map_for_output(&output);
            let before = map.non_exclusive_zone();
            map.arrange();
            let reserved = map.non_exclusive_zone() != before;
            (
                map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)?
                    .clone(),
                reserved,
            )
        };
        let configured = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .and_then(|data| data.lock().ok().map(|data| data.initial_configure_sent))
                .unwrap_or(true)
        });
        if !configured {
            layer.layer_surface().send_configure();
        }
        // A launcher or lock-ish overlay that wants every key gets them, the
        // moment it is up. `OnDemand` waits for a click (see the seat's
        // input path).
        let exclusive = layer.cached_state().keyboard_interactivity
            == KeyboardInteractivity::Exclusive
            && ABOVE.contains(&layer.layer());
        if exclusive && self.keyboard_focus().as_ref() != Some(surface) && self.lock.is_none() {
            self.focus_plain(surface.clone());
        }
        // The usable area changed under the windows: a panel appearing
        // shrinks it. Windows that fill it are fitted to what is left, and
        // a window whose titlebar the panel now covers comes out from under
        // it, so a bar started after the windows does not hide their frames.
        if reserved {
            self.refit_frames();
        }
        self.backend.redraw();
        layer.user_data().get::<SurfaceId>().copied()
    }

    #[cfg_attr(
        not(feature = "seat"),
        expect(dead_code, reason = "the seat's input path")
    )]
    /// The surface of a layer on `layers`, at `at` in global space, and its
    /// origin in global space, topmost first.
    pub(crate) fn layer_surface_under(
        &self,
        layers: &[Layer],
        at: Point<f64, Logical>,
    ) -> Option<(LayerSurface, WlSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(at).next()?;
        let origin = self.space.output_geometry(output)?.loc;
        let map = layer_map_for_output(output);
        layers.iter().rev().find_map(|&layer| {
            let found = map.layer_under(layer, at - origin.to_f64())?;
            let placed = map.layer_geometry(found)?.loc + origin;
            let (surface, offset) =
                found.surface_under(at - placed.to_f64(), WindowSurfaceType::ALL)?;
            Some((found.clone(), surface, (placed + offset).to_f64()))
        })
    }

    /// Every layer surface on every output, bottom of the stack first, each
    /// with where it is in global space. For the facts.
    pub(crate) fn layers_in(
        &self,
        layers: &[Layer],
    ) -> Vec<(LayerSurface, Rectangle<i32, Logical>)> {
        let mut found = Vec::new();
        for &which in layers {
            for output in self.space.outputs() {
                let Some(origin) = self.space.output_geometry(output).map(|area| area.loc) else {
                    continue;
                };
                let map = layer_map_for_output(output);
                for layer in map.layers_on(which) {
                    if let Some(mut placed) = map.layer_geometry(layer) {
                        placed.loc += origin;
                        // Headless there is no renderer to record buffer
                        // sizes, so Smithay's bounding box is empty -- the
                        // same gap `facts` works around for windows. The size
                        // the layer map configured and the client acked is
                        // the protocol's own answer, and the one to use.
                        if placed.size.is_empty()
                            && let Some(size) = layer.layer_surface().current_state().size
                        {
                            placed.size = size;
                        }
                        found.push((layer.clone(), placed));
                    }
                }
            }
        }
        found
    }

    /// The part of an output windows may use: all of it, less whatever
    /// panels have claimed as exclusive zones. In global space.
    pub(crate) fn usable_area(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        let area = self.space.output_geometry(output)?;
        let mut zone = layer_map_for_output(output).non_exclusive_zone();
        zone.loc += area.loc;
        Some(zone)
    }

    pub(crate) fn output_under_pointer(&self) -> Option<Output> {
        self.pointer
            .as_ref()
            .map(|pointer| pointer.current_location())
            .and_then(|at| self.space.output_under(at).next().cloned())
            .or_else(|| self.space.outputs().next().cloned())
    }
}

/// An output is going away: tell its layer surfaces, so a panel can move
/// itself to a screen that is still there.
pub(crate) fn close_on(output: &Output) {
    for layer in layer_map_for_output(output).layers() {
        layer.layer_surface().send_close();
    }
}
