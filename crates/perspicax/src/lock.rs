//! `perspicax lock`: lock the screen as the menus' Lock does.
//!
//! The menus' Lock runs `[shell] lock`, swaylock by default, dressed in the
//! theme. A key, or swayidle before the computer sleeps, starts a program
//! rather than open a menu, so this is that program:
//! `["swayidle", "-w", "before-sleep", "perspicax lock -f"]`. It reads the
//! session's config, builds the same command with its own arguments after
//! `lock`'s, and becomes it, so whatever started it waits on the lock program
//! itself.
//!
//! It never refuses to lock over the config: one it cannot read locks all the
//! same, in the default theme, rather than leave the screen open while its
//! owner walks away.

use std::{
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Error, anyhow};
use perspicax_config::{Profile, Shell, ShellBuilt};

/// Set in the environment of every program a session on a seat starts: the
/// config file the session read, which `perspicax lock` reads too.
pub const CONFIG: &str = "PERSPICAX_CONFIG";

/// The config to read: the session's, or else the one a session would read
/// by default.
///
/// `var` reads the environment, as [`crate::attach::socket_path`]'s does.
pub fn config_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    var(CONFIG)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(perspicax_config::default_path)
}

/// What locking runs: the `[shell]` at `config`, or the classic profile's
/// when there is none or it cannot be read, with `more` after `lock`'s own
/// words.
#[must_use]
pub fn command(config: Option<&Path>, more: &[String]) -> Vec<String> {
    let classic = || Shell::profile(Profile::Classic, ShellBuilt::FULL);
    let shell = match config {
        // What the shell was built with is not this command's to check.
        Some(path) => {
            perspicax_config::load_shell(path, ShellBuilt::FULL).unwrap_or_else(|error| {
                eprintln!(
                    "perspicax lock: {}: {error}; locking in the default theme",
                    path.display()
                );
                classic()
            })
        }
        None => classic(),
    };
    shell.lock_command(more)
}

/// Become what locking runs, with `more` after `lock`'s own words. Returns
/// only if it could not be started.
#[must_use]
pub fn lock(more: &[String]) -> Error {
    let command = command(
        config_path(|name| std::env::var(name).ok()).as_deref(),
        more,
    );
    let Some((program, arguments)) = command.split_first() else {
        return anyhow!("[shell] lock is empty");
    };
    let error = Command::new(program).args(arguments).exec();
    anyhow!(error).context(format!("could not run {program}"))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs};

    use super::*;

    fn words(written: &[&str]) -> Vec<String> {
        written.iter().map(|&word| word.to_owned()).collect()
    }

    /// A config file holding `text`, in a folder of the test's own.
    fn config(name: &str, text: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("perspicax-lock-{}-{name}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn the_sessions_config_is_read_or_else_the_default() {
        let env = HashMap::from([(CONFIG, "/run/session/config.toml")]);
        assert_eq!(
            config_path(|name| env.get(name).map(|&value| value.to_owned())),
            Some(PathBuf::from("/run/session/config.toml"))
        );
        let empty = HashMap::from([(CONFIG, "")]);
        assert_eq!(
            config_path(|name| empty.get(name).map(|&value| value.to_owned())),
            perspicax_config::default_path(),
            "empty is none"
        );
    }

    #[test]
    fn lock_runs_what_the_menus_lock_runs_with_more_after() {
        let path = config(
            "written",
            "[theme]\nname = \"breeze-light\"\n[shell]\nlock = [\"swaylock\", \"-e\"]\n",
        );
        let command = command(Some(&path), &words(&["-f"]));
        let shell = perspicax_config::load_shell(&path, ShellBuilt::FULL).unwrap();
        assert_eq!(command, shell.lock_command(&words(&["-f"])));
        assert_eq!(command[3..5], ["--color", "cbd8dfff"], "the theme's");
        assert_eq!(command[command.len() - 2..], ["-e", "-f"]);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_config_that_cannot_be_read_still_locks_in_the_default_theme() {
        let default = command(None, &[]);
        assert_eq!(default[..3], ["swaylock", "-C", "/dev/null"]);
        let broken = config("broken", "[shell]\nlock = 42\n");
        assert_eq!(command(Some(&broken), &[]), default);
        let missing = broken.with_file_name("missing.toml");
        assert_eq!(command(Some(&missing), &[]), default, "no file is classic");
        fs::remove_dir_all(broken.parent().unwrap()).unwrap();
    }
}
