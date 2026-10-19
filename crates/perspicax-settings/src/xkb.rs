//! The keyboard layouts xkb knows, by name and description, read from its
//! registry: the `.lst` file beside its rules, as `setxkbmap` reads it. A
//! layout is picked from these, so the Keyboard page never writes one xkb
//! does not know: the session only logs such a layout and keeps the old one.
//!
//! The `.lst` file lists xkb's main layouts and leaves out its extras, and a
//! person can keep layouts of their own in `~/.config/xkb`. So a layout
//! already written that the list lacks is kept as it is, and named as one
//! the list lacks, not refused.

use std::path::PathBuf;

use crate::{Edit, Key, Value};

/// Where xkb keeps its data, after `$XKB_CONFIG_ROOT`: xkeyboard-config 2's
/// own folder, then the X11 one older installs use.
const ROOTS: &[&str] = &["/usr/share/xkeyboard-config-2", "/usr/share/X11/xkb"];

/// The rules a keyboard with none written is compiled with.
const RULES: &str = "evdev";

/// A layout or a variant: its name, as the file writes it, and what xkb
/// calls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub description: String,
}

/// The layouts and variants xkb lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    layouts: Vec<Entry>,
    /// Each with the layout it is a variant of.
    variants: Vec<(String, Entry)>,
}

/// One of the keyboard's layouts: its name, and its variant, `""` for the
/// layout's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub name: String,
    pub variant: String,
}

impl Registry {
    /// The registry for `rules` (`""` for xkb's default), found where xkb
    /// looks: `$XKB_CONFIG_ROOT`, as `var` reads it, then the usual folders.
    /// `None` where there is none to read.
    pub fn find(var: impl Fn(&str) -> Option<String>, rules: &str) -> Option<Self> {
        let rules = if rules.is_empty() { RULES } else { rules };
        let roots = var("XKB_CONFIG_ROOT")
            .filter(|root| !root.is_empty())
            .into_iter()
            .chain(ROOTS.iter().map(|root| (*root).to_owned()));
        roots
            .map(|root| {
                PathBuf::from(root)
                    .join("rules")
                    .join(format!("{rules}.lst"))
            })
            .find_map(|path| std::fs::read_to_string(path).ok())
            .map(|text| Self::read(&text))
            .filter(|registry| !registry.layouts.is_empty())
    }

    /// A `.lst` file's `! layout` and `! variant` sections: a name, spaces,
    /// and a description, which for a variant begins with its layout's name
    /// and a colon.
    #[must_use]
    pub fn read(text: &str) -> Self {
        let mut registry = Self::default();
        let mut section = "";
        for line in text.lines() {
            if let Some(name) = line.strip_prefix('!') {
                section = name.trim();
                continue;
            }
            let Some((name, description)) = line.trim().split_once(char::is_whitespace) else {
                continue;
            };
            let description = description.trim();
            match section {
                "layout" => registry.layouts.push(Entry {
                    name: name.to_owned(),
                    description: description.to_owned(),
                }),
                "variant" => {
                    if let Some((layout, description)) = description.split_once(':') {
                        registry.variants.push((
                            layout.trim().to_owned(),
                            Entry {
                                name: name.to_owned(),
                                description: description.trim().to_owned(),
                            },
                        ));
                    }
                }
                _ => {}
            }
        }
        registry
    }

    /// Every layout, in xkb's order.
    #[must_use]
    pub fn layouts(&self) -> &[Entry] {
        &self.layouts
    }

    /// The layout called `name`, if xkb lists it.
    #[must_use]
    pub fn layout(&self, name: &str) -> Option<&Entry> {
        self.layouts.iter().find(|entry| entry.name == name)
    }

    /// `layout`'s variants, in xkb's order.
    pub fn variants<'a>(&'a self, layout: &'a str) -> impl Iterator<Item = &'a Entry> {
        self.variants
            .iter()
            .filter(move |(of, _)| of == layout)
            .map(|(_, entry)| entry)
    }

    /// `layout`'s variant called `name`, if xkb lists it.
    #[must_use]
    pub fn variant(&self, layout: &str, name: &str) -> Option<&Entry> {
        self.variants
            .iter()
            .find(|(of, entry)| of == layout && entry.name == name)
            .map(|(_, entry)| entry)
    }
}

/// The keyboard's layouts, as `[input.keyboard]` writes them: `layout` and
/// `variant` are lists, paired by position, and a variant list shorter than
/// its layouts leaves the rest their own.
#[must_use]
pub fn layouts(layout: &str, variant: &str) -> Vec<Layout> {
    if layout.trim().is_empty() {
        return Vec::new();
    }
    let mut variants = variant.split(',').map(str::trim);
    layout
        .split(',')
        .map(|name| Layout {
            name: name.trim().to_owned(),
            variant: variants.next().unwrap_or_default().to_owned(),
        })
        .collect()
}

/// The edits that write `layouts`: `layout` and `variant` set together, with
/// the variant list as short as it can be. No variants reset `variant`, and
/// no layouts reset `layout`, to xkb's default.
#[must_use]
pub fn edits(layouts: &[Layout]) -> Vec<Edit> {
    let key = |leaf| Key::new(&["input", "keyboard"], leaf);
    let joined = |layouts: &[Layout], part: fn(&Layout) -> &str| {
        layouts.iter().map(part).collect::<Vec<_>>().join(",")
    };
    // Up to the last layout with a variant of its own.
    let varied = layouts
        .iter()
        .rposition(|layout| !layout.variant.is_empty())
        .map_or(0, |last| last + 1);
    let names = joined(layouts, |layout| &layout.name);
    let variants = joined(&layouts[..varied], |layout| &layout.variant);
    let write = |leaf, text: String| {
        if text.is_empty() {
            Edit::Reset { key: key(leaf) }
        } else {
            Edit::Set {
                key: key(leaf),
                value: Value::String(text),
            }
        }
    };
    vec![write("layout", names), write("variant", variants)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slice of xkeyboard-config's `evdev.lst`, as it is laid out.
    const LST: &str = "! model\n  pc105           Generic 105-key PC\n\n! layout\n  \
                       us              English (US)\n  ru              Russian\n  \
                       de              German\n\n! variant\n  chr             us: Cherokee\n  \
                       phonetic        ru: Russian (phonetic)\n  \
                       ru              by: Russian (Belarus)\n\n! option\n  \
                       grp                  Switching to another layout\n";

    fn layout(name: &str, variant: &str) -> Layout {
        Layout {
            name: name.to_owned(),
            variant: variant.to_owned(),
        }
    }

    #[test]
    fn the_registry_lists_layouts_and_their_variants() {
        let registry = Registry::read(LST);
        let names: Vec<&str> = registry.layouts().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["us", "ru", "de"]);
        assert_eq!(registry.layout("ru").unwrap().description, "Russian");
        let ru: Vec<&str> = registry.variants("ru").map(|e| e.name.as_str()).collect();
        assert_eq!(ru, ["phonetic"], "by's `ru` is Belarus's, not Russia's");
        assert_eq!(
            registry.variant("ru", "phonetic").unwrap().description,
            "Russian (phonetic)"
        );
        assert!(registry.layout("pc105").is_none(), "a model is no layout");
        assert!(registry.layout("grp").is_none(), "nor is an option");
    }

    #[test]
    fn layouts_and_variants_pair_by_position_and_join_back() {
        let both = layouts("us, ru", ",phonetic");
        assert_eq!(both, [layout("us", ""), layout("ru", "phonetic")]);
        assert_eq!(
            layouts("us,ru,de", "intl"),
            [layout("us", "intl"), layout("ru", ""), layout("de", "")],
            "a short variant list leaves the rest their own"
        );
        assert_eq!(layouts("", ""), []);
        let set = |leaf: &str, text: &str| Edit::Set {
            key: Key::new(&["input", "keyboard"], leaf),
            value: Value::String(text.to_owned()),
        };
        let reset = |leaf: &str| Edit::Reset {
            key: Key::new(&["input", "keyboard"], leaf),
        };
        assert_eq!(
            edits(&both),
            [set("layout", "us,ru"), set("variant", ",phonetic")]
        );
        assert_eq!(
            edits(&[layout("ru", "phonetic"), layout("us", "")]),
            [set("layout", "ru,us"), set("variant", "phonetic")],
            "trailing layouts' own variants are left unsaid"
        );
        assert_eq!(
            edits(&[layout("us", ""), layout("ru", "")]),
            [set("layout", "us,ru"), reset("variant")]
        );
        assert_eq!(edits(&[]), [reset("layout"), reset("variant")]);
    }

    #[test]
    fn a_written_layout_the_registry_lacks_is_kept_and_named() {
        let registry = Registry::read(LST);
        let mine = layouts("us,dvorak-mine", "");
        assert!(registry.layout(&mine[1].name).is_none());
        assert_eq!(
            edits(&mine)[0],
            Edit::Set {
                key: Key::new(&["input", "keyboard"], "layout"),
                value: Value::String("us,dvorak-mine".to_owned()),
            },
            "written back as it was"
        );
    }

    #[test]
    fn the_registry_is_found_under_xkb_config_root() {
        let root = std::env::temp_dir().join(format!("perspicax-xkb-{}", std::process::id()));
        std::fs::create_dir_all(root.join("rules")).unwrap();
        std::fs::write(root.join("rules/evdev.lst"), LST).unwrap();
        let var = |name: &str| (name == "XKB_CONFIG_ROOT").then(|| root.display().to_string());
        let found = Registry::find(var, "");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(found, Some(Registry::read(LST)));
    }
}
