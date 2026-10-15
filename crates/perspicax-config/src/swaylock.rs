//! swaylock, dressed in the theme.
//!
//! swaylock reads one config file of its own: the one `-C` names, or else
//! the first of `~/.swaylock/config`, `$XDG_CONFIG_HOME/swaylock/config` and
//! `/etc/swaylock/config`. Then it reads its flags, in order, a later one
//! winning over an earlier. So perspicax hands it `-C /dev/null`, which is no
//! file at all, then the theme as flags, then whatever the person wrote: the
//! look is config.toml's alone, and a flag the person wrote still wins over
//! it. A person who names a file of their own with `-C` gets swaylock as they
//! wrote it, and so does any lock program that is not swaylock.
//!
//! Inside a perspicax session, perspicax draws the lock screen itself,
//! beneath swaylock: the `lock` colour and the logo. swaylock is then given
//! no colour for the screen, so its surface is clear but for its ring and
//! its words, and perspicax's screen shows through. Outside one, whatever
//! another compositor shows would show through instead, so there swaylock
//! paints the screen in `lock` itself, as it always has.
//!
//! An unknown flag makes swaylock print its usage and not lock at all, the
//! worst way a lock can fail: the screen stays open while its owner walks
//! away. So only the flags in [`PAINT`] and `--font` are written, every one
//! of which swaylock has known since 1.5.

use std::path::Path;

use perspicax_policy::{Family, Font, Palette, Rgba, Role};

/// The file swaylock is given in place of its own: none.
const NO_FILE: &str = "/dev/null";

/// swaylock's short options that take a value, as its getopt string has
/// them: in a bundle such as `-fC`, the rest of the word after one is its
/// value, and with nothing after it the next word is.
const VALUED: &[char] = &['c', 'i', 's', 'C', 'R'];

/// The flag for the colour of swaylock's whole screen, behind its ring.
const SCREEN: &str = "--color";

/// What each of swaylock's colours is painted, in the order written: a role,
/// or `None` for no colour at all, so the lock screen shows through. The
/// screen's own, [`SCREEN`], is painted `lock` only where nothing is drawn
/// beneath swaylock (see [`dressed`]).
const PAINT: &[(&str, Option<Role>)] = {
    use Role::*;
    &[
        (SCREEN, Some(Lock)),
        ("--inside-color", None),
        ("--inside-clear-color", None),
        ("--inside-caps-lock-color", None),
        ("--inside-ver-color", None),
        ("--inside-wrong-color", None),
        ("--ring-color", Some(LockRing)),
        ("--ring-clear-color", Some(LockRing)),
        ("--ring-caps-lock-color", Some(LockRing)),
        ("--ring-ver-color", Some(LockAccent)),
        ("--ring-wrong-color", Some(Alert)),
        ("--key-hl-color", Some(LockAccent)),
        ("--caps-lock-key-hl-color", Some(LockAccent)),
        ("--bs-hl-color", Some(Alert)),
        ("--caps-lock-bs-hl-color", Some(Alert)),
        ("--line-color", None),
        ("--line-clear-color", None),
        ("--line-caps-lock-color", None),
        ("--line-ver-color", None),
        ("--line-wrong-color", None),
        ("--separator-color", None),
        ("--text-color", Some(LockInk)),
        ("--text-clear-color", Some(LockInk)),
        ("--text-caps-lock-color", Some(LockInk)),
        ("--text-ver-color", Some(LockInk)),
        ("--text-wrong-color", Some(LockInk)),
        ("--layout-bg-color", None),
        ("--layout-border-color", None),
        ("--layout-text-color", Some(LockInk)),
    ]
};

/// `command`, dressed in `palette` and `font` when it is swaylock with no
/// config file named: `-C /dev/null`, the theme, then its own arguments as
/// written. Anything else comes back as it was.
///
/// `beneath` is whether perspicax draws the lock screen beneath it, as it
/// does inside its own session. Then the screen is left clear, and only the
/// ring and the words are swaylock's to paint.
pub(crate) fn dressed(
    command: Vec<String>,
    palette: &Palette,
    font: &Font,
    beneath: bool,
) -> Vec<String> {
    let Some((program, arguments)) = command.split_first() else {
        return command;
    };
    if !is_swaylock(program) || names_config(arguments) {
        return command;
    }
    let theme = PAINT
        .iter()
        .map(|&(flag, paint)| match flag {
            SCREEN if beneath => (flag, None),
            _ => (flag, paint),
        })
        .flat_map(|(flag, paint)| [flag.to_owned(), hex(paint, palette)])
        .chain(["--font".to_owned(), family(&font.family).to_owned()]);
    [program.clone(), "-C".to_owned(), NO_FILE.to_owned()]
        .into_iter()
        .chain(theme)
        .chain(arguments.iter().cloned())
        .collect()
}

/// Whether `program` is swaylock, by name or by path.
fn is_swaylock(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .is_some_and(|name| name == "swaylock")
}

/// Whether `arguments` name a config file, as swaylock's getopt reads them:
/// `--config`, `--config=…`, or a `C` among a bundle of short options before
/// any that takes a value.
fn names_config(arguments: &[String]) -> bool {
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            return false;
        }
        if word == "--config" || word.starts_with("--config=") {
            return true;
        }
        let Some(bundle) = word.strip_prefix('-').filter(|rest| !rest.starts_with('-')) else {
            continue;
        };
        for (at, letter) in bundle.char_indices() {
            if letter == 'C' {
                return true;
            }
            if VALUED.contains(&letter) {
                if bundle[at + letter.len_utf8()..].is_empty() {
                    words.next();
                }
                break;
            }
        }
    }
    false
}

/// A colour as swaylock writes one: `rrggbbaa`.
fn hex(paint: Option<Role>, palette: &Palette) -> String {
    let Rgba { r, g, b, a } = paint.map_or(Rgba::new(0, 0, 0, 0), |role| palette[role]);
    format!("{r:02x}{g:02x}{b:02x}{a:02x}")
}

/// The family as fontconfig names it, which is how swaylock asks for one.
fn family(family: &Family) -> &str {
    match family {
        Family::SansSerif => "sans-serif",
        Family::Serif => "serif",
        Family::Monospace => "monospace",
        Family::Named(name) => name,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use perspicax_policy::Builtin;

    use super::*;

    /// Every long option swaylock 1.5 knows (its `long_options`), as long
    /// as no release since has dropped one. A flag outside it could make
    /// some installed swaylock refuse to lock.
    const SWAYLOCK_1_5: &[&str] = &[
        "config",
        "color",
        "debug",
        "ignore-empty-password",
        "daemonize",
        "help",
        "image",
        "disable-caps-lock-text",
        "indicator-caps-lock",
        "line-uses-inside",
        "line-uses-ring",
        "scaling",
        "tiling",
        "no-unlock-indicator",
        "show-keyboard-layout",
        "hide-keyboard-layout",
        "show-failed-attempts",
        "version",
        "bs-hl-color",
        "caps-lock-bs-hl-color",
        "caps-lock-key-hl-color",
        "font",
        "font-size",
        "indicator-idle-visible",
        "indicator-radius",
        "indicator-thickness",
        "indicator-x-position",
        "indicator-y-position",
        "inside-color",
        "inside-clear-color",
        "inside-caps-lock-color",
        "inside-ver-color",
        "inside-wrong-color",
        "key-hl-color",
        "layout-bg-color",
        "layout-border-color",
        "layout-text-color",
        "line-color",
        "line-clear-color",
        "line-caps-lock-color",
        "line-ver-color",
        "line-wrong-color",
        "ring-color",
        "ring-clear-color",
        "ring-caps-lock-color",
        "ring-ver-color",
        "ring-wrong-color",
        "separator-color",
        "text-color",
        "text-clear-color",
        "text-caps-lock-color",
        "text-ver-color",
        "text-wrong-color",
    ];

    fn words(written: &[&str]) -> Vec<String> {
        written.iter().map(|&word| word.to_owned()).collect()
    }

    /// `written`, dressed as the session's menus dress it: with perspicax
    /// drawing the screen beneath.
    fn dress(written: &[&str]) -> Vec<String> {
        dressed(words(written), &Palette::default(), &Font::default(), true)
    }

    /// The value written after `flag`, the last time it is.
    fn value<'a>(command: &'a [String], flag: &str) -> Option<&'a str> {
        command
            .windows(2)
            .rev()
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
    }

    #[test]
    fn swaylock_reads_no_file_and_wears_the_theme() {
        let command = dress(&["swaylock"]);
        assert_eq!(command[..3], ["swaylock", "-C", "/dev/null"]);
        assert_eq!(
            value(&command, "--color"),
            Some("00000000"),
            "clear, over the screen perspicax draws"
        );
        assert_eq!(value(&command, "--ring-color"), Some("c06a3cff"));
        assert_eq!(value(&command, "--key-hl-color"), Some("3daee9ff"));
        assert_eq!(value(&command, "--ring-wrong-color"), Some("da4453ff"));
        assert_eq!(value(&command, "--text-color"), Some("ffffffff"));
        assert_eq!(value(&command, "--inside-color"), Some("00000000"));
        assert_eq!(value(&command, "--font"), Some("sans-serif"));
        assert_eq!(
            command.len(),
            3 + 2 * (PAINT.len() + 1),
            "nothing but the theme"
        );
    }

    #[test]
    fn with_nothing_drawn_beneath_swaylock_paints_the_screen_itself() {
        let alone = dressed(
            words(&["swaylock"]),
            &Palette::default(),
            &Font::default(),
            false,
        );
        assert_eq!(
            value(&alone, "--color"),
            Some("0c2440ff"),
            "the lock colour"
        );
        let over = dress(&["swaylock"]);
        let differ: Vec<_> = alone
            .iter()
            .zip(&over)
            .filter(|(alone, over)| alone != over)
            .collect();
        assert_eq!(
            differ,
            [(&"0c2440ff".to_owned(), &"00000000".to_owned())],
            "and nothing else is dressed differently"
        );
    }

    #[test]
    fn every_flag_written_is_one_every_swaylock_since_1_5_knows() {
        let command = dress(&["swaylock"]);
        let flags: Vec<&str> = command[3..].iter().step_by(2).map(String::as_str).collect();
        for flag in &flags {
            let name = flag.strip_prefix("--").expect("a long option");
            assert!(SWAYLOCK_1_5.contains(&name), "{flag}");
        }
        for behaviour in [
            "--daemonize",
            "--ignore-empty-password",
            "--show-failed-attempts",
        ] {
            assert!(!flags.contains(&behaviour), "{behaviour} is the person's");
        }
    }

    #[test]
    fn what_the_person_wrote_comes_after_the_theme_and_wins() {
        let command = dress(&["swaylock", "-f", "--color", "ff0000"]);
        assert_eq!(command[command.len() - 3..], ["-f", "--color", "ff0000"]);
        assert_eq!(value(&command, "--color"), Some("ff0000"), "the last wins");
    }

    #[test]
    fn a_written_palette_and_font_reach_the_lock_screen() {
        let palette = Palette::default().written_over(&BTreeMap::from([
            (Role::Accent, Rgba::new(0xe9, 0x3d, 0x5a, 0xff)),
            (Role::Lock, Rgba::new(0x20, 0x20, 0x20, 0xff)),
        ]));
        let font = Font {
            family: Family::Named("Noto Sans".to_owned()),
            ..Font::default()
        };
        let command = dressed(words(&["swaylock"]), &palette, &font, false);
        assert_eq!(
            value(&command, "--key-hl-color"),
            Some("e93d5aff"),
            "the lock's accent is the accent, unwritten"
        );
        assert_eq!(value(&command, "--color"), Some("202020ff"));
        assert_eq!(value(&command, "--font"), Some("Noto Sans"));

        let light = dressed(
            words(&["swaylock"]),
            &Builtin::BreezeLight.palette(),
            &Font::default(),
            false,
        );
        assert_eq!(value(&light, "--color"), Some("cbd8dfff"));
        assert_eq!(value(&light, "--text-color"), Some("000000ff"));
    }

    #[test]
    fn the_locks_own_accent_lights_the_keys_and_the_check() {
        let palette = Palette::default().written_over(&BTreeMap::from([
            (Role::Accent, Rgba::new(0xe9, 0x3d, 0x5a, 0xff)),
            (Role::LockAccent, Rgba::new(0x7b, 0xae, 0xcd, 0xff)),
        ]));
        let command = dressed(words(&["swaylock"]), &palette, &Font::default(), true);
        for flag in [
            "--key-hl-color",
            "--caps-lock-key-hl-color",
            "--ring-ver-color",
        ] {
            assert_eq!(value(&command, flag), Some("7baecdff"), "{flag}");
        }
        assert_eq!(value(&command, "--ring-color"), Some("c06a3cff"));
    }

    #[test]
    fn swaylock_by_its_path_is_dressed_too() {
        assert_eq!(dress(&["/usr/bin/swaylock"])[1..3], ["-C", "/dev/null"]);
    }

    #[test]
    fn a_config_file_named_or_another_program_is_left_as_written() {
        for written in [
            &["swaylock", "-C", "/home/me/swaylock"][..],
            &["swaylock", "-C/home/me/swaylock"],
            &["swaylock", "-fC", "/home/me/swaylock"],
            &["swaylock", "--config", "/home/me/swaylock"],
            &["swaylock", "-f", "--config=/home/me/swaylock"],
            &["gtklock"],
            &["sh", "-c", "swaylock"],
            &["swaylock-effects"],
            &[],
        ] {
            assert_eq!(dress(written), words(written), "{written:?}");
        }
    }

    #[test]
    fn a_c_in_a_value_names_no_config_file() {
        for written in [
            &["swaylock", "-cC0C0C0"][..],
            &["swaylock", "-c", "-C0C0C0"],
            &["swaylock", "-i", "/home/me/Copy.png"],
            &["swaylock", "--", "-C"],
        ] {
            assert_eq!(dress(written)[1..3], ["-C", "/dev/null"], "{written:?}");
        }
    }
}
