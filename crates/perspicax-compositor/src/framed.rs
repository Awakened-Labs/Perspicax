//! A window, together with the frame this compositor draws around it.
//!
//! Smithay's [`Window`] is the client's: its geometry, its surfaces, its
//! popups. A titlebar is none of those. It is pixels the compositor owns, drawn
//! outside the client's geometry. It still has to be hit by the pointer, kept
//! in z-order with its window, and counted when the space asks which outputs
//! a window is on. `Framed` is the element the [`Space`] holds, so that all
//! three come from one place.
//!
//! # The rule: geometry stays the client's own
//!
//! [`SpaceElement::geometry`] passes straight through. Every
//! `element_location` in this crate therefore keeps meaning the origin of the
//! client's window geometry, which is what accessibility rectangles are
//! relative to. The agent's click aiming in [`crate::act`] relies on that.
//! A frame that moved the location would land every agent click one title
//! height off, and the receipt would still say it hit. Only [`bbox`] and the
//! input region grow to take in the frame.
//!
//! [`Space`]: smithay::desktop::Space
//! [`bbox`]: SpaceElement::bbox

use std::ops::Deref;

use smithay::{
    backend::renderer::{
        ImportAll, Renderer,
        element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
    },
    desktop::{Window, space::SpaceElement},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale},
};

/// A client window, as the space holds it. Cheap to clone: [`Window`] is a
/// handle, and so is this.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Framed(Window);

impl From<Window> for Framed {
    fn from(window: Window) -> Self {
        Self(window)
    }
}

impl Deref for Framed {
    type Target = Window;

    fn deref(&self) -> &Window {
        &self.0
    }
}

impl IsAlive for Framed {
    fn alive(&self) -> bool {
        self.0.alive()
    }
}

impl SpaceElement for Framed {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        SpaceElement::geometry(&self.0)
    }

    fn bbox(&self) -> Rectangle<i32, Logical> {
        SpaceElement::bbox(&self.0)
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        SpaceElement::is_in_input_region(&self.0, point)
    }

    fn z_index(&self) -> u8 {
        SpaceElement::z_index(&self.0)
    }

    fn set_activate(&self, activated: bool) {
        SpaceElement::set_activate(&self.0, activated);
    }

    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        SpaceElement::output_enter(&self.0, output, overlap);
    }

    fn output_leave(&self, output: &Output) {
        SpaceElement::output_leave(&self.0, output);
    }

    fn refresh(&self) {
        SpaceElement::refresh(&self.0);
    }
}

impl<R> AsRenderElements<R> for Framed
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    type RenderElement = WaylandSurfaceRenderElement<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        self.0.render_elements(renderer, location, scale, alpha)
    }
}
