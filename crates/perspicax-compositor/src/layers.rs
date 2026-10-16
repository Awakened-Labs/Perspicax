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

use std::sync::PoisonError;

use smithay::{
    desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::with_states,
        shell::{
            wlr_layer::{
                KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData,
                LayerSurfaceState, WlrLayerShellHandler, WlrLayerShellState,
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
/// The panels' layer: over the windows, and under a fullscreen window that
/// is in use. See `crate::shell::covers_panels`.
pub(crate) const TOP: [Layer; 1] = [Layer::Top];
/// Over everything, a fullscreen window included.
pub(crate) const OVERLAY: [Layer; 1] = [Layer::Overlay];
/// Every layer, bottom first.
pub(crate) const ALL: [Layer; 4] = [Layer::Background, Layer::Bottom, Layer::Top, Layer::Overlay];

/// A layer as the index names it.
pub(crate) fn level(layer: Layer) -> perspicax_index::Layer {
    match layer {
        Layer::Background => perspicax_index::Layer::Background,
        Layer::Bottom => perspicax_index::Layer::Bottom,
        Layer::Top => perspicax_index::Layer::Top,
        Layer::Overlay => perspicax_index::Layer::Overlay,
    }
}

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
        self.map_layer_on(surface, namespace, &output);
    }

    /// A panel's menu, given to its panel. Already tracked: a panel's menu
    /// is made with no parent, so `XdgShellHandler::new_popup` tracked it as
    /// one waiting for one, and its first commit, after this, files it under
    /// the panel so it renders and hit-tests there. Tracking it here as well
    /// would file it twice, and smithay would draw it twice.
    ///
    /// One given to a panel its client unmapped is dismissed at once, as one
    /// opened from a window its client unmapped is: a menu needs its panel on
    /// screen.
    fn new_popup(&mut self, parent: WlrLayerSurface, popup: PopupSurface) {
        if self.is_withdrawn_layer(parent.wl_surface()) {
            popup.send_popup_done();
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        // One its client unmapped first was taken off then.
        self.withdrawn_layers
            .retain(|withdrawn| withdrawn.surface != surface);
        self.take_layer_off(surface.wl_surface());
        self.backend.redraw();
        self.publish_facts();
    }
}

/// A layer surface its client unmapped, waiting to be asked for again. See
/// [`Compositor::withdraw_layer`].
pub(crate) struct Withdrawn {
    /// The client's layer surface. The wrapper the layer map knew it by went
    /// with its place there.
    pub(crate) surface: WlrLayerSurface,
    /// The name its client gave it, which it is known by again.
    namespace: String,
    /// The output it was on, given at `get_layer_surface` or chosen then,
    /// which it goes back to.
    output: Output,
}

impl Compositor {
    /// A commit on a layer surface: rearrange its output (its size or
    /// exclusive zone may have changed), send its first configure if this is
    /// its first commit, and give it the keyboard if it asks for all of it
    /// and has something to show. Returns its id, for the damage bookkeeping
    /// every surface gets.
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
        // The wallpaper is up: the splash beneath it is done on this output,
        // unseen, since the wallpaper already covers it.
        #[cfg(any(feature = "seat", feature = "capture"))]
        if self.backdrop.splash_on(&output)
            && self.is_wallpaper(&output, &layer)
            && self.backdrop.splash_done_on(&output)
        {
            tracing::info!(
                output = output.name(),
                "the wallpaper is up; the splash is done"
            );
        }
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
        // A launcher or lock-ish overlay that wants every key gets them the
        // moment it is up, unless the session is locked (`focus_plain`
        // refuses). Up is its first picture, not the commit that asked for
        // its configure: until it shows something it is not mapped, and the
        // keys typed at it would go to a surface nobody can see. `OnDemand`
        // waits for a click (see the seat's input path).
        let exclusive = layer.cached_state().keyboard_interactivity
            == KeyboardInteractivity::Exclusive
            && ABOVE.contains(&layer.layer());
        let shown = crate::geometry::shown(surface).is_some();
        if exclusive && shown && self.keyboard_focus().as_ref() != Some(surface) {
            self.focus_plain(surface.clone());
        }
        // The usable area changed under the windows: a panel appearing
        // shrinks it. Windows that fill it are fitted to what is left, and
        // a window whose titlebar the panel now covers comes out from under
        // it, so a bar started after the windows does not hide their frames.
        if reserved {
            self.refit_frames();
        }
        // A surface that came up under the pointer has it, still or not.
        self.repoint();
        self.backend.redraw();
        layer.user_data().get::<SurfaceId>().copied()
    }

    /// Take a layer surface its client unmapped off its output, as though it
    /// had gone, and keep it until it is asked for again.
    ///
    /// Unlike a window's, the keyboard moves on headless too, as it does
    /// from a layer that went: a launcher takes the keyboard for itself,
    /// headless or not, and left with it, it would have the keys typed at
    /// a surface nobody can see.
    pub(crate) fn withdraw_layer(&mut self, surface: &WlSurface, id: SurfaceId) {
        let Some((layer, output)) = self.take_layer_off(surface) else {
            return;
        };
        // Its menus go once the keyboard has left them, so none comes back
        // with it.
        crate::popup::dismiss_all(surface);
        unconfigure(surface);
        self.withdrawn_layers.push(Withdrawn {
            surface: layer.layer_surface().clone(),
            namespace: layer.namespace().to_owned(),
            output,
        });
        tracing::info!(surface = id.0, "layer surface unmapped by its client");
    }

    /// A commit from a layer surface its client unmapped: its client asking
    /// for it again, or drawing in it. It is back on its output from this
    /// commit, as a new layer with a new id, and its room is kept again from
    /// now, as a new panel's is from its first commit. The commit is then
    /// taken in as a new layer's first is: it is sent the configure a new
    /// layer is sent, and it takes the keys once it shows something.
    pub(crate) fn map_layer_again(&mut self, surface: &WlSurface) {
        let Some(at) = self
            .withdrawn_layers
            .iter()
            .position(|withdrawn| withdrawn.surface.wl_surface() == surface)
        else {
            return;
        };
        let Withdrawn {
            surface: layer,
            namespace,
            output,
        } = self.withdrawn_layers.remove(at);
        // A menu refused while it was away, that its client drew all the
        // same, was filed under it then, and would come back with it. One its
        // client first draws from now on is filed under it on screen, which
        // smithay cannot be told not to do; its client was told it is done.
        crate::popup::dismiss_all(surface);
        self.map_layer_on(layer, namespace, &output);
    }

    /// Put a layer surface on `output` as a new layer: a new wrapper for the
    /// layer map to know it by, and a new id, so nothing an agent learned of
    /// it before is taken for what it is now.
    ///
    /// The layer map arranges it as it is mapped. A new layer has asked for
    /// no room yet, but one mapped again asked before it went, so its room is
    /// kept now, and the windows that fill are fitted to what is left: its
    /// own commit, which arranges again, finds nothing more changed.
    fn map_layer_on(&mut self, surface: WlrLayerSurface, namespace: String, output: &Output) {
        let layer = LayerSurface::new(surface, namespace);
        let id = self.mint_surface_id();
        layer.user_data().insert_if_missing(|| id);
        let reserved = {
            let mut map = layer_map_for_output(output);
            let before = map.non_exclusive_zone();
            if let Err(error) = map.map_layer(&layer) {
                tracing::warn!(%error, "could not map layer surface");
                return;
            }
            map.non_exclusive_zone() != before
        };
        if reserved {
            self.refit_frames();
        }
        tracing::info!(
            surface = id.0,
            namespace = layer.namespace(),
            output = output.name(),
            "layer surface mapped"
        );
    }

    /// Whether `surface` is that of a layer its client unmapped.
    pub(crate) fn is_withdrawn_layer(&self, surface: &WlSurface) -> bool {
        self.withdrawn_layers
            .iter()
            .any(|withdrawn| withdrawn.surface.wl_surface() == surface)
    }

    /// Take the layer whose surface is `surface` off its output, and give
    /// back what it held: its room to the windows that fill, the keyboard to
    /// the window on top, the pointer to what it uncovered. What a layer
    /// that went and one its client unmapped have in common. The layer as
    /// its map knew it, and the output it was on, if it was on one.
    fn take_layer_off(&mut self, surface: &WlSurface) -> Option<(LayerSurface, Output)> {
        // Asked first, while its layer map still knows it, and with no layer
        // map held, for asking takes them: the keys are the layer's when it
        // has them, or a menu of it does.
        let held = self.keyboard_focus().as_ref() == Some(surface)
            || self
                .layer_id(surface)
                .is_some_and(|id| self.keyboard_owner() == Some(id));
        let mut taken = None;
        let mut freed = false;
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let before = map.non_exclusive_zone();
            let gone = map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned();
            if let Some(layer) = gone {
                map.unmap_layer(&layer);
                map.arrange();
                freed |= map.non_exclusive_zone() != before;
                taken = Some((layer, output.clone()));
            }
        }
        // A panel that went gives its room back to the windows that fill.
        if freed {
            self.refit_frames();
        }
        // A launcher that held the keyboard gives it back to the window
        // beneath.
        if held {
            self.focus_top_window();
        }
        // And the pointer to what it uncovered.
        self.repoint();
        taken
    }

    /// Whether `layer` is a wallpaper that is up on `output`: in the
    /// background, over the whole output, with a buffer to show.
    #[cfg(any(feature = "seat", feature = "capture"))]
    fn is_wallpaper(&self, output: &Output, layer: &LayerSurface) -> bool {
        let Some(area) = self.space.output_geometry(output) else {
            return false;
        };
        layer.layer() == Layer::Background
            && layer_map_for_output(output).layer_geometry(layer)
                == Some(Rectangle::from_size(area.size))
            && smithay::backend::renderer::utils::with_renderer_surface_state(
                layer.wl_surface(),
                |drawn| drawn.buffer().is_some(),
            )
            .unwrap_or(false)
    }

    /// The id of the layer surface that `surface` is, rearranging nothing:
    /// for a commit on one of its subsurfaces, which changes what it shows
    /// and not where it goes.
    ///
    /// Its own surface only, not its popups'. A popup has a tree of its own,
    /// and damage in it is in the popup's coordinates until
    /// [`popup::hung`](crate::popup::hung) has placed it in the layer's.
    pub(crate) fn layer_id(&self, surface: &WlSurface) -> Option<SurfaceId> {
        self.space.outputs().find_map(|output| {
            layer_map_for_output(output)
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .and_then(|layer| layer.user_data().get::<SurfaceId>().copied())
        })
    }

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
        // Smithay takes each panel's room and margin from the zone by plain
        // subtraction, so panels asking for more than the monitor has leave
        // it with a negative side, which a `Size` built from it asserts
        // against in a debug build (#106). Nothing is left instead.
        zone.size.w = zone.size.w.max(0);
        zone.size.h = zone.size.h.max(0);
        Some(zone)
    }

    /// The id of the layer surface this surface belongs to: the layer's own,
    /// or a popup or subsurface of it, which a click on the layer can focus.
    pub(crate) fn layer_owning(&self, surface: &WlSurface) -> Option<SurfaceId> {
        self.space.outputs().find_map(|output| {
            layer_map_for_output(output)
                .layer_for_surface(surface, WindowSurfaceType::ALL)
                .and_then(|layer| layer.user_data().get::<SurfaceId>().copied())
        })
    }

    /// The layer surface an id names, with where it is in global space.
    pub(crate) fn layer_by_id(
        &self,
        id: SurfaceId,
    ) -> Option<(LayerSurface, Rectangle<i32, Logical>)> {
        self.layers_in(&ALL)
            .into_iter()
            .find(|(layer, _)| layer.user_data().get::<SurfaceId>() == Some(&id))
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
/// itself to a screen that is still there. Those their clients unmapped on
/// it are told too, and forgotten, for there is nothing left to map them on.
///
/// Every output taken out of the space is closed on first: a layer unmapped
/// on one that was not would come back on a monitor nobody sees, its output
/// kept alive by [`Withdrawn`] alone.
pub(crate) fn close_on(output: &Output, withdrawn: &mut Vec<Withdrawn>) {
    for layer in layer_map_for_output(output).layers() {
        layer.layer_surface().send_close();
    }
    withdrawn.retain(|withdrawn| {
        let on = withdrawn.output == *output;
        if on {
            withdrawn.surface.send_close();
        }
        !on
    });
}

/// Forget every configure the layer surface `surface` was sent, as smithay
/// does only when one is destroyed: back to the state it had right after
/// `get_layer_surface`, owed the configure a new layer is owed, as
/// layer-shell says an unmapped one is. What its client asked for, its
/// anchor, size, room and keys, it keeps: its client asks for it again
/// without saying them again, as on wlroots.
fn unconfigure(surface: &WlSurface) {
    with_states(surface, |states| {
        let Some(data) = states.data_map.get::<LayerSurfaceData>() else {
            return;
        };
        let mut attributes = data.lock().unwrap_or_else(PoisonError::into_inner);
        attributes.initial_configure_sent = false;
        attributes.configured = false;
        attributes.configure_serial = None;
        attributes.server_pending = None;
        attributes.last_acked = None;
        attributes.current = LayerSurfaceState::default();
    });
}
