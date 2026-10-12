//! `perspicax-shell`: the desktop, as perspicax starts it.
//!
//! perspicax runs this with `--config` naming the file it read itself, and
//! reads the exit status: 78 says the config cannot be used, and perspicax
//! waits for the file to change before trying again; 69 says the connection
//! to perspicax broke, and perspicax starts it again as after a crash. Run
//! by hand, it draws on whichever compositor `WAYLAND_DISPLAY` names.

use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use perspicax_shell::Options;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The config file. By default perspicax's own,
    /// `$XDG_CONFIG_HOME/perspicax/config.toml`.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    // With no `RUST_LOG`, its warnings, as a seat says its own: they go to
    // the session's stderr, and one of them may be the only word on why the
    // shell was cut off (see `wl::watch`).
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .init();

    let cli = Cli::parse();
    match perspicax_shell::run(Options {
        connection: None,
        config: cli.config,
        session_bus: None,
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error}");
            eprintln!("perspicax-shell: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}
