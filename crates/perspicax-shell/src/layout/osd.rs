//! The volume shown on the screen a moment when it changes: what it shows,
//! how big it is, and when a change is worth showing.
//!
//! It is a box [`WIDE`] by [`TALL`], [`ABOVE`] above the foot of the room
//! beside a panel along the bottom, of the speaker as the panel draws it, the
//! output's name, how loud it is in percent, and a bar as far as that.

use super::Rect;
use crate::model::sound::Level;

/// How big the box is, and how far it stands above the foot of the room
/// it has.
pub(crate) const WIDE: u32 = 280;
pub(crate) const TALL: u32 = 64;
pub(crate) const ABOVE: i32 = 48;
/// The room inside its edge, and the speaker's square.
const PAD: i32 = 12;
const GLYPH: i32 = 32;
/// How tall its name's line is, and the bar under it.
const LINE: i32 = 22;
const BAR: i32 = 6;

/// What it shows: the output's name, and how loud it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Indicator {
    pub(crate) name: String,
    pub(crate) level: Level,
}

/// Where each of its parts is, in its own logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placed {
    pub(crate) glyph: Rect,
    pub(crate) name: Rect,
    pub(crate) percent: Rect,
    pub(crate) bar: Rect,
}

/// Its parts, laid out.
pub(crate) fn lay_out() -> Placed {
    let (wide, tall) = (WIDE as i32, TALL as i32);
    let glyph = Rect::new(PAD, (tall - GLYPH) / 2, GLYPH, GLYPH);
    let left = glyph.right() + PAD;
    let percent_wide = 48;
    let top = (tall - LINE - PAD / 2 - BAR) / 2;
    let percent = Rect::new(wide - PAD - percent_wide, top, percent_wide, LINE);
    let name = Rect::new(left, top, percent.x - left, LINE);
    let bar = Rect::new(left, name.bottom() + PAD / 2, wide - PAD - left, BAR);
    Placed {
        glyph,
        name,
        percent,
        bar,
    }
}

/// Whether going from `before` to `now`, what was last heard of the
/// output and what is heard now, is worth showing: the output, its volume
/// or its muting changed. Nothing before, as when the server is first
/// heard, is no change; nor is no output now.
pub(crate) fn changed(before: Option<&Indicator>, now: Option<&Indicator>) -> bool {
    matches!((before, now), (Some(before), Some(now)) if before != now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(name: &str, percent: u32, muted: bool) -> Indicator {
        Indicator {
            name: name.to_owned(),
            level: Level { percent, muted },
        }
    }

    #[test]
    fn a_change_of_volume_muting_or_output_is_shown_and_nothing_else_is() {
        let speakers = at("Speakers", 40, false);
        for now in [
            at("Speakers", 45, false),
            at("Speakers", 40, true),
            at("E8", 40, false),
        ] {
            assert!(changed(Some(&speakers), Some(&now)), "{now:?}");
        }
        assert!(
            !changed(Some(&speakers), Some(&speakers.clone())),
            "the same"
        );
        assert!(!changed(None, Some(&speakers)), "first heard");
        assert!(!changed(Some(&speakers), None), "no output now");
    }

    #[test]
    fn its_parts_are_inside_it_the_name_and_percentage_over_the_bar() {
        let placed = lay_out();
        let whole = Rect::new(0, 0, WIDE as i32, TALL as i32);
        for part in [placed.glyph, placed.name, placed.percent, placed.bar] {
            assert!(
                part.x >= 0 && part.y >= 0 && part.right() <= whole.w && part.bottom() <= whole.h,
                "{part:?}"
            );
        }
        assert!(placed.glyph.right() < placed.name.x);
        assert!(placed.name.right() <= placed.percent.x);
        assert!(placed.bar.y > placed.name.bottom());
        assert_eq!(
            placed.bar.right(),
            placed.percent.right(),
            "as wide as the words"
        );
    }
}
