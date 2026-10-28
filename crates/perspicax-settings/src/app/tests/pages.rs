//! The behaviour pages: a key written as it is clicked or typed, given back
//! to its profile, and refused beside its field.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
};

use eframe::egui::{self, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable as _};
use perspicax_settings::Registry;

use super::{Compositor, Scratch, read, takes, window, window_with};
use crate::window::{App, Judge};

/// Show the page called `title`.
fn open(harness: &mut Harness<'_, App>, title: &str) {
    harness.get_by_label(title).click();
    harness.run();
}

/// The window on a file holding `text`, showing the page called `title`.
fn page(scratch: &Scratch, text: &str, title: &str) -> (PathBuf, Harness<'static, App>) {
    let path = scratch.config(text);
    let mut harness = window(&path, takes(), Some(&Compositor::default()));
    open(&mut harness, title);
    (path, harness)
}

/// A judge that takes everything, and counts the candidates it is shown.
fn counting() -> (Judge, Rc<Cell<usize>>) {
    let count = Rc::new(Cell::new(0));
    let seen = Rc::clone(&count);
    let judge: Judge = Box::new(move |candidate: &Path| {
        if !candidate.ends_with("config.toml") {
            seen.set(seen.get() + 1);
        }
        Ok(())
    });
    (judge, count)
}

/// Type `text` into the field for `leaf`, which holds nothing yet, and
/// leave it there. The tests run without fonts, so a cursor cannot move
/// through text that is there already: it stays at the start.
fn type_into(harness: &mut Harness<'_, App>, leaf: &str, text: &str) {
    let field = harness.get_by_role_and_label(Role::TextInput, leaf);
    assert_eq!(field.value().as_deref(), Some(""), "{leaf} holds nothing");
    field.focus();
    field.type_text(text);
    harness.run();
}

/// Press `key` in the field that has the keyboard.
fn press(harness: &mut Harness<'_, App>, key: egui::Key) {
    harness.key_press(key);
    harness.run();
}

#[test]
fn every_page_is_one_click_away() {
    let scratch = Scratch::new("pages");
    let path = scratch.config("");
    let mut harness = window(&path, takes(), None);
    for section in perspicax_settings::sections() {
        open(&mut harness, section.title);
        harness.get_by_label(section.about);
    }
    open(&mut harness, "Outputs");
    harness.get_by_label_contains("Nothing is written until Apply");
    open(&mut harness, "Protocols");
    harness.get_by_label(crate::access::WHY);
    open(&mut harness, "Keys");
    harness.get_by_label("Add a binding…");
    assert_eq!(read(&path), "");
}

#[test]
fn a_switch_is_written_as_soon_as_it_is_clicked() {
    let scratch = Scratch::new("switch");
    let (path, mut harness) = page(&scratch, "", "Focus");
    harness
        .get_by_role_and_label(Role::CheckBox, "autoraise")
        .click();
    harness.run();
    assert_eq!(read(&path), "[focus]\nautoraise = true\n");
    harness.get_by_label("yours; the profile's is off");
}

#[test]
fn a_choice_is_written_in_the_files_word() {
    let scratch = Scratch::new("choice");
    let (path, mut harness) = page(&scratch, "# mine\n", "Focus");
    harness
        .get_by_role_and_label(Role::RadioButton, "model: sloppy")
        .click();
    harness.run();
    assert_eq!(read(&path), "# mine\n\n[focus]\nmodel = \"sloppy\"\n");
}

#[test]
fn an_optional_switch_can_be_left_to_the_device() {
    let scratch = Scratch::new("optional");
    let (path, mut harness) = page(&scratch, "", "Pointer");
    let radio = "natural-scroll: the device's own";
    harness
        .get_by_role_and_label(Role::RadioButton, "natural-scroll: on")
        .click();
    harness.run();
    assert_eq!(read(&path), "[input.pointer]\nnatural-scroll = true\n");
    harness
        .get_by_role_and_label(Role::RadioButton, radio)
        .click();
    harness.run();
    assert_eq!(read(&path), "");
}

#[test]
fn a_number_is_written_once_when_its_field_lets_go_not_while_typing() {
    let scratch = Scratch::new("number");
    let path = scratch.config("");
    let (judge, judged) = counting();
    let mut harness = window(&path, judge, None);
    open(&mut harness, "Workspaces");
    type_into(&mut harness, "duration-ms", "300");
    assert_eq!((read(&path).as_str(), judged.get()), ("", 0));
    press(&mut harness, egui::Key::Enter);
    harness.run();
    harness.run();
    assert_eq!(read(&path), "[workspaces.transition]\nduration-ms = 300\n");
    assert_eq!(judged.get(), 1, "written once");
}

#[test]
fn a_number_outside_its_range_is_refused_beside_its_field_and_not_written() {
    let scratch = Scratch::new("range");
    let (path, mut harness) = page(&scratch, "", "Workspaces");
    type_into(&mut harness, "duration-ms", "5000");
    press(&mut harness, egui::Key::Enter);
    harness.get_by_label("5000 is outside 1 to 2000 milliseconds");
    assert_eq!(read(&path), "");
}

#[test]
fn a_value_the_session_refuses_comes_back_to_its_field_with_the_reason() {
    let scratch = Scratch::new("refused-field");
    let path = scratch.config("");
    let judge: Judge = Box::new(|candidate: &Path| {
        if candidate.ends_with("config.toml") {
            return Ok(());
        }
        Err("perspicax refuses it: a stand-in reason".to_owned())
    });
    let mut harness = window(&path, judge, None);
    open(&mut harness, "Workspaces");
    type_into(&mut harness, "duration-ms", "300");
    press(&mut harness, egui::Key::Enter);
    assert_eq!(read(&path), "");
    harness.get_by_label("Not written.");
    let field = harness.get_by_role_and_label(Role::TextInput, "duration-ms");
    assert_eq!(
        field.value().as_deref(),
        Some("300"),
        "what was typed is kept"
    );
    assert_eq!(
        harness
            .query_all_by_label("perspicax refuses it: a stand-in reason")
            .count(),
        2,
        "in the notice, and beside the field"
    );
}

#[test]
fn escape_then_another_frame_writes_nothing() {
    let scratch = Scratch::new("escape-field");
    let (path, mut harness) = page(&scratch, "", "Workspaces");
    type_into(&mut harness, "duration-ms", "300");
    press(&mut harness, egui::Key::Escape);
    harness.run();
    harness.run();
    assert_eq!(read(&path), "");
    let field = harness.get_by_role_and_label(Role::TextInput, "duration-ms");
    assert_eq!(field.value().as_deref(), Some(""), "what the file says");
}

#[test]
fn tabbing_through_untouched_fields_writes_nothing() {
    let scratch = Scratch::new("tab");
    let path = scratch.config("");
    let (judge, judged) = counting();
    let mut harness = window(&path, judge, None);
    open(&mut harness, "Windows");
    harness
        .get_by_role_and_label(Role::TextInput, "threshold")
        .focus();
    harness.run();
    for _ in 0..4 {
        harness.key_press(egui::Key::Tab);
        harness.run();
    }
    assert_eq!((read(&path).as_str(), judged.get()), ("", 0));
    assert!(
        harness
            .query_by_label("The file already says so; nothing was written.")
            .is_none()
    );
}

#[test]
fn what_is_typed_is_written_when_the_page_changes() {
    let scratch = Scratch::new("draft-page");
    let (path, mut harness) = page(&scratch, "", "Workspaces");
    type_into(&mut harness, "duration-ms", "300");
    open(&mut harness, "Focus");
    assert_eq!(read(&path), "[workspaces.transition]\nduration-ms = 300\n");
}

#[test]
fn reset_takes_the_line_out_and_the_profiles_value_returns() {
    let scratch = Scratch::new("reset-field");
    let (path, mut harness) = page(&scratch, "# mine\n\n[snap]\nthreshold = 20\n", "Windows");
    harness.get_by_label("yours; the profile's is 4 pixels");
    harness
        .get_by_role_and_label(Role::Button, "Reset threshold")
        .click();
    harness.run();
    assert_eq!(read(&path), "# mine\n");
    assert!(harness.query_by_label("Reset threshold").is_none());
    let field = harness.get_by_role_and_label(Role::TextInput, "threshold");
    assert_eq!(field.value().as_deref(), Some("4"));
}

#[test]
fn switching_to_minimal_with_panel_keys_offers_the_fixes_and_one_writes() {
    let scratch = Scratch::new("minimal");
    let mine = "[shell.panel]\nclock = \"%H:%M:%S\"\n";
    let (path, mut harness) = page(&scratch, mine, "Session");
    harness
        .get_by_role_and_label(Role::RadioButton, "profile: minimal")
        .click();
    harness.run();
    harness.get_by_label("Not written.");
    assert_eq!(read(&path), mine);
    harness
        .get_by_label("Turn the panel on: `enabled = true` in `[shell.panel]`")
        .click();
    harness.run();
    let fixed = read(&path);
    assert!(fixed.starts_with("profile = \"minimal\"\n"), "{fixed}");
    assert!(fixed.contains("enabled = true"), "{fixed}");
    assert!(fixed.contains("clock = \"%H:%M:%S\""), "{fixed}");
}

#[test]
fn the_panel_under_minimal_offers_only_turning_it_on() {
    let scratch = Scratch::new("panel-off");
    let (path, mut harness) = page(&scratch, "profile = \"minimal\"\n", "Panel");
    assert!(
        harness
            .query_by_role_and_label(Role::TextInput, "height")
            .is_none()
    );
    harness
        .get_by_role_and_label(Role::CheckBox, "enabled")
        .click();
    harness.run();
    assert_eq!(
        read(&path),
        "profile = \"minimal\"\n\n[shell.panel]\nenabled = true\n"
    );
    harness.get_by_role_and_label(Role::TextInput, "height");
}

#[test]
fn leaving_the_keys_page_gives_up_a_waiting_capture() {
    let scratch = Scratch::new("leave-keys");
    let path = scratch.config("");
    let compositor = Compositor::default();
    let mut harness = window(&path, takes(), Some(&compositor));
    harness.get_by_label("Add a binding…").click();
    harness.run();
    assert!(compositor.waiting());
    open(&mut harness, "Focus");
    assert!(!compositor.waiting(), "the next key typed is not a chord");
}

/// A slice of xkeyboard-config's list.
const LST: &str = "! layout\n  us              English (US)\n  ru              Russian\n\n\
                   ! variant\n  phonetic        ru: Russian (phonetic)\n";

#[test]
fn a_layout_picked_from_xkbs_list_writes_layout_and_variant_together() {
    let scratch = Scratch::new("layouts");
    let path = scratch.config("[input.keyboard]\nlayout = \"us\"\n");
    let registry = Some(Registry::read(LST));
    let mut harness = window_with(&path, takes(), None, registry);
    open(&mut harness, "Keyboard");
    harness.get_by_label("Add a layout…").click();
    harness.run();
    let find = harness.get_by_role_and_label(Role::TextInput, "Find a layout");
    find.focus();
    find.type_text("russ");
    harness.run();
    harness.get_by_label("Russian (ru)").click();
    harness.run();
    assert_eq!(read(&path), "[input.keyboard]\nlayout = \"us,ru\"\n");
    harness
        .get_by_role_and_label(Role::Button, "ru: variant…")
        .click();
    harness.run();
    harness.get_by_label("Russian (phonetic)").click();
    harness.run();
    assert_eq!(
        read(&path),
        "[input.keyboard]\nlayout = \"us,ru\"\nvariant = \",phonetic\"\n"
    );
}

#[test]
fn the_protocols_page_shows_each_rule_and_whose_it_is() {
    let scratch = Scratch::new("protocols");
    let (_, harness) = page(
        &scratch,
        "[protocols]\nscreencopy = [\"/usr/bin/grim\"]\n\n[agent]\nsocket = false\n\
         never = [\"ennius\", \"wallet\"]\n",
        "Protocols",
    );
    harness.get_by_label("/usr/bin/grim");
    harness.get_by_label("yours; the profile's is grim, wf-recorder, xdg-desktop-portal-wlr");
    harness.get_by_label("wlr-randr, kanshi, wdisplays, nwg-displays, perspicax-settings");
    harness.get_by_label("none, whatever `--mcp-socket` says");
    harness.get_by_label("ennius, wallet, perspicax-settings");
    harness.get_by_label("none: not offered");
}

/// Unwritten, the lock's rule follows `[shell] lock`; written "any", the
/// page warns of it beside the rule.
#[test]
fn the_protocols_page_warns_of_what_weakens_who_may_lock() {
    let scratch = Scratch::new("protocols-lock");
    let (_, harness) = page(&scratch, "[shell]\nlock = [\"gtklock\"]\n", "Protocols");
    harness.get_by_label("the system's gtklock");
    harness.get_by_label("follows [shell] lock");

    let scratch = Scratch::new("protocols-any-lock");
    let (_, harness) = page(
        &scratch,
        "[protocols]\nsession-lock = \"any\"\n",
        "Protocols",
    );
    harness.get_by_label(&perspicax_config::Doubt::AnyLock.to_string());
}

/// The lock program's field says when no copy the system installed is
/// there to lock with.
#[test]
fn the_lock_field_says_when_nothing_could_lock() {
    let scratch = Scratch::new("shell-lock");
    let (_, harness) = page(&scratch, "[shell]\nlock = [\"no-such-lock\"]\n", "Shell");
    let doubt = perspicax_config::Doubt::NotInstalled {
        name: "no-such-lock".to_owned(),
    };
    harness.get_by_label(&doubt.to_string());
}

#[test]
fn the_protocols_page_has_nothing_to_change() {
    let scratch = Scratch::new("protocols-read");
    let text = "[protocols]\nworkspace = \"off\"\n";
    let path = scratch.config(text);
    let (judge, judged) = counting();
    let mut harness = window(&path, judge, None);
    open(&mut harness, "Protocols");
    for role in [Role::CheckBox, Role::RadioButton, Role::TextInput] {
        assert_eq!(harness.query_all_by_role(role).count(), 0, "{role:?}");
    }
    assert_eq!((read(&path).as_str(), judged.get()), (text, 0));
}
