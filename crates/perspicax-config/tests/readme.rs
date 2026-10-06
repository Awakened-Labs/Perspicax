//! Every config the README shows is one perspicax takes, as the compositor
//! reads it and as the shell does, each built with everything.
//!
//! A sample is copied into somebody's `config.toml` more often than the rest
//! of the README is read. One put `autostart` below the `[[output]]` tables,
//! where TOML makes it a key of the last output, and the file it was copied
//! into was refused whole.

use perspicax_config::{Built, ShellBuilt};

const README: &str = include_str!("../../../README.md");

/// The README's TOML blocks, each with the line its first key is on.
fn samples() -> Vec<(usize, String)> {
    let mut samples = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (at, line) in README.lines().enumerate() {
        match (&mut open, line.trim_end()) {
            (None, "```toml") => open = Some((at + 2, String::new())),
            (Some(_), "```") => samples.extend(open.take()),
            (Some((_, text)), line) => {
                text.push_str(line);
                text.push('\n');
            }
            (None, _) => {}
        }
    }
    samples
}

#[test]
fn every_config_the_readme_shows_is_taken() {
    let samples = samples();
    assert!(
        samples.len() >= 4,
        "the README's samples were not found: {} TOML blocks",
        samples.len()
    );
    let everything = Built {
        seat: true,
        xwayland: true,
        capture: true,
    };
    for (line, text) in samples {
        if let Err(error) = perspicax_config::parse(&text, everything) {
            panic!("README.md:{line}: the compositor refuses it: {error}");
        }
        if let Err(error) = perspicax_config::shell(&text, ShellBuilt::FULL) {
            panic!("README.md:{line}: the shell refuses it: {error}");
        }
    }
}
