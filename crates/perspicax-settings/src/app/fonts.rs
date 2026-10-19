//! The theme's font, found on the system as the shell finds it: fontdb, with
//! fonts.conf saying what sans-serif and the other generic names mean here.
//!
//! egui is given no fonts of its own, so these are all it has. With none
//! found, text is drawn as nothing, as the shell draws it then, and the app
//! says why on stderr.

use std::{collections::BTreeMap, sync::Arc};

use eframe::egui::{FontData, FontDefinitions, FontFamily, FontId, Style, TextStyle};
use perspicax_config::{Family, Font};

/// What egui draws in. Prose: the theme's family, then the system's
/// sans-serif. Code: the system's monospace, then the theme's family. Each
/// family found once, whichever list it is in.
pub(crate) fn definitions(font: &Font) -> FontDefinitions {
    let mut system = fontdb::Database::new();
    system.load_system_fonts();
    let mut found = Found {
        system,
        definitions: FontDefinitions::empty(),
    };
    let prose = [font.family.clone(), Family::SansSerif];
    let code = [Family::Monospace, font.family.clone()];
    for (egui, families) in [
        (FontFamily::Proportional, &prose),
        (FontFamily::Monospace, &code),
    ] {
        let names: Vec<String> = families
            .iter()
            .filter_map(|family| found.face(family))
            .fold(Vec::new(), |mut names, name| {
                if !names.contains(&name) {
                    names.push(name);
                }
                names
            });
        found.definitions.families.insert(egui, names);
    }
    if found.definitions.font_data.is_empty() {
        eprintln!("perspicax-settings: no fonts are installed, so text is drawn as nothing");
    } else if let Family::Named(name) = &font.family
        && found.face(&font.family).is_none()
    {
        eprintln!(
            "perspicax-settings: no font of the theme's family `{name}` is installed; \
             writing in sans-serif"
        );
    }
    found.definitions
}

/// egui's text styles, at the theme's size: body text and buttons at it,
/// headings larger, small text smaller.
pub(crate) fn sized(style: &mut Style, size: u16) {
    let size = f32::from(size);
    let proportional = |scale: f32| FontId::new(size * scale, FontFamily::Proportional);
    style.text_styles = BTreeMap::from([
        (TextStyle::Small, proportional(0.8)),
        (TextStyle::Body, proportional(1.0)),
        (TextStyle::Button, proportional(1.0)),
        (TextStyle::Heading, proportional(1.4)),
        (
            TextStyle::Monospace,
            FontId::new(size, FontFamily::Monospace),
        ),
    ]);
}

struct Found {
    system: fontdb::Database,
    definitions: FontDefinitions,
}

impl Found {
    /// The regular face of `family`, read into the definitions under its
    /// own name; that name, or `None` where none is installed.
    fn face(&mut self, family: &Family) -> Option<String> {
        let generic = match family {
            Family::SansSerif => fontdb::Family::SansSerif,
            Family::Serif => fontdb::Family::Serif,
            Family::Monospace => fontdb::Family::Monospace,
            Family::Named(name) => fontdb::Family::Name(name),
        };
        let id = self.system.query(&fontdb::Query {
            families: &[generic],
            ..fontdb::Query::default()
        })?;
        let name = self.system.face(id)?.post_script_name.clone();
        if !self.definitions.font_data.contains_key(&name) {
            let data = self.system.with_face_data(id, |bytes, index| FontData {
                index,
                ..FontData::from_owned(bytes.to_vec())
            })?;
            self.definitions
                .font_data
                .insert(name.clone(), Arc::new(data));
        }
        Some(name)
    }
}
