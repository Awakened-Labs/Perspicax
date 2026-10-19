//! The window: the file, what the session makes of it, and the pages.
//!
//! Every change is one [`Document::apply`], so what is written is only ever
//! a file the installed perspicax takes. The file is read again whenever
//! the window gets the keyboard back, so an edit made in another editor
//! meanwhile is what the page shows.

use std::path::Path;

use eframe::egui::{self, Ui};
use perspicax_config::{Built, Config};
use perspicax_settings::{
    Applied, Binding, Document, Edit, Reason, Refused, Registry, Section, Setting,
};

use crate::{capture::Channel, fields, keys};

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
}

enum Notice {
    Applied(Applied),
    Refused(Refused),
    /// The file could not be read again.
    Unreadable(String),
}

impl App {
    pub(crate) fn new(
        document: Document,
        judge: Judge,
        capture: Result<Box<dyn Channel>, String>,
        registry: Option<Registry>,
    ) -> Self {
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
            let titles = std::iter::once((Shown::Keys, "Keys")).chain(
                self.sections
                    .iter()
                    .enumerate()
                    .map(|(at, section)| (Shown::Section(at), section.title)),
            );
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
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
        self.show(ui);
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
