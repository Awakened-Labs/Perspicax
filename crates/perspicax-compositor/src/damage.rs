//! Where a commit changed a window's pixels: the damage brought to a surface
//! and to the subsurfaces under it, in the coordinates of the surface at the
//! top of the tree.
//!
//! A window is a tree of surfaces. Its own surface is the root, and a client
//! may hang subsurfaces under it and draw into those instead. Firefox draws
//! every page into one and leaves its toplevel alone; video players, GTK 4's
//! graphics offload and Chromium's overlays do the same. What a client drew
//! is its window's, whichever surface it went into, so it is counted against
//! the window, moved by where the subsurface sits in it.
//!
//! A popup -- a menu, a combo list, a tooltip -- is the root of a tree of its
//! own, and the window it belongs to is found through xdg-shell instead:
//! [`popup`](crate::popup) says where it is drawn, and its tree is read here
//! from there, in the window's coordinates.
//!
//! # When a subsurface's damage arrives
//!
//! Smithay calls the compositor once for each surface whose state it applies,
//! after applying it. A synchronized subsurface's own commit applies nothing:
//! its state waits for the nearest unsynchronized surface above it to commit,
//! and is applied in that surface's transaction, ahead of it. So the call
//! worth reading is the one for the surface that committed, and what its
//! commit brought is in every surface under it. Smithay's renderer reads
//! buffers the same way (`on_commit_buffer_handler`), and this follows it:
//! a synchronized subsurface is skipped, and the tree is walked from the
//! surface that committed. A surface under it that brought nothing new adds
//! nothing, because what it brought before was taken when it did.
//!
//! # What a frame is
//!
//! A new buffer, or damage named. A commit that brings neither, which a
//! client sends to ask for a frame callback, changed nothing on screen and is
//! not counted. Loading a page, Firefox committed its subsurface 252 times
//! and drew in 79 of them.

use perspicax_node::{Rect, Vec2};
use smithay::{
    backend::renderer::buffer_dimensions,
    reexports::wayland_server::protocol::{wl_output, wl_surface::WlSurface},
    utils::{Buffer as BufferCoords, Logical, Size},
    wayland::compositor::{
        BufferAssignment, Damage, SubsurfaceCachedState, SurfaceAttributes, SurfaceData,
        TraversalAction, get_parent, with_states, with_surface_tree_downward,
    },
};

/// What a commit did to a surface's picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Buffer {
    /// A new one. The size it gives its surface, when Smithay can read the
    /// size of a buffer of its kind.
    New(Option<Size<i32, Logical>>),
    /// Taken away: the surface shows nothing now.
    Removed,
    /// The one it had before.
    Unchanged,
}

/// What a commit brought, across the tree under the surface that committed.
#[derive(Debug)]
pub(crate) struct Taken {
    /// Where pixels changed, in the root's coordinates.
    pub(crate) rects: Vec<Rect>,
    /// What the commit did to the committing surface's own picture.
    pub(crate) own: Buffer,
}

/// The surface at the top of `surface`'s subsurface tree, which is the one
/// a window or a layer is, and where `surface` sits in its coordinates. A
/// surface that is no subsurface is its own root, at its own origin.
pub(crate) fn root_of(surface: &WlSurface) -> (WlSurface, Vec2) {
    let mut root = surface.clone();
    let mut at = Vec2::ZERO;
    while let Some(parent) = get_parent(&root) {
        at += with_states(&root, offset_in_parent);
        root = parent;
    }
    (root, at)
}

/// Read what the commit of `start`, which sits at `at` in its root, brought
/// to it and to every surface under it.
///
/// `whole` is the root's whole extent: the answer for a change that cannot
/// say where it landed. When `releases`, each surface's damage is consumed
/// and its new buffer released, which is what a compositor that never reads
/// a pixel does with them; otherwise both are left for the renderer.
pub(crate) fn take(start: &WlSurface, at: Vec2, whole: Option<Rect>, releases: bool) -> Taken {
    let mut rects = Vec::new();
    let mut own = Buffer::Unchanged;
    with_surface_tree_downward(
        start,
        at,
        // All of it in the filter, as Smithay's own bounding-box walk does:
        // the filter is what hands the children their offset, and the
        // processor would be handed the parent's. Nothing in here may reach
        // another surface's state, either, because the walk holds this
        // surface's lock until everything under it has been visited.
        |surface, states, &above| {
            let at = if surface == start {
                above
            } else {
                above + offset_in_parent(states)
            };
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            let current = attributes.current();
            let scale = current.buffer_scale.max(1);
            let named: Vec<Rect> = current
                .damage
                .iter()
                .map(|damage| surface_local(damage, f64::from(scale)))
                .collect();
            let buffer = match &current.buffer {
                Some(BufferAssignment::NewBuffer(buffer)) => Buffer::New(
                    buffer_dimensions(buffer)
                        .map(|size| logical(size, scale, current.buffer_transform)),
                ),
                Some(BufferAssignment::Removed) => Buffer::Removed,
                None => Buffer::Unchanged,
            };
            // How far the window reaches is measured from these, so they are
            // remembered before the buffer is released below.
            match buffer {
                Buffer::New(size) => crate::geometry::record(states, size),
                Buffer::Removed => crate::geometry::record(states, None),
                Buffer::Unchanged => {}
            }
            rects.extend(damaged(&named, buffer, at, whole));
            if surface == start {
                own = buffer;
            }
            if releases {
                current.damage.clear();
                if let Some(BufferAssignment::NewBuffer(buffer)) = current.buffer.take() {
                    buffer.release();
                }
            }
            TraversalAction::DoChildren(at)
        },
        |_, _, _| {},
        |_, _, _| true,
    );
    Taken { rects, own }
}

/// Clear what damage is left on `start` and every surface under it: what the
/// renderer did not take, because no new buffer came with it. So no commit's
/// damage is ever counted twice.
#[cfg(any(feature = "seat", feature = "capture"))]
pub(crate) fn clear(start: &WlSurface) {
    with_surface_tree_downward(
        start,
        (),
        |_, states, &()| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .damage
                .clear();
            TraversalAction::DoChildren(())
        },
        |_, _, &()| {},
        |_, _, &()| true,
    );
}

/// What one surface's commit damaged, in its root's coordinates.
///
/// `named` is the damage it named, in its own coordinates, and `at` is where
/// it sits in the root. `whole` is the root's whole extent.
pub(crate) fn damaged(named: &[Rect], buffer: Buffer, at: Vec2, whole: Option<Rect>) -> Vec<Rect> {
    match buffer {
        // A new picture changed all of itself, if it named nothing, and at
        // most all of itself if it did: what it names is clipped to it, as
        // the renderer clips it. A toolkit names its whole buffer with the
        // largest rectangle the protocol allows, and unclipped, on a
        // subsurface, that would claim everything below and to the right.
        Buffer::New(Some(size)) if named.is_empty() => vec![extent(size) + at],
        Buffer::New(Some(size)) => named
            .iter()
            .map(|region| region.intersect(extent(size)))
            .filter(|region| !region.is_empty())
            .map(|region| region + at)
            .collect(),
        // A buffer of a kind whose size cannot be read, naming nothing: all
        // of the window, as far as anyone here can tell.
        Buffer::New(None) if named.is_empty() => whole.into_iter().collect(),
        Buffer::New(None) | Buffer::Unchanged => named.iter().map(|region| *region + at).collect(),
        // What the picture covered shows what is beneath it now, and where
        // that was went with the buffer, so the safe answer is all of it.
        Buffer::Removed => whole.into_iter().collect(),
    }
}

/// The size a buffer gives its surface: its own, divided by the scale and
/// turned by the transform the client declared for it, as smithay's
/// renderer sizes it.
fn logical(
    size: Size<i32, BufferCoords>,
    scale: i32,
    transform: wl_output::Transform,
) -> Size<i32, Logical> {
    size.to_logical(scale, transform.into())
}

/// The whole of a picture of `size`, in its surface's own coordinates.
fn extent(size: Size<i32, Logical>) -> Rect {
    Rect::new(0.0, 0.0, f64::from(size.w), f64::from(size.h))
}

/// A smithay rectangle as a [`Rect`], divided by `scale`. In floating point
/// throughout, so a client's `i32::MAX` extent cannot overflow on the way.
pub(crate) fn to_rect<Kind>(rect: smithay::utils::Rectangle<i32, Kind>, scale: f64) -> Rect {
    let (x, y) = (f64::from(rect.loc.x), f64::from(rect.loc.y));
    Rect::new(
        x / scale,
        y / scale,
        (x + f64::from(rect.size.w)) / scale,
        (y + f64::from(rect.size.h)) / scale,
    )
}

/// A damage rectangle in its surface's own coordinates.
fn surface_local(damage: &Damage, scale: f64) -> Rect {
    match *damage {
        Damage::Surface(rect) => to_rect(rect, 1.0),
        // Buffer coordinates are the surface's multiplied by the scale the
        // client declared, so dividing is what puts them back into the
        // space every other rectangle here uses.
        Damage::Buffer(rect) => to_rect(rect, scale),
    }
}

/// Where a subsurface sits in its parent. Zero for a surface that is none.
fn offset_in_parent(states: &SurfaceData) -> Vec2 {
    let location = states
        .cached_state
        .get::<SubsurfaceCachedState>()
        .current()
        .location;
    Vec2::new(f64::from(location.x), f64::from(location.y))
}

#[cfg(test)]
mod tests {
    use smithay::utils::Rectangle;

    use super::*;

    const AT: Vec2 = Vec2::new(40.0, 30.0);
    const WHOLE: Option<Rect> = Some(Rect::new(0.0, 0.0, 400.0, 300.0));

    /// A new picture 120 by 80.
    fn new() -> Buffer {
        Buffer::New(Some(Size::new(120, 80)))
    }

    #[test]
    fn damage_on_a_subsurface_moves_by_where_it_sits() {
        let named = [Rect::new(10.0, 10.0, 20.0, 20.0)];
        assert_eq!(
            damaged(&named, new(), AT, WHOLE),
            [Rect::new(50.0, 40.0, 60.0, 50.0)]
        );
    }

    #[test]
    fn buffer_damage_is_divided_by_its_own_surfaces_scale_before_it_moves() {
        let rect: Rectangle<i32, BufferCoords> = Rectangle::new((80, 60).into(), (40, 40).into());
        let named = [surface_local(&Damage::Buffer(rect), 2.0)];
        assert_eq!(named, [Rect::new(40.0, 30.0, 60.0, 50.0)]);
        assert_eq!(
            damaged(&named, new(), AT, WHOLE),
            [Rect::new(80.0, 60.0, 100.0, 80.0)]
        );
    }

    #[test]
    fn a_new_buffer_naming_no_damage_changed_all_of_itself() {
        assert_eq!(
            damaged(&[], new(), AT, WHOLE),
            [Rect::new(40.0, 30.0, 160.0, 110.0)]
        );
    }

    #[test]
    fn a_new_buffer_of_unknown_size_naming_no_damage_changed_the_whole_window() {
        assert_eq!(damaged(&[], Buffer::New(None), AT, WHOLE), WHOLE.as_slice());
    }

    #[test]
    fn damage_named_is_clipped_to_the_new_buffer() {
        let named = [
            Rect::new(0.0, 0.0, f64::from(i32::MAX), f64::from(i32::MAX)),
            // Entirely outside it: nothing.
            Rect::new(200.0, 200.0, 210.0, 210.0),
        ];
        assert_eq!(
            damaged(&named, new(), AT, WHOLE),
            [Rect::new(40.0, 30.0, 160.0, 110.0)]
        );
    }

    /// Sized as smithay's renderer sizes it: a buffer turned a quarter
    /// gives its surface its height for a width, and a scale of 2 halves
    /// both.
    #[test]
    fn a_buffer_turned_a_quarter_gives_its_surface_its_sides_swapped() {
        let size = Size::new(200, 100);
        assert_eq!(
            logical(size, 1, wl_output::Transform::_90),
            Size::new(100, 200)
        );
        assert_eq!(
            logical(size, 2, wl_output::Transform::Flipped270),
            Size::new(50, 100)
        );
        assert_eq!(
            logical(size, 2, wl_output::Transform::Normal),
            Size::new(100, 50)
        );
    }

    #[test]
    fn a_commit_with_no_new_buffer_and_no_damage_changed_nothing() {
        assert!(damaged(&[], Buffer::Unchanged, AT, WHOLE).is_empty());
    }

    #[test]
    fn damage_named_without_a_new_buffer_is_still_counted() {
        let named = [Rect::new(0.0, 0.0, 10.0, 10.0)];
        assert_eq!(
            damaged(&named, Buffer::Unchanged, AT, WHOLE),
            [Rect::new(40.0, 30.0, 50.0, 40.0)]
        );
    }

    #[test]
    fn a_picture_taken_away_changed_the_whole_window() {
        assert_eq!(damaged(&[], Buffer::Removed, AT, WHOLE), WHOLE.as_slice());
        // A root that has never shown anything has no whole to name.
        assert!(damaged(&[], Buffer::Removed, AT, None).is_empty());
    }

    #[test]
    fn an_enormous_damage_rectangle_does_not_overflow() {
        let rect: Rectangle<i32, Logical> =
            Rectangle::new((1, 1).into(), (i32::MAX, i32::MAX).into());
        let region = to_rect(rect, 1.0);
        assert_eq!(region.x1, f64::from(i32::MAX) + 1.0);
        assert_eq!(region.y1, f64::from(i32::MAX) + 1.0);
    }
}
