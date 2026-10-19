//! The Outputs page: each monitor's rule, staged until Apply, then kept, or
//! put back on its own.
//!
//! The session's list of monitors is a stand-in: a desk, a big monitor and
//! a laptop's own. egui's clock moves a quarter of a second each frame a
//! test runs, so the 15 s a change waits to be kept is 60 frames.

use std::fs;

use eframe::egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use perspicax_policy::{Head, HeadMode};

use super::{Compositor, Scratch, read, takes, window_on, window_seeing};
use crate::{
    monitors::{Monitor, NOT_OFFERED, Seen, WITHDRAWN},
    window::App,
};

fn mode(width: i32, height: i32, refresh: i32, preferred: bool) -> HeadMode {
    HeadMode {
        width,
        height,
        refresh,
        preferred,
    }
}

/// DP-1, a 144 Hz monitor at its preferred 60, with the laptop's own to its
/// right.
fn desk() -> Vec<Monitor> {
    vec![
        Monitor {
            head: Head {
                name: "DP-1".to_owned(),
                enabled: true,
                modes: vec![
                    mode(2560, 1440, 59_951, true),
                    mode(2560, 1440, 143_912, false),
                    mode(1920, 1080, 60_000, false),
                ],
                current: Some(0),
                position: (0, 0),
                scale: 1.0,
            },
            made: "Dell U2720Q".to_owned(),
        },
        Monitor {
            head: Head {
                name: "eDP-1".to_owned(),
                enabled: true,
                modes: vec![mode(1920, 1200, 60_000, true)],
                current: Some(0),
                position: (2560, 0),
                scale: 1.0,
            },
            made: String::new(),
        },
    ]
}

/// The window on a file holding `text`, the desk listed, on Outputs.
fn outputs(scratch: &Scratch, text: &str) -> (std::path::PathBuf, Harness<'static, App>) {
    let path = scratch.config(text);
    let mut harness = window_seeing(&path, takes(), Ok(desk()));
    click(&mut harness, Role::Button, "Outputs");
    (path, harness)
}

fn click(harness: &mut Harness<'_, App>, role: Role, label: &str) {
    harness.get_by_role_and_label(role, label).click();
    harness.run();
}

/// Type `text` into `monitor field`, which holds nothing yet: the tests
/// run without fonts, so a cursor cannot move through text already there.
fn type_into(harness: &mut Harness<'_, App>, field: &str, text: &str) {
    let node = harness.get_by_role_and_label(Role::TextInput, field);
    assert_eq!(node.value().as_deref(), Some(""), "{field} holds nothing");
    node.focus();
    node.type_text(text);
    harness.run();
}

/// Turn eDP-1 off and apply it: the file then says so, and waits.
fn turn_the_laptop_off(harness: &mut Harness<'_, App>) {
    click(harness, Role::CheckBox, "eDP-1: on");
    click(harness, Role::Button, "Apply");
}

const OFF: &str = "[[output]]\nname = \"eDP-1\"\nenable = false\n";

#[test]
fn the_page_lists_each_connected_monitor_and_each_written_rule() {
    let scratch = Scratch::new("outputs-list");
    let (_, harness) = outputs(
        &scratch,
        "[[output]]\nname = \"HDMI-A-1\"\nmode = \"1920x1080\"\n",
    );
    harness.get_by_label("DP-1 — Dell U2720Q");
    harness.get_by_label("Now: 2560 × 1440 at 59.95 Hz, at 0, 0, scale 1.");
    harness.get_by_role_and_label(Role::CheckBox, "eDP-1: on");
    harness.get_by_label("HDMI-A-1: not connected");
    let typed = harness.get_by_role_and_label(Role::TextInput, "HDMI-A-1 mode");
    assert_eq!(typed.value().as_deref(), Some("1920x1080"));
    harness.get_by_role_and_label(Role::RadioButton, "DP-1 mode: 2560 × 1440");
}

#[test]
fn nothing_is_written_until_apply() {
    let scratch = Scratch::new("outputs-staged");
    let (path, mut harness) = outputs(&scratch, "");
    click(&mut harness, Role::CheckBox, "eDP-1: on");
    type_into(&mut harness, "DP-1 scale", "1.5");
    harness.get_by_label("Changed here; nothing is written until Apply.");
    click(&mut harness, Role::Button, "Keys");
    click(&mut harness, Role::Button, "Outputs");
    assert_eq!(read(&path), "", "nor on leaving the page");
    click(&mut harness, Role::Button, "Apply");
    assert_eq!(
        read(&path),
        "[[output]]\nname = \"DP-1\"\nscale = 1.5\n\n[[output]]\nname = \"eDP-1\"\nenable = false\n"
    );
}

#[test]
fn apply_writes_the_rule_and_asks_to_keep_it() {
    let scratch = Scratch::new("outputs-apply");
    let (path, mut harness) = outputs(&scratch, "");
    turn_the_laptop_off(&mut harness);
    assert_eq!(read(&path), OFF);
    harness.get_by_label("Keep these settings? Reverting in 15 s.");
    harness.get_by_role_and_label(Role::Button, "Revert now");
    let apply = harness.get_by_role_and_label(Role::Button, "Apply");
    assert!(
        apply.accesskit_node().is_disabled(),
        "one change waits at a time"
    );
}

#[test]
fn keeping_ends_the_countdown_and_the_file_stays() {
    let scratch = Scratch::new("outputs-keep");
    let (path, mut harness) = outputs(&scratch, "");
    turn_the_laptop_off(&mut harness);
    click(&mut harness, Role::Button, "Keep");
    harness.run_steps(70);
    assert_eq!(read(&path), OFF);
    assert!(harness.query_by_label("Keep").is_none());
}

#[test]
fn the_countdown_puts_the_file_back_after_15_s() {
    let scratch = Scratch::new("outputs-countdown");
    let (path, mut harness) = outputs(&scratch, "# mine\n");
    turn_the_laptop_off(&mut harness);
    harness.run_steps(50);
    assert_eq!(read(&path), format!("# mine\n\n{OFF}"), "still waiting");
    harness.get_by_label_contains("Reverting in");
    harness.run_steps(12);
    assert_eq!(read(&path), "# mine\n", "put back");
    harness.get_by_label("Put back as it was. The session reads it now.");
}

#[test]
fn a_revert_is_made_over_a_change_written_meanwhile() {
    let scratch = Scratch::new("outputs-meanwhile");
    let (path, mut harness) = outputs(&scratch, "");
    turn_the_laptop_off(&mut harness);
    // Another program writes the file while the change waits.
    fs::write(&path, format!("[snap]\nthreshold = 9\n\n{OFF}")).unwrap();
    click(&mut harness, Role::Button, "Revert now");
    assert_eq!(read(&path), "[snap]\nthreshold = 9\n");
}

#[test]
fn closing_the_window_mid_countdown_reverts() {
    let scratch = Scratch::new("outputs-close");
    let (path, mut harness) = outputs(&scratch, "");
    turn_the_laptop_off(&mut harness);
    harness.state_mut().closing();
    assert_eq!(read(&path), "");
}

#[test]
fn turning_every_monitor_off_cannot_be_applied() {
    let scratch = Scratch::new("outputs-dark");
    let (path, mut harness) = outputs(&scratch, "");
    click(&mut harness, Role::CheckBox, "eDP-1: on");
    click(&mut harness, Role::CheckBox, "DP-1: on");
    click(&mut harness, Role::Button, "Apply");
    harness.get_by_label(
        "Nothing was written: that would leave no monitor on, and nothing to see the screen by.",
    );
    assert_eq!(read(&path), "");
}

#[test]
fn a_mode_picked_from_the_list_is_written_as_the_monitor_offers_it() {
    let scratch = Scratch::new("outputs-mode");
    let (path, mut harness) = outputs(&scratch, "");
    click(&mut harness, Role::RadioButton, "DP-1 mode: 2560 × 1440");
    click(&mut harness, Role::RadioButton, "DP-1 refresh: 143.91 Hz");
    click(&mut harness, Role::Button, "Apply");
    assert_eq!(
        read(&path),
        "[[output]]\nname = \"DP-1\"\nmode = \"2560x1440@143.912\"\n"
    );
}

#[test]
fn without_the_monitor_list_the_mode_is_typed_and_the_page_says_why() {
    let scratch = Scratch::new("outputs-unlisted");
    let path = scratch.config("[[output]]\nname = \"eDP-1\"\nscale = 2\n");
    let mut harness = window_seeing(&path, takes(), Err(NOT_OFFERED));
    click(&mut harness, Role::Button, "Outputs");
    harness.get_by_label(NOT_OFFERED);
    type_into(&mut harness, "eDP-1 mode", "1920x1200");
    click(&mut harness, Role::Button, "Apply");
    assert_eq!(
        read(&path),
        "[[output]]\nname = \"eDP-1\"\nscale = 2\nmode = \"1920x1200\"\n"
    );
}

#[test]
fn forgetting_a_rule_takes_its_entry_out() {
    let scratch = Scratch::new("outputs-forget");
    let (path, mut harness) = outputs(
        &scratch,
        "[agent]\nsocket = true\n\n[[output]]\nname = \"eDP-1\"\nscale = 0.9\n",
    );
    click(&mut harness, Role::Button, "Forget eDP-1's rule");
    click(&mut harness, Role::Button, "Apply");
    assert_eq!(read(&path), "[agent]\nsocket = true\n");
}

#[test]
fn a_scale_outside_its_range_is_refused_beside_its_field() {
    let scratch = Scratch::new("outputs-scale");
    let (path, mut harness) = outputs(&scratch, "");
    type_into(&mut harness, "DP-1 scale", "9");
    click(&mut harness, Role::Button, "Apply");
    harness.get_by_label("scale: 9 is outside 0.5 to 4");
    assert_eq!(read(&path), "");
}

#[test]
fn a_file_change_to_the_outputs_drops_what_is_staged() {
    let scratch = Scratch::new("outputs-rebase");
    let (path, mut harness) = outputs(&scratch, "");
    click(&mut harness, Role::CheckBox, "eDP-1: on");
    harness.input_mut().focused = false;
    harness.run();
    fs::write(&path, "[[output]]\nname = \"DP-1\"\nscale = 2\n").unwrap();
    harness.input_mut().focused = true;
    harness.run();
    assert!(
        harness
            .query_by_label("Changed here; nothing is written until Apply.")
            .is_none()
    );
    let apply = harness.get_by_role_and_label(Role::Button, "Apply");
    assert!(apply.accesskit_node().is_disabled());
}

#[test]
fn a_list_the_session_withdraws_leaves_the_files_rules_typed() {
    let scratch = Scratch::new("outputs-withdrawn");
    let path = scratch.config("[[output]]\nname = \"eDP-1\"\nscale = 2\n");
    let said = vec![Seen::Monitors(desk()), Seen::Withdrawn];
    let mut harness = window_on(&path, takes(), Some(&Compositor::default()), None, Ok(said));
    click(&mut harness, Role::Button, "Outputs");
    harness.run_steps(2);
    harness.get_by_label(WITHDRAWN);
    assert!(harness.query_by_label("DP-1 — Dell U2720Q").is_none());
    harness.get_by_role_and_label(Role::TextInput, "eDP-1 mode");
}
