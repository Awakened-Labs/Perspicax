//! The ranges each number in the file is held to, by the key it is written
//! at. The parser refuses a value outside its range with these ends in its
//! message, and the settings app offers the same ends: one number, read in
//! both places.

use std::ops::RangeInclusive;

use perspicax_policy::{FAR, OPAQUE, SCALES};

/// `[workspaces] grid`, each side. Generous: past this, a grid is a typo,
/// and a 1000x1000 one would be a million workspaces.
pub const GRID_SIDE: RangeInclusive<u16> = 1..=16;

/// `[workspaces.flip] delay-ms`. Longer than this and a person would think
/// edge flipping was broken.
pub const FLIP_DELAY_MS: RangeInclusive<u64> = 0..=5000;

/// `[workspaces.transition] duration-ms`. 0 is no transition, which is
/// `effect = "none"`; longer than this and a switch would feel stuck rather
/// than shown.
pub const TRANSITION_MS: RangeInclusive<u64> = 1..=2000;

/// `[[output]] position`, each of its two, and `offset`, in logical pixels:
/// as far as the world goes (#154), which is further than any desk of
/// monitors reaches.
pub const OUTPUT_PLACE: RangeInclusive<i32> = -FAR..=FAR;

/// `[snap] threshold`, in pixels from the edge.
pub const SNAP_THRESHOLD: RangeInclusive<i32> = 1..=64;

/// `[resistance] edges` and `seams`, in pixels. More than this and an edge
/// is a wall a person has to fight.
pub const RESISTANCE: RangeInclusive<i32> = 0..=128;

/// `[opacity] step`, `floor`, `unfocused` and each of `apps`, in percent.
pub const OPACITY: RangeInclusive<u8> = 1..=OPAQUE;

/// `[input.pointer] double-click-ms`. Faster than the shortest, a hand
/// cannot click twice on purpose; slower than the longest, two clicks a
/// person meant as two would open what they only meant to select.
pub const DOUBLE_CLICK_MS: RangeInclusive<u32> = 100..=2000;

/// `[input.pointer] accel`, from slowest to fastest.
pub const ACCEL: RangeInclusive<f64> = -1.0..=1.0;

/// `[input.keyboard] repeat-rate` (keys a second) and `repeat-delay`
/// (milliseconds): anything positive.
pub const REPEAT: RangeInclusive<i32> = 1..=i32::MAX;

/// How many layouts a keyboard holds, and so how many `layout-N` can name:
/// xkb holds four at most.
pub const LAYOUTS: RangeInclusive<u8> = 1..=4;

/// `[decorations] title-height`, in pixels.
pub const TITLE_HEIGHT: RangeInclusive<i32> = 8..=96;

/// `[decorations] border`, in pixels.
pub const BORDER: RangeInclusive<i32> = 0..=32;

/// `[decorations] corner-radius`, in pixels.
pub const CORNER_RADIUS: RangeInclusive<i32> = 0..=16;

/// `[decorations.shadow] size`, in pixels.
pub const SHADOW_SIZE: RangeInclusive<i32> = 0..=64;

/// `[decorations.shadow] offset`, each way, in pixels.
pub const SHADOW_OFFSET: RangeInclusive<i32> = -32..=32;

/// `[decorations.shadow] opacity`, in percent.
pub const SHADOW_OPACITY: RangeInclusive<i32> = 0..=100;

/// `[decorations.agent] outline`, in pixels; 0 draws none.
pub const AGENT_OUTLINE: RangeInclusive<i32> = 0..=8;

/// `[decorations.animation] shade-ms` and `hover-ms`; 0 is none.
pub const ANIMATION_MS: RangeInclusive<u32> = 0..=1000;

/// `[theme] font-size`, in pixels.
pub const FONT_SIZE: RangeInclusive<u16> = 10..=20;

/// `[theme] cursor-size`, in pixels.
pub const CURSOR_SIZE: RangeInclusive<u32> = 8..=128;

/// `[shell.panel] height`, in pixels: from the shortest panel that still
/// holds a line of text to the tallest that is not a typo.
pub const PANEL_HEIGHT: RangeInclusive<u32> = 16..=128;

/// `[shell.panel] width` written as a share of its monitor, in percent.
pub const PANEL_SHARE: RangeInclusive<u32> = 1..=100;

/// `[[output]] scale`, as a display tool may also ask for it.
pub const SCALE: RangeInclusive<f64> = SCALES;
