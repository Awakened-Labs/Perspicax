//! How far a window's surfaces reach, and so what its geometry is.
//!
//! A client says where its window is inside its surface with
//! `xdg_surface.set_window_geometry`: the visible frame, without the shadow
//! a toolkit draws around it. One that never says is not wrong. xdg-shell
//! defines the case: "If never set, the value is the full bounds of the
//! surface, including any subsurfaces." GStreamer's `waylandsink` is such a
//! client, and so, probably, are some SDL, GLFW, winit and plain EGL ones:
//! nothing in a window without client-side decorations needs a geometry.
//!
//! Smithay knows those bounds only from the buffer sizes its renderer
//! records, and only a backend that keeps buffers records any -- a seat, or
//! headless built with `capture`. A plain headless build releases every
//! buffer the instant it arrives, so to smithay every window there is `0x0`,
//! and a window that declared nothing would be described as nothing: left
//! out of the facts, covering nothing an agent was told it could see
//! (issue #44). So the bounds are measured here, from the sizes
//! [`damage::take`](crate::damage::take) reads as it walks each commit,
//! the same way on every backend.
//!
//! # The same answer smithay draws by
//!
//! [`window_geometry`] is smithay's own `Window::geometry()`: the declared
//! geometry clipped to the bounds, or the bounds. And the bounds are walked
//! as smithay's `bbox_from_surface_tree` walks them, measured at the same
//! moment it measures its own, so wherever smithay can answer, the facts
//! and the seat's drawing agree to the pixel -- the published
//! `buffer_origin` is where smithay draws the surface, and an agent's click
//! is aimed from where it draws it. A seat, and headless with `capture`,
//! check that agreement at every commit in a debug build
//! (`check_drawn_as_measured`).
//!
//! Two places differ, both on purpose. A window that has drawn nothing yet
//! keeps the geometry it declared, so it is still described, unmapped,
//! rather than at smithay's `0x0`. And a declared geometry without area is
//! no geometry at all, where smithay would clip it into a rectangle that
//! covers nothing: `0x0`, or a negative size, which a release build of
//! smithay takes without complaint (a debug build asserts against it as it
//! reads the request).
//!
//! # A viewport
//!
//! A surface is not always the size of its buffer. With `wp_viewporter` a
//! client crops its buffer to a source rectangle and says what size that is
//! shown at, and the destination is the surface's size. A client drawing at
//! a fractional scale does this on every frame: Firefox on a monitor at 0.9
//! draws a buffer 0.9 times its window and shows it at the window's size
//! (issue #135). So what is kept for each surface is its [`Picture`]: the
//! buffer as smithay's renderer keeps it, and how it is viewed, read the way
//! smithay's renderer reads it.

use std::sync::{Mutex, PoisonError};

use perspicax_node::Rect;
use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Buffer as BufferCoords, Logical, Point, Rectangle, Size, Transform},
    wayland::{
        compositor::{
            SubsurfaceCachedState, SurfaceAttributes, SurfaceData, TraversalAction, with_states,
            with_surface_tree_downward,
        },
        shell::xdg::SurfaceCachedState,
        viewporter::ViewportCachedState,
    },
};

/// The picture a surface shows: its buffer, as the last new one was
/// attached, and how much of it is shown at what size.
///
/// Smithay's renderer keeps the same (`RendererSurfaceState` and its
/// `SurfaceView`), and sizes the surface by it, so this is what keeps the
/// facts on the pixels it draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Picture {
    /// The buffer's own size, in its pixels: what damage named in buffer
    /// coordinates is named in.
    pub(crate) pixels: Size<i32, BufferCoords>,
    /// The scale the client drew it at, and how it turned it. Those it
    /// attached it with: a scale or a turn changed without a new buffer
    /// waits for the next one, as smithay's renderer has it.
    pub(crate) scale: i32,
    pub(crate) transform: Transform,
    /// The part of it shown, in its own logical size: the viewport's source,
    /// or all of it.
    pub(crate) src: Rectangle<f64, Logical>,
    /// The size it is shown at, which is the surface's size: the viewport's
    /// destination, or its source cut to whole pixels, or its own.
    pub(crate) size: Size<i32, Logical>,
}

impl Picture {
    /// A buffer of `pixels`, drawn at `scale` and turned by `transform`,
    /// shown as `viewport` says.
    ///
    /// Smithay's `SurfaceView::from_states`, which is private: the source
    /// is all of the buffer unless the viewport crops it, and the size is
    /// the viewport's (`ViewportCachedState::size`, whose source cut to
    /// whole pixels is smithay's own), else the buffer's own. Smithay also
    /// divides that last by the client's scale, which only an Xwayland run
    /// at a scale sets and this compositor never does, so it is 1.
    pub(crate) fn viewed(
        pixels: Size<i32, BufferCoords>,
        scale: i32,
        transform: Transform,
        viewport: &ViewportCachedState,
    ) -> Self {
        let own = pixels.to_logical(scale, transform);
        Self {
            pixels,
            scale,
            transform,
            src: viewport
                .src
                .unwrap_or_else(|| Rectangle::from_size(own.to_f64())),
            size: viewport.size().unwrap_or(own),
        }
    }

    /// The same buffer, shown as `viewport` says now. A viewport is the
    /// surface's state, not the buffer's, and can change without one.
    pub(crate) fn seen_through(self, viewport: &ViewportCachedState) -> Self {
        Self::viewed(self.pixels, self.scale, self.transform, viewport)
    }

    /// The picture's size in its own logical pixels, before any viewport:
    /// what a source rectangle is measured within.
    pub(crate) fn own(&self) -> Size<i32, Logical> {
        self.pixels.to_logical(self.scale, self.transform)
    }

    /// Where `rect`, a part of the picture in its own logical size, is
    /// shown on the surface: cropped to the source and scaled from it to
    /// the destination, in smithay's order (its surface element's damage).
    /// None for a part outside the source, which is not shown at all.
    pub(crate) fn place(&self, rect: Rectangle<f64, Logical>) -> Option<Rect> {
        let shown = rect.intersection(self.src)?;
        let (x, y) = (
            f64::from(self.size.w) / self.src.size.w,
            f64::from(self.size.h) / self.src.size.h,
        );
        let at = shown.loc - self.src.loc;
        Some(Rect::new(
            at.x * x,
            at.y * y,
            (at.x + shown.size.w) * x,
            (at.y + shown.size.h) * y,
        ))
    }
}

/// The picture a surface shows, as the last commit to change it left it.
/// `None` when it shows nothing: its buffer was taken away, or was one whose
/// size cannot be read, which smithay's renderer draws as nothing too.
struct Shows(Mutex<Option<Picture>>);

/// What a tree of surfaces covered when it was last measured.
#[derive(Debug, Clone, Default)]
struct Tree {
    /// The full bounds of the surfaces that show something, in the root's
    /// coordinates. `None` while the root shows nothing.
    bounds: Option<Rectangle<i32, Logical>>,
    /// Where each subsurface showing something is opaque, in the root's
    /// coordinates: the region it declared, or all of it if it declared
    /// none.
    covers: Vec<Rect>,
}

/// A root's [`Tree`], kept on it.
struct Measured(Mutex<Tree>);

/// Remember the picture a commit gave a surface, or that it took the
/// picture away. Called from the walk that reads the commit, while the
/// buffer is still there to be measured.
pub(crate) fn record(states: &SurfaceData, picture: Option<Picture>) {
    states
        .data_map
        .insert_if_missing_threadsafe(|| Shows(Mutex::new(None)));
    if let Some(shows) = states.data_map.get::<Shows>() {
        *shows.0.lock().unwrap_or_else(PoisonError::into_inner) = picture;
    }
}

/// The size `surface` is shown at, as its last commit left it: `None` while
/// it shows nothing.
pub(crate) fn shown(surface: &WlSurface) -> Option<Size<i32, Logical>> {
    with_states(surface, recorded)
}

/// The size at which [`record`] last left the surface whose states these
/// are, for a walk that already holds them.
pub(crate) fn recorded(states: &SurfaceData) -> Option<Size<i32, Logical>> {
    picture(states).map(|picture| picture.size)
}

/// The picture [`record`] last kept for the surface whose states these are.
pub(crate) fn picture(states: &SurfaceData) -> Option<Picture> {
    states
        .data_map
        .get::<Shows>()
        .and_then(|shows| *shows.0.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Measure the tree under `root` again, after a commit changed something in
/// it, and keep the answer on the root.
///
/// Walked as smithay's `bbox_from_surface_tree` walks: from the root's
/// origin, taking in every surface that shows a picture, where it sits; a
/// surface showing nothing hides everything under it, as the protocol
/// says. Kept rather than walked again on every read, because smithay keeps
/// its own: when a subsurface goes without a commit after it, both answers
/// stay as they were until the next one, and stay equal.
pub(crate) fn measure(root: &WlSurface) {
    let mut bounds = Rectangle::default();
    let mut shown = false;
    let mut covers = Vec::new();
    with_surface_tree_downward(
        root,
        Point::default(),
        // All of it in the filter, as in `damage::take`: the filter is what
        // hands the children their offset.
        |surface, states, &above| {
            let at = if surface == root {
                above
            } else {
                above
                    + states
                        .cached_state
                        .get::<SubsurfaceCachedState>()
                        .current()
                        .location
            };
            let Some(size) = recorded(states) else {
                return TraversalAction::SkipChildren;
            };
            let placed = Rectangle::new(at, size);
            bounds = bounds.merge(placed);
            shown = true;
            if surface != root {
                covers.extend(opaque_in_root(states, placed));
            }
            TraversalAction::DoChildren(at)
        },
        |_, _, _| {},
        |_, _, _| true,
    );
    let tree = Tree {
        bounds: shown.then_some(bounds),
        covers,
    };
    with_states(root, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe(|| Measured(Mutex::new(Tree::default())));
        if let Some(measured) = states.data_map.get::<Measured>() {
            *measured.0.lock().unwrap_or_else(PoisonError::into_inner) = tree;
        }
    });
}

/// Where a subsurface placed at `placed` in its root is opaque: the region
/// it declared, clipped to it, or all of it. A surface that declared nothing
/// proves nothing, so it is taken to cover everything it shows.
fn opaque_in_root(states: &SurfaceData, placed: Rectangle<i32, Logical>) -> Vec<Rect> {
    let whole = crate::damage::to_rect(placed);
    let declared = states
        .cached_state
        .get::<SurfaceAttributes>()
        .current()
        .opaque_region
        .as_ref()
        .map(crate::facts::regions);
    match declared {
        Some(regions) => regions
            .into_iter()
            .map(|region| {
                Rect::new(
                    region.x0 + whole.x0,
                    region.y0 + whole.y0,
                    region.x1 + whole.x0,
                    region.y1 + whole.y0,
                )
                .intersect(whole)
            })
            .filter(|region| !region.is_empty())
            .collect(),
        None => vec![whole],
    }
}

/// A window's geometry, in its surface's own coordinates: where the window
/// is inside the surface, and how big.
///
/// What its client declared, clipped to what it draws; or, if it declared
/// none, the full bounds of what it draws, as xdg-shell says. A window that
/// has drawn nothing has only what it declared, and one that has declared
/// nothing either has no geometry: it is not on screen, and there is nothing
/// to describe.
pub(crate) fn window_geometry(surface: &WlSurface) -> Option<Rectangle<i32, Logical>> {
    let (declared, bounds) = with_states(surface, |states| {
        let declared = states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry;
        (declared, bounds(states))
    });
    // Not `Rectangle::is_empty`, which only asks whether a side is zero: a
    // release build of smithay takes a negative size without complaint, and
    // a rectangle turned inside out covers nothing.
    let declared = declared.filter(|declared| declared.size.w > 0 && declared.size.h > 0);
    match bounds {
        Some(bounds) => Some(
            declared
                .and_then(|declared| declared.intersection(bounds))
                .unwrap_or(bounds),
        ),
        None => declared,
    }
}

/// Everything the tree under `surface` draws, in its surface's own
/// coordinates: the full bounds of every surface in it that shows
/// something, as the last commit left them. `None` while it shows nothing.
///
/// Where a window is inside what it draws is its client's to say, and
/// [`window_geometry`] says it. How far the drawing reaches is not, and
/// this is what covers whatever is under it (issue #103).
pub(crate) fn drawn(surface: &WlSurface) -> Option<Rectangle<i32, Logical>> {
    with_states(surface, bounds)
}

/// The bounds [`measure`] last kept on the root whose states these are.
fn bounds(states: &SurfaceData) -> Option<Rectangle<i32, Logical>> {
    states.data_map.get::<Measured>().and_then(|measured| {
        measured
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bounds
    })
}

/// Where the subsurfaces of the window whose surface is `surface` are
/// opaque, in its coordinates. Added to the region its own surface
/// declared, which says nothing about what is drawn over it.
pub(crate) fn covers(surface: &WlSurface) -> Vec<Rect> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<Measured>()
            .map(|measured| {
                measured
                    .0
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .covers
                    .clone()
            })
            .unwrap_or_default()
    })
}

/// In a debug build, that the bounds measured here are the ones smithay's
/// renderer measured for the same window at the same commit. Every live
/// test built with `capture` runs through here, so a walk that drifted from
/// smithay's would fail them rather than misplace a click on a seat.
#[cfg(any(feature = "seat", feature = "capture"))]
pub(crate) fn check_drawn_as_measured(window: &smithay::desktop::Window) {
    let Some(toplevel) = window.toplevel() else {
        return;
    };
    let measured = drawn(toplevel.wl_surface());
    debug_assert_eq!(
        measured.unwrap_or_default(),
        window.bbox(),
        "the bounds measured for the facts are the ones smithay draws by"
    );
}
