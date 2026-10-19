//! The keyboard's layouts, picked from xkb's list by what xkb calls them,
//! so a name xkb does not know is never written. Each change writes
//! `layout` and `variant` together.

use eframe::egui::{self, Ui, WidgetType};
use perspicax_config::limits;
use perspicax_settings::{Edit, Layout, Registry};

use crate::fields::named;

/// What is being done to the layouts.
#[derive(Default)]
pub(crate) struct Editor {
    /// Finding a layout to add: what is typed to find it.
    finding: Option<String>,
    /// The layout, by its place, whose variant is being chosen.
    varying: Option<usize>,
}

/// How many layouts the finder lists at once.
const LISTED: usize = 40;

impl Editor {
    /// The layouts `layout` and `variant` write, and the edits that change
    /// them, if one is made.
    pub(crate) fn ui(
        &mut self,
        ui: &mut Ui,
        registry: &Registry,
        layout: &str,
        variant: &str,
    ) -> Option<Vec<Edit>> {
        let now = perspicax_settings::layouts(layout, variant);
        let mut next: Option<Vec<Layout>> = None;
        if now.is_empty() {
            ui.weak("None written: xkb's default.");
        }
        for (at, written) in now.iter().enumerate() {
            let name = &written.name;
            ui.horizontal_wrapped(|ui| {
                match registry.layout(name) {
                    Some(entry) => ui.label(format!("{name} — {}", entry.description)),
                    None => ui.label(format!("{name}: not in xkb's list here")),
                };
                let variant = match (
                    written.variant.as_str(),
                    registry.variant(name, &written.variant),
                ) {
                    ("", _) => "its own".to_owned(),
                    (_, Some(entry)) => entry.description.clone(),
                    (variant, None) => variant.to_owned(),
                };
                ui.weak(variant);
                let button = |ui: &mut Ui, enabled: bool, says: &str| {
                    let button = ui.add_enabled(enabled, egui::Button::new(says).small());
                    named(
                        &button,
                        WidgetType::Button,
                        None,
                        &format!("{name}: {says}"),
                    );
                    button.clicked()
                };
                if button(ui, at > 0, "up") {
                    next = Some(swapped(&now, at, at - 1));
                }
                if button(ui, at + 1 < now.len(), "down") {
                    next = Some(swapped(&now, at, at + 1));
                }
                if button(ui, true, "variant…") {
                    self.varying = (self.varying != Some(at)).then_some(at);
                }
                if button(ui, true, "remove") {
                    let mut list = now.clone();
                    list.remove(at);
                    next = Some(list);
                }
            });
            if self.varying == Some(at) {
                ui.indent(("variants", at), |ui| {
                    let own = format!("{name}'s own");
                    let variants = std::iter::once(("", own.as_str())).chain(
                        registry
                            .variants(name)
                            .map(|entry| (entry.name.as_str(), entry.description.as_str())),
                    );
                    for (variant, says) in variants {
                        if ui
                            .selectable_label(written.variant == variant, says)
                            .clicked()
                        {
                            let mut list = now.clone();
                            list[at].variant = variant.to_owned();
                            next = Some(list);
                            self.varying = None;
                        }
                    }
                });
            }
        }
        if now.len() < usize::from(*limits::LAYOUTS.end()) {
            self.finder(ui, registry, &now, &mut next);
        }
        next.map(|layouts| perspicax_settings::layout_edits(&layouts))
    }

    /// "Add a layout…", and the layouts that match what is typed.
    fn finder(
        &mut self,
        ui: &mut Ui,
        registry: &Registry,
        now: &[Layout],
        next: &mut Option<Vec<Layout>>,
    ) {
        let Some(find) = &mut self.finding else {
            if ui.button("Add a layout…").clicked() {
                self.finding = Some(String::new());
            }
            return;
        };
        let mut done = false;
        ui.horizontal(|ui| {
            let label = ui.label("Find a layout");
            ui.add(egui::TextEdit::singleline(find).hint_text("Russian"))
                .labelled_by(label.id);
            done = ui.button("Done").clicked();
        });
        let needle = find.trim().to_lowercase();
        let found = registry
            .layouts()
            .iter()
            .filter(|entry| now.iter().all(|layout| layout.name != entry.name))
            .filter(|entry| {
                entry.name.contains(&needle) || entry.description.to_lowercase().contains(&needle)
            })
            .take(LISTED);
        for entry in found {
            let says = format!("{} ({})", entry.description, entry.name);
            if ui.selectable_label(false, says).clicked() {
                let mut list = now.to_vec();
                list.push(Layout {
                    name: entry.name.clone(),
                    variant: String::new(),
                });
                *next = Some(list);
                done = true;
            }
        }
        if done {
            self.finding = None;
        }
    }
}

/// `layouts` with the two at `one` and `other` swapped.
fn swapped(layouts: &[Layout], one: usize, other: usize) -> Vec<Layout> {
    let mut list = layouts.to_vec();
    list.swap(one, other);
    list
}
