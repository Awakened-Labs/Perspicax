//! The mixer: where it opens, and where each of its controls is.
//!
//! It opens beside the panel's mixer item, away from the panel, as a menu
//! opens beside its button, [`WIDE`] wide. It holds a section for the
//! output and one for the input, each a title, then a row of a button that
//! mutes, a slider for the volume, and the volume in percent, then a row
//! for each device that side can be moved to. A device with nothing
//! plugged into it is folded away, unless the sound is on it, under a row
//! that counts what it holds and unfolds it. A slider reaches from silence
//! at its left end to the limit at its right, or to where the volume is,
//! if that is louder.
//!
//! Taller than the room beside the panel, it is cut to fit, and what is in
//! it scrolls.

use std::hash::{DefaultHasher, Hash as _, Hasher as _};

use perspicax_config::Edge;

use super::Rect;
use crate::model::sound::{self, Kind, Level, Side};

/// How wide the mixer is.
pub(crate) const WIDE: i32 = 340;
/// Its border, the room inside it, between its sections, and around a
/// control.
pub(crate) const BORDER: i32 = 1;
const PAD: i32 = 12;
const GAP: i32 = 8;
/// How tall a section's title is, its row of controls, a device's row, and
/// the row that unfolds what is folded away.
const TITLE: i32 = 22;
const ROW: i32 = 32;
const LISTED: i32 = 28;
const FOLD: i32 = 24;
/// A mute button is square, and the percentage this wide.
pub(crate) const BUTTON: i32 = 28;
pub(crate) const PERCENT: i32 = 48;
/// How far in from its ends a slider's track starts, for its knob.
pub(crate) const KNOB: i32 = 7;

/// Where the mixer opens: beside `button`, the mixer item's place on its
/// monitor, on a panel along `edge`, within `area`, the room on the monitor
/// beside the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Anchor {
    pub(crate) button: Rect,
    pub(crate) edge: Edge,
    pub(crate) area: Rect,
}

/// What the mixer shows of the sound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Mixed {
    /// The default output, or `None` with no sound server or none.
    pub(crate) output: Option<Master>,
    /// The default input, or `None` with none.
    pub(crate) input: Option<Master>,
    /// What each side can be moved to, in the order it is listed.
    pub(crate) outputs: Vec<Listed>,
    pub(crate) inputs: Vec<Listed>,
    /// `[shell.mixer] step` and `limit`.
    pub(crate) step: u32,
    pub(crate) limit: u32,
}

/// A default device, as the mixer shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Master {
    /// Its name: "Speakers", "Built-in Audio Analog Stereo".
    pub(crate) name: String,
    pub(crate) level: Level,
}

/// A device a side can be moved to, as the mixer lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    /// What choosing it asks for.
    pub(crate) key: String,
    /// Its card, and its jack and what is on it, if it is a card's: "E8",
    /// "Headphones · Bluetooth".
    pub(crate) device: String,
    pub(crate) jack: Option<String>,
    pub(crate) kind: Kind,
    /// Whether nothing is known to be unplugged from it.
    pub(crate) connected: bool,
    /// Whether the sound is on it.
    pub(crate) chosen: bool,
}

impl Listed {
    /// Its whole name: "E8 — Headphones · Bluetooth".
    pub(crate) fn label(&self) -> String {
        sound::long(&self.device, self.jack.as_deref())
    }

    /// Whether it is folded away: nothing is plugged into it, and the sound
    /// is not on it.
    pub(crate) fn folded(&self) -> bool {
        !self.connected && !self.chosen
    }
}

impl Mixed {
    /// The device on `side`.
    pub(crate) fn on(&self, side: Side) -> Option<&Master> {
        match side {
            Side::Output => self.output.as_ref(),
            Side::Input => self.input.as_ref(),
        }
    }

    /// What `side` can be moved to.
    pub(crate) fn listed(&self, side: Side) -> &[Listed] {
        match side {
            Side::Output => &self.outputs,
            Side::Input => &self.inputs,
        }
    }

    /// The device listed on `side` whose key is numbered `number`.
    pub(crate) fn find(&self, side: Side, number: u64) -> Option<&Listed> {
        self.listed(side)
            .iter()
            .find(|listed| self::number(&listed.key) == number)
    }

    /// The loudest a slider on `side` reaches: the limit, or where its
    /// volume is, if that is louder.
    pub(crate) fn most(&self, side: Side) -> u32 {
        let now = self.on(side).map_or(0, |master| master.level.percent);
        self.limit.max(now).max(1)
    }
}

/// The number a listed device's control goes by: its key, hashed, which
/// stays the same while others come and go beside it. Two bits short, for
/// an accessibility node's id to say it is a device, and on which side.
pub(crate) fn number(key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish() >> 2
}

/// One of the mixer's controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Control {
    /// The button that mutes the device on this side.
    Mute(Side),
    /// The slider of its volume.
    Volume(Side),
    /// A device listed on this side, by its key's [`number`]: choosing it
    /// moves the side to it.
    Choose(Side, u64),
    /// The row that unfolds what is folded away on this side, or folds it
    /// away again.
    Fold(Side),
}

impl Control {
    /// The controls that are no device's.
    pub(crate) const FIXED: [Self; 6] = [
        Self::Mute(Side::Output),
        Self::Volume(Side::Output),
        Self::Fold(Side::Output),
        Self::Mute(Side::Input),
        Self::Volume(Side::Input),
        Self::Fold(Side::Input),
    ];

    /// The side it is on.
    pub(crate) fn side(self) -> Side {
        let (Self::Mute(side) | Self::Volume(side) | Self::Choose(side, _) | Self::Fold(side)) =
            self;
        side
    }
}

/// The mixer, laid out, in its monitor's logical pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placed {
    /// The whole of it.
    pub(crate) rect: Rect,
    /// Where what is in it shows: what is placed beyond is scrolled out of
    /// sight.
    pub(crate) inside: Rect,
    /// How far down what is in it is scrolled, and the furthest it can be.
    pub(crate) scroll: i32,
    pub(crate) most: i32,
    /// Each section's title, and the side it is of.
    pub(crate) titles: Vec<(Side, Rect)>,
    /// Each control, in the order the keyboard moves through them, and
    /// where a slider's percentage is written beside it.
    pub(crate) controls: Vec<(Control, Rect)>,
    pub(crate) percents: Vec<(Side, Rect)>,
    /// How many devices each side has folded away, and whether they are
    /// unfolded, the output's first.
    pub(crate) folded: [usize; 2],
    pub(crate) unfolded: [bool; 2],
}

/// A line of the mixer, top down.
enum Line {
    Title(Side),
    /// The mute button, the slider and the percentage.
    Master(Side),
    Row(Control),
    /// The room between the sections.
    Gap,
}

/// The mixer laid out, opened at `anchor`, showing `mixed`, with each
/// side's devices folded away unless `unfolded` says otherwise, scrolled
/// `scroll` down, or as near as it goes.
pub(crate) fn lay_out(anchor: &Anchor, mixed: &Mixed, unfolded: [bool; 2], scroll: i32) -> Placed {
    let Anchor { button, edge, area } = *anchor;
    let mut lines = Vec::new();
    let mut line = |line, tall| lines.push((line, tall));
    let mut folded = [0; 2];
    for side in [Side::Output, Side::Input] {
        let listed = mixed.listed(side);
        let device = |listed: &Listed| Line::Row(Control::Choose(side, number(&listed.key)));
        line(Line::Title(side), TITLE);
        line(Line::Master(side), ROW);
        for one in listed.iter().filter(|one| !one.folded()) {
            line(device(one), LISTED);
        }
        folded[side.at()] = listed.iter().filter(|one| one.folded()).count();
        if folded[side.at()] > 0 {
            line(Line::Row(Control::Fold(side)), FOLD);
            if unfolded[side.at()] {
                for one in listed.iter().filter(|one| one.folded()) {
                    line(device(one), LISTED);
                }
            }
        }
        if side == Side::Output {
            line(Line::Gap, GAP);
        }
    }
    let y: i32 = lines.iter().map(|&(_, tall)| tall).sum();
    let room = 2 * (BORDER + PAD);
    let tall = (y + room).min(area.h).max(room);
    let x = (button.right() - WIDE).clamp(area.x, (area.right() - WIDE).max(area.x));
    let top = match edge {
        Edge::Bottom => button.y - tall,
        Edge::Top => button.bottom(),
    }
    .clamp(area.y, (area.bottom() - tall).max(area.y));
    let rect = Rect::new(x, top, WIDE, tall);
    let inside = Rect::new(
        x + BORDER + PAD,
        top + BORDER + PAD,
        WIDE - room,
        tall - room,
    );
    let most = (y - inside.h).max(0);
    let scroll = scroll.clamp(0, most);
    let (mut titles, mut controls, mut percents) = (Vec::new(), Vec::new(), Vec::new());
    let mut row = inside.y - scroll;
    for (line, tall) in lines {
        match line {
            Line::Title(side) => titles.push((side, Rect::new(inside.x, row, inside.w, tall))),
            Line::Master(side) => {
                let button = Rect::new(inside.x, row + (tall - BUTTON) / 2, BUTTON, BUTTON);
                let percent = Rect::new(inside.right() - PERCENT, row, PERCENT, tall);
                let slider = Rect::new(
                    button.right() + GAP,
                    row,
                    percent.x - GAP - (button.right() + GAP),
                    tall,
                );
                controls.push((Control::Mute(side), button));
                controls.push((Control::Volume(side), slider));
                percents.push((side, percent));
            }
            Line::Row(control) => {
                controls.push((control, Rect::new(inside.x, row, inside.w, tall)));
            }
            Line::Gap => {}
        }
        row += tall;
    }
    Placed {
        rect,
        inside,
        scroll,
        most,
        titles,
        controls,
        percents,
        folded,
        unfolded,
    }
}

impl Placed {
    /// The control at `point`, if one is there and not scrolled out of
    /// sight.
    pub(crate) fn at(&self, point: (f64, f64)) -> Option<Control> {
        if !self.inside.contains(point) {
            return None;
        }
        self.controls
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|&(control, _)| control)
    }

    /// Where `control` is.
    pub(crate) fn of(&self, control: Control) -> Option<Rect> {
        self.controls
            .iter()
            .find(|&&(had, _)| had == control)
            .map(|&(_, rect)| rect)
    }

    /// How far down to scroll to bring `control` wholly into sight, and
    /// the title above a mute button or a slider with it, moving as little
    /// as can be.
    pub(crate) fn reveal(&self, control: Control) -> i32 {
        let Some(rect) = self.of(control) else {
            return self.scroll;
        };
        let top = match control {
            Control::Mute(side) | Control::Volume(side) => self
                .titles
                .iter()
                .find(|&&(had, _)| had == side)
                .map_or(rect.y, |&(_, title)| title.y),
            Control::Choose(..) | Control::Fold(_) => rect.y,
        };
        let above = self.inside.y - top;
        let below = rect.bottom() - self.inside.bottom();
        if above > 0 {
            self.scroll - above
        } else if below > 0 {
            self.scroll + below
        } else {
            self.scroll
        }
    }
}

/// The track of a slider at `slider`: the line its knob runs along.
pub(crate) fn track(slider: Rect) -> Rect {
    Rect::new(
        slider.x + KNOB,
        slider.y,
        (slider.w - 2 * KNOB).max(1),
        slider.h,
    )
}

/// The volume, in percent, a press at `x` on the slider at `slider` sets:
/// silence at the left end of its track, `most` at the right.
pub(crate) fn percent_at(slider: Rect, x: f64, most: u32) -> u32 {
    let track = track(slider);
    let along = ((x - f64::from(track.x)) / f64::from(track.w)).clamp(0.0, 1.0);
    (along * f64::from(most)).round() as u32
}

/// Where along the track of `slider` the knob is for `percent` of `most`.
pub(crate) fn knob_at(slider: Rect, percent: u32, most: u32) -> i32 {
    let track = track(slider);
    let along = f64::from(percent.min(most)) / f64::from(most.max(1));
    track.x + (along * f64::from(track.w)).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1280 by 800 monitor with a 40-pixel panel along the bottom, the
    /// mixer item at its right end.
    const AREA: Rect = Rect::new(0, 0, 1280, 760);
    const ITEM: Rect = Rect::new(1180, 760, 100, 40);
    const ANCHOR: Anchor = Anchor {
        button: ITEM,
        edge: Edge::Bottom,
        area: AREA,
    };

    fn level(percent: u32) -> Level {
        Level {
            percent,
            muted: false,
        }
    }

    fn listed(key: &str, connected: bool, chosen: bool) -> Listed {
        Listed {
            key: key.to_owned(),
            device: key.to_owned(),
            jack: None,
            kind: Kind::Speaker,
            connected,
            chosen,
        }
    }

    /// Speakers with the sound on them, a jack with nothing in it, and a
    /// microphone.
    fn mixed() -> Mixed {
        Mixed {
            output: Some(Master {
                name: "Speakers".to_owned(),
                level: level(40),
            }),
            input: Some(Master {
                name: "Microphone".to_owned(),
                level: level(130),
            }),
            outputs: vec![
                listed("speakers", true, true),
                listed("headphones", false, false),
            ],
            inputs: vec![listed("mic", true, true)],
            step: 5,
            limit: 100,
        }
    }

    fn at(anchor: Anchor) -> Placed {
        lay_out(&anchor, &mixed(), [false; 2], 0)
    }

    fn controls(placed: &Placed) -> Vec<Control> {
        placed
            .controls
            .iter()
            .map(|&(control, _)| control)
            .collect()
    }

    fn point(rect: Rect) -> (f64, f64) {
        (f64::from(rect.x) + 2.0, f64::from(rect.y) + 2.0)
    }

    #[test]
    fn it_opens_above_a_bottom_panel_beside_its_item_and_on_the_monitor() {
        let placed = at(ANCHOR);
        assert_eq!(
            placed.rect.right(),
            ITEM.right(),
            "its right end at the item's"
        );
        assert_eq!(placed.rect.bottom(), ITEM.y, "standing on the panel");
        assert_eq!(placed.rect.w, WIDE);
        let left = at(Anchor {
            button: Rect::new(0, 760, 30, 40),
            ..ANCHOR
        });
        assert_eq!(left.rect.x, 0, "kept on the monitor");
        let top = at(Anchor {
            button: Rect::new(1180, 0, 100, 40),
            edge: Edge::Top,
            area: Rect::new(0, 40, 1280, 760),
        });
        assert_eq!(top.rect.y, 40, "below a top panel");
        let whole = at(Anchor {
            area: Rect::new(0, 0, 1280, 800),
            ..ANCHOR
        });
        assert_eq!(
            whole.rect.bottom(),
            ITEM.y,
            "above the item, room under the panel or not"
        );
    }

    #[test]
    fn each_side_has_a_title_a_mute_button_a_slider_a_percentage_and_its_devices() {
        let placed = at(ANCHOR);
        let speakers = Control::Choose(Side::Output, number("speakers"));
        let mic = Control::Choose(Side::Input, number("mic"));
        assert_eq!(
            controls(&placed),
            [
                Control::Mute(Side::Output),
                Control::Volume(Side::Output),
                speakers,
                Control::Fold(Side::Output),
                Control::Mute(Side::Input),
                Control::Volume(Side::Input),
                mic,
            ]
        );
        let output = placed.of(Control::Volume(Side::Output)).expect("a slider");
        let input = placed.of(Control::Volume(Side::Input)).expect("a slider");
        assert!(input.y > output.bottom(), "the input below the output");
        let mute = placed.of(Control::Mute(Side::Output)).expect("a button");
        assert!(mute.right() < output.x, "the button left of the slider");
        let (_, percent) = placed.percents[0];
        assert!(output.right() < percent.x, "the percentage right of it");
        let row = placed.of(speakers).expect("a row");
        assert_eq!(row.y, output.bottom(), "the devices under the slider");
        assert_eq!(row.w, placed.inside.w, "the width of the mixer");
        assert_eq!(
            placed.at(point(output)),
            Some(Control::Volume(Side::Output))
        );
        assert_eq!(placed.at(point(row)), Some(speakers));
        assert_eq!(
            placed.at(point(placed.titles[0].1)),
            None,
            "a title is no control"
        );
        assert_eq!(
            placed.rect.h,
            2 * (BORDER + PAD) + 2 * (TITLE + ROW + LISTED) + FOLD + GAP,
            "as tall as what is in it"
        );
        assert_eq!((placed.scroll, placed.most), (0, 0));
    }

    #[test]
    fn a_device_with_nothing_plugged_in_is_folded_away_unless_the_sound_is_on_it() {
        let headphones = Control::Choose(Side::Output, number("headphones"));
        let placed = at(ANCHOR);
        assert_eq!(placed.folded, [1, 0]);
        assert_eq!(placed.of(headphones), None, "folded away");
        assert_eq!(
            placed.of(Control::Fold(Side::Input)),
            None,
            "nothing to fold"
        );
        let unfolded = lay_out(&ANCHOR, &mixed(), [true, false], 0);
        let fold = unfolded.of(Control::Fold(Side::Output)).expect("a fold");
        let row = unfolded.of(headphones).expect("unfolded");
        assert_eq!(row.y, fold.bottom(), "under its fold");
        assert_eq!(unfolded.rect.h, placed.rect.h + LISTED, "taller by it");
        assert_eq!(
            unfolded.rect.bottom(),
            ITEM.y,
            "growing away from the panel"
        );
        let mut chosen = mixed();
        chosen.outputs[1].chosen = true;
        let placed = lay_out(&ANCHOR, &chosen, [false; 2], 0);
        assert!(placed.of(headphones).is_some(), "the sound is on it");
        assert_eq!(placed.folded, [0, 0]);
    }

    #[test]
    fn too_tall_for_the_room_it_is_cut_to_fit_and_what_is_in_it_scrolls() {
        let anchor = Anchor {
            area: Rect::new(0, 660, 1280, 100),
            ..ANCHOR
        };
        let placed = lay_out(&anchor, &mixed(), [false; 2], 0);
        assert_eq!(placed.rect, Rect::new(940, 660, WIDE, 100), "cut to fit");
        let tall = 2 * (TITLE + ROW + LISTED) + FOLD + GAP;
        assert_eq!(placed.most, tall - placed.inside.h);
        let mic = Control::Choose(Side::Input, number("mic"));
        let rect = placed.of(mic).expect("placed");
        assert!(rect.y > placed.inside.bottom(), "out of sight below");
        assert_eq!(placed.at(point(rect)), None, "and not pressed");
        assert_eq!(placed.reveal(mic), rect.bottom() - placed.inside.bottom());
        let scrolled = lay_out(&anchor, &mixed(), [false; 2], 1000);
        assert_eq!(scrolled.scroll, scrolled.most, "as far as it goes");
        let rect = scrolled.of(mic).expect("placed");
        assert_eq!(rect.bottom(), scrolled.inside.bottom(), "the last in sight");
        assert_eq!(scrolled.at(point(rect)), Some(mic));
        let partly = lay_out(&anchor, &mixed(), [false; 2], 40);
        let output = Control::Volume(Side::Output);
        let slider = partly.of(output).expect("placed");
        assert!(slider.bottom() > partly.inside.y, "partly in sight");
        let cut = (f64::from(slider.x) + 2.0, f64::from(partly.inside.y) - 1.0);
        assert!(slider.contains(cut));
        assert_eq!(partly.at(cut), None, "not where it is cut off");
        assert_eq!(partly.reveal(output), 0, "its title with it");
        assert_eq!(
            lay_out(&anchor, &mixed(), [false; 2], -5).scroll,
            0,
            "never above the top"
        );
    }

    #[test]
    fn a_press_along_a_slider_sets_where_it_is_of_the_most() {
        let slider = Rect::new(100, 0, 214, 32);
        let track = track(slider);
        assert_eq!(percent_at(slider, f64::from(track.x), 100), 0);
        assert_eq!(percent_at(slider, f64::from(track.right()), 100), 100);
        assert_eq!(
            percent_at(slider, f64::from(track.x + track.w / 2), 100),
            50
        );
        assert_eq!(percent_at(slider, 0.0, 100), 0, "left of it, silence");
        assert_eq!(
            percent_at(slider, 1000.0, 150),
            150,
            "right of it, the most"
        );
        assert_eq!(knob_at(slider, 50, 100), track.x + track.w / 2);
        assert_eq!(
            knob_at(slider, 200, 100),
            track.right(),
            "no further than the end"
        );
    }

    #[test]
    fn a_slider_reaches_the_limit_or_a_louder_volume() {
        let mixed = mixed();
        assert_eq!(mixed.most(Side::Output), 100);
        assert_eq!(mixed.most(Side::Input), 130);
        assert_eq!(Mixed::default().most(Side::Output), 1, "never nothing");
    }

    #[test]
    fn a_device_is_found_by_its_keys_number_on_its_own_side() {
        let mixed = mixed();
        assert_eq!(
            mixed
                .find(Side::Output, number("headphones"))
                .map(|one| one.key.as_str()),
            Some("headphones")
        );
        assert_eq!(mixed.find(Side::Input, number("headphones")), None);
        assert_ne!(number("speakers"), number("headphones"));
        assert!(
            (0..64).all(|at| number(&format!("key {at}")) >> 62 == 0),
            "two bits short"
        );
        for control in Control::FIXED {
            assert_eq!(
                Control::FIXED.iter().filter(|&&had| had == control).count(),
                1
            );
        }
        assert_eq!(Control::Choose(Side::Input, 7).side(), Side::Input);
        assert_eq!(Control::Fold(Side::Output).side(), Side::Output);
    }
}
