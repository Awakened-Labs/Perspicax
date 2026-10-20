//! The Outputs page: each monitor's rule, staged until Apply.
//!
//! Every other page writes a change as it is made. A monitor's can leave
//! the screen dark, so here nothing is written until Apply, and the window
//! then asks whether to keep it, and puts the file back on its own when no
//! one answers (see [`crate::window`]). What is staged stays while other
//! pages are shown, and goes when the file's rules change under it.
//!
//! The monitors connected come first, as the session lists them to a
//! display tool, then each rule the file writes for one that is not. Every
//! control is named for its monitor, as a screen reader, an agent or a test
//! finds it: `DP-1: on`, `DP-1 mode: 2560 × 1440`, `DP-1 scale`.

use std::collections::BTreeMap;

use eframe::egui::{self, Response, Ui, WidgetType};
use perspicax_config::limits;
use perspicax_policy::{HeadMode, Side};
use perspicax_settings::{Arranged, Kind, Placed, Rule, Value};

use crate::{fields::named, monitors::Monitor};

/// How wide a typed number's field is drawn.
const NUMBER_WIDTH: f32 = 70.0;

/// What Apply asks to write: the rules staged, `to`, and those the file
/// wrote when they were staged, `base`, which a revert puts back.
pub(crate) struct Apply {
    pub(crate) base: Vec<Rule>,
    pub(crate) to: Vec<Rule>,
}

/// What the page is shown with.
pub(crate) struct View<'a> {
    /// The monitors connected, or why they are not listed.
    pub(crate) monitors: Result<&'a [Monitor], &'a str>,
    /// Whether the last change waits to be kept: Apply waits with it.
    pub(crate) counting: bool,
}

/// What is being done on the Outputs page.
#[derive(Default)]
pub(crate) struct Page {
    /// The file's rules, as the page was last shown them.
    base: Vec<Rule>,
    /// Each monitor's rule as staged, by name.
    staged: BTreeMap<String, Draft>,
    /// Why a typed field kept Apply from writing, by monitor and field.
    wrong: BTreeMap<(String, &'static str), String>,
    /// Why Apply wrote nothing, said for the whole page.
    refused: Option<String>,
}

/// One monitor's rule as staged, each typed field as typed.
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    /// The rule goes from the file.
    forgotten: bool,
    enable: Option<bool>,
    /// Empty is the monitor's preferred mode.
    mode: String,
    /// Empty is 1.
    scale: String,
    place: Place,
}

/// Where a staged rule puts its monitor, as typed.
#[derive(Debug, Clone, PartialEq)]
enum Place {
    Auto,
    At {
        x: String,
        y: String,
    },
    Beside {
        side: Side,
        of: String,
        offset: String,
    },
}

impl Draft {
    fn of(rule: &Rule) -> Self {
        Self {
            forgotten: false,
            enable: rule.enable,
            mode: rule.mode.clone().unwrap_or_default(),
            scale: rule
                .scale
                .map_or_else(String::new, |scale| scale.to_string()),
            place: match &rule.place {
                Placed::Auto => Place::Auto,
                Placed::At(x, y) => Place::At {
                    x: x.to_string(),
                    y: y.to_string(),
                },
                Placed::Beside { side, of, offset } => Place::Beside {
                    side: *side,
                    of: of.clone(),
                    offset: offset.map_or_else(String::new, |offset| offset.to_string()),
                },
            },
        }
    }

    /// The rule it stages for `name`, `None` for one forgotten, or why each
    /// field that cannot be read cannot, by its name.
    fn rule(&self, name: &str) -> Result<Option<Rule>, Vec<(&'static str, String)>> {
        if self.forgotten {
            return Ok(None);
        }
        let mut wrong = Vec::new();
        let mut read = |field: &'static str, read: Result<Option<i32>, String>| {
            read.unwrap_or_else(|why| {
                wrong.push((field, why));
                None
            })
        };
        let place = match &self.place {
            Place::Auto => Placed::Auto,
            Place::At { x, y } => {
                let x = read("x", pixels(x, false));
                let y = read("y", pixels(y, false));
                Placed::At(x.unwrap_or(0), y.unwrap_or(0))
            }
            Place::Beside { side, of, offset } => Placed::Beside {
                side: *side,
                of: of.clone(),
                offset: read("offset", pixels(offset, true)),
            },
        };
        let mode = self.mode.trim();
        if !mode.is_empty()
            && let Err(why) = mode.parse::<perspicax_config::Mode>()
        {
            wrong.push(("mode", why));
        }
        let scale = match self.scale.trim() {
            "" => None,
            scale => match (Kind::Fraction {
                range: limits::SCALE,
            })
            .parse(scale)
            {
                Ok(Value::Float(scale)) => Some(scale),
                Ok(_) => None,
                Err(why) => {
                    wrong.push(("scale", why));
                    None
                }
            },
        };
        if !wrong.is_empty() {
            return Err(wrong);
        }
        Ok(Some(Rule {
            name: name.to_owned(),
            enable: self.enable,
            mode: (!mode.is_empty()).then(|| mode.to_owned()),
            place,
            scale,
        }))
    }
}

/// A whole number of pixels, as typed; nothing for an empty field where
/// `optional`.
fn pixels(text: &str, optional: bool) -> Result<Option<i32>, String> {
    let range = limits::OUTPUT_PLACE;
    match text.trim() {
        "" if optional => Ok(None),
        "" => Err("write a whole number of pixels".to_owned()),
        text => match text.parse() {
            Ok(pixels) if range.contains(&pixels) => Ok(Some(pixels)),
            Ok(pixels) => Err(format!(
                "{pixels} is outside {} to {} pixels",
                range.start(),
                range.end()
            )),
            Err(_) => Err(format!("`{text}` is not a whole number of pixels")),
        },
    }
}

impl Page {
    /// The file's rules now: what is staged goes when they changed under
    /// it, as it was staged over what they said.
    pub(crate) fn rebase(&mut self, rules: Vec<Rule>) {
        if rules != self.base {
            self.staged.clear();
            self.wrong.clear();
            self.refused = None;
            self.base = rules;
        }
    }

    /// What was applied is written, or the file said it already: nothing
    /// is staged any more.
    pub(crate) fn applied(&mut self) {
        self.staged.clear();
    }

    /// Draw the page, and say what to write when Apply is pressed.
    pub(crate) fn ui(&mut self, ui: &mut Ui, view: &View<'_>) -> Option<Apply> {
        ui.heading("Outputs");
        ui.label(
            "Each monitor's rule. Nothing is written until Apply, and then the change is put \
             back in 15 seconds unless it is kept.",
        );
        ui.add_space(6.0);
        let monitors = view.monitors.unwrap_or_default();
        if let Err(why) = view.monitors {
            ui.weak(why);
        }
        let apply = self.foot(ui, view, monitors);
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            if !monitors.is_empty() {
                preview(ui, &self.arranged(monitors));
            }
            for monitor in monitors {
                self.monitor(ui, &monitor.head.name, Some(monitor), monitors);
            }
            let unconnected: Vec<String> = self
                .base
                .iter()
                .map(|rule| rule.name.clone())
                .filter(|name| !monitors.iter().any(|monitor| monitor.head.name == *name))
                .collect();
            for name in unconnected {
                self.monitor(ui, &name, None, monitors);
            }
        });
        apply
    }

    /// The rules as staged, each read as far as it can be: what the
    /// preview places.
    fn staged_rules(&self) -> Vec<Rule> {
        let names = self
            .base
            .iter()
            .map(|rule| rule.name.clone())
            .chain(self.staged.keys().cloned());
        let mut rules: Vec<Rule> = Vec::new();
        for name in names {
            if rules.iter().any(|rule| rule.name == name) {
                continue;
            }
            let base = self.base.iter().find(|rule| rule.name == name);
            let staged = match self.staged.get(&name) {
                Some(draft) => draft.rule(&name).unwrap_or_else(|_| base.cloned()),
                None => base.cloned(),
            };
            rules.extend(staged);
        }
        rules
    }

    fn arranged(&self, monitors: &[Monitor]) -> Vec<Arranged> {
        let heads: Vec<_> = monitors
            .iter()
            .map(|monitor| monitor.head.clone())
            .collect();
        perspicax_settings::arranged(&self.staged_rules(), &heads)
    }

    /// One monitor's rule: what it is now, then each thing the rule says.
    fn monitor(&mut self, ui: &mut Ui, name: &str, monitor: Option<&Monitor>, all: &[Monitor]) {
        let base = self.base.iter().find(|rule| rule.name == name).cloned();
        let base_draft = Draft::of(base.as_ref().unwrap_or(&Rule::new(name)));
        let mut draft = self
            .staged
            .get(name)
            .cloned()
            .unwrap_or_else(|| base_draft.clone());
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let made = monitor.map_or("", |monitor| monitor.made.as_str());
                let title = match (monitor, made) {
                    (None, _) => format!("{name}: not connected"),
                    (Some(_), "") => name.to_owned(),
                    (Some(_), made) => format!("{name} — {made}"),
                };
                ui.strong(title);
                if base.is_some() && !draft.forgotten {
                    let forget = ui.small_button(format!("Forget {name}'s rule"));
                    if forget.clicked() {
                        draft = Draft {
                            forgotten: true,
                            ..Draft::of(&Rule::new(name))
                        };
                    }
                }
            });
            if let Some(monitor) = monitor {
                ui.weak(now(monitor));
            }
            if draft.forgotten {
                ui.label("Its rule goes from the file when this is applied.");
            } else {
                self.controls(ui, name, monitor, all, &mut draft);
            }
            if draft != base_draft {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{name}: changed here, not yet applied."),
                );
            }
        });
        if draft == base_draft {
            self.staged.remove(name);
        } else {
            self.staged.insert(name.to_owned(), draft);
        }
        ui.add_space(4.0);
    }

    fn controls(
        &mut self,
        ui: &mut Ui,
        name: &str,
        monitor: Option<&Monitor>,
        all: &[Monitor],
        draft: &mut Draft,
    ) {
        let base_on = self
            .base
            .iter()
            .find(|rule| rule.name == name)
            .is_none_or(Rule::on);
        let mut on = draft.enable != Some(false);
        let check = ui.checkbox(&mut on, "on");
        named(
            &check,
            WidgetType::Checkbox,
            Some(on),
            &format!("{name}: on"),
        );
        if check.changed() {
            // A key the file writes stays written; one it doesn't is
            // written only to turn the monitor off.
            let written = self.base.iter().find(|rule| rule.name == name);
            draft.enable = if on == base_on {
                written.and_then(|rule| rule.enable)
            } else {
                Some(on)
            };
        }
        match monitor {
            Some(monitor) if !monitor.head.modes.is_empty() => {
                listed_mode(ui, name, &monitor.head.modes, &mut draft.mode);
            }
            _ => {
                self.typed(ui, name, "mode", &mut draft.mode, "2560x1440@144", 160.0);
                ui.weak("Empty is the monitor's preferred mode.");
            }
        }
        ui.horizontal(|ui| {
            self.typed(ui, name, "scale", &mut draft.scale, "1", NUMBER_WIDTH);
            ui.weak(format!(
                "{} to {}; empty is 1",
                limits::SCALE.start(),
                limits::SCALE.end()
            ));
        });
        self.place(ui, name, all, draft);
        for field in ["mode", "scale", "x", "y", "offset"] {
            if let Some(wrong) = self.wrong.get(&(name.to_owned(), field)) {
                ui.colored_label(ui.visuals().error_fg_color, format!("{field}: {wrong}"));
            }
        }
    }

    /// Where the monitor goes: to the right of the others, at a point, or
    /// beside another.
    fn place(&mut self, ui: &mut Ui, name: &str, all: &[Monitor], draft: &mut Draft) {
        let others: Vec<String> = all
            .iter()
            .map(|monitor| monitor.head.name.clone())
            .chain(self.base.iter().map(|rule| rule.name.clone()))
            .filter(|other| other != name)
            .fold(Vec::new(), |mut names, other| {
                if !names.contains(&other) {
                    names.push(other);
                }
                names
            });
        let kind = match draft.place {
            Place::Auto => "right of the others",
            Place::At { .. } => "at",
            Place::Beside { .. } => "beside",
        };
        // "at" starts from where the monitor is now, where that is known.
        let (x, y) = all
            .iter()
            .find(|monitor| monitor.head.name == name && monitor.head.enabled)
            .map_or_else(Default::default, |monitor| {
                let (x, y) = monitor.head.position;
                (x.to_string(), y.to_string())
            });
        ui.horizontal_wrapped(|ui| {
            ui.label("place:");
            for (words, place) in [
                ("right of the others", Place::Auto),
                (
                    "at",
                    Place::At {
                        x: x.clone(),
                        y: y.clone(),
                    },
                ),
                (
                    "beside",
                    Place::Beside {
                        side: Side::RightOf,
                        of: others.first().cloned().unwrap_or_default(),
                        offset: String::new(),
                    },
                ),
            ] {
                if radio(ui, &format!("{name} place"), words, kind == words) {
                    draft.place = place;
                }
            }
        });
        match &mut draft.place {
            Place::Auto => {}
            Place::At { x, y } => {
                ui.horizontal(|ui| {
                    self.typed(ui, name, "x", x, "0", NUMBER_WIDTH);
                    self.typed(ui, name, "y", y, "0", NUMBER_WIDTH);
                    ui.weak("its top-left corner, in logical pixels");
                });
            }
            Place::Beside { side, of, offset } => {
                ui.horizontal_wrapped(|ui| {
                    for each in Side::ALL {
                        if radio(ui, &format!("{name} side"), each.key(), *side == each) {
                            *side = each;
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("of:");
                    for other in &others {
                        if radio(ui, &format!("{name} beside"), other, of == other) {
                            of.clone_from(other);
                        }
                    }
                    if others.is_empty() {
                        self.typed(ui, name, "of", of, "DP-1", 120.0);
                    }
                });
                ui.horizontal(|ui| {
                    self.typed(ui, name, "offset", offset, "0", NUMBER_WIDTH);
                    ui.weak("pixels along the shared edge");
                });
            }
        }
    }

    /// A field typed into, named `name field` for whoever reads the window.
    fn typed(
        &mut self,
        ui: &mut Ui,
        name: &str,
        field: &'static str,
        text: &mut String,
        hint: &str,
        width: f32,
    ) {
        let label = ui.label(field);
        let edit = ui
            .add(
                egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .desired_width(width),
            )
            .labelled_by(label.id);
        name_field(&edit, &format!("{name} {field}"));
        if edit.changed() {
            self.wrong.remove(&(name.to_owned(), field));
            self.refused = None;
        }
    }

    /// Apply, and putting back what the file says; Apply's rules, when it
    /// is pressed and every field reads.
    fn foot(&mut self, ui: &mut Ui, view: &View<'_>, monitors: &[Monitor]) -> Option<Apply> {
        let mut apply = None;
        ui.horizontal(|ui| {
            let ready = !self.staged.is_empty() && !view.counting;
            let button = ui.add_enabled(ready, egui::Button::new("Apply"));
            let button = if view.counting {
                button.on_disabled_hover_text("Keep the last change, or put it back, first.")
            } else {
                button
            };
            if button.clicked() {
                apply = self.apply(monitors);
            }
            let back = ui.add_enabled(
                !self.staged.is_empty(),
                egui::Button::new("Put back what the file says"),
            );
            if back.clicked() {
                self.staged.clear();
                self.wrong.clear();
                self.refused = None;
            }
        });
        if !self.staged.is_empty() {
            ui.weak("Changed here; nothing is written until Apply.");
        }
        if let Some(refused) = &self.refused {
            ui.colored_label(ui.visuals().error_fg_color, refused);
        }
        apply
    }

    /// The rules staged, if each reads and they leave a monitor on.
    fn apply(&mut self, monitors: &[Monitor]) -> Option<Apply> {
        self.wrong.clear();
        self.refused = None;
        let mut to: Vec<Rule> = Vec::new();
        let names = self
            .base
            .iter()
            .map(|rule| rule.name.clone())
            .chain(self.staged.keys().cloned())
            .collect::<Vec<_>>();
        for name in &names {
            if to.iter().any(|rule| rule.name == *name) {
                continue;
            }
            let rule = match self.staged.get(name) {
                Some(draft) => match draft.rule(name) {
                    Ok(rule) => rule,
                    Err(wrong) => {
                        for (field, why) in wrong {
                            self.wrong.insert((name.clone(), field), why);
                        }
                        None
                    }
                },
                None => self.base.iter().find(|rule| rule.name == *name).cloned(),
            };
            to.extend(rule.filter(|rule| *rule != Rule::new(&rule.name)));
        }
        if !self.wrong.is_empty() {
            self.refused = Some("Nothing was written: a field above cannot be read.".to_owned());
            return None;
        }
        let connected: Vec<&str> = monitors
            .iter()
            .map(|monitor| monitor.head.name.as_str())
            .collect();
        if !connected.is_empty() && !perspicax_settings::leaves_one_on(&to, &connected) {
            self.refused = Some(
                "Nothing was written: that would leave no monitor on, and nothing to see the \
                 screen by."
                    .to_owned(),
            );
            return None;
        }
        Some(Apply {
            base: self.base.clone(),
            to,
        })
    }
}

/// The monitor as it is now, in words.
fn now(monitor: &Monitor) -> String {
    let head = &monitor.head;
    let Some(mode) = head.current.and_then(|at| head.modes.get(at)) else {
        return "Off now.".to_owned();
    };
    format!(
        "Now: {}, at {}, {}, scale {}.",
        perspicax_settings::mode_shown(mode),
        head.position.0,
        head.position.1,
        head.scale
    )
}

/// The modes a monitor offers: its sizes, then the refresh rates at the
/// size chosen. What is chosen is written as the monitor offers it, or as
/// its size alone for the fastest.
fn listed_mode(ui: &mut Ui, name: &str, modes: &[HeadMode], mode: &mut String) {
    let rule = Rule {
        mode: (!mode.trim().is_empty()).then(|| mode.trim().to_owned()),
        ..Rule::new(name)
    };
    let picked = perspicax_settings::picked(Some(&rule), modes);
    let written = rule
        .mode
        .as_deref()
        .and_then(|text| text.parse::<perspicax_config::Mode>().ok());
    let size = written.map(|written| (i32::from(written.width), i32::from(written.height)));
    let mut sizes: Vec<(i32, i32)> = Vec::new();
    for offered in modes {
        if !sizes.contains(&(offered.width, offered.height)) {
            sizes.push((offered.width, offered.height));
        }
    }
    let offered_size = size.filter(|size| sizes.contains(size));
    ui.horizontal_wrapped(|ui| {
        ui.label("mode:");
        let preferred = perspicax_settings::picked(None, modes)
            .map(|(at, _)| perspicax_settings::mode_shown(&modes[at]));
        let words = preferred.map_or_else(
            || "the preferred".to_owned(),
            |preferred| format!("the preferred ({preferred})"),
        );
        if radio(ui, &format!("{name} mode"), &words, rule.mode.is_none()) {
            mode.clear();
        }
        for (width, height) in &sizes {
            let words = format!("{width} × {height}");
            let chosen = rule.mode.is_some() && offered_size == Some((*width, *height));
            if radio(ui, &format!("{name} mode"), &words, chosen) {
                *mode = format!("{width}x{height}");
            }
        }
    });
    if let Some((width, height)) = offered_size {
        ui.horizontal_wrapped(|ui| {
            ui.label("refresh:");
            let fastest = written.is_some_and(|written| written.refresh.is_none());
            if radio(ui, &format!("{name} refresh"), "the fastest", fastest) {
                *mode = format!("{width}x{height}");
            }
            for (at, offered) in modes.iter().enumerate() {
                if (offered.width, offered.height) != (width, height) || offered.refresh <= 0 {
                    continue;
                }
                let hertz = format!("{:.2}", offered.hertz());
                let words = format!("{} Hz", hertz.trim_end_matches('0').trim_end_matches('.'));
                let chosen = !fastest && picked.is_some_and(|(picked, _)| picked == at);
                if radio(ui, &format!("{name} refresh"), &words, chosen) {
                    *mode = perspicax_settings::mode_word(offered);
                }
            }
        });
    }
    if let (Some(written), Some((at, false))) = (rule.mode.as_deref(), picked) {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "{name} does not offer {written}: it shows its preferred, {}.",
                perspicax_settings::mode_shown(&modes[at])
            ),
        );
    }
}

/// A radio showing `words`, named `group: words`; whether it was chosen.
fn radio(ui: &mut Ui, group: &str, words: &str, selected: bool) -> bool {
    let radio = ui.radio(selected, words);
    named(
        &radio,
        WidgetType::RadioButton,
        Some(selected),
        &format!("{group}: {words}"),
    );
    radio.clicked() && !selected
}

/// Name a text field for whoever reads the window: `DP-1 scale`, where the
/// label beside it says only `scale`.
fn name_field(response: &Response, name: &str) {
    named(response, WidgetType::TextEdit, None, name);
}

/// Where the monitors would go, drawn small: each a box with its name.
fn preview(ui: &mut Ui, arranged: &[Arranged]) {
    let (Some(left), Some(top)) = (
        arranged.iter().map(|one| one.at.0).min(),
        arranged.iter().map(|one| one.at.1).min(),
    ) else {
        return;
    };
    let right = arranged
        .iter()
        .map(|one| one.at.0 + one.size.0)
        .max()
        .unwrap_or(left);
    let bottom = arranged
        .iter()
        .map(|one| one.at.1 + one.size.1)
        .max()
        .unwrap_or(top);
    let (wide, high) = ((right - left).max(1) as f32, (bottom - top).max(1) as f32);
    let shrink = (ui.available_width().min(420.0) / wide).min(160.0 / high);
    let (area, _) = ui.allocate_exact_size(
        egui::vec2(wide * shrink, high * shrink),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(area);
    let visuals = ui.visuals();
    for one in arranged {
        let min = area.min
            + egui::vec2(
                (one.at.0 - left) as f32 * shrink,
                (one.at.1 - top) as f32 * shrink,
            );
        let size = egui::vec2(one.size.0 as f32 * shrink, one.size.1 as f32 * shrink);
        let rect = egui::Rect::from_min_size(min, size).shrink(1.0);
        painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
        painter.rect_stroke(
            rect,
            2.0,
            visuals.widgets.noninteractive.fg_stroke,
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            &one.name,
            egui::FontId::proportional(12.0),
            visuals.text_color(),
        );
    }
    ui.add_space(6.0);
}
