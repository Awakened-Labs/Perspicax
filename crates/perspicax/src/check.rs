//! `perspicax config check`: whether a session would take a config file, in
//! the words of the two programs that read it.
//!
//! A session reads the file twice, with two programs built with features of
//! their own: perspicax reads all of it, and perspicax-shell reads `[shell]`.
//! Either can refuse it, and a refused save is otherwise only a line in the
//! log while the session carries on with the config it had. So both are
//! asked, each as it was built: perspicax here, and the shell by running it
//! with `--check`, found where the session would start it from. Asking the
//! installed shell, rather than reading `[shell]` here, is what makes a tray
//! written for a shell built without one a refusal and not a surprise.

use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};
use perspicax_config::Built;

use crate::lock;

/// The shell's binary, as the session finds it.
const SHELL: &str = "perspicax-shell";

/// `EX_CONFIG`, from sysexits.h: a config that cannot be used. The shell's
/// own status for one, which the session reads the same way.
const EX_CONFIG: i32 = 78;

/// `EX_NOINPUT`, from sysexits.h: a file named to check that is not there.
const EX_NOINPUT: i32 = 66;

/// What a program said of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// It takes the file.
    Takes(String),
    /// It refuses it, and this is why.
    Refuses(String),
    /// It could not be asked, and this is what was done instead.
    Unasked(String),
}

/// Check the file at `named`, or the session's own, as this build of
/// perspicax (`built`) and the shell installed with it read it; say what
/// each said; and return the exit status: 0 taken, 78 refused (`EX_CONFIG`,
/// as the shell's own), 66 for a named file that is not there, 1 for a
/// check that could not be made.
#[must_use]
pub fn run(named: Option<&Path>, built: Built) -> i32 {
    let var = |name: &str| std::env::var(name).ok();
    let Some(path) = named.map(Path::to_owned).or_else(|| lock::config_path(var)) else {
        eprintln!(
            "perspicax: no config to check: none of {}, XDG_CONFIG_HOME or HOME is set",
            lock::CONFIG
        );
        return 1;
    };
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) if named.is_some() => {
            eprintln!("{}: there is no such file", path.display());
            return EX_NOINPUT;
        }
        Ok(false) => {
            println!(
                "{}: there is no config here, so a session runs the classic profile",
                path.display()
            );
            return 0;
        }
        Err(error) => {
            eprintln!("perspicax: could not check {}: {error}", path.display());
            return 1;
        }
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_owned));
    let on_path = std::env::var_os("PATH");
    let shell = perspicax_config::find_program(SHELL, beside.as_deref(), on_path.as_deref());
    match check(&path, built, shell.as_deref()) {
        Ok(said) => {
            for said in &said {
                match said {
                    Said::Takes(line) => println!("{line}"),
                    Said::Refuses(line) | Said::Unasked(line) => eprintln!("{line}"),
                }
            }
            if said.iter().any(|said| matches!(said, Said::Refuses(_))) {
                EX_CONFIG
            } else {
                0
            }
        }
        Err(error) => {
            eprintln!("perspicax: could not check {}: {error:#}", path.display());
            1
        }
    }
}

/// What perspicax, built with `built`, and the shell at `shell` say of the
/// file at `path`: perspicax's word first, then the shell's. With no shell,
/// the shell's word is that it was not asked, and `[shell]` stands as
/// perspicax read it, as if the shell were built with every component.
///
/// # Errors
///
/// A shell that could not be run, or that ended without an answer.
pub fn check(path: &Path, built: Built, shell: Option<&Path>) -> Result<Vec<Said>> {
    let shown = path.display();
    let ours = match perspicax_config::load(path, built) {
        Ok(_) => Said::Takes(format!("{shown}: perspicax takes it")),
        Err(error) => Said::Refuses(format!("{shown}: perspicax refuses it: {error}")),
    };
    let theirs = match shell {
        Some(shell) => ask(shell, path)?,
        None => Said::Unasked(format!(
            "{shown}: no {SHELL} beside perspicax or on PATH to ask, so `[shell]` was checked as \
             if it were built with every component"
        )),
    };
    Ok(vec![ours, theirs])
}

/// Run the shell at `shell` with `--check` on `path`, and read its answer
/// from its exit status, as the session does when it starts one.
fn ask(shell: &Path, path: &Path) -> Result<Said> {
    let output = Command::new(shell)
        .arg("--check")
        .arg("--config")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("could not run {}", shell.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Its own words, without the name it signs them with.
    let words = stderr
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{SHELL}: ")))
        .map(|words| words.strip_prefix("config: ").unwrap_or(words))
        .unwrap_or_else(|| stderr.trim());
    let (shown, which) = (path.display(), shell.display());
    Ok(match output.status.code() {
        Some(0) => Said::Takes(format!("{shown}: {SHELL} takes it ({which})")),
        Some(EX_CONFIG) => Said::Refuses(format!("{shown}: {SHELL} refuses it ({which}): {words}")),
        // clap's word for an argument it does not know: a shell from before
        // `--check`.
        Some(2) => Said::Unasked(format!(
            "{shown}: {which} is older than this perspicax and cannot check a config, so \
             `[shell]` was checked as if it were built with every component"
        )),
        _ => bail!("{which} ended with {}: {words}", output.status),
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

    use super::*;

    const EVERYTHING: Built = Built {
        seat: true,
        xwayland: true,
        capture: true,
        svg: true,
    };

    /// A fresh folder for one test, emptied when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("perspicax-check-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("a scratch folder");
            Self(dir)
        }

        /// A config file in it holding `text`.
        fn config(&self, text: &str) -> PathBuf {
            let path = self.0.join("config.toml");
            fs::write(&path, text).expect("a config");
            path
        }

        /// A stand-in shell that says `said` on stderr and exits with
        /// `status`, whatever it is asked.
        fn shell(&self, said: &str, status: i32) -> PathBuf {
            let path = self.0.join(SHELL);
            fs::write(
                &path,
                format!("#!/bin/sh\necho '{said}' >&2\nexit {status}\n"),
            )
            .expect("a shell");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("its mode");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn a_shell_that_refuses_makes_the_check_refuse_in_its_words() {
        let scratch = Scratch::new("refuses");
        let config = scratch.config("");
        let shell = scratch.shell("perspicax-shell: config: `shell.tray`: no tray here", 78);
        let said = check(&config, EVERYTHING, Some(&shell)).unwrap();
        assert!(matches!(&said[0], Said::Takes(_)), "{said:?}");
        let Said::Refuses(line) = &said[1] else {
            panic!("{said:?}");
        };
        assert!(line.ends_with("): `shell.tray`: no tray here"), "{line}");
    }

    #[test]
    fn a_shell_that_takes_it_says_so() {
        let scratch = Scratch::new("takes");
        let config = scratch.config("");
        let shell = scratch.shell("", 0);
        let said = check(&config, EVERYTHING, Some(&shell)).unwrap();
        assert!(matches!(&said[1], Said::Takes(line) if line.contains("perspicax-shell takes it")));
    }

    #[test]
    fn a_shell_too_old_to_check_is_said_to_be() {
        let scratch = Scratch::new("old");
        let config = scratch.config("");
        let shell = scratch.shell("error: unexpected argument '--check' found", 2);
        let said = check(&config, EVERYTHING, Some(&shell)).unwrap();
        let Said::Unasked(line) = &said[1] else {
            panic!("{said:?}");
        };
        assert!(line.contains("is older than this perspicax"), "{line}");
    }

    #[test]
    fn a_shell_that_ends_another_way_is_no_answer() {
        let scratch = Scratch::new("crash");
        let config = scratch.config("");
        let shell = scratch.shell("thread 'main' panicked", 101);
        let error = check(&config, EVERYTHING, Some(&shell)).unwrap_err();
        assert!(error.to_string().contains("panicked"), "{error}");
    }

    #[test]
    fn no_shell_found_is_said_and_the_check_still_answers() {
        let scratch = Scratch::new("none");
        let config = scratch.config("[keys]\n\"Ctrl+Alt+F2\" = \"close\"\n");
        let said = check(&config, EVERYTHING, None).unwrap();
        let Said::Refuses(line) = &said[0] else {
            panic!("{said:?}");
        };
        assert!(line.contains("`keys.Ctrl+Alt+F2`"), "{line}");
        assert!(
            matches!(&said[1], Said::Unasked(line) if line.contains("no perspicax-shell")),
            "{said:?}"
        );
    }
}
