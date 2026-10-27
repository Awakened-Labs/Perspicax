//! `perspicax claude-code` and `perspicax ennius`, run as a person runs them,
//! against a Claude Code and an ennius that are stand-ins: scripts on a
//! `PATH` of the test's own, writing down what they were asked.
//!
//! The binary is run with nothing of the environment it was started from
//! but a home folder and that `PATH`, so a test run inside a session, beside a
//! real Claude Code or ennius, touches neither.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

/// A home folder of this test's own, with a `bin` for its `PATH`, removed
/// when dropped.
/// Write `text` to `path`, a program that may be run, from a shell of its
/// own (#272). The tests here run in parallel and start programs, and a fork
/// takes every file this process has open with it until the child runs its
/// program: a stand-in this process held open for writing as another test
/// forked would be busy (`ETXTBSY`) if run in that moment. Written by a
/// child, it is never open here. `crates/perspicax/src/forked.rs` says more.
fn program(path: &Path, text: &str) {
    let written = Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$1" > "$0" && chmod 755 "$0""#])
        .arg(path)
        .arg(text)
        .status()
        .expect("a shell to write it");
    assert!(written.success(), "{} not written", path.display());
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pxw-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("bin")).expect("a scratch folder");
        Self(dir)
    }

    /// A stand-in `claude`, which writes each command line it is given to
    /// `claude.log` and then runs `body`. Only the shell's own words are
    /// there for it to use: its `PATH` holds nothing else.
    fn claude(&self, body: &str) {
        let path = self.0.join("bin/claude");
        program(
            &path,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{body}\n",
                self.0.join("claude.log").display()
            ),
        );
    }

    /// A stand-in `ennius`, which answers `tools list` with `servers`, and
    /// `config show` with a grant of `tools` that its config file lists, and
    /// writes every other command line it is given to `ennius.log`, after
    /// the environment perspicax asked it in.
    fn ennius(&self, servers: &str, tools: &str) {
        let path = self.0.join("bin/ennius");
        let show = format!(
            r#"{{"config":{{"interactive":{{"grant":{{"tools":{tools}}}}}}},"provenance":{{"interactive.grant.tools":"user"}}}}"#
        );
        program(
            &path,
            &format!(
                "#!/bin/sh
\
                 case \"$*\" in\n\
                 '--json tools list') printf '%s\\n' '{{\"tools\":[],\"mcp_servers\":{servers}}}' ;;\n\
                 '--json config show') printf '%s\\n' '{show}' ;;\n\
                 *) printf '%s %s %s\\n' \"$ENNIUS_DISABLE_KEYRING\" \"$ENNIUS_DISABLE_ANALYTICS\" \"$*\" >> '{}' ;;\n\
                 esac\n",
                self.0.join("ennius.log").display()
            ),
        );
    }

    /// What the stand-in `claude` was asked, a line a command.
    fn asked(&self) -> Vec<String> {
        self.log("claude.log")
    }

    /// The lines a stand-in wrote to `name`.
    fn log(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.0.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Claude Code's config, holding `text`.
    fn config(&self, text: &str) {
        fs::write(self.0.join(".claude.json"), text).expect("a config");
    }

    /// The binary under test, run as `args` in this home.
    fn perspicax(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_perspicax"))
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", self.0.join("bin"))
            .stdin(Stdio::null())
            .output()
            .expect("perspicax runs")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The binary under test, as Claude Code would be told to run it.
fn this() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_perspicax"))
        .canonicalize()
        .expect("the binary under test")
}

#[test]
fn installing_tells_claude_code_to_run_this_perspicax_attach_and_once_is_enough() {
    let scratch = Scratch::new();
    scratch.claude("exit 0");
    let output = scratch.perspicax(&["claude-code", "install"]);
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    assert!(
        said(&output).contains("added `perspicax`"),
        "{}",
        said(&output)
    );
    let added = format!(
        r#"{{"args":["attach"],"command":"{}","type":"stdio"}}"#,
        this().display()
    );
    assert_eq!(
        scratch.asked(),
        [format!("mcp add-json -s user perspicax {added}")]
    );

    // What the stand-in was told is what Claude Code would then hold.
    scratch.config(&format!(r#"{{"mcpServers":{{"perspicax":{added}}}}}"#));
    let output = scratch.perspicax(&["claude-code", "install"]);
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    assert!(
        said(&output).contains("unchanged: wired as `perspicax`"),
        "{}",
        said(&output)
    );
    assert_eq!(scratch.asked().len(), 1, "and nothing more was run");

    let output = scratch.perspicax(&["claude-code", "status"]);
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    assert!(
        said(&output).starts_with("mcp-server  ok  wired as `perspicax`"),
        "{}",
        said(&output)
    );
}

#[test]
fn status_without_claude_code_is_skipped_and_is_no_failure() {
    let scratch = Scratch::new();
    let output = scratch.perspicax(&["claude-code", "status"]);
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    assert!(said(&output).contains("skipped"), "{}", said(&output));
}

#[test]
fn a_name_another_server_holds_is_refused_and_the_refusal_is_a_failure() {
    let scratch = Scratch::new();
    scratch.claude("exit 0");
    scratch.config(
        r#"{"mcpServers":{"perspicax":{"command":"perspicax","args":["--headless","--mcp"]}}}"#,
    );
    let output = scratch.perspicax(&["claude-code", "install"]);
    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
    assert!(said(&output).contains("1 of 1 failed"), "{}", said(&output));
    assert!(scratch.asked().is_empty(), "nothing was run");
}

#[test]
fn uninstalling_takes_out_what_runs_perspicax_attach() {
    let scratch = Scratch::new();
    scratch.claude("exit 0");
    scratch
        .config(r#"{"mcpServers":{"perpicax":{"command":"/gone/perspicax","args":["attach"]}}}"#);
    let output = scratch.perspicax(&["claude-code", "uninstall", "--dry-run"]);
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    assert!(
        said(&output).contains("would take out `perpicax`"),
        "{}",
        said(&output)
    );
    assert!(scratch.asked().is_empty(), "a dry run runs nothing");
    let output = scratch.perspicax(&["claude-code", "uninstall"]);
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    assert_eq!(scratch.asked(), ["mcp remove -s user perpicax"]);
}

#[test]
fn installing_into_ennius_adds_the_server_and_grants_its_tools_with_no_keyring() {
    let scratch = Scratch::new();
    scratch.ennius("[]", r#"["read_file"]"#);
    let output = scratch.perspicax(&["ennius", "install"]);
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    let asked = scratch.log("ennius.log");
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_eq!(
        asked[0],
        format!(
            "1 1 --json tools mcp add perspicax -- {} attach",
            this().display()
        ),
        "asked with the keyring and analytics off"
    );
    assert!(
        asked[1].starts_with("1 1 --json config grant add --tool mcp:perspicax:act --tool"),
        "{asked:?}"
    );
    assert_eq!(
        asked[1].matches("--tool mcp:perspicax:").count(),
        11,
        "{asked:?}"
    );
}

#[test]
fn enniuss_status_says_what_is_missing_and_is_no_failure() {
    let scratch = Scratch::new();
    scratch.ennius(
        &format!(
            r#"[{{"server":"perspicax","command":"{}","args":["attach"],"enabled":true,"lifetime":"ephemeral"}}]"#,
            this().display()
        ),
        r#"["mcp:perspicax:observe"]"#,
    );
    let output = scratch.perspicax(&["ennius", "status"]);
    assert_eq!(output.status.code(), Some(0), "{}", said(&output));
    let text = said(&output);
    assert!(
        text.contains("server  ok    wired as `perspicax`"),
        "{text}"
    );
    assert!(
        text.contains("grant   warn  1 of 11 granted; missing act,"),
        "{text}"
    );
    assert!(
        scratch.log("ennius.log").is_empty(),
        "status changes nothing"
    );
}
