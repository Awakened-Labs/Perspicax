//! The Keys page: every chord the session binds and every one the file
//! writes, each with whose it is, in the file's own words; and a chord bound
//! by pressing it.
//!
//! A capture is asked for only from a click in this window, so the window
//! has the keyboard, as the compositor needs. While one waits, nothing here
//! takes typing: the compositor would take the first key as the chord. So a
//! chord is typed only once the capture is given up, and read as the config
//! reads it.

use eframe::egui::{self, Ui};
use perspicax_config::{Spelled, Trigger};
use perspicax_policy::{Action, Hatch};
use perspicax_settings::{Binding, Edit, Mark, Pressed, key_name};

use crate::capture::{Channel, Heard, Why};

/// The compositor's channel, or why there is none.
pub(crate) type Capture<'a> = Result<&'a mut (dyn Channel + 'static), &'a str>;

/// What the page shows, and the words a binding may use.
pub(crate) struct View<'a> {
    pub(crate) rows: &'a [Binding],
    /// Every action name, in the order the picker offers them.
    pub(crate) names: &'a [String],
    /// The pies `[shell.pie.menus]` names.
    pub(crate) pies: &'a [String],
}

/// The page, and what is being done on it.
#[derive(Default)]
pub(crate) struct Page {
    editing: Editing,
}

#[derive(Default)]
enum Editing {
    #[default]
    Nothing,
    /// A row chosen, with what can be done to it.
    Chosen(Trigger),
    /// Waiting for a chord, pressed or typed.
    Capturing(Capturing),
    /// The action for a chord.
    Choosing(Choosing),
}

struct Capturing {
    /// The chord whose action moves to the one pressed; `None` for a new
    /// binding.
    from: Option<Trigger>,
    /// Whether a capture waits.
    waiting: bool,
    /// Why the last capture bound nothing, when it heard no chord.
    said: Option<String>,
    typed: String,
    /// Why `typed` is no chord.
    wrong: Option<String>,
}

impl Capturing {
    fn new(from: Option<Trigger>, waiting: bool) -> Self {
        Self {
            from,
            waiting,
            said: None,
            typed: String::new(),
            wrong: None,
        }
    }
}

struct Choosing {
    trigger: Trigger,
    written: String,
    /// What it does now, which saving replaces.
    replaces: Option<String>,
    kind: Kind,
    name: String,
    command: String,
    agent: bool,
    pie: String,
    /// Why the choice cannot be saved.
    wrong: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Name,
    Spawn,
    Pie,
}

impl Page {
    /// Draw the page, and say what to write, if anything. `capture` is the
    /// compositor's channel, or why there is none.
    pub(crate) fn ui(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        mut capture: Capture<'_>,
    ) -> Option<Vec<Edit>> {
        let mut edits = None;
        if let (Editing::Capturing(Capturing { waiting: true, .. }), Ok(channel)) =
            (&self.editing, &mut capture)
            && let Some(heard) = channel.heard()
        {
            edits = self.hear(heard, view);
        }
        ui.horizontal(|ui| {
            ui.heading("Keys");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Add a binding…").clicked() {
                    self.capture(None, &mut capture);
                }
            });
        });
        ui.label(
            "What each chord does, and whose it is: the profile's, or yours, written in \
             the [keys] table of the file.",
        );
        ui.add_space(6.0);
        if let Some(more) = self.editor(ui, view, &mut capture) {
            edits = Some(more);
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| self.rows(ui, view, &mut capture));
        edits
    }

    /// Begin waiting for a chord, for a new binding or to move `from`.
    fn capture(&mut self, from: Option<Trigger>, capture: &mut Capture<'_>) {
        let waiting = match capture {
            Ok(channel) => {
                channel.ask();
                true
            }
            Err(_) => false,
        };
        self.editing = Editing::Capturing(Capturing::new(from, waiting));
    }

    /// Leave whatever is being done, giving up a capture that waits.
    fn leave(&mut self, next: Editing, capture: &mut Capture<'_>) {
        if let (Editing::Capturing(Capturing { waiting: true, .. }), Ok(channel)) =
            (&self.editing, capture)
        {
            channel.stop();
        }
        self.editing = next;
    }

    /// What a capture heard.
    fn hear(&mut self, heard: Heard, view: &View<'_>) -> Option<Vec<Edit>> {
        let Editing::Capturing(capturing) = &mut self.editing else {
            return None;
        };
        capturing.waiting = false;
        let (trigger, written) = match heard {
            Heard::Pressed(Pressed::Chord { trigger, written }) => (trigger, written),
            Heard::Tap => (Trigger::LogoTap, "Logo".to_owned()),
            Heard::Pressed(Pressed::GaveUp) => {
                self.editing = capturing.from.map_or(Editing::Nothing, Editing::Chosen);
                return None;
            }
            Heard::Pressed(Pressed::NotBindable(key)) => {
                capturing.said = Some(format!(
                    "{} can't be written in a config yet, so it can't be bound. Press \
                     another, or type one.",
                    key_name(key)
                ));
                return None;
            }
            Heard::Hatch(hatch) => {
                capturing.said = Some(hatch_words(hatch));
                return None;
            }
            Heard::Cancelled(why) => {
                capturing.said = Some(why_words(why).to_owned());
                return None;
            }
        };
        self.pressed(trigger, written, view)
    }

    /// A chord pressed or typed: the action moved to it, or chosen for it.
    fn pressed(&mut self, trigger: Trigger, written: String, view: &View<'_>) -> Option<Vec<Edit>> {
        let Editing::Capturing(capturing) = &self.editing else {
            return None;
        };
        if let Some(from) = capturing.from {
            self.editing = Editing::Chosen(trigger);
            return moved(view.rows, from, trigger);
        }
        self.editing = Editing::Choosing(Choosing::new(trigger, written, view.rows));
        None
    }

    /// The panel for what is being done.
    fn editor(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        capture: &mut Capture<'_>,
    ) -> Option<Vec<Edit>> {
        if matches!(self.editing, Editing::Nothing) {
            return None;
        }
        egui::Frame::group(ui.style())
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                match &self.editing {
                    Editing::Nothing => None,
                    Editing::Chosen(trigger) => self.chosen(ui, *trigger, view, capture),
                    Editing::Capturing(_) => self.capturing(ui, view, capture),
                    Editing::Choosing(_) => self.choosing(ui, view),
                }
            })
            .inner
    }

    /// A row chosen: what it does, and what can be done to it.
    fn chosen(
        &mut self,
        ui: &mut Ui,
        trigger: Trigger,
        view: &View<'_>,
        capture: &mut Capture<'_>,
    ) -> Option<Vec<Edit>> {
        let Some(row) = view.rows.iter().find(|row| row.trigger == trigger) else {
            ui.label("That chord is no longer in the file.");
            return None;
        };
        ui.label(egui::RichText::new(format!("{}: {}", row.written_as, doing(row))).strong());
        ui.label(whose(row));
        let mut edits = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Change the action…").clicked() {
                let choosing = Choosing::new(trigger, row.written_as.clone(), view.rows);
                self.leave(Editing::Choosing(choosing), capture);
            }
            if row.action().is_some() && ui.button("Move to another chord…").clicked() {
                self.capture(Some(trigger), capture);
            }
            if row.action().is_some() && row.profile.is_some() && ui.button("Unbind").clicked() {
                edits = Some(vec![Edit::Bind {
                    trigger,
                    action: None,
                }]);
            }
            if row.written.is_some() {
                let reset = if row.profile.is_some() {
                    "Reset to the profile's"
                } else {
                    "Remove"
                };
                if ui.button(reset).clicked() {
                    edits = Some(vec![Edit::ResetBinding { trigger }]);
                }
            }
        });
        edits
    }

    /// Waiting for a chord, or for one typed.
    fn capturing(
        &mut self,
        ui: &mut Ui,
        view: &View<'_>,
        capture: &mut Capture<'_>,
    ) -> Option<Vec<Edit>> {
        let Editing::Capturing(capturing) = &mut self.editing else {
            return None;
        };
        match capturing
            .from
            .and_then(|from| view.rows.iter().find(|row| row.trigger == from))
        {
            Some(from) => ui.label(format!(
                "Moving {} ({}) to another chord.",
                from.written_as,
                doing(from)
            )),
            None => ui.label("A new binding."),
        };
        if capturing.waiting {
            ui.label(egui::RichText::new("Press the chord. Escape on its own gives up.").strong());
            let (mut typing, mut giving_up) = (false, false);
            ui.horizontal(|ui| {
                typing = ui.button("Type it instead").clicked();
                giving_up = ui.button("Give up").clicked();
            });
            if let Ok(channel) = capture
                && (typing || giving_up)
            {
                channel.stop();
                capturing.waiting = false;
            }
            if giving_up {
                self.editing = capturing.from.map_or(Editing::Nothing, Editing::Chosen);
            }
            return None;
        }
        match (&capturing.said, &capture) {
            (Some(said), _) => ui.label(said.as_str()),
            (None, Err(why)) => ui.label(*why),
            (None, Ok(_)) => ui.label("Type the chord, as the file would write it."),
        };
        let mut used = false;
        let (mut pressing, mut giving_up) = (false, false);
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut capturing.typed).hint_text("Logo+Shift+a");
            let entered =
                ui.add(field).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            used = ui.button("Use").clicked() || entered;
            if capture.is_ok() {
                pressing = ui.button("Press it instead").clicked();
            }
            giving_up = ui.button("Give up").clicked();
        });
        if let Some(wrong) = &capturing.wrong {
            ui.colored_label(ui.visuals().error_fg_color, wrong);
        }
        if giving_up {
            self.editing = capturing.from.map_or(Editing::Nothing, Editing::Chosen);
            return None;
        }
        if pressing {
            let from = capturing.from;
            self.capture(from, capture);
            return None;
        }
        if used {
            match typed(&capturing.typed) {
                Ok((trigger, written)) => return self.pressed(trigger, written, view),
                Err(wrong) => capturing.wrong = Some(wrong),
            }
        }
        None
    }

    /// Choosing what a chord does.
    fn choosing(&mut self, ui: &mut Ui, view: &View<'_>) -> Option<Vec<Edit>> {
        let Editing::Choosing(choosing) = &mut self.editing else {
            return None;
        };
        ui.label(egui::RichText::new(format!("{}: what should it do?", choosing.written)).strong());
        if let Some(replaces) = &choosing.replaces {
            ui.label(format!("Saving replaces what it does now: {replaces}."));
        }
        ui.horizontal(|ui| {
            ui.radio_value(&mut choosing.kind, Kind::Name, "An action");
            ui.radio_value(&mut choosing.kind, Kind::Spawn, "Start a program");
            if !view.pies.is_empty() {
                ui.radio_value(&mut choosing.kind, Kind::Pie, "Open a pie");
            }
        });
        match choosing.kind {
            Kind::Name => pick(ui, "Action", &mut choosing.name, view.names),
            Kind::Spawn => {
                ui.horizontal(|ui| {
                    ui.label("Command");
                    ui.add(
                        egui::TextEdit::singleline(&mut choosing.command).hint_text("foot -e htop"),
                    );
                });
                ui.checkbox(&mut choosing.agent, "An agent may act on what it starts");
            }
            Kind::Pie => pick(ui, "Pie", &mut choosing.pie, view.pies),
        }
        if let Some(wrong) = &choosing.wrong {
            ui.colored_label(ui.visuals().error_fg_color, wrong);
        }
        let (mut saving, mut giving_up) = (false, false);
        ui.horizontal(|ui| {
            saving = ui.button("Save").clicked();
            giving_up = ui.button("Give up").clicked();
        });
        if giving_up {
            self.editing = Editing::Nothing;
            return None;
        }
        if !saving {
            return None;
        }
        match choosing.spelled() {
            Ok(action) => {
                let trigger = choosing.trigger;
                self.editing = Editing::Chosen(trigger);
                Some(vec![Edit::Bind {
                    trigger,
                    action: Some(action),
                }])
            }
            Err(wrong) => {
                choosing.wrong = Some(wrong);
                None
            }
        }
    }

    /// The table: each chord, what it does, and whose it is.
    fn rows(&mut self, ui: &mut Ui, view: &View<'_>, capture: &mut Capture<'_>) {
        egui::Grid::new("keys")
            .num_columns(3)
            .striped(true)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                ui.strong("Chord");
                ui.strong("Does");
                ui.strong("Whose");
                ui.end_row();
                for row in view.rows {
                    let chosen =
                        matches!(self.editing, Editing::Chosen(trigger) if trigger == row.trigger);
                    if ui.selectable_label(chosen, &row.written_as).clicked() {
                        self.leave(Editing::Chosen(row.trigger), capture);
                    }
                    ui.label(doing(row));
                    ui.label(whose(row));
                    ui.end_row();
                }
            });
    }
}

impl Choosing {
    /// Choosing for `trigger`, from what it does now.
    fn new(trigger: Trigger, written: String, rows: &[Binding]) -> Self {
        let now = rows
            .iter()
            .find(|row| row.trigger == trigger)
            .and_then(Binding::action);
        let mut choosing = Self {
            trigger,
            written,
            replaces: now.map(words),
            kind: Kind::Name,
            name: String::new(),
            command: String::new(),
            agent: false,
            pie: String::new(),
            wrong: None,
        };
        match now.and_then(perspicax_config::spell_action) {
            Some(Spelled::Name(name)) => choosing.name = name,
            Some(Spelled::Spawn { command, agent }) => {
                choosing.kind = Kind::Spawn;
                choosing.command = joined(&command);
                choosing.agent = agent;
            }
            Some(Spelled::Pie(pie)) => {
                choosing.kind = Kind::Pie;
                choosing.pie = pie;
            }
            None => {}
        }
        choosing
    }

    /// The action chosen, as the file will say it.
    fn spelled(&self) -> Result<Spelled, String> {
        match self.kind {
            Kind::Name if self.name.is_empty() => Err("Choose an action.".to_owned()),
            Kind::Name => Ok(Spelled::Name(self.name.clone())),
            Kind::Spawn => match shlex::split(&self.command) {
                Some(command) if !command.is_empty() => Ok(Spelled::Spawn {
                    command,
                    agent: self.agent,
                }),
                Some(_) => Err("Write the program to start.".to_owned()),
                None => Err("A quote in the command is never closed.".to_owned()),
            },
            Kind::Pie if self.pie.is_empty() => Err("Choose a pie.".to_owned()),
            Kind::Pie => Ok(Spelled::Pie(self.pie.clone())),
        }
    }
}

/// A list to choose one word from.
fn pick(ui: &mut Ui, label: &str, chosen: &mut String, words: &[String]) {
    ui.horizontal(|ui| {
        ui.label(label);
        let shown = if chosen.is_empty() {
            "Choose…"
        } else {
            chosen.as_str()
        };
        egui::ComboBox::from_id_salt(label)
            .selected_text(shown.to_owned())
            .height(320.0)
            .show_ui(ui, |ui| {
                for word in words {
                    ui.selectable_value(chosen, word.clone(), word);
                }
            });
    });
}

/// The edits that move `from`'s action to `to`: bound there, and `from`
/// left doing nothing, which for a chord the profile binds takes `"none"`.
fn moved(rows: &[Binding], from: Trigger, to: Trigger) -> Option<Vec<Edit>> {
    if from == to {
        return None;
    }
    let row = rows.iter().find(|row| row.trigger == from)?;
    let action = perspicax_config::spell_action(row.action()?)?;
    let off = if row.profile.is_some() {
        Edit::Bind {
            trigger: from,
            action: None,
        }
    } else {
        Edit::ResetBinding { trigger: from }
    };
    Some(vec![
        Edit::Bind {
            trigger: to,
            action: Some(action),
        },
        off,
    ])
}

/// A chord typed, read as the config reads it: the trigger, and how the file
/// will write it.
fn typed(text: &str) -> Result<(Trigger, String), String> {
    let trigger = perspicax_config::trigger(text)?;
    if let Trigger::Chord(chord) = trigger
        && let Some(hatch) = perspicax_policy::hatch(chord.mods, [chord.key])
    {
        return Err(hatch_words(hatch));
    }
    let written =
        perspicax_config::spell_trigger(trigger).unwrap_or_else(|| text.trim().to_owned());
    Ok((trigger, written))
}

fn hatch_words(hatch: Hatch) -> String {
    match hatch {
        Hatch::Exit => "Ctrl+Alt+Backspace ends the session; nothing can bind it.".to_owned(),
        Hatch::Vt(vt) => format!("Ctrl+Alt+F{vt} switches to VT {vt}; nothing can bind it."),
    }
}

fn why_words(why: Why) -> &'static str {
    match why {
        Why::Unfocused => "The window lost the keyboard before a chord was pressed.",
        Why::Locked => "The session locked before a chord was pressed.",
        Why::Replaced => "Another program asked for the next chord.",
        Why::Withdrawn => {
            "perspicax's [protocols] settings rule no longer admits this program, so it \
             can capture no chord; type it instead."
        }
        Why::Unknown => "The capture ended with nothing.",
    }
}

/// What a row does, in the file's words: `"—"` for nothing.
fn doing(row: &Binding) -> String {
    row.action().map_or_else(|| "—".to_owned(), words)
}

/// Whose a row is, in words.
fn whose(row: &Binding) -> String {
    match (row.mark(), &row.profile) {
        (Mark::Profile, _) => "the profile's".to_owned(),
        (Mark::Own, _) => "yours".to_owned(),
        (Mark::Overridden, Some(profile)) => {
            format!("yours, over the profile's {}", words(profile))
        }
        (Mark::Overridden, None) => "yours".to_owned(),
        (Mark::Unbound, Some(profile)) => {
            format!("unbound by you; the profile's is {}", words(profile))
        }
        (Mark::Unbound, None) => "unbound by you".to_owned(),
    }
}

/// An action in the file's words: `close`, `spawn foot -e htop`, `pie
/// launchers`.
fn words(action: &Action) -> String {
    match perspicax_config::spell_action(action) {
        Some(Spelled::Name(name)) => name,
        Some(Spelled::Spawn { command, agent }) => {
            let run = format!("spawn {}", joined(&command));
            if agent {
                format!("{run}, for an agent")
            } else {
                run
            }
        }
        Some(Spelled::Pie(pie)) => format!("pie {pie}"),
        None => format!("{action:?}"),
    }
}

/// A command's words as a shell would write them, quoted where they must be.
fn joined(command: &[String]) -> String {
    shlex::try_join(command.iter().map(String::as_str)).unwrap_or_else(|_| command.join(" "))
}
