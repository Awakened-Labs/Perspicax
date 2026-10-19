//! The writer, on a person's own config: dave's, as he writes it by hand,
//! with its aligned `=`, its trailing comments and its commented-out lines.
//! Sanitised (one path), plus two examples of the shape a person leaves for
//! later: `# border = 2` under `[decorations]`, and a `# [splash]` header.

use std::{
    cell::{Cell, RefCell},
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use perspicax_config::{Built, Error, Spelled, Trigger};
use perspicax_policy::{Chord, Keysym, Mods};
use perspicax_settings::{Applied, Document, Edit, Key, Reason, Refused, Value};

const FIXTURE: &str = include_str!("fixtures/commented.toml");

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
            std::env::temp_dir().join(format!("perspicax-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch folder");
        Self(dir)
    }

    /// `config.toml` in it, holding `text`.
    fn config(&self, text: &str) -> PathBuf {
        let path = self.0.join("config.toml");
        fs::write(&path, text).expect("a config");
        path
    }

    /// The names in it, sorted.
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .expect("the folder")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A judge that takes everything.
fn takes(_: &Path) -> Result<(), String> {
    Ok(())
}

fn set(table: &[&str], leaf: &str, value: Value) -> Edit {
    Edit::Set {
        key: Key::new(table, leaf),
        value,
    }
}

fn chord(text: &str) -> Trigger {
    perspicax_config::trigger(text).expect("a chord")
}

fn bind(text: &str, action: &str) -> Edit {
    Edit::Bind {
        trigger: chord(text),
        action: Some(Spelled::Name(action.to_owned())),
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("the config")
}

/// The lines that differ between two texts with as many lines, each as its
/// 0-based number, then before and after.
fn differing<'a>(before: &'a str, after: &'a str) -> Vec<(usize, &'a str, &'a str)> {
    let (before, after): (Vec<&str>, Vec<&str>) =
        (before.lines().collect(), after.lines().collect());
    assert_eq!(before.len(), after.len(), "lines added or removed");
    before
        .into_iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (was, is))| was != is)
        .map(|(at, (was, is))| (at, was, is))
        .collect()
}

/// The 0-based number of the fixture's one line that is exactly `line`.
fn line(line: &str) -> usize {
    let at: Vec<usize> = FIXTURE
        .lines()
        .enumerate()
        .filter(|&(_, l)| l == line)
        .map(|(at, _)| at)
        .collect();
    assert_eq!(at.len(), 1, "the fixture holds {line:?} once");
    at[0]
}

fn comments(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| line.trim_start().starts_with('#'))
        .collect()
}

fn refused(applied: Result<Applied, Refused>) -> Refused {
    applied.expect_err("a refusal")
}

#[test]
fn the_fixture_is_a_config_perspicax_takes() {
    perspicax_config::parse(FIXTURE, EVERYTHING).expect("the fixture is taken");
}

#[test]
fn editing_one_key_changes_only_its_line() {
    let scratch = Scratch::new("one-line");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let edit = set(&["decorations"], "title-height", Value::Integer(24));
    assert_eq!(doc.apply(&[edit], &takes).unwrap(), Applied::Written);
    let after = read(&path);
    assert_eq!(
        differing(FIXTURE, &after),
        [(
            line("title-height = 20"),
            "title-height = 20",
            "title-height = 24"
        )]
    );
    assert_eq!(doc.text(), after);
}

#[test]
fn a_trailing_comment_on_a_changed_line_survives() {
    let scratch = Scratch::new("trailing");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let edit = set(&["opacity"], "step", Value::Integer(5));
    doc.apply(&[edit], &takes).unwrap();
    let was = "step = 10          # defaults; every value a whole percent, 1–100";
    assert_eq!(
        differing(FIXTURE, &read(&path)),
        [(
            line(was),
            was,
            "step = 5          # defaults; every value a whole percent, 1–100"
        )]
    );
}

#[test]
fn an_aligned_equals_sign_stays_aligned() {
    let scratch = Scratch::new("aligned");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let edits = [
        bind("Logo+Left", "workspace-left"),
        bind("Logo+Up", "toggle-maximize"),
    ];
    doc.apply(&edits, &takes).unwrap();
    let (left, up) = (
        "\"Logo+Left\"  = \"snap-left\"",
        "\"Logo+Up\"    = \"snap-up\"     # maximize; on a snapped half, its top quarter",
    );
    assert_eq!(
        differing(FIXTURE, &read(&path)),
        [
            (line(left), left, "\"Logo+Left\"  = \"workspace-left\""),
            (
                line(up),
                up,
                "\"Logo+Up\"    = \"toggle-maximize\"     # maximize; on a snapped half, its top \
                 quarter"
            ),
        ]
    );
}

#[test]
fn commented_out_lines_and_their_order_survive() {
    let scratch = Scratch::new("comments");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let edits = [
        set(&["decorations"], "title-height", Value::Integer(24)),
        bind("Logo+t", "toggle-shade"),
        bind("Logo+Down", "minimize"),
        Edit::Reset {
            key: Key::new(&["snap"], "drag"),
        },
        set(&["agent"], "socket", Value::Bool(false)),
        set(&["shell"], "leave", Value::Strings(vec!["lock".to_owned()])),
    ];
    assert_eq!(doc.apply(&edits, &takes).unwrap(), Applied::Written);
    let after = read(&path);
    assert_ne!(after, FIXTURE);
    assert_eq!(comments(&after), comments(FIXTURE));
}

#[test]
fn a_commented_out_example_is_filled_where_it_stands() {
    let scratch = Scratch::new("example");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let edit = set(&["decorations"], "border", Value::Integer(3));
    doc.apply(&[edit], &takes).unwrap();
    assert_eq!(
        differing(FIXTURE, &read(&path)),
        [(line("# border = 2"), "# border = 2", "border = 3")]
    );
}

#[test]
fn a_commented_out_header_is_uncommented_for_its_first_key() {
    let scratch = Scratch::new("header");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let edit = set(&["splash"], "enabled", Value::Bool(false));
    doc.apply(&[edit], &takes).unwrap();
    let example = "# enabled = false   # on by default, wherever there is a seat";
    assert_eq!(
        differing(FIXTURE, &read(&path)),
        [
            (line("# [splash]"), "# [splash]", "[splash]"),
            (
                line(example),
                example,
                "enabled = false   # on by default, wherever there is a seat"
            ),
        ]
    );
}

#[test]
fn a_key_at_the_value_in_force_is_not_written() {
    let scratch = Scratch::new("in-force");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let asked = Cell::new(false);
    let judge = |_: &Path| {
        asked.set(true);
        Ok(())
    };
    // The minimal profile's own focus, and its own Alt+F4, under `[focus]`'s
    // `# model = "click"` example: neither is written, nor the example filled.
    let edits = [
        set(&["focus"], "model", Value::String("sloppy".to_owned())),
        bind("Alt+F4", "close"),
    ];
    assert_eq!(doc.apply(&edits, &judge).unwrap(), Applied::AlreadySo);
    assert_eq!(read(&path), FIXTURE);
    assert!(!asked.get(), "nothing to write, so nothing to judge");
}

#[test]
fn a_written_key_set_to_what_it_says_keeps_its_spelling() {
    let scratch = Scratch::new("same");
    let text = "[theme]\nname = 'breeze-dark'   # a literal string\n";
    let path = scratch.config(text);
    let mut doc = Document::open(&path).unwrap();
    let edit = set(&["theme"], "name", Value::String("breeze-dark".to_owned()));
    assert_eq!(doc.apply(&[edit], &takes).unwrap(), Applied::AlreadySo);
    assert_eq!(read(&path), text);
}

#[test]
fn a_written_key_stays_written_at_the_profiles_value() {
    let scratch = Scratch::new("stays");
    let path = scratch.config(FIXTURE);
    // The fixture's `drag = false` is the minimal profile's own value.
    let unwritten = FIXTURE.replace("[snap]\ndrag = false\n\n", "");
    assert_ne!(unwritten, FIXTURE);
    assert_eq!(
        perspicax_config::parse(&unwritten, EVERYTHING).unwrap(),
        perspicax_config::parse(FIXTURE, EVERYTHING).unwrap()
    );
    let mut doc = Document::open(&path).unwrap();
    let drag = |on| set(&["snap"], "drag", Value::Bool(on));
    assert_eq!(doc.apply(&[drag(true)], &takes).unwrap(), Applied::Written);
    assert!(read(&path).contains("[snap]\ndrag = true\n"));
    assert_eq!(doc.apply(&[drag(false)], &takes).unwrap(), Applied::Written);
    assert_eq!(read(&path), FIXTURE);
}

#[test]
fn reset_to_profile_removes_the_key_and_retires_an_emptied_header() {
    let scratch = Scratch::new("reset");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let reset = [Edit::Reset {
        key: Key::new(&["snap"], "drag"),
    }];
    assert_eq!(doc.apply(&reset, &takes).unwrap(), Applied::Written);
    assert_eq!(read(&path), FIXTURE.replace("[snap]\ndrag = false\n\n", ""));
    // Resetting what is not written is already so.
    assert_eq!(doc.apply(&reset, &takes).unwrap(), Applied::AlreadySo);
}

#[test]
fn a_binding_is_found_by_its_chord_not_its_spelling() {
    let scratch = Scratch::new("by-chord");
    let path = scratch.config("[keys]\n\"Super+a\" = \"close\"   # mine\n\"Logo+b\" = \"shade\"\n");
    let mut doc = Document::open(&path).unwrap();
    doc.apply(&[bind("Logo+a", "toggle-sticky")], &takes)
        .unwrap();
    assert_eq!(
        read(&path),
        "[keys]\n\"Super+a\" = \"toggle-sticky\"   # mine\n\"Logo+b\" = \"shade\"\n"
    );
    let reset = Edit::ResetBinding {
        trigger: chord("Logo+a"),
    };
    doc.apply(&[reset], &takes).unwrap();
    assert_eq!(read(&path), "[keys]\n\"Logo+b\" = \"shade\"\n");
}

#[test]
fn unbinding_a_profile_chord_writes_none_and_resetting_removes_the_line() {
    let scratch = Scratch::new("unbind");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let unbind = Edit::Bind {
        trigger: chord("Alt+F4"),
        action: None,
    };
    assert_eq!(doc.apply(&[unbind], &takes).unwrap(), Applied::Written);
    let last = "\"Logo+End\" = \"opacity-reset\"\n";
    assert_eq!(
        read(&path),
        FIXTURE.replace(last, &format!("{last}\"Alt+F4\" = \"none\"\n"))
    );
    let reset = Edit::ResetBinding {
        trigger: chord("Alt+F4"),
    };
    assert_eq!(doc.apply(&[reset], &takes).unwrap(), Applied::Written);
    assert_eq!(read(&path), FIXTURE);
}

#[test]
fn a_spawn_binding_is_written_as_an_inline_table() {
    let scratch = Scratch::new("spawn");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let spawn = |agent| {
        Some(Spelled::Spawn {
            command: vec!["foot".to_owned(), "-e".to_owned(), "htop".to_owned()],
            agent,
        })
    };
    let edits = [
        Edit::Bind {
            trigger: chord("Logo+t"),
            action: spawn(false),
        },
        Edit::Bind {
            trigger: chord("Logo+Shift+t"),
            action: spawn(true),
        },
        Edit::Bind {
            trigger: chord("Logo+m"),
            action: Some(Spelled::Pie("launchers".to_owned())),
        },
    ];
    doc.apply(&edits, &takes).unwrap();
    let last = "\"Logo+End\" = \"opacity-reset\"\n";
    let added = "\"Logo+t\" = { spawn = [\"foot\", \"-e\", \"htop\"] }\n\
                 \"Logo+Shift+t\" = { spawn = [\"foot\", \"-e\", \"htop\"], agent = true }\n\
                 \"Logo+m\" = { pie = \"launchers\" }\n";
    assert_eq!(
        read(&path),
        FIXTURE.replace(last, &format!("{last}{added}"))
    );
}

#[test]
fn a_chord_no_config_can_spell_yet_is_refused_and_writes_nothing() {
    let scratch = Scratch::new("unspellable");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let keypad = Edit::Bind {
        trigger: Trigger::Chord(Chord {
            mods: Mods::alt(),
            key: Keysym::KP_Enter,
        }),
        action: Some(Spelled::Name("close".to_owned())),
    };
    let refusal = refused(doc.apply(&[keypad], &takes));
    assert!(
        matches!(refusal.reason, Reason::Unwritable(_)),
        "{:?}",
        refusal.reason
    );
    assert_eq!(read(&path), FIXTURE);
}

#[test]
fn an_edit_made_while_another_program_rewrote_the_file_is_replayed_on_top() {
    let scratch = Scratch::new("replayed");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let theirs = FIXTURE.replace("title-height = 20", "title-height = 22");
    fs::write(&path, &theirs).unwrap();
    let edit = set(&["opacity"], "step", Value::Integer(5));
    assert_eq!(doc.apply(&[edit], &takes).unwrap(), Applied::Replayed);
    let after = read(&path);
    assert_eq!(
        after,
        theirs.replace("step = 10          #", "step = 5          #")
    );
    assert_eq!(doc.text(), after);
}

#[test]
fn a_file_rewritten_while_the_candidate_was_judged_is_replayed_on_top() {
    let scratch = Scratch::new("judged-meanwhile");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let theirs = FIXTURE.replace("title-height = 20", "title-height = 22");
    let judged = Cell::new(0);
    let judge = |_: &Path| {
        judged.set(judged.get() + 1);
        if judged.get() == 1 {
            fs::write(&path, &theirs).unwrap();
        }
        Ok(())
    };
    let edit = set(&["opacity"], "step", Value::Integer(5));
    assert_eq!(doc.apply(&[edit], &judge).unwrap(), Applied::Replayed);
    assert_eq!(judged.get(), 2, "judged again, on top of theirs");
    assert_eq!(
        read(&path),
        theirs.replace("step = 10          #", "step = 5          #")
    );
}

#[test]
fn a_file_that_never_stops_changing_is_given_up_on() {
    let scratch = Scratch::new("busy");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let judged = Cell::new(0);
    let judge = |_: &Path| {
        judged.set(judged.get() + 1);
        fs::write(
            &path,
            FIXTURE.replace("edges = 40", &format!("edges = {}", judged.get())),
        )
        .unwrap();
        Ok(())
    };
    let edit = set(&["opacity"], "step", Value::Integer(5));
    let refusal = refused(doc.apply(&[edit], &judge));
    assert!(
        matches!(refusal.reason, Reason::Busy),
        "{:?}",
        refusal.reason
    );
    assert!(read(&path).contains("step = 10 "), "ours was never written");
    assert_eq!(scratch.names(), ["config.toml"]);
}

#[test]
fn a_replayed_edit_the_new_file_refuses_is_refused_with_the_reason_and_writes_nothing() {
    let scratch = Scratch::new("replay-refused");
    let path = scratch.0.join("config.toml");
    let mut doc = Document::open(&path).unwrap();
    assert_eq!(
        doc.text(),
        "",
        "no file yet: the classic profile, which has a panel"
    );
    let theirs = "profile = \"minimal\"\n";
    fs::write(&path, theirs).unwrap();
    let edit = set(&["shell", "panel"], "height", Value::Integer(30));
    let refusal = refused(doc.apply(&[edit], &takes));
    assert!(
        matches!(&refusal.reason, Reason::Config(Error::Invalid { key, .. }) if key == "shell.panel"),
        "{:?}",
        refusal.reason
    );
    assert_eq!(read(&path), theirs);
    assert_eq!(doc.text(), theirs);
    assert_eq!(scratch.names(), ["config.toml"]);
}

#[test]
fn a_file_that_is_not_toml_is_never_written() {
    let scratch = Scratch::new("not-toml");
    let text = "[keys\n\"Logo+a\" = \"close\"\n";
    let path = scratch.config(text);
    let mut doc = Document::open(&path).unwrap();
    let asked = Cell::new(false);
    let judge = |_: &Path| {
        asked.set(true);
        Ok(())
    };
    let refusal = refused(doc.apply(&[bind("Logo+a", "shade")], &judge));
    assert!(
        matches!(refusal.reason, Reason::NotToml(_)),
        "{:?}",
        refusal.reason
    );
    assert_eq!(read(&path), text);
    assert!(!asked.get());
    assert_eq!(scratch.names(), ["config.toml"]);
}

#[test]
fn a_refused_check_leaves_the_file_and_no_temp_behind() {
    let scratch = Scratch::new("judge-refuses");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    let judge =
        |_: &Path| Err("perspicax-shell refuses it: `shell.panel.items`: no tray".to_owned());
    let edit = set(&["decorations"], "title-height", Value::Integer(24));
    let refusal = refused(doc.apply(&[edit], &judge));
    assert!(
        matches!(&refusal.reason, Reason::Judged(words) if words.contains("no tray")),
        "{:?}",
        refusal.reason
    );
    assert_eq!(read(&path), FIXTURE);
    assert_eq!(doc.text(), FIXTURE);
    assert_eq!(scratch.names(), ["config.toml"]);
}

#[test]
fn the_write_is_a_rename_of_a_file_beside_it() {
    let scratch = Scratch::new("rename");
    let path = scratch.config(FIXTURE);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut doc = Document::open(&path).unwrap();
    let judged = RefCell::new(None);
    let judge = |candidate: &Path| {
        let meta = fs::metadata(candidate).unwrap();
        *judged.borrow_mut() = Some((candidate.to_owned(), meta.ino(), read(candidate)));
        // Until it is renamed, the file is as it was.
        assert_eq!(read(&path), FIXTURE);
        Ok(())
    };
    let edit = set(&["decorations"], "title-height", Value::Integer(24));
    doc.apply(&[edit], &judge).unwrap();
    let (candidate, ino, text) = judged.take().expect("the candidate was judged");
    assert_eq!(candidate.parent(), path.parent());
    assert_eq!(
        candidate.file_name().unwrap().to_string_lossy(),
        format!(".config.toml.perspicax-settings-{}", std::process::id())
    );
    let meta = fs::metadata(&path).unwrap();
    assert_eq!(meta.ino(), ino, "the candidate is the file now");
    assert_eq!(
        meta.permissions().mode() & 0o777,
        0o640,
        "with the old file's mode"
    );
    assert_eq!(read(&path), text);
    assert_eq!(scratch.names(), ["config.toml"]);
}

#[test]
fn a_linked_config_is_written_beside_its_target_and_stays_a_link() {
    let scratch = Scratch::new("link");
    let dotfiles = scratch.0.join("dotfiles");
    fs::create_dir(&dotfiles).unwrap();
    let real = dotfiles.join("config.toml");
    fs::write(&real, FIXTURE).unwrap();
    let link = scratch.0.join("config.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut doc = Document::open(&link).unwrap();
    let judged = RefCell::new(None);
    let judge = |candidate: &Path| {
        *judged.borrow_mut() = candidate.parent().map(Path::to_owned);
        Ok(())
    };
    let edit = set(&["decorations"], "title-height", Value::Integer(24));
    doc.apply(&[edit], &judge).unwrap();
    assert_eq!(judged.take().as_deref(), Some(dotfiles.as_path()));
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(read(&real).contains("title-height = 24"));
}

#[test]
fn a_missing_file_is_made_with_its_folder() {
    let scratch = Scratch::new("missing");
    let path = scratch.0.join("perspicax").join("config.toml");
    let mut doc = Document::open(&path).unwrap();
    let edit = Edit::Bind {
        trigger: chord("Logo+t"),
        action: Some(Spelled::Spawn {
            command: vec!["foot".to_owned()],
            agent: false,
        }),
    };
    assert_eq!(doc.apply(&[edit], &takes).unwrap(), Applied::Written);
    assert_eq!(read(&path), "[keys]\n\"Logo+t\" = { spawn = [\"foot\"] }\n");
}

/// toml_edit writes a file back LF, so a CRLF file is read as LF too: the
/// layout's lines then read as the file it writes will, and a new table is
/// set one blank line below the last, not two.
#[test]
fn a_crlf_file_is_read_as_lf_and_written_back_lf() {
    let scratch = Scratch::new("crlf");
    let path = scratch.config("[decorations]\r\n# border = 2\r\nmode = \"server\"\r\n\r\n");
    let mut doc = Document::open(&path).unwrap();
    assert_eq!(
        doc.text(),
        "[decorations]\n# border = 2\nmode = \"server\"\n\n"
    );
    let edits = [
        set(&["decorations"], "border", Value::Integer(3)),
        set(&["splash"], "enabled", Value::Bool(false)),
    ];
    doc.apply(&edits, &takes).unwrap();
    assert_eq!(
        read(&path),
        "[decorations]\nborder = 3\nmode = \"server\"\n\n[splash]\nenabled = false\n"
    );
}

/// The 2026-10-07 shape: a `[shell.panel]` written under classic, which has
/// a panel, and then a switch to minimal, which has none unless told.
#[test]
fn switching_to_minimal_with_panel_keys_offers_both_fixes_and_each_validates() {
    let classic = FIXTURE
        .replace("profile = \"minimal\"", "profile = \"classic\"")
        .replace("[shell.panel]\nenabled = true\n", "[shell.panel]\n");
    perspicax_config::parse(&classic, EVERYTHING).expect("classic has a panel to configure");
    let switch = [set(&[], "profile", Value::String("minimal".to_owned()))];

    let scratch = Scratch::new("fixes");
    let path = scratch.config(&classic);
    let mut doc = Document::open(&path).unwrap();
    let refusal = refused(doc.apply(&switch, &takes));
    assert!(
        matches!(&refusal.reason, Reason::Config(Error::Invalid { key, .. }) if key == "shell.panel"),
        "{:?}",
        refusal.reason
    );
    assert_eq!(read(&path), classic);
    let says: Vec<&str> = refusal.fixes.iter().map(|fix| fix.says.as_str()).collect();
    assert_eq!(
        says,
        [
            "Turn the panel on: `enabled = true` in `[shell.panel]`",
            "Reset `shell.panel` to the profile's",
        ]
    );

    for fix in &refusal.fixes {
        fs::write(&path, &classic).unwrap();
        let mut doc = Document::open(&path).unwrap();
        assert_eq!(
            doc.apply(&fix.edits, &takes).unwrap(),
            Applied::Written,
            "{}",
            fix.says
        );
        let config = perspicax_config::parse(&read(&path), EVERYTHING).expect("each fix is taken");
        assert_eq!(
            config.profile,
            perspicax_config::Profile::Minimal,
            "{}",
            fix.says
        );
    }
    let [on, reset] = &refusal.fixes[..] else {
        unreachable!("two fixes");
    };
    fs::write(&path, &classic).unwrap();
    Document::open(&path)
        .unwrap()
        .apply(&on.edits, &takes)
        .unwrap();
    assert!(read(&path).contains("task-titles = true\nenabled = true\n"));
    fs::write(&path, &classic).unwrap();
    Document::open(&path)
        .unwrap()
        .apply(&reset.edits, &takes)
        .unwrap();
    assert!(!read(&path).contains("[shell.panel]"));
}

#[test]
fn a_file_the_session_refuses_is_refused_as_it_stands_with_its_fixes() {
    // A switch to minimal made in an editor, as on 2026-10-07: the panel's
    // keys stay, and minimal has no panel unless it is turned on.
    let minimal = FIXTURE.replace("[shell.panel]\nenabled = true\n", "[shell.panel]\n");
    let scratch = Scratch::new("check-refused");
    let path = scratch.config(&minimal);
    let doc = Document::open(&path).unwrap();
    let refusal = doc.check(&takes).unwrap_err();
    assert!(
        matches!(&refusal.reason, Reason::Config(Error::Invalid { key, .. }) if key == "shell.panel"),
        "{:?}",
        refusal.reason
    );
    let says: Vec<&str> = refusal.fixes.iter().map(|fix| fix.says.as_str()).collect();
    assert_eq!(
        says,
        [
            "Turn the panel on: `enabled = true` in `[shell.panel]`",
            "Reset `shell.panel` to the profile's",
        ]
    );
    let mut doc = doc;
    assert_eq!(
        doc.apply(&refusal.fixes[0].edits, &takes).unwrap(),
        Applied::Written
    );
    assert_eq!(doc.check(&takes).ok(), Some(()));
}

#[test]
fn a_file_the_judge_refuses_as_it_stands_is_refused_in_its_words() {
    let scratch = Scratch::new("check-judged");
    let path = scratch.config(FIXTURE);
    let asked = RefCell::new(Vec::new());
    let judge = |candidate: &Path| {
        asked.borrow_mut().push(candidate.to_owned());
        Err("perspicax refuses it: `xwayland` is not built".to_owned())
    };
    let refusal = Document::open(&path).unwrap().check(&judge).unwrap_err();
    assert!(
        matches!(&refusal.reason, Reason::Judged(words) if words.contains("xwayland")),
        "{:?}",
        refusal.reason
    );
    assert_eq!(*asked.borrow(), [path], "the file itself is judged");
    let missing = Document::open(scratch.0.join("nowhere.toml")).unwrap();
    assert_eq!(
        missing.check(&judge).ok(),
        Some(()),
        "no file is the classic profile"
    );
}

#[test]
fn a_file_that_is_not_toml_is_refused_with_no_fix() {
    let scratch = Scratch::new("check-not-toml");
    let path = scratch.config("[keys\n");
    let refusal = Document::open(&path).unwrap().check(&takes).unwrap_err();
    assert!(
        matches!(refusal.reason, Reason::NotToml(_)),
        "{:?}",
        refusal.reason
    );
    assert_eq!(refusal.fixes, []);
}

#[test]
fn a_reload_reads_what_another_program_wrote() {
    let scratch = Scratch::new("reload");
    let path = scratch.config(FIXTURE);
    let mut doc = Document::open(&path).unwrap();
    assert!(!doc.reload().unwrap(), "nothing changed");
    let edited = format!("{FIXTURE}\n# vi was here\n");
    fs::write(&path, &edited).unwrap();
    assert!(doc.reload().unwrap());
    assert_eq!(doc.text(), edited);
}
