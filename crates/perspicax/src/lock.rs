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
//!
//! A bare name is the copy of that program the system installed, wherever on
//! `PATH` it is: only that copy may lock the session (#178), so a copy the
//! person could have written, ahead of it on `PATH`, is passed over rather
//! than run to be refused. A path is run as written: the file the person
//! chose.
//!
//! Inside a perspicax session, the compositor draws the lock screen beneath
//! swaylock, which is dressed clear but for its ring. Outside one, nothing of
//! perspicax's would be beneath it, and another compositor's desktop would
//! show through a clear lock; so there swaylock paints its screen itself. It
//! is inside a session when the session's `PERSPICAX_CONFIG` is set at all,
//! even empty, as it is for a session that read no config.

use std::{
    ffi::OsStr,
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Error, anyhow};
use perspicax_config::{Profile, Shell, ShellBuilt};
use perspicax_policy::Person;

/// Set in the environment of every program a session on a seat starts: the
/// config file the session read, which `perspicax lock` reads too.
pub const CONFIG: &str = perspicax_config::SESSION_CONFIG;

/// Whether this runs inside a perspicax session, which draws the lock screen
/// beneath the lock program: whether `PERSPICAX_CONFIG` is set, even empty.
///
/// `var` reads the environment, as [`config_path`]'s does.
pub fn in_session(var: impl Fn(&str) -> Option<String>) -> bool {
    var(CONFIG).is_some()
}

/// The config to read: the session's, or else the one a session would read
/// by default.
///
/// `var` reads the environment, as [`crate::attach::socket_path`]'s does.
pub fn config_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    perspicax_config::session_path(var)
}

/// What locking runs: the `[shell]` at `config`, or the classic profile's
/// when there is none or it cannot be read, with `more` after `lock`'s own
/// words. `beneath` is whether the session draws the lock screen beneath it.
#[must_use]
pub fn command(config: Option<&Path>, more: &[String], beneath: bool) -> Vec<String> {
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
    shell.lock_command(more, beneath)
}

/// Become what locking runs, with `more` after `lock`'s own words. Returns
/// only if it could not be started.
#[must_use]
pub fn lock(more: &[String]) -> Error {
    let var = |name: &str| std::env::var(name).ok();
    let command = command(config_path(var).as_deref(), more, in_session(var));
    let Some((program, arguments)) = command.split_first() else {
        return anyhow!("[shell] lock is empty");
    };
    let path = std::env::var_os("PATH");
    let file = match found(
        program,
        path.as_deref(),
        perspicax_config::person().as_ref(),
    ) {
        Ok(file) => file,
        Err(error) => return error,
    };
    // Still called what `lock` calls it, for what it says of itself.
    let error = Command::new(&file).arg0(program).args(arguments).exec();
    anyhow!(error).context(format!("could not run {}", file.display()))
}

/// The file `program` runs from, as [`perspicax_config::which_installed`]
/// finds it on `path` for `person`, or why there is none.
fn found(program: &str, path: Option<&OsStr>, person: Option<&Person>) -> Result<PathBuf, Error> {
    if let Some(file) = perspicax_config::which_installed(program, path, person) {
        return Ok(file);
    }
    if program.contains('/') {
        return Err(anyhow!("{program} is not a program that may be run"));
    }
    let passed_over = perspicax_config::which(program, path).map_or_else(String::new, |copy| {
        format!(
            ", only {}, which a program of yours could have written",
            copy.display()
        )
    });
    Err(anyhow!(
        "no {program} the system installed is on PATH{passed_over}; only that copy may lock \
         the screen. Install it, or name the one to run by its full path in [shell] lock"
    ))
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
    fn a_session_is_known_by_its_config_being_said_even_empty() {
        let said = |value: Option<&str>| {
            let env: HashMap<&str, &str> = value.map(|value| (CONFIG, value)).into_iter().collect();
            in_session(|name| env.get(name).map(|&value| value.to_owned()))
        };
        assert!(said(Some("/run/session/config.toml")));
        assert!(said(Some("")), "a session that read no config");
        assert!(!said(None), "outside any");
    }

    #[test]
    fn lock_runs_what_the_menus_lock_runs_with_more_after() {
        let path = config(
            "written",
            "[theme]\nname = \"breeze-light\"\n[shell]\nlock = [\"swaylock\", \"-e\"]\n",
        );
        let command = command(Some(&path), &words(&["-f"]), true);
        let shell = perspicax_config::load_shell(&path, ShellBuilt::FULL).unwrap();
        assert_eq!(command, shell.lock_command(&words(&["-f"]), true));
        assert_eq!(
            command[3..5],
            ["--color", "00000000"],
            "clear, in the session"
        );
        assert_eq!(command[command.len() - 2..], ["-e", "-f"]);

        let outside = super::command(Some(&path), &words(&["-f"]), false);
        assert_eq!(
            outside[3..5],
            ["--color", "cbd8dfff"],
            "outside one, the theme's lock colour"
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// The system's `sh` stands in for a lock program, behind a copy planted
    /// ahead of it on PATH in a folder anybody may write.
    #[test]
    fn a_bare_name_runs_the_copy_the_system_installed() {
        use std::os::unix::fs::PermissionsExt as _;
        let folder =
            std::env::temp_dir().join(format!("perspicax-lock-{}-planted", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o777)).unwrap();
        let planted = folder.join("sh");
        fs::write(&planted, "").unwrap();
        fs::set_permissions(&planted, fs::Permissions::from_mode(0o755)).unwrap();
        let person = Person {
            uid: 4242,
            groups: vec![4242],
        };
        let path = std::env::join_paths([folder.as_path(), Path::new("/bin")]).unwrap();
        assert_eq!(
            found("sh", Some(&path), Some(&person)).unwrap(),
            fs::canonicalize("/bin/sh").unwrap()
        );

        let alone = std::env::join_paths([folder.as_path()]).unwrap();
        let refused = found("sh", Some(&alone), Some(&person))
            .unwrap_err()
            .to_string();
        assert!(
            refused.starts_with("no sh the system installed is on PATH"),
            "{refused}"
        );
        assert!(
            refused.contains(&planted.display().to_string()),
            "{refused}"
        );
        assert!(refused.contains("[shell] lock"), "{refused}");

        assert_eq!(
            found(planted.to_str().unwrap(), None, Some(&person)).unwrap(),
            planted,
            "a path is run as written"
        );
        let missing = folder.join("missing");
        assert!(found(missing.to_str().unwrap(), None, Some(&person)).is_err());
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_config_that_cannot_be_read_still_locks_in_the_default_theme() {
        let default = command(None, &[], true);
        assert_eq!(default[..3], ["swaylock", "-C", "/dev/null"]);
        let broken = config("broken", "[shell]\nlock = 42\n");
        assert_eq!(command(Some(&broken), &[], true), default);
        let missing = broken.with_file_name("missing.toml");
        assert_eq!(
            command(Some(&missing), &[], true),
            default,
            "no file is classic"
        );
        fs::remove_dir_all(broken.parent().unwrap()).unwrap();
    }
}
