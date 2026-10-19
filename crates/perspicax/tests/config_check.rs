//! `perspicax config check`, run as a person runs it.
//!
//! The shell's half of the answer depends on which perspicax-shell the
//! check finds beside the binary under test, which a test run may or may not
//! have built, so these assert on perspicax's own word, and on exit statuses
//! no shell build could change: none of these files writes a `[shell]` key a
//! shell built without components would refuse.

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

/// The binary under test, asked about `args`, with the session's config
/// variable taken away so nothing but `args` names a file.
fn check(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_perspicax"))
        .args(["config", "check"])
        .args(args)
        .env_remove("PERSPICAX_CONFIG")
        .stdin(Stdio::null())
        .output()
        .expect("perspicax runs")
}

/// A folder of this test's own, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pxc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).expect("a scratch folder");
        Self(dir)
    }

    /// A config file holding `text`, as an argument.
    fn config(&self, text: &str) -> String {
        let path = self.0.join("config.toml");
        fs::write(&path, text).expect("a config");
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

fn said(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn checking_a_good_config_says_it_is_taken() {
    let scratch = Scratch::new();
    let config = scratch.config("profile = \"minimal\"\n");
    let output = check(&[&config]);
    let (out, err) = said(&output);
    assert_eq!(output.status.code(), Some(0), "{out}{err}");
    assert!(
        out.contains(&format!("{config}: perspicax takes it")),
        "{out}"
    );
}

#[test]
fn checking_a_config_the_compositor_refuses_names_the_key_and_exits_78() {
    let scratch = Scratch::new();
    let config = scratch.config("[keys]\n\"Ctrl+Alt+F2\" = \"close\"\n");
    let output = check(&[&config]);
    let (out, err) = said(&output);
    assert_eq!(output.status.code(), Some(78), "{out}{err}");
    assert!(
        err.contains(&format!(
            "{config}: perspicax refuses it: `keys.Ctrl+Alt+F2`"
        )),
        "{err}"
    );
    assert!(err.contains("switches to VT 2"), "{err}");
}

#[test]
fn the_panel_under_minimal_without_enabled_is_refused() {
    let scratch = Scratch::new();
    let config = scratch.config("profile = \"minimal\"\n[shell.panel]\nedge = \"top\"\n");
    let output = check(&[&config]);
    let (out, err) = said(&output);
    assert_eq!(output.status.code(), Some(78), "{out}{err}");
    assert!(err.contains("perspicax refuses it: `shell.panel`"), "{err}");
    assert!(err.contains("enabled = true"), "{err}");
}

#[test]
fn a_screencopy_rule_is_refused_only_by_a_build_without_capture() {
    let scratch = Scratch::new();
    let config = scratch.config("[protocols]\nscreencopy = [\"grim\"]\n");
    let output = check(&[&config]);
    let (out, err) = said(&output);
    if cfg!(feature = "capture") {
        assert_eq!(output.status.code(), Some(0), "{out}{err}");
    } else {
        assert_eq!(output.status.code(), Some(78), "{out}{err}");
        assert!(err.contains("`protocols.screencopy`"), "{err}");
        assert!(err.contains("perspicax/capture"), "{err}");
    }
}

#[test]
fn checking_a_file_that_is_not_there_says_so() {
    let scratch = Scratch::new();
    let missing = scratch.0.join("nowhere.toml");
    let missing = missing.to_string_lossy();
    let output = check(&[&missing]);
    let (out, err) = said(&output);
    assert_eq!(output.status.code(), Some(66), "{out}{err}");
    assert!(err.contains("there is no such file"), "{err}");
}

#[test]
fn the_default_path_is_the_sessions_config() {
    let scratch = Scratch::new();
    let config = scratch.config("[keys]\n\"Ctrl+Alt+BackSpace\" = \"none\"\n");
    let output = Command::new(env!("CARGO_BIN_EXE_perspicax"))
        .args(["config", "check"])
        .env("PERSPICAX_CONFIG", &config)
        .stdin(Stdio::null())
        .output()
        .expect("perspicax runs");
    let (out, err) = said(&output);
    assert_eq!(output.status.code(), Some(78), "{out}{err}");
    assert!(err.contains(&config), "{err}");
    assert!(err.contains("ends the session"), "{err}");
}

#[test]
fn with_no_config_the_session_runs_the_classic_profile_and_says_so() {
    let scratch = Scratch::new();
    let output = Command::new(env!("CARGO_BIN_EXE_perspicax"))
        .args(["config", "check"])
        .env_remove("PERSPICAX_CONFIG")
        .env("XDG_CONFIG_HOME", &scratch.0)
        .stdin(Stdio::null())
        .output()
        .expect("perspicax runs");
    let (out, err) = said(&output);
    assert_eq!(output.status.code(), Some(0), "{out}{err}");
    assert!(out.contains("classic profile"), "{out}");
}
