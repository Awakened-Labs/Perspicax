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

use std::{cell::RefCell, ops::Deref};

use perspicax_policy::{Insets, Rect, frame_rects};
use smithay::{
    backend::renderer::{
        ImportAll, Renderer,
        element::{
            AsRenderElements, Kind,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::WaylandSurfaceRenderElement,
        },
    },
    desktop::{Window, space::SpaceElement},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale},
};

smithay::backend::renderer::element::render_elements! {
    /// What a framed window draws as: the client's surfaces, and the frame's
    /// strips.
    pub(crate) FramedElement<R> where R: ImportAll;
    Surface=WaylandSurfaceRenderElement<R>,
    Bar=SolidColorRenderElement,
}

/// How a window's frame looks right now, set by the compositor before each
/// frame is drawn (see `Compositor::dress_frames`), and read while drawing,
/// which cannot ask the compositor anything.
///
/// The buffers are kept rather than made each frame: a buffer's id is what
/// damage tracking compares, and a new one every frame would redraw every
/// frame in full.
#[derive(Debug, Default)]
struct Dress {
    insets: Insets,
    colour: [f32; 4],
    strips: Vec<SolidColorBuffer>,
}

#[cfg(feature = "seat")]
fn rgba(colour: perspicax_policy::Colour) -> [f32; 4] {
    let channel = |value: u8| f32::from(value) / 255.0;
    [channel(colour.r), channel(colour.g), channel(colour.b), 1.0]
}

/// A client window, as the space holds it. Cheap to clone: [`Window`] is a
/// handle, and so is this.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Framed(Window);

impl Framed {
    fn dress<T>(&self, f: impl FnOnce(&mut Dress) -> T) -> T {
        self.user_data()
            .insert_if_missing(|| RefCell::new(Dress::default()));
        let cell = self
            .user_data()
            .get::<RefCell<Dress>>()
            .expect("inserted on the line above");
        f(&mut cell.borrow_mut())
    }

    /// Set how the frame looks: how far it reaches, and in what colour.
    #[cfg(feature = "seat")]
    pub(crate) fn wear(&self, insets: Insets, colour: perspicax_policy::Colour) {
        self.dress(|dress| {
            dress.insets = insets;
            dress.colour = rgba(colour);
        });
    }

    /// The frame's outside, in the same coordinates as the window's own
    /// geometry.
    fn outer(&self) -> Rectangle<i32, Logical> {
        let insets = self.dress(|dress| dress.insets);
        let mut outer = self.0.geometry();
        outer.loc -= Point::from((insets.left, insets.top));
        outer.size += (insets.left + insets.right, insets.top + insets.bottom).into();
        outer
    }
}

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
        SpaceElement::bbox(&self.0).merge(self.outer())
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
    type RenderElement = FramedElement<R>;

    /// The client's surfaces, then the frame: front to back, so popups,
    /// which come first among the client's, still draw over the titlebar.
    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        let mut elements: Vec<C> = self
            .0
            .render_elements::<WaylandSurfaceRenderElement<R>>(renderer, location, scale, alpha)
            .into_iter()
            .map(|element| C::from(FramedElement::Surface(element)))
            .collect();

        // `location` is where the surface's own origin goes; the client
        // geometry, which the frame is measured from, is offset within it.
        let geometry = self.0.geometry();
        let origin = location + geometry.loc.to_physical_precise_round(scale);
        let client = Rect::new(0, 0, geometry.size.w, geometry.size.h);
        self.dress(|dress| {
            let strips = frame_rects(client, dress.insets);
            dress
                .strips
                .resize_with(strips.len(), || SolidColorBuffer::new((0, 0), [0.0; 4]));
            for (strip, buffer) in strips.iter().zip(&mut dress.strips) {
                buffer.update((strip.w, strip.h), dress.colour);
                let at = origin + Point::from((strip.x, strip.y)).to_physical_precise_round(scale);
                elements.push(C::from(FramedElement::Bar(
                    SolidColorRenderElement::from_buffer(
                        buffer,
                        at,
                        scale,
                        alpha,
                        Kind::Unspecified,
                    ),
                )));
            }
        });
        elements
    }
}
