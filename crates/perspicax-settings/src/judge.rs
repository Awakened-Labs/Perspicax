//! Who decides that a candidate may be written.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// `EX_CONFIG`, from sysexits.h: `perspicax config check`'s status for a
/// config one of the session's programs refuses.
const EX_CONFIG: i32 = 78;

/// Whether a candidate file may replace the config.
pub trait Judge {
    /// `Ok` to write the file at `candidate`; otherwise why not, in words a
    /// person is shown.
    ///
    /// # Errors
    ///
    /// The refusal, as the judge words it.
    fn judge(&self, candidate: &Path) -> Result<(), String>;
}

/// A closure is a judge: a test's, or one that takes everything.
impl<F> Judge for F
where
    F: Fn(&Path) -> Result<(), String>,
{
    fn judge(&self, candidate: &Path) -> Result<(), String> {
        self(candidate)
    }
}

/// The installed perspicax, asked with `perspicax config check`: the two
/// programs a session starts, each as it was built.
#[derive(Debug, Clone)]
pub struct Installed {
    perspicax: PathBuf,
}

impl Installed {
    /// The perspicax at `perspicax`.
    #[must_use]
    pub fn at(perspicax: impl Into<PathBuf>) -> Self {
        Self {
            perspicax: perspicax.into(),
        }
    }

    /// The perspicax beside this program, or else on `PATH`, as the session
    /// finds its shell.
    #[must_use]
    pub fn find() -> Option<Self> {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_owned));
        let on_path = std::env::var_os("PATH");
        perspicax_config::find_program("perspicax", beside.as_deref(), on_path.as_deref())
            .map(Self::at)
    }
}

impl Judge for Installed {
    fn judge(&self, candidate: &Path) -> Result<(), String> {
        let which = self.perspicax.display();
        let output = Command::new(&self.perspicax)
            .args(["config", "check"])
            .arg(candidate)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("could not run {which}: {error}"))?;
        // Each line names the file it checked first, and that file is the
        // candidate, whose name is nobody's business: the words without it.
        let named = format!("{}: ", candidate.display());
        let stderr = String::from_utf8_lossy(&output.stderr);
        let said: Vec<&str> = stderr
            .lines()
            .map(|line| line.strip_prefix(&named).unwrap_or(line))
            .collect();
        let said = said.join("\n");
        match output.status.code() {
            Some(0) => Ok(()),
            Some(EX_CONFIG) => Err(said),
            _ => Err(format!(
                "{which} could not check it ({}): {said}",
                output.status
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        sync::{Mutex, MutexGuard, PoisonError},
    };

    use super::*;

    /// Held to write a stand-in and to run one. Another test's fork, made
    /// while a stand-in is still open for writing, takes that open file with
    /// it, and running the stand-in then fails as busy (`ETXTBSY`).
    static SPAWNING: Mutex<()> = Mutex::new(());

    fn spawning() -> MutexGuard<'static, ()> {
        SPAWNING.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `perspicax`'s judgement of `candidate`.
    fn ask(perspicax: &Installed, candidate: &Path) -> Result<(), String> {
        let _held = spawning();
        perspicax.judge(candidate)
    }

    /// A fresh folder for one test, emptied when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "perspicax-settings-judge-{name}-{}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).expect("a scratch folder");
            Self(dir)
        }

        /// A stand-in perspicax that says `said` on stderr, naming the file
        /// it was asked about as the real one does, and exits with `status`.
        fn perspicax(&self, said: &str, status: i32) -> Installed {
            let path = self.0.join("perspicax");
            let _held = spawning();
            fs::write(
                &path,
                format!("#!/bin/sh\nprintf '%s: %s\\n' \"$3\" '{said}' >&2\nexit {status}\n"),
            )
            .expect("a perspicax");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("its mode");
            Installed::at(path)
        }

        fn candidate(&self) -> PathBuf {
            let path = self.0.join(".config.toml.perspicax-settings-1");
            fs::write(&path, "").expect("a candidate");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_check_that_takes_it_judges_it_taken() {
        let scratch = Scratch::new("takes");
        let perspicax = scratch.perspicax("perspicax takes it", 0);
        assert_eq!(ask(&perspicax, &scratch.candidate()), Ok(()));
    }

    #[test]
    fn a_check_that_refuses_is_a_refusal_in_its_words_without_the_candidates_name() {
        let scratch = Scratch::new("refuses");
        let perspicax = scratch.perspicax("perspicax-shell refuses it: `shell.tray`: no tray", 78);
        assert_eq!(
            ask(&perspicax, &scratch.candidate()),
            Err("perspicax-shell refuses it: `shell.tray`: no tray".to_owned())
        );
    }

    #[test]
    fn a_perspicax_that_cannot_check_is_a_refusal_naming_it() {
        let scratch = Scratch::new("cannot");
        let perspicax = scratch.perspicax("unrecognized subcommand 'config'", 2);
        let refusal = ask(&perspicax, &scratch.candidate()).unwrap_err();
        assert!(refusal.contains("could not check it"), "{refusal}");
        assert!(refusal.contains("unrecognized subcommand"), "{refusal}");
    }

    #[test]
    fn a_perspicax_that_is_not_there_is_a_refusal_naming_it() {
        let scratch = Scratch::new("missing");
        let missing = Installed::at(scratch.0.join("nowhere"));
        let refusal = ask(&missing, &scratch.candidate()).unwrap_err();
        assert!(refusal.starts_with("could not run"), "{refusal}");
    }
}
