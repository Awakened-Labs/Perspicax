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
//! (`perspicax_policy::rolled_up`); its [`bbox`] and its input region are
//! that frame and no more, so a click where the client was falls through to whatever is
//! under it, and a picture of the monitor shows that. A picture of the
//! window alone draws all of it, as one of a minimized window does
//! ([`Framed::elements`]).
//!
//! Rolling up or down, as the person asks, it is drawn as a client cut short
//! at the bottom, with the frame round what is left, for as long as the roll
//! takes. Rolling up, its bbox is what is still drawn; rolling down, all of
//! it at once, as its input region is, for it is unrolled already and the
//! facts say so.
//!
//! [`Space`]: smithay::desktop::Space
//! [`bbox`]: SpaceElement::bbox

use std::{cell::RefCell, ops::Deref};

use perspicax_policy::{
    Insets, Rect, Rgba, Shadow, Stretch, outset, rounded_outline, rounded_rects, shadow_at,
    shadow_pieces,
};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportAll, ImportMem, Renderer,
            element::{
                AsRenderElements, Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::{SolidColorBuffer, SolidColorRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                utils::CropRenderElement,
            },
        },
    },
    desktop::{PopupManager, Window, space::SpaceElement},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::shell::is_shaded;

smithay::backend::renderer::element::render_elements! {
    /// What a framed window draws as: the client's surfaces, the title, and
    /// the frame's strips.
    pub(crate) FramedElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Text=MemoryRenderBufferRenderElement<R>,
    Bar=SolidColorRenderElement,
    /// The client's own surfaces, cut short while it rolls.
    Cropped=CropRenderElement<WaylandSurfaceRenderElement<R>>,
}

/// How a window is drawn, beyond where and how opaque.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drawn {
    /// Rolled up to its titlebar: its frame alone.
    pub(crate) rolled: bool,
    /// Rolling up or down: how much of its client is drawn, from 1, all of
    /// it, to 0, none, in place of `rolled`.
    pub(crate) rolling: Option<f32>,
    /// With the shadow it casts, if it casts one.
    pub(crate) shadow: bool,
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
    /// Which reading of the person's icons it is drawn from.
    pub(crate) icons: u64,
}

/// A titlebar button lit under the pointer: where it is, relative to the
/// client's geometry, and what it is lit in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Wash {
    pub(crate) at: Rect,
    pub(crate) colour: Rgba,
    /// How far its light has come up, or still is as it goes out, out of 1.
    pub(crate) alpha: f32,
}

/// What a wash's disc was drawn from: its side, its colour, and the whole
/// scale it was drawn at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WashKey {
    side: i32,
    colour: Rgba,
    scale: i32,
}

/// The most washes kept drawn per window: one for each scale it is showing
/// at, as with titles.
const WASHES: usize = 2;

/// What a rounded corner's quarter was drawn from: how many logical pixels
/// a side, from how far round its centre it starts, in what, at what whole
/// scale, and which of the two top corners it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CornerKey {
    side: i32,
    inner: i32,
    colour: Rgba,
    scale: i32,
    right: bool,
}

/// The most corners kept drawn per window: the frame's two and its line's
/// two, at each of two scales.
const CORNERS: usize = 8;

/// What a piece of a shadow was drawn from: which piece, the shadow, how
/// round the frame casting it is, and the whole scale. A piece looks the
/// same whatever size the window is, so it is drawn once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShadowKey {
    piece: usize,
    shadow: Shadow,
    radius: i32,
    scale: i32,
}

/// The most shadow pieces kept drawn per window: eight, at each of two
/// scales.
const SHADOWS: usize = 16;

/// Where a window's pieces go, and how opaque: what every one of them is
/// drawn with.
struct Drawing {
    /// The client's geometry's origin, in the output's pixels.
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
    alpha: f32,
}

/// A lit button's disc, drawn from a buffer kept in `kept` by what it was
/// drawn from, at the window's alpha times how lit it is.
fn washed<R>(
    renderer: &mut R,
    kept: &mut Vec<(WashKey, MemoryRenderBuffer)>,
    wash: Wash,
    drawing: &Drawing,
) -> Option<MemoryRenderBufferRenderElement<R>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let whole = whole_scale(drawing.scale.x);
    let key = WashKey {
        side: wash.at.h,
        colour: wash.colour,
        scale: whole,
    };
    if !kept.iter().any(|(drawn, _)| *drawn == key) {
        let side = key.side * whole;
        let pixels = crate::raster::disc(side, key.colour);
        let buffer = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (side, side),
            whole,
            Transform::Normal,
            None,
        );
        kept.retain(|(drawn, _)| drawn.scale != whole);
        kept.insert(0, (key, buffer));
        kept.truncate(WASHES);
    }
    let at = drawing.origin
        + Point::from((wash.at.x, wash.at.y)).to_physical_precise_round(drawing.scale);
    let (_, buffer) = kept.iter().find(|(drawn, _)| *drawn == key)?;
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        at.to_f64(),
        buffer,
        Some(drawing.alpha * wash.alpha),
        None,
        Some((wash.at.w, wash.at.h).into()),
        Kind::Unspecified,
    )
    .ok()
}

/// How tall a window's client is drawn with `share` of it shown.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a share between 0 and 1 of a window's height"
)]
fn drawn_height(window: &Framed, share: f32) -> i32 {
    (window.geometry().size.h as f32 * share.clamp(0.0, 1.0)).round() as i32
}

/// A colour as the renderer takes it, back in bytes, for drawing by hand.
fn bytes(colour: [f32; 4]) -> Rgba {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba::new(
        byte(colour[0]),
        byte(colour[1]),
        byte(colour[2]),
        byte(colour[3]),
    )
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
    /// How round the frame's top corners are, and its line's with them.
    radius: i32,
    /// Those corners, drawn, by what they were drawn from.
    corners: Vec<(CornerKey, MemoryRenderBuffer)>,
    /// How much of its client is drawn while it rolls, if it is rolling.
    rolling: Option<f32>,
    /// The shadow it casts, if it casts one.
    shadow: Option<Shadow>,
    /// Its pieces, drawn, by what they were drawn from.
    shadows: Vec<(ShadowKey, MemoryRenderBuffer)>,
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
    /// The titlebar button lit under the pointer, if it is one of this
    /// window's.
    wash: Option<Wash>,
    /// Its disc, drawn, by what it was drawn from.
    washes: Vec<(WashKey, MemoryRenderBuffer)>,
    /// The one lit before it, if it is one of this window's and its light
    /// is still going out.
    fading: Option<Wash>,
    /// Its disc, kept apart: one buffer drawn twice in a frame would give
    /// two elements one id.
    faded: Vec<(WashKey, MemoryRenderBuffer)>,
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
    /// The client's geometry, held to the world (#154): smithay's
    /// `Window::geometry()`, its corner and far edges no further from the
    /// surface's corner than [`perspicax_policy::FAR`].
    ///
    /// The space places and draws the window by this, so it is the same
    /// answer [`crate::geometry::window_geometry`] gives the facts, and an
    /// agent is told the window is where it is drawn. Everything that reads
    /// a window's geometry through the space, or calls `geometry()` on a
    /// `Framed`, which finds this before the `Window` it derefs to, gets it
    /// held: a client's corner plus its width can be past an `i32`, and
    /// what is reckoned from it, a frame round it or a resize of it, adds
    /// more.
    pub(crate) fn geometry(&self) -> Rectangle<i32, Logical> {
        crate::shell::in_world(self.0.geometry())
    }

    fn dress<T>(&self, f: impl FnOnce(&mut Dress) -> T) -> T {
        self.user_data()
            .insert_if_missing(|| RefCell::new(Dress::default()));
        let cell = self
            .user_data()
            .get::<RefCell<Dress>>()
            .expect("inserted on the line above");
        f(&mut cell.borrow_mut())
    }

    /// Set how the frame looks: how far it reaches, how round its top
    /// corners are, in what colours, and where its title goes, relative to
    /// the client's geometry.
    #[cfg(feature = "seat")]
    pub(crate) fn wear(
        &self,
        insets: Insets,
        grip: i32,
        radius: i32,
        colour: perspicax_policy::Colour,
        ink: perspicax_policy::Colour,
        title_at: Option<Rect>,
    ) {
        self.dress(|dress| {
            dress.insets = insets;
            dress.grip = grip;
            dress.radius = radius;
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

    /// Draw the window rolling, `share` of its client drawn, or settled.
    #[cfg(feature = "seat")]
    pub(crate) fn rolling(&self, share: Option<f64>) {
        #[expect(clippy::cast_possible_truncation, reason = "a share between 0 and 1")]
        self.dress(|dress| dress.rolling = share.map(|share| share as f32));
    }

    /// Cast a shadow under the window, or none.
    #[cfg(feature = "seat")]
    pub(crate) fn cast(&self, shadow: Option<Shadow>) {
        self.dress(|dress| dress.shadow = shadow);
    }

    /// Light one of the titlebar's buttons, or none.
    #[cfg(feature = "seat")]
    pub(crate) fn light(&self, wash: Option<Wash>) {
        self.dress(|dress| dress.wash = wash);
    }

    /// Show the light of a button lit before going out, or none.
    #[cfg(feature = "seat")]
    pub(crate) fn fade_light(&self, wash: Option<Wash>) {
        self.dress(|dress| dress.fading = wash);
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
        self.outer_as(None)
            .map_or(window, |outer| window.merge(outer))
    }

    /// The frame's outside, and the grip beyond it, in the same coordinates
    /// as the window's own geometry. Nothing for a window with no frame.
    fn outer(&self) -> Option<Rectangle<i32, Logical>> {
        self.outer_as(is_shaded(self).then_some(0))
    }

    /// How much of its client is drawn while it rolls, if it is rolling.
    fn rolling_share(&self) -> Option<f32> {
        self.dress(|dress| dress.rolling)
    }

    /// [`Self::outer`], round a client cut to `height`, or whole: cut, as a
    /// window rolled up or rolling is, with no grip, since nothing resizes
    /// it then.
    fn outer_as(&self, height: Option<i32>) -> Option<Rectangle<i32, Logical>> {
        let (insets, grip) = self.dress(|dress| (dress.insets, dress.grip));
        if insets.is_none() {
            return None;
        }
        let grip = if height.is_some() { 0 } else { grip };
        let mut outer = self.geometry();
        if let Some(height) = height {
            outer.size.h = height;
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
        Framed::geometry(self)
    }

    /// The client's, its menus included, and the frame round it. Rolled up
    /// to its titlebar, the frame alone, which is all that is drawn: nothing,
    /// headless, where no frame is worn. Rolling up, the frame round what is
    /// still drawn of the client.
    fn bbox(&self) -> Rectangle<i32, Logical> {
        if is_shaded(self) {
            let height = self
                .rolling_share()
                .map_or(0, |share| drawn_height(self, share));
            return self
                .outer_as(Some(height))
                .unwrap_or_else(|| Rectangle::new(self.geometry().loc, Size::default()));
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
    /// surfaces, then the title, then the button lit under the pointer, then
    /// the bars: front to back, as smithay draws a window but for the
    /// outline between. The outline is in front of the client's own
    /// picture, for a client's shadow is drawn just where it goes, and would
    /// hide it. Its menus are in front of all of it, as they are of the
    /// titlebar: the index stacks them directly above their window (#112),
    /// and what is drawn is what it describes. The title is over the light
    /// behind its button, and both are over the bar.
    ///
    /// `rolled` up to its titlebar, the frame alone, measured from a client
    /// of no height, and nothing of the client: what is drawn of a window
    /// rolled up. A picture of the window alone draws it whole. Behind it
    /// all, its `shadow`, if it casts one and one is wanted: not in a
    /// picture of the window alone, which ends a grip's width past its
    /// frame and would cut a shadow short.
    pub(crate) fn elements<R, C>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
        drawn: Drawn,
    ) -> Vec<C>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
        C: From<FramedElement<R>>,
    {
        // `location` is where the surface's own origin goes; the client
        // geometry, which the frame is measured from, is offset within it.
        let geometry = self.geometry();
        let origin = location + geometry.loc.to_physical_precise_round(scale);
        let height = match drawn.rolling {
            Some(share) => drawn_height(self, share),
            None if drawn.rolled => 0,
            None => geometry.size.h,
        };
        let client = Rect::new(0, 0, geometry.size.w, height);
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
            _ if height == 0 => (Vec::new(), Vec::new()),
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
        let drawing = Drawing {
            origin,
            scale,
            alpha,
        };
        elements.extend(self.dress(|dress| {
            let around = outset(client, dress.outline_around);
            let (strips, corners) = rounded_outline(around, dress.outline, dress.radius);
            let colour = dress.outline_colour;
            let (rounded, unmade) =
                dress.corners(renderer, &corners, dress.radius, colour, &drawing);
            let strips: Vec<Rect> = strips.into_iter().chain(unmade).collect();
            let buffers = &mut dress.outline_strips;
            buffers.resize_with(strips.len(), || SolidColorBuffer::new((0, 0), [0.0; 4]));
            rounded
                .into_iter()
                .map(|corner| C::from(FramedElement::Text(corner)))
                .chain(
                    strips
                        .iter()
                        .zip(buffers)
                        .map(|(strip, buffer)| solid(strip, buffer, colour)),
                )
                .collect::<Vec<_>>()
        }));
        if height < geometry.size.h {
            // Rolling: cut short at the foot of what is drawn of it.
            let down: Point<i32, Physical> =
                Point::<i32, Logical>::from((0, height)).to_physical_precise_round(scale);
            let foot = origin.y + down.y;
            let kept = Rectangle::<i32, Physical>::from_extremities(
                (i32::MIN / 2, i32::MIN / 2),
                (i32::MAX / 2, foot),
            );
            elements.extend(
                own.into_iter()
                    .filter_map(|element| CropRenderElement::from_element(element, scale, kept))
                    .map(|element| C::from(FramedElement::Cropped(element))),
            );
        } else {
            elements.extend(
                own.into_iter()
                    .map(|element| C::from(FramedElement::Surface(element))),
            );
        }

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

            for (wash, kept) in [
                (dress.wash, &mut dress.washes),
                (dress.fading, &mut dress.faded),
            ] {
                if let Some(lit) = wash.and_then(|wash| washed(renderer, kept, wash, &drawing)) {
                    elements.push(C::from(FramedElement::Text(lit)));
                }
            }

            // A corner that cannot be drawn rounded is drawn square, never
            // left a hole in the frame.
            let (strips, corners) = rounded_rects(client, dress.insets, dress.radius);
            let colour = dress.colour;
            let (rounded, unmade) = dress.corners(renderer, &corners, 0, colour, &drawing);
            let strips: Vec<Rect> = strips.into_iter().chain(unmade).collect();
            dress
                .strips
                .resize_with(strips.len(), || SolidColorBuffer::new((0, 0), [0.0; 4]));
            for (strip, buffer) in strips.iter().zip(&mut dress.strips) {
                elements.push(solid(strip, buffer, colour));
            }
            elements.extend(
                rounded
                    .into_iter()
                    .map(|corner| C::from(FramedElement::Text(corner))),
            );
            if drawn.shadow {
                let outer = outset(client, dress.insets);
                elements.extend(
                    dress
                        .shadow_pieces(renderer, outer, &drawing)
                        .into_iter()
                        .map(|piece| C::from(FramedElement::Text(piece))),
                );
            }
        });
        elements
    }
}

impl Dress {
    /// The pieces of the shadow cast by a frame whose outside is `outer`,
    /// if it casts one: each drawn once, a corner as it is and an edge as
    /// one line across it, stretched along it.
    fn shadow_pieces<R>(
        &mut self,
        renderer: &mut R,
        outer: Rect,
        drawing: &Drawing,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(shadow) = self.shadow else {
            return Vec::new();
        };
        let whole = whole_scale(drawing.scale.x);
        let radius = self.radius;
        let colour = Rgba::new(
            shadow.colour.r,
            shadow.colour.g,
            shadow.colour.b,
            u8::try_from(u32::from(shadow.opacity.min(100)) * 255 / 100).unwrap_or(u8::MAX),
        );
        let mut pieces = Vec::new();
        for (n, piece) in shadow_pieces(outer, radius, &shadow)
            .into_iter()
            .enumerate()
        {
            let at = piece.at;
            let sample = match piece.stretch {
                Stretch::None => at,
                Stretch::Across => Rect::new(at.x + at.w / 2, at.y, 1, at.h),
                Stretch::Down => Rect::new(at.x, at.y + at.h / 2, at.w, 1),
            };
            let key = ShadowKey {
                piece: n,
                shadow,
                radius,
                scale: whole,
            };
            if !self.shadows.iter().any(|(drawn, _)| *drawn == key) {
                let size = (sample.w * whole, sample.h * whole);
                let step = 1.0 / f64::from(whole);
                let pixels = crate::raster::shaded(size, colour, |x, y| {
                    let x = f64::from(sample.x) + x * step;
                    let y = f64::from(sample.y) + y * step;
                    shadow_at(x, y, outer, radius, &shadow)
                });
                let buffer = MemoryRenderBuffer::from_slice(
                    &pixels,
                    Fourcc::Argb8888,
                    size,
                    whole,
                    Transform::Normal,
                    None,
                );
                self.shadows.insert(0, (key, buffer));
                self.shadows.truncate(SHADOWS);
            }
            let to =
                drawing.origin + Point::from((at.x, at.y)).to_physical_precise_round(drawing.scale);
            // Its whole picture, stretched to the piece: with no `src`,
            // smithay would read as much of the picture as the piece is big.
            let src = Rectangle::from_size((f64::from(sample.w), f64::from(sample.h)).into());
            if let Some(element) = self
                .shadows
                .iter()
                .find(|(drawn, _)| *drawn == key)
                .and_then(|(_, buffer)| {
                    MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        to.to_f64(),
                        buffer,
                        Some(drawing.alpha),
                        Some(src),
                        Some((at.w, at.h).into()),
                        Kind::Unspecified,
                    )
                    .ok()
                })
            {
                pieces.push(element);
            }
        }
        pieces
    }

    /// `corners`, the top left and the top right, each drawn as a quarter
    /// ring from `inner` round to its square's side, in `colour`, from a
    /// buffer kept for the next frame; and any that could not be made, to
    /// be drawn square.
    fn corners<R>(
        &mut self,
        renderer: &mut R,
        corners: &[Rect],
        inner: i32,
        colour: [f32; 4],
        drawing: &Drawing,
    ) -> (Vec<MemoryRenderBufferRenderElement<R>>, Vec<Rect>)
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let whole = whole_scale(drawing.scale.x);
        let mut rounded = Vec::new();
        let mut unmade = Vec::new();
        for (n, corner) in corners.iter().enumerate() {
            let key = CornerKey {
                side: corner.w,
                inner,
                colour: bytes(colour),
                scale: whole,
                right: n == 1,
            };
            if !self.corners.iter().any(|(drawn, _)| *drawn == key) {
                let side = key.side * whole;
                let pixels =
                    crate::raster::corner(side, (key.inner * whole) as f32, key.colour, key.right);
                let buffer = MemoryRenderBuffer::from_slice(
                    &pixels,
                    Fourcc::Argb8888,
                    (side, side),
                    whole,
                    Transform::Normal,
                    None,
                );
                self.corners.insert(0, (key, buffer));
                self.corners.truncate(CORNERS);
            }
            let at = drawing.origin
                + Point::from((corner.x, corner.y)).to_physical_precise_round(drawing.scale);
            let made = self
                .corners
                .iter()
                .find(|(drawn, _)| *drawn == key)
                .and_then(|(_, buffer)| {
                    MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        at.to_f64(),
                        buffer,
                        Some(drawing.alpha),
                        None,
                        Some((corner.w, corner.h).into()),
                        Kind::Unspecified,
                    )
                    .ok()
                });
            match made {
                Some(element) => rounded.push(element),
                None => unmade.push(*corner),
            }
        }
        (rounded, unmade)
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
        let drawn = Drawn {
            rolled: is_shaded(self),
            rolling: self.rolling_share(),
            shadow: true,
        };
        self.elements(renderer, location, scale, alpha, drawn)
    }
}
