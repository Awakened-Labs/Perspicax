//! `perspicax-settings`: perspicax's settings, written into its config file
//! in place, so that this and a text editor can be used turn and turn about.
//!
//! It writes the session's own file (`$PERSPICAX_CONFIG`), or else the one a
//! session would read by default, or the one `--config` names. A chord is
//! bound by pressing it: the compositor reports it from where bindings are
//! matched, over `perspicax-settings-v1`, which `[protocols] settings`
//! offers to this program by name. So it must be run as
//! `perspicax-settings`. Nothing is written that the installed perspicax,
//! asked with `perspicax config check`, would refuse.
//!
//! No agent may drive it: the file it writes is what holds an agent back.
//! perspicax refuses to hand it to one, whatever was granted.

mod capture;
mod fonts;
mod keys;
#[cfg(test)]
mod tests;
mod window;

use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use eframe::egui;
use perspicax_settings::{Document, Installed, Judge as _};

use crate::window::{App, Judge};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The config file to write. By default the session's own
    /// (`$PERSPICAX_CONFIG`), or else `$XDG_CONFIG_HOME/perspicax/config.toml`.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(path) = cli
        .config
        .or_else(|| perspicax_config::session_path(|name| std::env::var(name).ok()))
    else {
        eprintln!(
            "perspicax-settings: no config file to write: name one with --config, or set \
             XDG_CONFIG_HOME or HOME"
        );
        return ExitCode::FAILURE;
    };
    let document = match Document::open(&path) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("perspicax-settings: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let judge: Judge = match Installed::find() {
        Some(installed) => Box::new(move |candidate| installed.judge(candidate)),
        None => Box::new(|_| {
            Err(
                "There is no perspicax beside this program or on PATH to check the file \
                 with, so nothing is written."
                    .to_owned(),
            )
        }),
    };
    let font = perspicax_config::parse(document.text(), window::EVERYTHING)
        .map(|config| config.theme.font)
        .unwrap_or_default();
    let fonts = fonts::definitions(&font);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(perspicax_config::SETTINGS_APP)
            .with_title("Settings")
            .with_inner_size([760.0, 600.0]),
        renderer: eframe::Renderer::Glow,
        ..eframe::NativeOptions::default()
    };
    let ran = eframe::run_native(
        perspicax_config::SETTINGS_APP,
        options,
        Box::new(move |made| {
            let ctx = made.egui_ctx.clone();
            ctx.set_fonts(fonts);
            ctx.all_styles_mut(|style| fonts::sized(style, font.size));
            let woken = ctx.clone();
            let capture = capture::Wayland::connect(move || woken.request_repaint())
                .map(|channel| Box::new(channel) as Box<dyn capture::Channel>);
            Ok(Box::new(App::new(document, judge, capture)))
        }),
    );
    match ran {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("perspicax-settings: {error}");
            ExitCode::FAILURE
        }
    }
}
