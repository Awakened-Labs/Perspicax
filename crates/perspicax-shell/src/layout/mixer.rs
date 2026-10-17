//! The mixer: where it opens, and where each of its controls is.
//!
//! It opens beside the panel's mixer item, away from the panel, as a menu
//! opens beside its button, [`WIDE`] wide. It holds a section for the
//! output and one for the input, each a title, then a row of a button that
//! mutes, a slider for the volume, and the volume in percent, then a row
//! for each device that side can be moved to. A device with nothing
//! plugged into it is folded away, unless the sound is on it, under a row
//! that counts what it holds and unfolds it; one the person hid, under
//! another. A device's row ends in a button that unfolds what tells it
//! apart, a field to name it, and buttons to hide it or its whole card, set
//! in under it, one device's at a time. While programs play, a third
//! section holds each: a row of its name and where it plays, which unfolds
//! the outputs it can be moved to, set in under it, then its own row of
//! controls. A slider
//! reaches from silence at its left end to the limit at its right, or to
//! where the volume is, if that is louder.
//!
//! Taller than the room beside the panel, it is cut to fit, and what is in
//! it scrolls.

use std::hash::{DefaultHasher, Hash as _, Hasher as _};

use perspicax_config::Edge;

use super::Rect;
use crate::model::sound::{self, Kind, Level, Side, Whose};

/// How wide the mixer is.
pub(crate) const WIDE: i32 = 340;
/// Its border, the room inside it, between its sections, and around a
/// control.
pub(crate) const BORDER: i32 = 1;
const PAD: i32 = 12;
const GAP: i32 = 8;
/// How tall a section's title is, its row of controls, a device's row, the
/// row that unfolds what is folded away, and a line of what tells a device
/// apart.
const TITLE: i32 = 22;
const ROW: i32 = 32;
const LISTED: i32 = 28;
const FOLD: i32 = 24;
const DETAIL: i32 = 20;
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
    /// What programs play, and the outputs a program can be moved to, by
    /// their sinks' numbers.
    pub(crate) programs: Vec<Program>,
    pub(crate) sinks: Vec<(u32, Listed)>,
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
    /// What the panel calls it: "E8".
    pub(crate) short: String,
    /// The person's name for it, listed in place of its own, if they gave
    /// one.
    pub(crate) name: Option<String>,
    pub(crate) kind: Kind,
    /// Whether nothing is known to be unplugged from it.
    pub(crate) connected: bool,
    /// Whether the sound is on it.
    pub(crate) chosen: bool,
    /// Whether the person hid it, and the card it is a jack of, if it is
    /// one's.
    pub(crate) hidden: bool,
    pub(crate) card: Option<Owner>,
    /// What tells it apart, line by line.
    pub(crate) details: Vec<(&'static str, String)>,
    /// The sink playing to it now, by its number, an output's; and the
    /// source that meters it, by name.
    pub(crate) live: Option<u32>,
    pub(crate) meter: Option<String>,
}

/// The card a device is a jack of, as far as hiding it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owner {
    /// Its name, by which it is hidden.
    pub(crate) key: String,
    /// What it calls itself: "TU104 HD Audio Controller".
    pub(crate) name: String,
    pub(crate) hidden: bool,
}

/// Where a device is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shelf {
    /// Among its side's devices.
    Shown,
    /// Under the fold of those with nothing plugged in.
    Unplugged,
    /// Under the fold of those the person hid.
    Hidden,
}

/// A program playing, as the mixer shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Program {
    /// Its stream's number.
    pub(crate) stream: u32,
    pub(crate) name: String,
    /// The icon to show for it, by name, if one is known.
    pub(crate) icon: Option<String>,
    pub(crate) level: Level,
    /// The sink it plays into, by its number.
    pub(crate) sink: u32,
}

impl Listed {
    /// Its whole name: the person's for it, or its own, "E8 — Headphones ·
    /// Bluetooth".
    pub(crate) fn label(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => sound::long(&self.device, self.jack.as_deref()),
        }
    }

    /// Whether the person hid it, or its whole card.
    pub(crate) fn concealed(&self) -> bool {
        self.hidden || self.card.as_ref().is_some_and(|card| card.hidden)
    }

    /// Where it is listed: the one the sound is on among the rest, whatever
    /// else is so of it; then one the person hid under "Hidden"; then one
    /// with nothing plugged in under "Not connected".
    pub(crate) fn shelf(&self) -> Shelf {
        if self.chosen {
            Shelf::Shown
        } else if self.concealed() {
            Shelf::Hidden
        } else if self.connected {
            Shelf::Shown
        } else {
            Shelf::Unplugged
        }
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

    /// The program playing the stream numbered `stream`.
    pub(crate) fn program(&self, stream: u32) -> Option<&Program> {
        self.programs
            .iter()
            .find(|program| program.stream == stream)
    }

    /// The output `program` plays to.
    pub(crate) fn playing_on(&self, program: &Program) -> Option<&Listed> {
        self.sinks
            .iter()
            .find(|&&(sink, _)| sink == program.sink)
            .map(|(_, listed)| listed)
    }

    /// How loud `whose` is, if it is there.
    pub(crate) fn level(&self, whose: Whose) -> Option<Level> {
        match whose {
            Whose::Side(side) => self.on(side).map(|master| master.level),
            Whose::App(stream) => self.program(stream).map(|program| program.level),
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

    /// The loudest the slider of `whose` reaches: the limit, or where its
    /// volume is, if that is louder.
    pub(crate) fn most(&self, whose: Whose) -> u32 {
        let now = self.level(whose).map_or(0, |level| level.percent);
        self.limit.max(now).max(1)
    }
}

/// The number a listed device's controls go by: its key, hashed, which
/// stays the same while others come and go beside it. Five bits short, for
/// an accessibility node's id to say what kind of control it is, and on
/// which side.
pub(crate) fn number(key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish() >> 5
}

/// One of the mixer's controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Control {
    /// The button that mutes a side's device, or a program.
    Mute(Whose),
    /// The slider of its volume.
    Volume(Whose),
    /// A device listed on this side, by its key's [`number`]: choosing it
    /// moves the side to it.
    Choose(Side, u64),
    /// The row that unfolds what is folded away on this side, or folds it
    /// away again.
    Fold(Side),
    /// The button at the end of a device's row, which unfolds what tells
    /// it apart and what can be done with it, or folds them away again.
    More(Side, u64),
    /// The field a device is named in.
    Rename(Side, u64),
    /// The button that plays the test sound on an output.
    Test(Side, u64),
    /// The button that hides a device, or brings it back.
    Hide(Side, u64),
    /// The button that hides the whole card a device is a jack of, or
    /// brings it back.
    HideCard(Side, u64),
    /// The row that unfolds what the person hid on this side, or folds it
    /// away again.
    Hidden(Side),
    /// The row of the program playing this stream, which unfolds the
    /// outputs it can be moved to, or folds them away again.
    Plays(u32),
    /// An output a program can be moved to: its stream's number, and the
    /// sink's.
    Move(u32, u32),
}

impl Control {
    /// The controls of the sides that are no device's.
    pub(crate) const FIXED: [Self; 8] = [
        Self::Mute(Whose::Side(Side::Output)),
        Self::Volume(Whose::Side(Side::Output)),
        Self::Fold(Side::Output),
        Self::Hidden(Side::Output),
        Self::Mute(Whose::Side(Side::Input)),
        Self::Volume(Whose::Side(Side::Input)),
        Self::Fold(Side::Input),
        Self::Hidden(Side::Input),
    ];

    /// The side it is on, if it is a side's.
    pub(crate) fn side(self) -> Option<Side> {
        match self {
            Self::Mute(Whose::Side(side))
            | Self::Volume(Whose::Side(side))
            | Self::Choose(side, _)
            | Self::Fold(side)
            | Self::More(side, _)
            | Self::Rename(side, _)
            | Self::Test(side, _)
            | Self::Hide(side, _)
            | Self::HideCard(side, _)
            | Self::Hidden(side) => Some(side),
            Self::Mute(Whose::App(_))
            | Self::Volume(Whose::App(_))
            | Self::Plays(_)
            | Self::Move(..) => None,
        }
    }
}

/// What of the mixer is unfolded: each side's devices with nothing plugged
/// in, and those hidden, the output's first; the device whose details are
/// shown, by its side and number; and the outputs of each program, by its
/// stream's number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Unfolded {
    pub(crate) unplugged: [bool; 2],
    pub(crate) hidden: [bool; 2],
    pub(crate) more: Option<(Side, u64)>,
    pub(crate) programs: Vec<u32>,
}

/// A section's title: a side's, or the programs'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Heading {
    Side(Side),
    Programs,
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
    /// Each section's title.
    pub(crate) titles: Vec<(Heading, Rect)>,
    /// Each control, in the order the keyboard moves through them, and
    /// where a slider's percentage is written beside it.
    pub(crate) controls: Vec<(Control, Rect)>,
    pub(crate) percents: Vec<(Whose, Rect)>,
    /// Each line of what tells the device unfolded apart.
    pub(crate) details: Vec<(String, Rect)>,
    /// How many devices each side has with nothing plugged in, and hidden,
    /// folded away, the output's first, and what is unfolded.
    pub(crate) unplugged: [usize; 2],
    pub(crate) hidden: [usize; 2],
    pub(crate) unfolded: Unfolded,
}

/// A line of the mixer, top down.
enum Line {
    Title(Heading),
    /// The mute button, the slider and the percentage.
    Master(Whose),
    /// A device's row, and the button at its end.
    Device(Side, u64),
    Row(Control),
    /// What tells a device apart.
    Detail(String),
    /// The room between the sections.
    Gap,
}

/// The lines of `listed`, on `side`: its row, and what is unfolded of it.
fn device(lines: &mut Vec<(Line, i32)>, side: Side, listed: &Listed, unfolded: &Unfolded) {
    let number = number(&listed.key);
    lines.push((Line::Device(side, number), LISTED));
    if unfolded.more != Some((side, number)) {
        return;
    }
    for (label, value) in &listed.details {
        lines.push((Line::Detail(format!("{label}: {value}")), DETAIL));
    }
    lines.push((Line::Row(Control::Rename(side, number)), LISTED));
    if side == Side::Output {
        lines.push((Line::Row(Control::Test(side, number)), LISTED));
    }
    // Hidden with its card, it comes back with its card.
    if !listed.card.as_ref().is_some_and(|card| card.hidden) {
        lines.push((Line::Row(Control::Hide(side, number)), LISTED));
    }
    if listed.card.is_some() {
        lines.push((Line::Row(Control::HideCard(side, number)), LISTED));
    }
}

/// The mixer laid out, opened at `anchor`, showing `mixed`, with what is
/// folded away unfolded as `unfolded` says, scrolled `scroll` down, or as
/// near as it goes.
pub(crate) fn lay_out(anchor: &Anchor, mixed: &Mixed, unfolded: &Unfolded, scroll: i32) -> Placed {
    let Anchor { button, edge, area } = *anchor;
    let mut lines = Vec::new();
    let (mut unplugged, mut hidden) = ([0; 2], [0; 2]);
    for side in [Side::Output, Side::Input] {
        let listed = mixed.listed(side);
        let shelf = |shelf| listed.iter().filter(move |one| one.shelf() == shelf);
        lines.push((Line::Title(Heading::Side(side)), TITLE));
        lines.push((Line::Master(Whose::Side(side)), ROW));
        for one in shelf(Shelf::Shown) {
            device(&mut lines, side, one, unfolded);
        }
        for (shelf, count, fold, open) in [
            (
                Shelf::Unplugged,
                &mut unplugged,
                Control::Fold(side),
                unfolded.unplugged,
            ),
            (
                Shelf::Hidden,
                &mut hidden,
                Control::Hidden(side),
                unfolded.hidden,
            ),
        ] {
            count[side.at()] = listed.iter().filter(|one| one.shelf() == shelf).count();
            if count[side.at()] == 0 {
                continue;
            }
            lines.push((Line::Row(fold), FOLD));
            if open[side.at()] {
                for one in listed.iter().filter(|one| one.shelf() == shelf) {
                    device(&mut lines, side, one, unfolded);
                }
            }
        }
        if side == Side::Output {
            lines.push((Line::Gap, GAP));
        }
    }
    if !mixed.programs.is_empty() {
        lines.push((Line::Gap, GAP));
        lines.push((Line::Title(Heading::Programs), TITLE));
    }
    for program in &mixed.programs {
        let stream = program.stream;
        lines.push((Line::Row(Control::Plays(stream)), LISTED));
        lines.push((Line::Master(Whose::App(stream)), ROW));
        if unfolded.programs.contains(&stream) {
            for &(sink, _) in &mixed.sinks {
                lines.push((Line::Row(Control::Move(stream, sink)), LISTED));
            }
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
    let (mut titles, mut controls, mut percents, mut details) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let set_in = |row, tall| Rect::new(inside.x + BUTTON, row, inside.w - BUTTON, tall);
    let mut row = inside.y - scroll;
    for (line, tall) in lines {
        match line {
            Line::Title(heading) => {
                titles.push((heading, Rect::new(inside.x, row, inside.w, tall)));
            }
            Line::Master(whose) => {
                let button = Rect::new(inside.x, row + (tall - BUTTON) / 2, BUTTON, BUTTON);
                let percent = Rect::new(inside.right() - PERCENT, row, PERCENT, tall);
                let slider = Rect::new(
                    button.right() + GAP,
                    row,
                    percent.x - GAP - (button.right() + GAP),
                    tall,
                );
                controls.push((Control::Mute(whose), button));
                controls.push((Control::Volume(whose), slider));
                percents.push((whose, percent));
            }
            Line::Device(side, number) => {
                let more = Rect::new(inside.right() - BUTTON, row, BUTTON, tall);
                let choose = Rect::new(inside.x, row, inside.w - BUTTON, tall);
                controls.push((Control::Choose(side, number), choose));
                controls.push((Control::More(side, number), more));
            }
            // A program's outputs are its own, and what can be done with a
            // device its own, set in under it.
            Line::Row(
                control @ (Control::Move(..)
                | Control::Rename(..)
                | Control::Test(..)
                | Control::Hide(..)
                | Control::HideCard(..)),
            ) => controls.push((control, set_in(row, tall))),
            Line::Row(control) => {
                controls.push((control, Rect::new(inside.x, row, inside.w, tall)));
            }
            Line::Detail(text) => details.push((text, set_in(row, tall))),
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
        details,
        unplugged,
        hidden,
        unfolded: unfolded.clone(),
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

    /// How far down to scroll to bring `control` wholly into sight, moving
    /// as little as can be; a mute button or a slider with what names it
    /// above, a side's title or a program's row.
    pub(crate) fn reveal(&self, control: Control) -> i32 {
        let Some(rect) = self.of(control) else {
            return self.scroll;
        };
        let named = match control {
            Control::Mute(Whose::Side(side)) | Control::Volume(Whose::Side(side)) => self
                .titles
                .iter()
                .find(|&&(had, _)| had == Heading::Side(side))
                .map(|&(_, title)| title),
            Control::Mute(Whose::App(stream)) | Control::Volume(Whose::App(stream)) => {
                self.of(Control::Plays(stream))
            }
            Control::Choose(..)
            | Control::Fold(_)
            | Control::More(..)
            | Control::Rename(..)
            | Control::Test(..)
            | Control::Hide(..)
            | Control::HideCard(..)
            | Control::Hidden(_)
            | Control::Plays(_)
            | Control::Move(..) => None,
        };
        let top = named.map_or(rect.y, |named| named.y);
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
            short: key.to_owned(),
            kind: Kind::Speaker,
            live: None,
            meter: None,
            connected,
            chosen,
            name: None,
            hidden: false,
            card: None,
            details: Vec::new(),
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
            ..Mixed::default()
        }
    }

    fn at(anchor: Anchor) -> Placed {
        lay_out(&anchor, &mixed(), &Unfolded::default(), 0)
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
                Control::Mute(Whose::Side(Side::Output)),
                Control::Volume(Whose::Side(Side::Output)),
                speakers,
                Control::More(Side::Output, number("speakers")),
                Control::Fold(Side::Output),
                Control::Mute(Whose::Side(Side::Input)),
                Control::Volume(Whose::Side(Side::Input)),
                mic,
                Control::More(Side::Input, number("mic")),
            ]
        );
        let output = placed
            .of(Control::Volume(Whose::Side(Side::Output)))
            .expect("a slider");
        let input = placed
            .of(Control::Volume(Whose::Side(Side::Input)))
            .expect("a slider");
        assert!(input.y > output.bottom(), "the input below the output");
        let mute = placed
            .of(Control::Mute(Whose::Side(Side::Output)))
            .expect("a button");
        assert!(mute.right() < output.x, "the button left of the slider");
        let (_, percent) = placed.percents[0];
        assert!(output.right() < percent.x, "the percentage right of it");
        let row = placed.of(speakers).expect("a row");
        assert_eq!(row.y, output.bottom(), "the devices under the slider");
        let more = placed
            .of(Control::More(Side::Output, number("speakers")))
            .expect("a button at its end");
        assert_eq!(
            (row.x, row.right(), more.right()),
            (placed.inside.x, more.x, placed.inside.right()),
            "the width of the mixer, the button at its end"
        );
        assert_eq!(
            placed.at(point(output)),
            Some(Control::Volume(Whose::Side(Side::Output)))
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
        assert_eq!(placed.unplugged, [1, 0]);
        assert_eq!(placed.of(headphones), None, "folded away");
        assert_eq!(
            placed.of(Control::Fold(Side::Input)),
            None,
            "nothing to fold"
        );
        let sides = Unfolded {
            unplugged: [true, false],
            ..Unfolded::default()
        };
        let unfolded = lay_out(&ANCHOR, &mixed(), &sides, 0);
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
        let placed = lay_out(&ANCHOR, &chosen, &Unfolded::default(), 0);
        assert!(placed.of(headphones).is_some(), "the sound is on it");
        assert_eq!(placed.unplugged, [0, 0]);
    }

    /// The laptop's outputs: the speakers in use, a headset hidden, and the
    /// NVIDIA card's jack, with nothing in it, hidden with its card.
    fn hidden() -> Mixed {
        let card = |hidden| Owner {
            key: "nvidia".to_owned(),
            name: "TU104 HD Audio Controller".to_owned(),
            hidden,
        };
        Mixed {
            outputs: vec![
                listed("speakers", true, true),
                Listed {
                    hidden: true,
                    ..listed("headset", true, false)
                },
                Listed {
                    card: Some(card(true)),
                    details: vec![
                        ("Chip", "GPU".to_owned()),
                        ("Key", "nvidia/hdmi".to_owned()),
                    ],
                    ..listed("hdmi", false, false)
                },
            ],
            ..mixed()
        }
    }

    #[test]
    fn a_hidden_device_is_under_hidden_unless_the_sound_is_on_it() {
        let placed = lay_out(&ANCHOR, &hidden(), &Unfolded::default(), 0);
        assert_eq!(placed.hidden, [2, 0], "the headset, and the card's jack");
        assert_eq!(placed.unplugged, [0, 0], "hidden first, plugged in or not");
        assert_eq!(
            controls(&placed)[4],
            Control::Hidden(Side::Output),
            "after the speakers"
        );
        let open = Unfolded {
            hidden: [true, false],
            ..Unfolded::default()
        };
        let placed = lay_out(&ANCHOR, &hidden(), &open, 0);
        assert_eq!(
            controls(&placed)[5..9],
            [
                Control::Choose(Side::Output, number("headset")),
                Control::More(Side::Output, number("headset")),
                Control::Choose(Side::Output, number("hdmi")),
                Control::More(Side::Output, number("hdmi")),
            ],
            "unfolded under it"
        );
        let mut in_use = hidden();
        in_use.outputs[1].chosen = true;
        in_use.outputs[0].chosen = false;
        let placed = lay_out(&ANCHOR, &in_use, &Unfolded::default(), 0);
        assert!(
            placed
                .of(Control::Choose(Side::Output, number("headset")))
                .is_some(),
            "the sound is on it, so it stays in sight"
        );
        assert_eq!(placed.hidden, [1, 0]);
    }

    #[test]
    fn a_devices_details_name_and_hiding_are_set_in_under_it() {
        let hdmi = number("hdmi");
        let open = Unfolded {
            hidden: [true, false],
            more: Some((Side::Output, hdmi)),
            ..Unfolded::default()
        };
        let placed = lay_out(&ANCHOR, &hidden(), &open, 0);
        let row = placed
            .of(Control::Choose(Side::Output, hdmi))
            .expect("listed");
        let lines: Vec<&str> = placed
            .details
            .iter()
            .map(|(line, _)| line.as_str())
            .collect();
        assert_eq!(lines, ["Chip: GPU", "Key: nvidia/hdmi"]);
        let (_, first) = placed.details[0];
        assert_eq!(
            (first.x, first.y),
            (placed.inside.x + BUTTON, row.bottom()),
            "set in under it"
        );
        let after: Vec<Control> = controls(&placed)
            .into_iter()
            .skip_while(|&control| control != Control::More(Side::Output, hdmi))
            .skip(1)
            .take(3)
            .collect();
        assert_eq!(
            after,
            [
                Control::Rename(Side::Output, hdmi),
                Control::Test(Side::Output, hdmi),
                Control::HideCard(Side::Output, hdmi)
            ],
            "an output's test, and hidden with its card, it comes back only with its card"
        );
        let field = placed
            .of(Control::Rename(Side::Output, hdmi))
            .expect("placed");
        assert_eq!(
            field.y,
            placed.details[1].1.bottom(),
            "under what tells it apart"
        );
        assert_eq!(field.x, placed.inside.x + BUTTON);
        let headset = number("headset");
        let open = Unfolded {
            more: Some((Side::Output, headset)),
            ..open
        };
        let placed = lay_out(&ANCHOR, &hidden(), &open, 0);
        assert!(placed.of(Control::Hide(Side::Output, headset)).is_some());
        assert_eq!(
            placed.of(Control::HideCard(Side::Output, headset)),
            None,
            "a device of no card"
        );
        assert_eq!(
            placed.of(Control::Rename(Side::Output, hdmi)),
            None,
            "one at a time"
        );
    }

    #[test]
    fn a_devices_own_name_gives_way_to_the_persons() {
        let mut one = listed("speakers", true, false);
        one.device = "Built-in Audio".to_owned();
        one.jack = Some("Speakers".to_owned());
        assert_eq!(one.label(), "Built-in Audio \u{2014} Speakers");
        one.name = Some("Desk".to_owned());
        assert_eq!(one.label(), "Desk");
    }

    #[test]
    fn too_tall_for_the_room_it_is_cut_to_fit_and_what_is_in_it_scrolls() {
        let anchor = Anchor {
            area: Rect::new(0, 660, 1280, 100),
            ..ANCHOR
        };
        let placed = lay_out(&anchor, &mixed(), &Unfolded::default(), 0);
        assert_eq!(placed.rect, Rect::new(940, 660, WIDE, 100), "cut to fit");
        let tall = 2 * (TITLE + ROW + LISTED) + FOLD + GAP;
        assert_eq!(placed.most, tall - placed.inside.h);
        let mic = Control::Choose(Side::Input, number("mic"));
        let rect = placed.of(mic).expect("placed");
        assert!(rect.y > placed.inside.bottom(), "out of sight below");
        assert_eq!(placed.at(point(rect)), None, "and not pressed");
        assert_eq!(placed.reveal(mic), rect.bottom() - placed.inside.bottom());
        let scrolled = lay_out(&anchor, &mixed(), &Unfolded::default(), 1000);
        assert_eq!(scrolled.scroll, scrolled.most, "as far as it goes");
        let rect = scrolled.of(mic).expect("placed");
        assert_eq!(rect.bottom(), scrolled.inside.bottom(), "the last in sight");
        assert_eq!(scrolled.at(point(rect)), Some(mic));
        let partly = lay_out(&anchor, &mixed(), &Unfolded::default(), 40);
        let output = Control::Volume(Whose::Side(Side::Output));
        let slider = partly.of(output).expect("placed");
        assert!(slider.bottom() > partly.inside.y, "partly in sight");
        let cut = (f64::from(slider.x) + 2.0, f64::from(partly.inside.y) - 1.0);
        assert!(slider.contains(cut));
        assert_eq!(partly.at(cut), None, "not where it is cut off");
        assert_eq!(partly.reveal(output), 0, "its title with it");
        assert_eq!(
            lay_out(&anchor, &mixed(), &Unfolded::default(), -5).scroll,
            0,
            "never above the top"
        );
    }

    #[test]
    fn each_program_has_a_row_naming_it_then_its_controls_and_unfolded_its_outputs() {
        let mut mixed = mixed();
        mixed.programs = [40, 41]
            .map(|stream| Program {
                stream,
                name: format!("program {stream}"),
                icon: None,
                level: level(70),
                sink: 0,
            })
            .into();
        mixed.sinks = vec![
            (0, listed("speakers", true, true)),
            (1, listed("e8", true, false)),
        ];
        let placed = lay_out(&ANCHOR, &mixed, &Unfolded::default(), 0);
        assert_eq!(placed.titles[2].0, Heading::Programs);
        let first = &controls(&placed)[9..];
        assert_eq!(
            first,
            [
                Control::Plays(40),
                Control::Mute(Whose::App(40)),
                Control::Volume(Whose::App(40)),
                Control::Plays(41),
                Control::Mute(Whose::App(41)),
                Control::Volume(Whose::App(41)),
            ]
        );
        assert_eq!(placed.percents[2].0, Whose::App(40));
        let name = placed.of(Control::Plays(40)).expect("placed");
        let slider = placed.of(Control::Volume(Whose::App(40))).expect("placed");
        assert_eq!(slider.y, name.bottom(), "its controls under its name");
        assert!(name.y > placed.titles[1].1.bottom(), "below the input");
        let unfolded = Unfolded {
            programs: vec![40],
            ..Unfolded::default()
        };
        let placed = lay_out(&ANCHOR, &mixed, &unfolded, 0);
        assert_eq!(
            controls(&placed)[12..14],
            [Control::Move(40, 0), Control::Move(40, 1)],
            "an output apiece under its controls"
        );
        let output = placed.of(Control::Move(40, 0)).expect("placed");
        assert_eq!(
            (output.x, output.right()),
            (placed.inside.x + BUTTON, placed.inside.right()),
            "set in under it"
        );
        assert_eq!(placed.unfolded, unfolded);
        let cramped = Anchor {
            area: Rect::new(0, 660, 1280, 100),
            ..ANCHOR
        };
        let bottom = lay_out(&cramped, &mixed, &unfolded, i32::MAX);
        let to = bottom.reveal(Control::Volume(Whose::App(40)));
        let revealed = lay_out(&cramped, &mixed, &unfolded, to);
        assert_eq!(
            revealed.of(Control::Plays(40)).expect("placed").y,
            revealed.inside.y,
            "its name in sight with it"
        );
        assert_eq!(mixed.most(Whose::App(40)), 100);
        assert_eq!(
            mixed
                .playing_on(&mixed.programs[0])
                .map(|listed| listed.key.as_str()),
            Some("speakers")
        );
        assert_eq!(mixed.level(Whose::App(9)), None, "no such program");
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
        assert_eq!(mixed.most(Whose::Side(Side::Output)), 100);
        assert_eq!(mixed.most(Whose::Side(Side::Input)), 130);
        assert_eq!(
            Mixed::default().most(Whose::Side(Side::Output)),
            1,
            "never nothing"
        );
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
            (0..64).all(|at| number(&format!("key {at}")) >> 61 == 0),
            "three bits short"
        );
        for control in Control::FIXED {
            assert_eq!(
                Control::FIXED.iter().filter(|&&had| had == control).count(),
                1
            );
        }
        assert_eq!(Control::Choose(Side::Input, 7).side(), Some(Side::Input));
        assert_eq!(Control::Fold(Side::Output).side(), Some(Side::Output));
        assert_eq!(Control::Volume(Whose::App(7)).side(), None);
    }
}
