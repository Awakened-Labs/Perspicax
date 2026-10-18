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
//! client's window geometry, which is window space's origin: where the index
//! puts every accessible rectangle, whatever origin its toolkit measured it
//! from. The agent's click aiming in [`crate::act`] relies on that.
//! A frame that moved the location would land every agent click one title
//! height off, and the receipt would still say it hit. Only [`bbox`] and the
//! input region grow to take in the frame.
//!
//! The line marking a window an agent may act on grows neither. It is no part
//! of the window to press on, and in the bbox it would tell a maximized
//! window it is on the next monitor too, which only the line reaches.
//!
//! # Rolled up
//!
//! A window rolled up to its titlebar (`crate::shell::is_shaded`) keeps its
//! geometry, by the rule above: its client keeps its size and its place, and
//! an agent's click is still aimed from there. What changes is everything
//! else. It is drawn as its frame alone, measured from a client of no height
//! ([`rolled_up`]); its [`bbox`] and its input region are that frame and no
//! more, so a click where the client was falls through to whatever is
//! under it, and a picture of the monitor shows that. A picture of the
//! window alone draws all of it, as one of a minimized window does
//! ([`Framed::elements`]).
//!
//! [`Space`]: smithay::desktop::Space
//! [`bbox`]: SpaceElement::bbox

use std::{cell::RefCell, ops::Deref};

use perspicax_policy::{Insets, Rect, frame_rects, outline_rects, outset, rolled_up};
use smithay::{
    backend::renderer::{
        ImportAll, ImportMem, Renderer,
        element::{
            AsRenderElements, Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
        },
    },
    desktop::{PopupManager, Window, space::SpaceElement},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size},
};

use crate::shell::is_shaded;

smithay::backend::renderer::element::render_elements! {
    /// What a framed window draws as: the client's surfaces, the title, and
    /// the frame's strips.
    pub(crate) FramedElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Text=MemoryRenderBufferRenderElement<R>,
    Bar=SolidColorRenderElement,
}

/// A titlebar's title and buttons, rasterised, and what they were rasterised
/// from: when any of that changes, they are drawn again.
#[derive(Debug)]
pub(crate) struct Title {
    pub(crate) key: TitleKey,
    pub(crate) image: MemoryRenderBuffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TitleKey {
    /// One label per tab, the window's own title when it is in no group.
    pub(crate) labels: Vec<String>,
    /// Which tab is in front.
    pub(crate) front: usize,
    /// The titlebar's size, in logical pixels.
    pub(crate) size: (i32, i32),
    pub(crate) ink: perspicax_policy::Colour,
    /// The family it is written in.
    pub(crate) family: perspicax_policy::Family,
    /// The whole scale it is rasterised at. A buffer's scale is whole, so a
    /// fractional output gets the next one up, scaled down.
    pub(crate) scale: i32,
    /// Which buttons it carries, at which end.
    pub(crate) buttons: perspicax_policy::Buttons,
    /// Whether it has a shade button: not a strip of tabs over a frame its
    /// client draws, which has no titlebar of ours to roll up to.
    pub(crate) shades: bool,
    /// Whether the window is rolled up, which its shade button shows.
    pub(crate) shaded: bool,
    /// Whether it is on every workspace, which its sticky button shows.
    pub(crate) sticky: bool,
    /// Whether it is maximized, which its maximize button shows as restore.
    pub(crate) maximized: bool,
}

/// The whole scale a title is rasterised at for an output at `scale`.
pub(crate) fn whole_scale(scale: f64) -> i32 {
    (scale.ceil() as i32).max(1)
}

/// The most titles kept per window: one for each scale it is showing at,
/// which is two for a window astride two monitors that differ.
#[cfg(feature = "seat")]
const TITLES: usize = 2;

/// How a window and its frame look right now, set by the compositor before
/// each frame is drawn (see `Compositor::dress_frames`), and read while
/// drawing, which cannot ask the compositor anything.
///
/// The buffers are kept rather than made each frame: a buffer's id is what
/// damage tracking compares, and a new one every frame would redraw every
/// frame in full.
#[derive(Debug, Default)]
struct Dress {
    insets: Insets,
    /// How far outside the frame the pointer can still grab an edge: zero
    /// for a window that cannot be resized from its frame.
    grip: i32,
    colour: [f32; 4],
    strips: Vec<SolidColorBuffer>,
    /// The titlebar, where the title and its buttons are drawn, relative to
    /// the client's geometry.
    title_at: Option<Rect>,
    /// What the title and buttons are written in: the theme's ink for the
    /// bar. `None` until the frame is first dressed.
    #[cfg(feature = "seat")]
    ink: Option<perspicax_policy::Colour>,
    /// The alpha the whole window is drawn at, frame and menus included:
    /// how opaque the person has it (`Compositor::opacity_of`). `None`, read
    /// as opaque, until it is first dressed; a derived 0.0 would be nothing.
    #[cfg(feature = "seat")]
    alpha: Option<f32>,
    titles: Vec<Title>,
    /// The line round a window an agent may act on: how wide, 0 for none.
    outline: i32,
    /// The frame it goes round, which is the compositor's word on it rather
    /// than `insets`: headless, where no frame is worn, still has frames.
    outline_around: Insets,
    outline_colour: [f32; 4],
    outline_strips: Vec<SolidColorBuffer>,
}

#[cfg(any(feature = "seat", feature = "capture"))]
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

    /// Set how the frame looks: how far it reaches, in what colours, and
    /// where its title goes, relative to the client's geometry.
    #[cfg(feature = "seat")]
    pub(crate) fn wear(
        &self,
        insets: Insets,
        grip: i32,
        colour: perspicax_policy::Colour,
        ink: perspicax_policy::Colour,
        title_at: Option<Rect>,
    ) {
        self.dress(|dress| {
            dress.insets = insets;
            dress.grip = grip;
            dress.colour = rgba(colour);
            dress.title_at = title_at;
            dress.ink = Some(ink);
            if title_at.is_none() {
                dress.titles.clear();
            }
        });
    }

    /// Mark the window out as one an agent may act on, or not: a line
    /// `width` wide, 0 for none, round the frame `around` reaches, in
    /// `colour`.
    #[cfg(any(feature = "seat", feature = "capture"))]
    pub(crate) fn mark(&self, around: Insets, width: i32, colour: perspicax_policy::Colour) {
        self.dress(|dress| {
            dress.outline = width;
            dress.outline_around = around;
            dress.outline_colour = rgba(colour);
        });
    }

    /// Set the alpha the whole window is drawn at.
    #[cfg(feature = "seat")]
    pub(crate) fn fade(&self, alpha: f32) {
        self.dress(|dress| dress.alpha = Some(alpha));
    }

    /// The alpha the whole window is drawn at: opaque until it is dressed.
    #[cfg(feature = "seat")]
    pub(crate) fn alpha(&self) -> f32 {
        self.dress(|dress| dress.alpha.unwrap_or(1.0))
    }

    /// Where the title is written, relative to the client's geometry.
    #[cfg(feature = "seat")]
    pub(crate) fn title_at(&self) -> Option<Rect> {
        self.dress(|dress| dress.title_at)
    }

    /// What the title is written in.
    #[cfg(feature = "seat")]
    pub(crate) fn ink(&self) -> Option<perspicax_policy::Colour> {
        self.dress(|dress| dress.ink)
    }

    /// Whether the title drawn for `key` is already kept.
    #[cfg(feature = "seat")]
    pub(crate) fn has_title(&self, key: &TitleKey) -> bool {
        self.dress(|dress| dress.titles.iter().any(|title| title.key == *key))
    }

    /// Keep a newly drawn title, in place of the one at the same scale.
    #[cfg(feature = "seat")]
    pub(crate) fn put_title(&self, title: Title) {
        self.dress(|dress| {
            dress
                .titles
                .retain(|kept| kept.key.scale != title.key.scale);
            dress.titles.insert(0, title);
            dress.titles.truncate(TITLES);
        });
    }

    /// The window's bounds as if it were not rolled up: its client, its
    /// menus and the frame round it. A picture of the window alone is this
    /// big, rolled up or not.
    pub(crate) fn whole_bbox(&self) -> Rectangle<i32, Logical> {
        let window = SpaceElement::bbox(&self.0);
        self.outer_as(false)
            .map_or(window, |outer| window.merge(outer))
    }

    /// The frame's outside, and the grip beyond it, in the same coordinates
    /// as the window's own geometry. Nothing for a window with no frame.
    fn outer(&self) -> Option<Rectangle<i32, Logical>> {
        self.outer_as(is_shaded(self))
    }

    /// [`Self::outer`], `rolled` up to its titlebar or not: rolled up, the
    /// frame of a client of no height, with no grip, since nothing resizes
    /// a window rolled up.
    fn outer_as(&self, rolled: bool) -> Option<Rectangle<i32, Logical>> {
        let (insets, grip) = self.dress(|dress| (dress.insets, dress.grip));
        if insets.is_none() {
            return None;
        }
        let grip = if rolled { 0 } else { grip };
        let mut outer = self.0.geometry();
        if rolled {
            outer.size.h = 0;
        }
        outer.loc -= Point::from((insets.left + grip, insets.top + grip));
        outer.size += (
            insets.left + insets.right + 2 * grip,
            insets.top + insets.bottom + 2 * grip,
        )
            .into();
        Some(outer)
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

    /// The client's, its menus included, and the frame round it. Rolled up
    /// to its titlebar, the frame alone, which is all that is drawn: nothing,
    /// headless, where no frame is worn.
    fn bbox(&self) -> Rectangle<i32, Logical> {
        if is_shaded(self) {
            return self
                .outer_as(true)
                .unwrap_or_else(|| Rectangle::new(self.0.geometry().loc, Size::default()));
        }
        self.whole_bbox()
    }

    /// The client's input region, and the frame: a press on the titlebar is
    /// the compositor's to handle, and must not fall through to whatever is
    /// under the window. Rolled up, the frame alone: a press where the
    /// client was goes to whatever is under it.
    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        (!is_shaded(self) && SpaceElement::is_in_input_region(&self.0, point))
            || self
                .outer()
                .is_some_and(|outer| outer.to_f64().contains(*point))
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

impl Framed {
    /// The client's menus, then the agent's outline, then the client's own
    /// surfaces, then the title, then the bars: front to back, as smithay
    /// draws a window but for the outline between. The outline is in front
    /// of the client's own picture, for a client's shadow is drawn just
    /// where it goes, and would hide it. Its menus are in front of all of
    /// it, as they are of the titlebar: the index stacks them directly above
    /// their window (#112), and what is drawn is what it describes. The title
    /// is over the bar it is written on.
    ///
    /// `rolled` up to its titlebar, the frame alone, measured from a client
    /// of no height, and nothing of the client: what is drawn of a window
    /// rolled up. A picture of the window alone draws it whole.
    pub(crate) fn elements<R, C>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
        rolled: bool,
    ) -> Vec<C>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
        C: From<FramedElement<R>>,
    {
        // `location` is where the surface's own origin goes; the client
        // geometry, which the frame is measured from, is offset within it.
        let geometry = self.0.geometry();
        let origin = location + geometry.loc.to_physical_precise_round(scale);
        let client = Rect::new(0, 0, geometry.size.w, geometry.size.h);
        let client = if rolled { rolled_up(client) } else { client };
        let solid = |strip: &Rect, buffer: &mut SolidColorBuffer, colour: [f32; 4]| {
            buffer.update((strip.w, strip.h), colour);
            let at = origin + Point::from((strip.x, strip.y)).to_physical_precise_round(scale);
            C::from(FramedElement::Bar(SolidColorRenderElement::from_buffer(
                buffer,
                at,
                scale,
                alpha,
                Kind::Unspecified,
            )))
        };

        // An xdg window's menus and its own tree, apart, as smithay's
        // `Window::render_elements` draws them together. An X11 window's
        // menus are windows of their own.
        let (menus, own): (
            Vec<WaylandSurfaceRenderElement<R>>,
            Vec<WaylandSurfaceRenderElement<R>>,
        ) = match self.0.toplevel() {
            // Rolled up, nothing of the client is drawn, nor its menus,
            // which it has none of then.
            _ if rolled => (Vec::new(), Vec::new()),
            Some(toplevel) => {
                let root = toplevel.wl_surface();
                let mut menus = Vec::new();
                for (popup, offset) in PopupManager::popups_for_surface(root) {
                    let offset = (geometry.loc + offset - popup.geometry().loc)
                        .to_physical_precise_round(scale);
                    menus.extend(render_elements_from_surface_tree(
                        renderer,
                        popup.wl_surface(),
                        location + offset,
                        scale,
                        alpha,
                        Kind::Unspecified,
                    ));
                }
                let own = render_elements_from_surface_tree(
                    renderer,
                    root,
                    location,
                    scale,
                    alpha,
                    Kind::Unspecified,
                );
                (menus, own)
            }
            None => (
                Vec::new(),
                self.0.render_elements(renderer, location, scale, alpha),
            ),
        };

        let mut elements: Vec<C> = menus
            .into_iter()
            .map(|element| C::from(FramedElement::Surface(element)))
            .collect();
        elements.extend(self.dress(|dress| {
            let strips = outline_rects(outset(client, dress.outline_around), dress.outline);
            let colour = dress.outline_colour;
            let buffers = &mut dress.outline_strips;
            buffers.resize_with(strips.len(), || SolidColorBuffer::new((0, 0), [0.0; 4]));
            strips
                .iter()
                .zip(buffers)
                .map(|(strip, buffer)| solid(strip, buffer, colour))
                .collect::<Vec<_>>()
        }));
        elements.extend(
            own.into_iter()
                .map(|element| C::from(FramedElement::Surface(element))),
        );

        self.dress(|dress| {
            let whole = whole_scale(scale.x);
            let title = dress.titles.iter().find(|title| title.key.scale == whole);
            if let (Some(title), Some(area)) = (title, dress.title_at) {
                let at = origin + Point::from((area.x, area.y)).to_physical_precise_round(scale);
                if let Ok(text) = MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    at.to_f64(),
                    &title.image,
                    Some(alpha),
                    None,
                    Some((area.w, area.h).into()),
                    Kind::Unspecified,
                ) {
                    elements.push(C::from(FramedElement::Text(text)));
                }
            }

            let strips = frame_rects(client, dress.insets);
            let colour = dress.colour;
            dress
                .strips
                .resize_with(strips.len(), || SolidColorBuffer::new((0, 0), [0.0; 4]));
            for (strip, buffer) in strips.iter().zip(&mut dress.strips) {
                elements.push(solid(strip, buffer, colour));
            }
        });
        elements
    }
}

impl<R> AsRenderElements<R> for Framed
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    type RenderElement = FramedElement<R>;

    /// The window as it is drawn: see [`Framed::elements`].
    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        self.elements(renderer, location, scale, alpha, is_shaded(self))
    }
}
