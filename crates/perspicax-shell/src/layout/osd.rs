//! What is shown on the screen a moment: the volume when it changes, and
//! why something the person asked for was refused. What each shows, how
//! big its box is and where its parts go, and when a change of volume is
//! worth showing.
//!
//! The box stands [`ABOVE`] above the foot of the room beside a panel along
//! the bottom. The volume's is [`WIDE`] by [`TALL`], of the speaker as the
//! panel draws it, the output's name, how loud it is in percent, and a bar
//! as far as that. A notice's is as wide as its longer line needs, from
//! [`WIDE`] to [`WIDEST`], and [`TALL_NOTICE`]: a "no" sign, the headline,
//! and why under it, each line cut short if it is longer still.

use super::{Measure, Rect};
use crate::model::notice::Notice;
#[cfg(feature = "audio")]
use crate::model::sound::Level;

/// How wide the box is at least, and at most; and how far it stands above
/// the foot of the room it has.
pub(crate) const WIDE: u32 = 280;
pub(crate) const WIDEST: u32 = 560;
pub(crate) const ABOVE: i32 = 48;
/// How tall the volume's box is, and a notice's.
#[cfg(feature = "audio")]
pub(crate) const TALL: u32 = 64;
pub(crate) const TALL_NOTICE: u32 = 72;
/// The room inside its edge, and the glyph's square.
const PAD: i32 = 12;
const GLYPH: i32 = 32;
/// How tall a line of text is, the room between a notice's two, and the
/// volume's bar.
const LINE: i32 = 22;
const GAP: i32 = 4;
#[cfg(feature = "audio")]
const BAR: i32 = 6;

/// What the box shows: the last of these to come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Showing {
    #[cfg(feature = "audio")]
    Volume(Indicator),
    Notice(Notice),
}

impl Showing {
    /// How big its box is, its text `measure`d.
    pub(crate) fn size(&self, measure: &mut impl Measure) -> (u32, u32) {
        match self {
            #[cfg(feature = "audio")]
            Self::Volume(_) => (WIDE, TALL),
            Self::Notice(notice) => {
                let longer = measure
                    .width(&notice.headline)
                    .max(measure.width(&notice.why));
                // Two pixels over, so a line just as wide is not cut short.
                let wide = u32::try_from(GLYPH + 3 * PAD + longer.ceil() as i32 + 2).unwrap_or(0);
                (wide.clamp(WIDE, WIDEST), TALL_NOTICE)
            }
        }
    }
}

/// What the volume shows: the output's name, and how loud it is.
#[cfg(feature = "audio")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Indicator {
    pub(crate) name: String,
    pub(crate) level: Level,
}

/// Where each of the volume's parts is, in its own logical pixels.
#[cfg(feature = "audio")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placed {
    pub(crate) glyph: Rect,
    pub(crate) name: Rect,
    pub(crate) percent: Rect,
    pub(crate) bar: Rect,
}

/// The volume's parts, laid out.
#[cfg(feature = "audio")]
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

/// Where each of a notice's parts is, in its own logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlacedNotice {
    pub(crate) glyph: Rect,
    pub(crate) headline: Rect,
    pub(crate) why: Rect,
}

/// A notice's parts, laid out in a box `wide` by `tall`: the size it asked
/// for, or another the compositor gave it.
pub(crate) fn lay_out_notice((wide, tall): (i32, i32)) -> PlacedNotice {
    let glyph = Rect::new(PAD, (tall - GLYPH) / 2, GLYPH, GLYPH);
    let left = glyph.right() + PAD;
    let line = |top| Rect::new(left, top, (wide - PAD - left).max(0), LINE);
    let headline = line(PAD);
    PlacedNotice {
        glyph,
        headline,
        why: line(headline.bottom() + GAP),
    }
}

/// Whether going from `before` to `now`, what was last heard of the
/// output and what is heard now, is worth showing: the output, its volume
/// or its muting changed. Nothing before, as when the server is first
/// heard, is no change; nor is no output now.
#[cfg(feature = "audio")]
pub(crate) fn changed(before: Option<&Indicator>, now: Option<&Indicator>) -> bool {
    matches!((before, now), (Some(before), Some(now)) if before != now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Monospace;

    fn inside(part: Rect, (wide, tall): (u32, u32)) -> bool {
        part.x >= 0 && part.y >= 0 && part.right() <= wide as i32 && part.bottom() <= tall as i32
    }

    fn notice(headline: &str, why: &str) -> Notice {
        Notice {
            headline: headline.to_owned(),
            why: why.to_owned(),
        }
    }

    #[test]
    fn a_notice_is_as_wide_as_its_longer_line_from_the_volumes_width_to_twice_it() {
        let size = |headline: &str, why: &str| {
            Showing::Notice(notice(headline, why)).size(&mut Monospace(8.0))
        };
        assert_eq!(size("No", "Why"), (WIDE, TALL_NOTICE), "never narrower");
        let why = "w".repeat(40);
        let wide = 12 + 32 + 12 + 40 * 8 + 2 + 12;
        assert_eq!(size("Not given", &why), (wide, TALL_NOTICE), "the why");
        assert_eq!(size(&why, "Short"), (wide, TALL_NOTICE), "or the headline");
        assert_eq!(
            size("Long", &"w".repeat(200)),
            (WIDEST, TALL_NOTICE),
            "and never wider"
        );
    }

    #[test]
    fn a_notices_parts_are_inside_it_the_sign_beside_the_headline_over_why() {
        for size in [(WIDE, TALL_NOTICE), (WIDEST, TALL_NOTICE), (WIDE, 64)] {
            let placed = lay_out_notice((size.0 as i32, size.1 as i32));
            for part in [placed.glyph, placed.headline, placed.why] {
                assert!(inside(part, size), "{part:?} in {size:?}");
            }
            assert!(placed.glyph.right() < placed.headline.x);
            assert_eq!(placed.headline.x, placed.why.x, "one under the other");
            assert!(placed.headline.bottom() < placed.why.y);
            assert_eq!(
                placed.headline.right(),
                size.0 as i32 - PAD,
                "as far as the edge's room"
            );
        }
        let placed = lay_out_notice((WIDE as i32, TALL_NOTICE as i32));
        assert_eq!(
            placed.glyph.y + GLYPH / 2,
            TALL_NOTICE as i32 / 2,
            "the sign in the middle"
        );
        assert_eq!(
            placed.why.bottom() + PAD,
            TALL_NOTICE as i32,
            "the lines filling it"
        );
    }

    #[cfg(feature = "audio")]
    fn at(name: &str, percent: u32, muted: bool) -> Indicator {
        Indicator {
            name: name.to_owned(),
            level: Level { percent, muted },
        }
    }

    #[cfg(feature = "audio")]
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

    #[cfg(feature = "audio")]
    #[test]
    fn its_parts_are_inside_it_the_name_and_percentage_over_the_bar() {
        let placed = lay_out();
        for part in [placed.glyph, placed.name, placed.percent, placed.bar] {
            assert!(inside(part, (WIDE, TALL)), "{part:?}");
        }
        assert!(placed.glyph.right() < placed.name.x);
        assert!(placed.name.right() <= placed.percent.x);
        assert!(placed.bar.y > placed.name.bottom());
        assert_eq!(
            placed.bar.right(),
            placed.percent.right(),
            "as wide as the words"
        );
        assert_eq!(
            Showing::Volume(at("Speakers", 40, false)).size(&mut Monospace(8.0)),
            (WIDE, TALL),
            "whatever its words"
        );
    }
}
