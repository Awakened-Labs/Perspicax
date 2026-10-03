//! Starting what a menu item names.
//!
//! A program is started as its words, never through `sh -c`: a desktop
//! entry's `Exec` was split by the spec's own quoting, and a shell would
//! split it again by its own. Each runs in a process group of its own, so
//! it outlives the shell: perspicax restarting a shell that crashed must not
//! take the person's applications with it. It starts in its entry's folder,
//! or the home folder.
//!
//! An entry that runs in a terminal is started in `[shell] terminal`, or in
//! the first terminal installed of those this knows how to hand a command
//! to.
//!
//! The shell is each program's parent, so it collects each one's exit
//! status when it ends, or the ended programs would linger as zombies for
//! as long as the shell runs.

use std::{
    io,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use crate::model::{
    apps::Run,
    fs::{Fs, which},
};

/// The terminals found when none is configured, in the order looked for,
/// each with the arguments that hand it a command to run.
const TERMINALS: [(&str, &[&str]); 8] = [
    ("foot", &[]),
    ("alacritty", &["-e"]),
    ("kitty", &[]),
    ("wezterm", &["start", "--"]),
    ("konsole", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("xfce4-terminal", &["-x"]),
    ("xterm", &["-e"]),
];

/// The first terminal installed of those this knows, as the words before
/// the command it is to run.
pub(crate) fn find_terminal(fs: &impl Fs, path: &[PathBuf]) -> Option<Vec<String>> {
    TERMINALS.iter().find_map(|&(program, arguments)| {
        which(fs, program, path)?;
        Some(
            std::iter::once(program)
                .chain(arguments.iter().copied())
                .map(str::to_owned)
                .collect(),
        )
    })
}

/// The words to run for `run`: its own, or after `terminal`'s for one that
/// runs in a terminal. `None` for a program that wants a terminal when
/// there is none.
pub(crate) fn command_line(run: &Run, terminal: Option<&[String]>) -> Option<Vec<String>> {
    if !run.terminal {
        return Some(run.argv.clone());
    }
    Some(terminal?.iter().chain(&run.argv).cloned().collect())
}

/// What the shell has started, until each has ended.
pub(crate) struct Launcher {
    /// The terminal's words, configured or found.
    terminal: Option<Vec<String>>,
    home: Option<PathBuf>,
    children: Vec<Child>,
}

impl Launcher {
    pub(crate) fn new(terminal: Option<Vec<String>>, home: Option<PathBuf>) -> Self {
        Self {
            terminal,
            home,
            children: Vec::new(),
        }
    }

    /// Use `terminal` from now on.
    pub(crate) fn set_terminal(&mut self, terminal: Option<Vec<String>>) {
        self.terminal = terminal;
    }

    /// Start `run`.
    pub(crate) fn run(&mut self, run: &Run) -> io::Result<u32> {
        let words = command_line(run, self.terminal.as_deref()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "it runs in a terminal, and no terminal is installed; set [shell] terminal",
            )
        })?;
        let (program, arguments) = words
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "an empty command"))?;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .process_group(0);
        let folder = run
            .dir
            .as_deref()
            .filter(|dir| dir.is_dir())
            .or(self.home.as_deref().filter(|home| home.is_dir()));
        if let Some(folder) = folder {
            command.current_dir(folder);
        }
        let child = command.spawn()?;
        let pid = child.id();
        tracing::info!(pid, command = words.join(" "), "started");
        self.children.push(child);
        Ok(pid)
    }

    /// Collect the programs that have ended. Whether any are still running.
    pub(crate) fn reap(&mut self) -> bool {
        self.children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        !self.children.is_empty()
    }
}

/// The folder process `pid` runs in.
#[cfg(test)]
fn folder_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// The process group `pid` is in.
#[cfg(test)]
fn group_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // After the command's name, in parentheses, which may hold spaces: the
    // state, the parent, then the group.
    stat.rsplit_once(") ")?.1.split(' ').nth(2)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        time::{Duration, Instant},
    };

    use super::*;
    use crate::model::fs::fake::Files;

    fn run(argv: &[&str], terminal: bool) -> Run {
        Run {
            argv: argv.iter().map(|word| (*word).to_owned()).collect(),
            terminal,
            dir: None,
        }
    }

    #[test]
    fn a_terminal_program_runs_in_the_terminal() {
        let terminal = ["alacritty".to_owned(), "-e".to_owned()];
        assert_eq!(
            command_line(&run(&["htop", "-d", "10"], true), Some(&terminal)),
            Some(
                ["alacritty", "-e", "htop", "-d", "10"]
                    .map(str::to_owned)
                    .to_vec()
            )
        );
        assert_eq!(
            command_line(&run(&["firefox"], false), Some(&terminal)),
            Some(vec!["firefox".to_owned()]),
            "and any other on its own"
        );
        assert_eq!(command_line(&run(&["htop"], true), None), None);
    }

    #[test]
    fn the_first_terminal_installed_is_the_one() {
        let path = [PathBuf::from("/usr/bin")];
        let files = Files::default()
            .program("/usr/bin/xterm")
            .program("/usr/bin/konsole");
        assert_eq!(
            find_terminal(&files, &path),
            Some(vec!["konsole".to_owned(), "-e".to_owned()])
        );
        assert_eq!(find_terminal(&Files::default(), &path), None);
    }

    #[test]
    fn a_program_runs_in_a_group_of_its_own_in_its_folder_and_is_collected() {
        let mut launcher = Launcher::new(None, Some(PathBuf::from("/")));
        let mut sleeping = run(&["sleep", "30"], false);
        sleeping.dir = Some(std::env::temp_dir());
        let pid = launcher.run(&sleeping).expect("started");
        assert_eq!(group_of(pid), Some(pid), "the leader of its own group");
        assert_eq!(folder_of(pid), std::env::temp_dir().canonicalize().ok());
        assert!(launcher.reap(), "still running");

        let quick = launcher.run(&run(&["true"], false)).expect("started");
        let deadline = Instant::now() + Duration::from_secs(5);
        while launcher.children.iter().any(|child| child.id() == quick) {
            assert!(Instant::now() < deadline, "collected once it ended");
            launcher.reap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !Path::new(&format!("/proc/{quick}")).exists(),
            "and gone, not left a zombie"
        );

        for child in &mut launcher.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(
            launcher
                .run(&run(&["perspicax-no-such-program"], false))
                .is_err(),
            "a program that is not installed says so"
        );
    }
}
