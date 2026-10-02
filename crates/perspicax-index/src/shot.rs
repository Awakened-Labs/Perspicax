//! A picture of the screen, and who drew which part of it.
//!
//! Pixels are the fallback, for what no accessibility bridge describes: a
//! canvas, a video, a toolkit with no bridge. A picture alone would throw
//! away what this project exists to keep, so a shot carries the provenance
//! beside the pixels: every surface in it, where it is in the picture, and
//! the process that drew it. And what the agent holds no consent for is not
//! in the picture at all, but painted over, and listed as such.

use perspicax_node::{Origin, Rect, SurfaceId};

/// What to take a picture of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShotTarget {
    /// A monitor, as it looks: by name, or the first when `None`.
    Output(Option<String>),
    /// One window by itself, as it would look with nothing over it, whether
    /// or not anything is.
    Window(SurfaceId),
}

/// A picture, and an account of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    /// In pixels.
    pub width: u32,
    pub height: u32,
    /// Pixels per logical unit: what a rectangle here is multiplied by to
    /// land in `rgba`.
    pub scale: f64,
    /// The monitor it is of, for a monitor.
    pub output: Option<String>,
    /// Top row first, four bytes a pixel: red, green, blue, alpha.
    pub rgba: Vec<u8>,
    /// Every surface in the picture, front first, with where it is in it in
    /// logical units and who drew it.
    pub drawn: Vec<Drawn>,
    /// Surfaces painted over instead, because the agent holds no consent for
    /// whoever drew them.
    pub redacted: Vec<Drawn>,
}

/// One surface's part in a picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawn {
    pub surface: SurfaceId,
    /// Where it is in the picture, in logical units from its top left.
    pub rect: Rect,
    pub origin: Origin,
}
