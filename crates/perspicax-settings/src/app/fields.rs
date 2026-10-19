//! A behaviour page: any [`Section`]'s keys, each with its value, whose it
//! is, and a way back to the profile's.
//!
//! A switch, a choice or a list is written the moment it is clicked. A
//! typed value is written when its field lets go of the keyboard -- Enter,
//! Tab, or a click elsewhere -- and Escape puts back what the file says. A
//! value the library refuses is said beside its field and not written; one
//! the session refuses comes back to its field with the reason.
//!
//! Every control is named for its key, as a screen reader, an agent or a
//! test finds it: a checkbox by its key, a radio as `model: sloppy`, a
//! button as `Reset model`.

use eframe::egui::{self, Id, Response, Ui, WidgetInfo, WidgetType};
use perspicax_settings::{
    Edit, Field, Key, Kind, Refused, Registry, Section, Setting, Source, Value,
};

use crate::layouts;

/// How wide a key's name and what it decides are drawn.
const KEY_WIDTH: f32 = 190.0;
/// How wide its control, and whose it is, are drawn.
const CONTROL_WIDTH: f32 = 360.0;

/// What is being done on a behaviour page.
#[derive(Default)]
pub(crate) struct Page {
    /// What is typed into a field, until it is written or given up.
    draft: Option<Draft>,
    /// The draft last handed on to be written, until the write is settled.
    sent: Option<Draft>,
    layouts: layouts::Editor,
}

/// A field's text as typed, and why it can't be written, if it can't.
struct Draft {
    key: Key,
    text: String,
    wrong: Option<String>,
}

impl Page {
    /// Draw `section`, its keys as `settings` has them, and say what to
    /// write, if anything.
    pub(crate) fn ui(
        &mut self,
        ui: &mut Ui,
        section: &Section,
        settings: &[Setting],
        registry: Option<&Registry>,
    ) -> Option<Vec<Edit>> {
        ui.heading(section.title);
        ui.label(section.about);
        ui.add_space(6.0);
        let mut edits = None;
        let rows: Vec<(&Field, &Setting)> = section.fields.iter().zip(settings).collect();
        let (switch, rest) = match rows.split_first() {
            Some((first, rest)) if section.switched => (Some(*first), rest),
            _ => (None, &rows[..]),
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some((field, setting)) = switch {
                if let Some(more) = self.table(ui, section, &[(field, setting)], registry) {
                    edits = Some(more);
                }
                if setting.in_force == Some(Value::Bool(false)) {
                    ui.label(
                        "Off. What the file writes for the rest stays as it is, and applies once \
                         this is on.",
                    );
                    return;
                }
            }
            for group in rest.chunk_by(|(one, _), (next, _)| one.key.table == next.key.table) {
                ui.add_space(8.0);
                let table = &group[0].0.key.table;
                if table.is_empty() {
                    ui.strong("At the top of the file");
                } else {
                    ui.strong(egui::RichText::new(format!("[{}]", table.join("."))).monospace());
                }
                if let Some(more) = self.table(ui, section, group, registry) {
                    edits = Some(more);
                }
            }
        });
        edits
    }

    /// The draft for whatever page is left, as the edit it makes: what was
    /// typed is not lost to a click on another page.
    pub(crate) fn flush(&mut self, section: &Section) -> Option<Vec<Edit>> {
        let draft = self.draft.take()?;
        let field = section.fields.iter().find(|field| field.key == draft.key)?;
        let edit = field.edit(&draft.text).ok()?;
        self.sent = Some(draft);
        Some(vec![edit])
    }

    /// Drop what is typed: the file it was typed over has changed, or the
    /// page has.
    pub(crate) fn forget(&mut self) {
        self.draft = None;
    }

    /// The last write is settled: a refused one gives its draft back to its
    /// field, with the reason.
    pub(crate) fn settled(&mut self, refused: Option<&Refused>) {
        let sent = self.sent.take();
        if let (Some(refused), Some(sent)) = (refused, sent) {
            self.draft = Some(Draft {
                wrong: Some(refused.reason.to_string()),
                ..sent
            });
        }
    }

    /// One table's keys, a row each.
    fn table(
        &mut self,
        ui: &mut Ui,
        section: &Section,
        rows: &[(&Field, &Setting)],
        registry: Option<&Registry>,
    ) -> Option<Vec<Edit>> {
        // Named by its first key: a section's switch sits in a grid of its
        // own above the rest of its table.
        let first = rows
            .first()
            .map(|(field, _)| (field.key.table.join("."), &field.key.leaf));
        let mut edits = None;
        egui::Grid::new((section.title, first))
            .num_columns(2)
            .striped(true)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                for &(field, setting) in rows {
                    if let Some(more) = self.row(ui, field, setting, rows, registry) {
                        edits = Some(more);
                    }
                }
            });
        edits
    }

    /// One key: its name and what it decides; then its control, whose it
    /// is, and its reset. Each column is as wide on every frame, so text
    /// wrapped in one never changes how wide the grid makes it.
    fn row(
        &mut self,
        ui: &mut Ui,
        field: &Field,
        setting: &Setting,
        rows: &[(&Field, &Setting)],
        registry: Option<&Registry>,
    ) -> Option<Vec<Edit>> {
        // With xkb's list, the variants are chosen beside their layouts.
        if registry.is_some() && field.kind == Kind::Variants {
            return None;
        }
        let leaf = field.key.leaf.as_str();
        let label = ui
            .vertical(|ui| {
                ui.set_width(KEY_WIDTH);
                let label = ui.monospace(leaf);
                if !field.says.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(field.says).small().weak()).wrap());
                }
                label.id
            })
            .inner;
        let edits = ui
            .vertical(|ui| {
                ui.set_width(CONTROL_WIDTH);
                let mut edits = self.control(ui, field, setting, label, rows, registry);
                if let Some(reset) = self.whose(ui, field, setting, rows, registry) {
                    edits = Some(reset);
                }
                edits
            })
            .inner;
        ui.end_row();
        edits
    }

    /// Whose `field`'s value is, what is in force where another key
    /// overrides it, and its reset; the reset's edits, if it is pressed.
    fn whose(
        &mut self,
        ui: &mut Ui,
        field: &Field,
        setting: &Setting,
        rows: &[(&Field, &Setting)],
        registry: Option<&Registry>,
    ) -> Option<Vec<Edit>> {
        let layouts = registry.is_some() && field.kind == Kind::Layouts;
        let variant = rows.iter().find(|(field, _)| field.kind == Kind::Variants);
        let written = setting.written.is_some()
            || (layouts && variant.is_some_and(|(_, setting)| setting.written.is_some()));
        let mut edits = None;
        ui.horizontal_wrapped(|ui| {
            let whose = ui.weak(whose(field, setting));
            if let Err(why) = &setting.profile {
                whose.on_hover_text(format!("Resetting it alone is refused: {why}"));
            }
            if !written {
                return;
            }
            let reset = ui.small_button("Reset to the profile's");
            named(
                &reset,
                WidgetType::Button,
                None,
                &format!("Reset {}", field.key.leaf),
            );
            if reset.clicked() {
                // The button wins over what was being typed in its row.
                self.draft = self.draft.take().filter(|draft| draft.key != field.key);
                self.sent = None;
                let mut reset = vec![field.reset()];
                if let Some((variant, _)) = variant.filter(|_| layouts) {
                    reset.push(variant.reset());
                }
                edits = Some(reset);
            }
        });
        if setting.overridden() {
            ui.weak(format!(
                "{} is in force: another setting overrides it",
                words(field, setting.in_force.as_ref())
            ));
        }
        edits
    }

    /// The control for `field`'s value.
    fn control(
        &mut self,
        ui: &mut Ui,
        field: &Field,
        setting: &Setting,
        label: Id,
        rows: &[(&Field, &Setting)],
        registry: Option<&Registry>,
    ) -> Option<Vec<Edit>> {
        let leaf = field.key.leaf.as_str();
        match (&field.kind, field.unset, registry) {
            (Kind::Switch, None, _) => {
                let mut on = setting.shown() == Some(&Value::Bool(true));
                ui.checkbox(&mut on, leaf)
                    .changed()
                    .then(|| vec![field.set(Value::Bool(on))])
            }
            (Kind::Switch, Some(unset), _) => {
                let shown = setting.shown();
                let mut chosen = None;
                ui.horizontal_wrapped(|ui| {
                    for (value, words) in [(None, unset), (Some(true), "on"), (Some(false), "off")]
                    {
                        let value = value.map(Value::Bool);
                        if radio(ui, leaf, words, shown == value.as_ref()) {
                            chosen = Some(value);
                        }
                    }
                });
                chosen.map(|value| {
                    vec![match value {
                        None => field.reset(),
                        Some(value) => field.set(value),
                    }]
                })
            }
            (Kind::Choice(words), ..) => {
                let shown = setting.shown();
                let mut chosen = None;
                ui.horizontal_wrapped(|ui| {
                    for word in words {
                        let value = Value::String((*word).to_owned());
                        if radio(ui, leaf, word, shown == Some(&value)) {
                            chosen = Some(value);
                        }
                    }
                });
                if shown.is_none()
                    && let Some(unset) = field.unset
                {
                    ui.weak(unset);
                }
                chosen.map(|value| vec![field.set(value)])
            }
            (Kind::List(words), ..) => list(ui, field, setting, words),
            (Kind::Layouts, _, Some(registry)) => {
                let text = |setting: &Setting| match setting.shown() {
                    Some(Value::String(text)) => text.clone(),
                    _ => String::new(),
                };
                let variant = rows
                    .iter()
                    .find(|(field, _)| field.kind == Kind::Variants)
                    .map_or_else(String::new, |(_, setting)| text(setting));
                self.layouts.ui(ui, registry, &text(setting), &variant)
            }
            _ => self.typed(ui, field, setting, label),
        }
    }

    /// A field typed into, written when it lets go of the keyboard.
    fn typed(
        &mut self,
        ui: &mut Ui,
        field: &Field,
        setting: &Setting,
        label: Id,
    ) -> Option<Vec<Edit>> {
        let shown = setting
            .shown()
            .map_or_else(String::new, |value| field.kind.text(value));
        let mine = self.draft.as_ref().filter(|draft| draft.key == field.key);
        let mut text = mine.map_or_else(|| shown.clone(), |draft| draft.text.clone());
        let hint = field.unset.unwrap_or(match &field.kind {
            Kind::Text { example } => example,
            Kind::Colour => "#rrggbb",
            Kind::Command => "foot -e htop",
            _ => "",
        });
        let mut edits = None;
        ui.horizontal(|ui| {
            if field.kind == Kind::Colour {
                swatch(ui, &text);
            }
            let response = ui
                .add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text(hint)
                        .desired_width(220.0),
                )
                .labelled_by(label);
            if response.changed() {
                self.draft = Some(Draft {
                    key: field.key.clone(),
                    text: text.clone(),
                    wrong: None,
                });
            }
            // True on two frames running: the draft is taken on the first.
            if response.lost_focus() {
                let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                edits = self.commit(field, &shown, escaped);
            }
            if let Some(bounds) = bounds(&field.kind) {
                ui.weak(bounds);
            }
        });
        if let Some(wrong) = self
            .draft
            .as_ref()
            .filter(|draft| draft.key == field.key)
            .and_then(|draft| draft.wrong.as_ref())
        {
            ui.colored_label(ui.visuals().error_fg_color, wrong);
        }
        edits
    }

    /// The edit `field`'s draft makes, as it lets go of the keyboard: none
    /// for Escape, or for text that says what the field showed.
    fn commit(&mut self, field: &Field, shown: &str, escaped: bool) -> Option<Vec<Edit>> {
        let draft = self.draft.take_if(|draft| draft.key == field.key)?;
        if escaped || draft.text == shown {
            return None;
        }
        match field.edit(&draft.text) {
            Ok(edit) => {
                self.sent = Some(draft);
                Some(vec![edit])
            }
            Err(wrong) => {
                self.draft = Some(Draft {
                    wrong: Some(wrong),
                    ..draft
                });
                None
            }
        }
    }
}

/// A list of words, each once, in order: each chosen one can move or go,
/// and each other can join at the end.
fn list(ui: &mut Ui, field: &Field, setting: &Setting, words: &[&str]) -> Option<Vec<Edit>> {
    let leaf = field.key.leaf.as_str();
    let chosen: Vec<String> = match setting.shown() {
        Some(Value::Strings(chosen)) => chosen.clone(),
        _ => Vec::new(),
    };
    let mut next = None;
    for (at, item) in chosen.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(item);
            let mut moved = |ui: &mut Ui, enabled: bool, says: &str, to: usize| {
                let button = ui.add_enabled(enabled, egui::Button::new(says).small());
                named(
                    &button,
                    WidgetType::Button,
                    None,
                    &format!("{leaf}: {item} {says}"),
                );
                if button.clicked() {
                    let mut list = chosen.clone();
                    list.swap(at, to);
                    next = Some(list);
                }
            };
            moved(ui, at > 0, "up", at.saturating_sub(1));
            moved(ui, at + 1 < chosen.len(), "down", at + 1);
            let remove = ui.add(egui::Button::new("remove").small());
            named(
                &remove,
                WidgetType::Button,
                None,
                &format!("{leaf}: {item} remove"),
            );
            if remove.clicked() {
                let mut list = chosen.clone();
                list.remove(at);
                next = Some(list);
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        for word in words
            .iter()
            .filter(|word| !chosen.iter().any(|item| item == *word))
        {
            let add = ui.add(egui::Button::new(format!("+ {word}")).small());
            named(
                &add,
                WidgetType::Button,
                None,
                &format!("{leaf}: add {word}"),
            );
            if add.clicked() {
                let mut list = chosen.clone();
                list.push((*word).to_owned());
                next = Some(list);
            }
        }
    });
    next.map(|list| vec![field.set(Value::Strings(list))])
}

/// A radio showing `words`, named `leaf: words`; whether it was chosen.
fn radio(ui: &mut Ui, leaf: &str, words: &str, selected: bool) -> bool {
    let radio = ui.radio(selected, words);
    named(
        &radio,
        WidgetType::RadioButton,
        Some(selected),
        &format!("{leaf}: {words}"),
    );
    radio.clicked() && !selected
}

/// Name `response`'s widget `name` for whoever reads the window: egui names
/// a widget by its own text, which on a page of many keys says too little.
pub(crate) fn named(response: &Response, typ: WidgetType, selected: Option<bool>, name: &str) {
    let enabled = response.enabled();
    response.widget_info(|| match selected {
        Some(selected) => WidgetInfo::selected(typ, enabled, selected, name),
        None => WidgetInfo::labeled(typ, enabled, name),
    });
}

/// A colour's swatch, beside the field it is typed into.
fn swatch(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    if let Some(colour) = perspicax_config::Rgba::parse(text.trim()) {
        let fill = egui::Color32::from_rgba_unmultiplied(colour.r, colour.g, colour.b, colour.a);
        ui.painter().rect_filled(rect, 3.0, fill);
    }
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
}

/// A number's range in words, beside its field.
fn bounds(kind: &Kind) -> Option<String> {
    let text = match kind {
        Kind::Number { range, unit } if *range.end() >= i64::from(i32::MAX) => {
            format!("{} or more {unit}", range.start())
        }
        Kind::Number { range, unit } => format!("{} to {} {unit}", range.start(), range.end()),
        Kind::Fraction { range } => format!("{} to {}", range.start(), range.end()),
        Kind::Pair { names, range, unit } => format!(
            "{}, then {}: each {} to {} {unit}",
            names[0],
            names[1],
            range.start(),
            range.end()
        ),
        _ => return None,
    };
    Some(text.trim_end().to_owned())
}

/// Whose a key's value is, in words.
fn whose(field: &Field, setting: &Setting) -> String {
    match (&setting.written, &setting.profile) {
        (None, _) => match &setting.source {
            Some(Source::Follows(role)) => format!("follows {}", role.key()),
            Some(Source::WrittenAt(key)) => {
                format!("written as {} in [{}]", key.leaf, key.table.join("."))
            }
            None => "the profile's".to_owned(),
        },
        (Some(written), Ok(profile)) if profile.as_ref() == Some(written) => {
            "yours, the same as the profile's".to_owned()
        }
        (Some(_), Ok(profile)) => {
            format!("yours; the profile's is {}", words(field, profile.as_ref()))
        }
        (Some(_), Err(_)) => "yours".to_owned(),
    }
}

/// A value in words, or what nothing means for this key.
fn words(field: &Field, value: Option<&Value>) -> String {
    value.map_or_else(
        || field.unset.unwrap_or("nothing").to_owned(),
        |value| field.kind.shown(value),
    )
}
