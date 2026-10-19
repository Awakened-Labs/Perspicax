//! The Protocols page: who may reach past their own windows, and what an
//! agent may do, each rule in force beside its profile's. It only reads.
//!
//! These rules are where the file draws its boundary, so the app never
//! writes them: they are changed in an editor, and nothing that drives the
//! app can widen them.

use eframe::egui::{self, Ui};
use perspicax_config::AgentSocket;
use perspicax_settings::{Grant, rule_words, socket_words};

/// How wide a rule's key and what it decides are drawn.
const KEY_WIDTH: f32 = 230.0;
/// How wide the rule in force, and whose it is, are drawn.
const RULE_WIDTH: f32 = 320.0;

/// Why the page has nothing to change.
pub(crate) const WHY: &str = "This page only reads. What may reach past its own windows, and \
    what an agent may do, is the file's to say: the app never writes it, so nothing that \
    drives the app can widen it. Change it in an editor.";

/// Draw the page; whether the file is to be opened.
pub(crate) fn ui(ui: &mut Ui, grants: &[Grant], socket: &AgentSocket) -> bool {
    ui.heading("Protocols");
    ui.label("Who may reach past their own windows, and what an agent may do.");
    ui.add_space(6.0);
    ui.label(WHY);
    let open = ui.button("Open the file").clicked();
    ui.add_space(6.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        for rows in grants.chunk_by(|one, next| one.table == next.table) {
            ui.add_space(8.0);
            let table = rows[0].table;
            ui.strong(egui::RichText::new(format!("[{table}]")).monospace());
            egui::Grid::new(("grants", table))
                .num_columns(2)
                .striped(true)
                .spacing([14.0, 10.0])
                .show(ui, |ui| {
                    for grant in rows {
                        row(ui, grant.key, grant.about, |ui| {
                            ui.label(rule_words(&grant.in_force));
                            ui.weak(whose(grant));
                        });
                    }
                    if table == "agent" {
                        row(
                            ui,
                            "socket",
                            "where an agent inside the session reaches it",
                            |ui| {
                                ui.label(socket_words(socket));
                                ui.weak("read once, at login: a change waits for the next");
                            },
                        );
                    }
                });
        }
        ui.add_space(8.0);
        ui.weak(
            "A name admits any program of that name, wherever it is; a path admits only \
             itself. While the screen is locked, none of them is.",
        );
    });
    open
}

/// One rule: its key and what it decides, then what `value` draws. Each
/// column is as wide on every frame, so text wrapped in one never changes
/// how wide the grid makes it.
fn row(ui: &mut Ui, key: &str, about: &str, value: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.set_width(KEY_WIDTH);
        ui.monospace(key);
        ui.add(egui::Label::new(egui::RichText::new(about).small().weak()).wrap());
    });
    ui.vertical(|ui| {
        ui.set_width(RULE_WIDTH);
        value(ui);
    });
    ui.end_row();
}

/// Whose a rule is, in words.
fn whose(grant: &Grant) -> String {
    match (grant.written, grant.in_force == grant.profile) {
        (false, _) => "the profile's".to_owned(),
        (true, true) => "yours, the same as the profile's".to_owned(),
        (true, false) => format!("yours; the profile's is {}", rule_words(&grant.profile)),
    }
}
