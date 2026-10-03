//! `perspicax-shell`: the desktop, as perspicax starts it.
//!
//! perspicax runs this with `--config` naming the file it read itself, and
//! reads the exit status: 78 says the config cannot be used, and perspicax
//! waits for the file to change before trying again. Run by hand, it draws
//! on whichever compositor `WAYLAND_DISPLAY` names.

use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use perspicax_shell::Options;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The config file. By default perspicax's own,
    /// `$XDG_CONFIG_HOME/perspicax/config.toml`.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match perspicax_shell::run(Options {
        connection: None,
        config: cli.config,
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error}");
            eprintln!("perspicax-shell: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}
