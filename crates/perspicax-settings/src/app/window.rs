//! The window: the file, what the session makes of it, and the pages.
//!
//! Every change is one [`Document::apply`], so what is written is only ever
//! a file the installed perspicax takes. The file is read again whenever
//! the window gets the keyboard back, so an edit made in another editor
//! meanwhile is what the page shows.
//!
//! A change to the monitors waits to be kept: unless Keep is pressed within
//! 15 seconds, or if the window is closed first, the values it changed are
//! written back as they were, on top of whatever else the file says by
//! then. A program killed in that time puts nothing back: its change stays
//! written.

use std::{path::Path, time::Duration};

use eframe::egui::{self, Ui};
use perspicax_config::{Built, Config};
use perspicax_settings::{
    Applied, Binding, Document, Edit, Reason, Refused, Registry, Rule, Section, Setting,
};

use crate::{
    capture::Channel,
    fields, keys,
    monitors::{self, Monitor, Monitors, Seen},
    outputs,
};

/// How long a change to the monitors waits to be kept, in seconds.
const KEEP_FOR: f64 = 15.0;

/// What decides that a file may be written: the installed perspicax, or a
/// test's stand-in.
pub(crate) type Judge = Box<dyn Fn(&Path) -> Result<(), String>>;

/// Every feature on, as the writer reads a file: what this build of
/// perspicax lacks is the judge's to say.
pub(crate) const EVERYTHING: Built = Built {
    seat: true,
    xwayland: true,
    capture: true,
    svg: true,
};

pub(crate) struct App {
    document: Document,
    judge: Judge,
    capture: Result<Box<dyn Channel>, String>,
    /// Why the file as it stands is refused, if it is: the session then runs
    /// on the last one it took.
    standing: Option<Refused>,
    /// What the file binds, or nothing where perspicax refuses it.
    rows: Vec<Binding>,
    names: Vec<String>,
    pies: Vec<String>,
    keys: keys::Page,
    /// The behaviour pages, and each one's keys as the file stands: none
    /// where perspicax refuses the file before it can be read.
    sections: Vec<Section>,
    settings: Vec<Vec<Setting>>,
    fields: fields::Page,
    /// xkb's layouts, where they could be found.
    registry: Option<Registry>,
    /// Where the monitors are heard of, and what was last heard of them, or
    /// why nothing is.
    monitors: Option<Box<dyn Monitors>>,
    seen: Result<Vec<Monitor>, String>,
    outputs: outputs::Page,
    /// The last change to the monitors, waiting to be kept.
    countdown: Option<Countdown>,
    shown: Shown,
    /// What the last change did.
    notice: Option<Notice>,
    /// Whether the window had the keyboard when last drawn.
    focused: bool,
}

/// The page on show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Keys,
    /// The behaviour page at this place in [`App::sections`].
    Section(usize),
    Outputs,
}

enum Notice {
    Applied(Applied),
    Refused(Refused),
    /// The file could not be read again.
    Unreadable(String),
    /// A change to the monitors was kept.
    Kept,
    /// A change to the monitors was put back.
    Reverted,
    /// A change to the monitors could not be put back.
    NotReverted(Refused),
}

/// A change to the monitors, written and waiting to be kept: the edits that
/// put back what it changed, and when they are made unasked, by egui's
/// clock.
struct Countdown {
    revert: Vec<Edit>,
    until: f64,
}

impl App {
    pub(crate) fn new(
        document: Document,
        judge: Judge,
        capture: Result<Box<dyn Channel>, String>,
        registry: Option<Registry>,
        monitors: Result<Box<dyn Monitors>, String>,
    ) -> Self {
        let seen = monitors.as_ref().map(|_| Vec::new()).map_err(Clone::clone);
        let mut app = Self {
            document,
            judge,
            capture,
            standing: None,
            rows: Vec::new(),
            names: Vec::new(),
            pies: Vec::new(),
            keys: keys::Page::default(),
            sections: perspicax_settings::sections(),
            settings: Vec::new(),
            fields: fields::Page::default(),
            registry,
            monitors: monitors.ok(),
            seen,
            outputs: outputs::Page::default(),
            countdown: None,
            shown: Shown::Keys,
            notice: None,
            focused: true,
        };
        app.refresh();
        app
    }

    /// Read what the file says now.
    fn refresh(&mut self) {
        let text = self.document.text();
        self.standing = self.document.check(&self.judge).err();
        self.outputs.rebase(perspicax_settings::output_rules(text));
        self.rows = perspicax_settings::bindings(text).unwrap_or_default();
        let parsed = perspicax_config::parse(text, EVERYTHING).ok();
        self.settings = parsed.as_ref().map_or_else(Vec::new, |config| {
            self.sections
                .iter()
                .map(|section| perspicax_settings::settings(text, config, section))
                .collect()
        });
        let config = parsed
            .unwrap_or_else(|| Config::profile(perspicax_config::Profile::Classic, EVERYTHING));
        let layouts = config.keyboard.layout.split(',').count();
        self.names = perspicax_config::action_names(
            config.workspaces.grid.len(),
            u8::try_from(layouts).unwrap_or(u8::MAX),
        );
        self.pies = config
            .shell
            .pie
            .map(|pie| pie.menus.into_keys().collect())
            .unwrap_or_default();
    }

    /// Make `edits`, and say what came of it.
    fn apply(&mut self, edits: &[Edit]) {
        let applied = self.document.apply(edits, &self.judge);
        self.fields.settled(applied.as_ref().err());
        self.notice = Some(match applied {
            Ok(applied) => Notice::Applied(applied),
            Err(refused) => Notice::Refused(refused),
        });
        self.refresh();
    }

    /// Write the rules the Outputs page staged, then wait for them to be
    /// kept. What a revert puts back is each value the write changed, as
    /// the page was shown it.
    fn apply_outputs(&mut self, apply: &outputs::Apply, now: f64) {
        let edits = perspicax_settings::output_edits(&apply.base, &apply.to);
        let applied = self.document.apply(&edits, &self.judge);
        if let Ok(done) = &applied {
            self.outputs.applied();
            if *done != Applied::AlreadySo {
                let changed = |rule: &&Rule| {
                    let named = |rules: &[Rule]| {
                        rules.iter().find(|other| other.name == rule.name).cloned()
                    };
                    named(&apply.base) != named(&apply.to)
                };
                let written = perspicax_settings::output_rules(self.document.text());
                let from: Vec<Rule> = written.iter().filter(changed).cloned().collect();
                let back: Vec<Rule> = apply.base.iter().filter(changed).cloned().collect();
                self.countdown = Some(Countdown {
                    revert: perspicax_settings::output_edits(&from, &back),
                    until: now + KEEP_FOR,
                });
            }
        }
        self.notice = Some(match applied {
            Ok(applied) => Notice::Applied(applied),
            Err(refused) => Notice::Refused(refused),
        });
        self.refresh();
    }

    /// The change waiting to be kept: Keep, Revert now, and how long is
    /// left before it is put back unasked.
    fn countdown(&mut self, ui: &mut Ui, now: f64) {
        let Some(countdown) = &self.countdown else {
            return;
        };
        let left = countdown.until - now;
        let mut keep = false;
        let mut back = left <= 0.0;
        if !back {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(format!(
                    "Keep these settings? Reverting in {} s.",
                    left.ceil()
                ));
                ui.horizontal(|ui| {
                    keep = ui.button("Keep").clicked();
                    back = ui.button("Revert now").clicked();
                });
            });
            ui.add_space(6.0);
            // The count is in whole seconds, so a second is soon enough.
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
        if keep {
            self.countdown = None;
            self.notice = Some(Notice::Kept);
        } else if back {
            self.put_back();
        }
    }

    /// Put back what the change waiting to be kept changed, on top of
    /// whatever else the file says now.
    fn put_back(&mut self) {
        let Some(countdown) = self.countdown.take() else {
            return;
        };
        self.notice = Some(match self.document.apply(&countdown.revert, &self.judge) {
            Ok(_) => Notice::Reverted,
            Err(refused) => Notice::NotReverted(refused),
        });
        self.refresh();
    }

    /// The window is closing: a change not kept is put back, since only
    /// Keep keeps it.
    pub(crate) fn closing(&mut self) {
        self.put_back();
    }

    /// Show `page`: what is typed on the page left is written first, and a
    /// capture the Keys page waits for is given up.
    fn show_page(&mut self, page: Shown) {
        if page == self.shown {
            return;
        }
        if let Shown::Section(at) = self.shown
            && let Some(edits) = self.sections.get(at).and_then(|s| self.fields.flush(s))
        {
            self.apply(&edits);
        }
        if self.shown == Shown::Keys
            && let Ok(channel) = &mut self.capture
        {
            self.keys.give_up(channel.as_mut());
        }
        self.fields.forget();
        self.shown = page;
    }

    /// The side panel: every page, by its title.
    fn pages(&mut self, ui: &mut Ui) {
        let mut chosen = None;
        egui::Panel::left("pages").show(ui, |ui| {
            ui.add_space(6.0);
            let mut titles = vec![(Shown::Keys, "Keys")];
            for (at, section) in self.sections.iter().enumerate() {
                titles.push((Shown::Section(at), section.title));
                // The monitors join the devices, after the pointer.
                if section.title == "Pointer" {
                    titles.push((Shown::Outputs, "Outputs"));
                }
            }
            for (page, title) in titles {
                if ui.selectable_label(self.shown == page, title).clicked() {
                    chosen = Some(page);
                }
            }
        });
        if let Some(page) = chosen {
            self.show_page(page);
        }
    }

    pub(crate) fn show(&mut self, ui: &mut Ui) {
        if let Some(seen) = self.monitors.as_mut().and_then(|monitors| monitors.seen()) {
            self.seen = match seen {
                Seen::Monitors(monitors) => Ok(monitors),
                Seen::Withdrawn => Err(monitors::WITHDRAWN.to_owned()),
            };
        }
        let focused = ui.input(|i| i.focused);
        if focused && !self.focused {
            match self.document.reload() {
                Ok(true) => {
                    self.fields.forget();
                    self.refresh();
                }
                Ok(false) => {}
                Err(error) => self.notice = Some(Notice::Unreadable(error.to_string())),
            }
        }
        self.focused = focused;

        let path = self.document.path().to_owned();
        egui::Panel::bottom("file").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Writing {}", path.display()));
                if ui.button("Open in an editor").clicked() {
                    open(&path);
                }
            });
        });
        self.pages(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            let now = ui.input(|i| i.time);
            self.countdown(ui, now);
            let fixed = self.standing(ui);
            let mut edits = self.notice(ui).or(fixed);
            if !self.standing_is_unreadable()
                && let Some(more) = self.page(ui)
            {
                edits = Some(more);
            }
            if let Some(edits) = edits {
                self.apply(&edits);
            }
        });
    }

    /// The page on show, and what it says to write.
    fn page(&mut self, ui: &mut Ui) -> Option<Vec<Edit>> {
        match self.shown {
            Shown::Keys => {
                let view = keys::View {
                    rows: &self.rows,
                    names: &self.names,
                    pies: &self.pies,
                };
                let capture = match &mut self.capture {
                    Ok(channel) => Ok(channel.as_mut()),
                    Err(why) => Err(why.as_str()),
                };
                self.keys.ui(ui, &view, capture)
            }
            Shown::Section(at) => {
                let section = self.sections.get(at)?;
                let settings = self.settings.get(at)?;
                self.fields
                    .ui(ui, section, settings, self.registry.as_ref())
            }
            Shown::Outputs => {
                let view = outputs::View {
                    monitors: self.seen.as_deref().map_err(String::as_str),
                    counting: self.countdown.is_some(),
                };
                if let Some(apply) = self.outputs.ui(ui, &view) {
                    let now = ui.input(|i| i.time);
                    self.apply_outputs(&apply, now);
                }
                None
            }
        }
    }

    /// Whether the file is one nothing here can show: not TOML, or refused
    /// before its bindings could be read.
    fn standing_is_unreadable(&self) -> bool {
        self.standing
            .as_ref()
            .is_some_and(|refused| matches!(refused.reason, Reason::NotToml(_) | Reason::Config(_)))
    }

    /// The file as it stands, when the session refuses it: why, and the
    /// fixes. A fix chosen is what to write.
    fn standing(&self, ui: &mut Ui) -> Option<Vec<Edit>> {
        let refused = self.standing.as_ref()?;
        let (title, rest) = match refused.reason {
            Reason::NotToml(_) => (
                "This file is not TOML, so nothing is written into it.",
                "Mend it in an editor; the session runs on the last file it took.",
            ),
            _ => (
                "perspicax refuses this file as it stands.",
                "The session runs on the last file it took. Nothing can be written until \
                 this is mended: take a fix, or mend it in an editor.",
            ),
        };
        refusal(ui, title, rest, refused)
    }

    /// What the last change did.
    fn notice(&self, ui: &mut Ui) -> Option<Vec<Edit>> {
        match self.notice.as_ref()? {
            Notice::Applied(Applied::Written) => {
                ui.label("Written. The session reads it now.");
                None
            }
            Notice::Applied(Applied::Replayed) => {
                ui.label(
                    "Written, on top of what another program wrote to the file meanwhile. \
                     The session reads it now.",
                );
                None
            }
            Notice::Applied(Applied::AlreadySo) => {
                ui.label("The file already says so; nothing was written.");
                None
            }
            Notice::Refused(refused) => {
                refusal(ui, "Not written.", "The file is as it was.", refused)
            }
            Notice::Unreadable(error) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("The file could not be read again: {error}"),
                );
                None
            }
            Notice::Kept => {
                ui.label("Kept.");
                None
            }
            Notice::Reverted => {
                ui.label("Put back as it was. The session reads it now.");
                None
            }
            Notice::NotReverted(refused) => refusal(
                ui,
                "Not put back.",
                "The file keeps what was applied.",
                refused,
            ),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
        self.show(ui);
    }

    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        self.closing();
    }
}

/// A refusal, in its own words, with a button for each fix; the fix chosen,
/// if one is.
fn refusal(ui: &mut Ui, title: &str, rest: &str, refused: &Refused) -> Option<Vec<Edit>> {
    let mut chosen = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.colored_label(
            ui.visuals().error_fg_color,
            egui::RichText::new(title).strong(),
        );
        ui.label(refused.reason.to_string());
        ui.label(rest);
        for fix in &refused.fixes {
            if ui.button(fix.says.as_str()).clicked() {
                chosen = Some(fix.edits.clone());
            }
        }
    });
    ui.add_space(6.0);
    chosen
}

/// Open the file in whatever opens text here, without waiting for it.
fn open(path: &Path) {
    match std::process::Command::new("xdg-open").arg(path).spawn() {
        // Waited for on a thread of its own, so it leaves no zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(error) => eprintln!("perspicax-settings: could not run xdg-open: {error}"),
    }
}
