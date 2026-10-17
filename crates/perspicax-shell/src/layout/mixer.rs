//! The mixer: where it opens, and where each of its controls is.
//!
//! It opens beside the panel's mixer item, away from the panel, as a menu
//! opens beside its button, [`WIDE`] wide. It holds a section for the
//! output and one for the input, each a title, then a row of a button that
//! mutes, a slider for the volume, and the volume in percent. A slider
//! reaches from silence at its left end to the limit at its right, or to
//! where the volume is, if that is louder.

use perspicax_config::Edge;

use super::Rect;
use crate::model::sound::{Level, Side};

/// How wide the mixer is.
pub(crate) const WIDE: i32 = 340;
/// Its border, the room inside it, between its sections, and around a
/// control.
pub(crate) const BORDER: i32 = 1;
const PAD: i32 = 12;
const GAP: i32 = 8;
/// How tall a section's title is, and its row of controls.
const TITLE: i32 = 22;
const ROW: i32 = 32;
/// A mute button is square, and the percentage this wide.
const BUTTON: i32 = 28;
pub(crate) const PERCENT: i32 = 48;
/// How far in from its ends a slider's track starts, for its knob.
pub(crate) const KNOB: i32 = 7;

/// What the mixer shows of the sound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Mixed {
    /// The default output, or `None` with no sound server or none.
    pub(crate) output: Option<Master>,
    /// The default input, or `None` with none.
    pub(crate) input: Option<Master>,
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

impl Mixed {
    /// The device on `side`.
    pub(crate) fn on(&self, side: Side) -> Option<&Master> {
        match side {
            Side::Output => self.output.as_ref(),
            Side::Input => self.input.as_ref(),
        }
    }

    /// The loudest a slider on `side` reaches: the limit, or where its
    /// volume is, if that is louder.
    pub(crate) fn most(&self, side: Side) -> u32 {
        let now = self.on(side).map_or(0, |master| master.level.percent);
        self.limit.max(now).max(1)
    }
}

/// One of the mixer's controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Control {
    /// The button that mutes the device on this side.
    Mute(Side),
    /// The slider of its volume.
    Volume(Side),
}

impl Control {
    /// Every control, in the order the keyboard moves through them.
    pub(crate) const ALL: [Self; 4] = [
        Self::Mute(Side::Output),
        Self::Volume(Side::Output),
        Self::Mute(Side::Input),
        Self::Volume(Side::Input),
    ];
}

/// The mixer, laid out, in its monitor's logical pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placed {
    /// The whole of it.
    pub(crate) rect: Rect,
    /// Each section's title, and the side it is of.
    pub(crate) titles: Vec<(Side, Rect)>,
    /// Each control, and where its percentage is written beside a slider.
    pub(crate) controls: Vec<(Control, Rect)>,
    pub(crate) percents: Vec<(Side, Rect)>,
}

/// The mixer laid out, opened from `button`, the mixer item's place on its
/// monitor, on a panel along `edge`, within `area`, the room on the monitor
/// beside the panel.
pub(crate) fn lay_out(button: Rect, edge: Edge, area: Rect) -> Placed {
    let tall = 2 * BORDER + 2 * PAD + 2 * (TITLE + ROW) + GAP;
    let x = (button.right() - WIDE).clamp(area.x, (area.right() - WIDE).max(area.x));
    let y = match edge {
        Edge::Bottom => button.y - tall,
        Edge::Top => button.bottom(),
    }
    .clamp(area.y, (area.bottom() - tall).max(area.y));
    let rect = Rect::new(x, y, WIDE, tall);
    let inner = Rect::new(
        x + BORDER + PAD,
        y + BORDER + PAD,
        WIDE - 2 * (BORDER + PAD),
        tall - 2 * (BORDER + PAD),
    );
    let (mut titles, mut controls, mut percents) = (Vec::new(), Vec::new(), Vec::new());
    for (at, side) in [Side::Output, Side::Input].into_iter().enumerate() {
        let top = inner.y + at as i32 * (TITLE + ROW + GAP);
        titles.push((side, Rect::new(inner.x, top, inner.w, TITLE)));
        let row = top + TITLE;
        let button = Rect::new(inner.x, row + (ROW - BUTTON) / 2, BUTTON, BUTTON);
        let percent = Rect::new(inner.right() - PERCENT, row, PERCENT, ROW);
        let slider = Rect::new(
            button.right() + GAP,
            row,
            percent.x - GAP - (button.right() + GAP),
            ROW,
        );
        controls.push((Control::Mute(side), button));
        controls.push((Control::Volume(side), slider));
        percents.push((side, percent));
    }
    Placed {
        rect,
        titles,
        controls,
        percents,
    }
}

impl Placed {
    /// The control at `point`, if one is there.
    pub(crate) fn at(&self, point: (f64, f64)) -> Option<Control> {
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

    #[test]
    fn it_opens_above_a_bottom_panel_beside_its_item_and_on_the_monitor() {
        let placed = lay_out(ITEM, Edge::Bottom, AREA);
        assert_eq!(
            placed.rect.right(),
            ITEM.right(),
            "its right end at the item's"
        );
        assert_eq!(placed.rect.bottom(), ITEM.y, "standing on the panel");
        assert_eq!(placed.rect.w, WIDE);
        let left = lay_out(Rect::new(0, 760, 30, 40), Edge::Bottom, AREA);
        assert_eq!(left.rect.x, 0, "kept on the monitor");
        let top = lay_out(
            Rect::new(1180, 0, 100, 40),
            Edge::Top,
            Rect::new(0, 40, 1280, 760),
        );
        assert_eq!(top.rect.y, 40, "below a top panel");
        let whole = lay_out(ITEM, Edge::Bottom, Rect::new(0, 0, 1280, 800));
        assert_eq!(
            whole.rect.bottom(),
            ITEM.y,
            "above the item, room under the panel or not"
        );
    }

    #[test]
    fn each_side_has_a_title_a_mute_button_a_slider_and_a_percentage_in_order() {
        let placed = lay_out(ITEM, Edge::Bottom, AREA);
        let controls: Vec<Control> = placed
            .controls
            .iter()
            .map(|&(control, _)| control)
            .collect();
        assert_eq!(controls, Control::ALL);
        let output = placed.of(Control::Volume(Side::Output)).expect("a slider");
        let input = placed.of(Control::Volume(Side::Input)).expect("a slider");
        assert!(input.y > output.bottom(), "the input below the output");
        let mute = placed.of(Control::Mute(Side::Output)).expect("a button");
        assert!(mute.right() < output.x, "the button left of the slider");
        let (_, percent) = placed.percents[0];
        assert!(output.right() < percent.x, "the percentage right of it");
        let middle = |rect: Rect| (f64::from(rect.x) + 2.0, f64::from(rect.y) + 2.0);
        assert_eq!(
            placed.at(middle(output)),
            Some(Control::Volume(Side::Output))
        );
        assert_eq!(
            placed.at(middle(placed.titles[0].1)),
            None,
            "a title is no control"
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
        let level = |percent| Level {
            percent,
            muted: false,
        };
        let mixed = Mixed {
            output: Some(Master {
                name: "Speakers".to_owned(),
                level: level(40),
            }),
            input: Some(Master {
                name: "Microphone".to_owned(),
                level: level(130),
            }),
            step: 5,
            limit: 100,
        };
        assert_eq!(mixed.most(Side::Output), 100);
        assert_eq!(mixed.most(Side::Input), 130);
        assert_eq!(Mixed::default().most(Side::Output), 1, "never nothing");
    }
}
