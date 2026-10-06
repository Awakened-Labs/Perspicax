//! The settings portal's backend: what applications are told about the
//! theme.
//!
//! An application asks xdg-desktop-portal, on the session bus, whether to be
//! dark or light and what accent to use; xdg-desktop-portal asks each backend
//! that `perspicax-portals.conf` names for this desktop. GTK 4 and libadwaita,
//! Qt 6 with a platform theme that reads the portal, Firefox and Chromium all
//! ask, and follow the answer as it changes.
//!
//! Served here, in the composition root, on the thread that already speaks to
//! the session bus, rather than by perspicax-shell: a session whose shell is
//! turned off still has a theme. It says only what the theme says. The
//! default theme says nothing, so an application decides as it did before
//! there were themes, and the next backend in the conf file is asked.

use std::collections::HashMap;

use perspicax_config::Appearance;
use zbus::{
    Connection,
    fdo::RequestNameFlags,
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Value},
};

/// The name xdg-desktop-portal finds this backend by, from `perspicax.portal`.
pub const NAME: &str = "org.freedesktop.impl.portal.desktop.perspicax";

/// Where every portal backend serves its interfaces.
pub const PATH: &str = "/org/freedesktop/portal/desktop";

/// The one namespace this backend answers for.
pub const APPEARANCE: &str = "org.freedesktop.appearance";

/// Every key in [`APPEARANCE`] this backend knows.
const KEYS: [&str; 3] = ["color-scheme", "accent-color", "contrast"];

/// `key`'s value as `appearance` says it, or `None` where it says nothing.
#[must_use]
pub fn setting(appearance: &Appearance, key: &str) -> Option<Value<'static>> {
    match key {
        "color-scheme" => appearance
            .color_scheme
            .map(|scheme| Value::from(scheme as u32)),
        "accent-color" => appearance.accent.map(|accent| {
            let channel = |value: u8| f64::from(value) / 255.0;
            Value::from((channel(accent.r), channel(accent.g), channel(accent.b)))
        }),
        "contrast" => appearance
            .contrast
            .map(|contrast| Value::from(contrast as u32)),
        _ => None,
    }
}

/// What a key says once `appearance` stops saying it: no preference, normal
/// contrast, and an accent out of range, which the portal's documentation
/// says to read as none.
fn unsaid(key: &str) -> Value<'static> {
    match key {
        "accent-color" => Value::from((-1.0_f64, -1.0_f64, -1.0_f64)),
        _ => Value::from(0_u32),
    }
}

/// Whether a `ReadAll` asking for `patterns` wants `namespace`: every
/// namespace for no patterns or an empty one, otherwise one named exactly or
/// matched by a trailing `.*`.
#[must_use]
pub fn wanted(namespace: &str, patterns: &[String]) -> bool {
    patterns.is_empty()
        || patterns.iter().any(|pattern| {
            pattern.is_empty()
                || pattern == namespace
                || pattern
                    .strip_suffix('*')
                    .is_some_and(|prefix| prefix.ends_with('.') && namespace.starts_with(prefix))
        })
}

/// The portal's own errors, under the name xdg-desktop-portal looks for to
/// ask the next backend instead.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotFound(String),
}

/// `org.freedesktop.impl.portal.Settings`, answering from what the theme
/// says now.
struct Settings {
    appearance: Appearance,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Settings")]
impl Settings {
    fn read_all(&self, namespaces: Vec<String>) -> HashMap<String, HashMap<String, OwnedValue>> {
        let mut all = HashMap::new();
        if !wanted(APPEARANCE, &namespaces) {
            return all;
        }
        let said: HashMap<String, OwnedValue> = KEYS
            .iter()
            .filter_map(|key| {
                let value = setting(&self.appearance, key)?;
                Some(((*key).to_owned(), OwnedValue::try_from(value).ok()?))
            })
            .collect();
        if !said.is_empty() {
            all.insert(APPEARANCE.to_owned(), said);
        }
        all
    }

    fn read(&self, namespace: &str, key: &str) -> Result<OwnedValue, Error> {
        let value = (namespace == APPEARANCE)
            .then(|| setting(&self.appearance, key))
            .flatten()
            .ok_or_else(|| Error::NotFound(format!("{namespace} {key} is not set here")))?;
        OwnedValue::try_from(value).map_err(|error| Error::ZBus(error.into()))
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }

    #[zbus(signal)]
    async fn setting_changed(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: Value<'_>,
    ) -> zbus::Result<()>;
}

/// The backend, served, and what it last said.
pub struct Portal {
    connection: Connection,
    said: Appearance,
}

impl Portal {
    /// Serve the backend on `connection` and take its name, from any other
    /// program holding it -- a session that left one behind on a shared bus
    /// -- and letting a later one take it in turn. `None`, after saying why,
    /// when it cannot be served.
    pub async fn serve(connection: &Connection) -> Option<Self> {
        let settings = Settings {
            appearance: Appearance::default(),
        };
        if let Err(error) = connection.object_server().at(PATH, settings).await {
            tracing::warn!(%error, "the settings portal could not be served");
            return None;
        }
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;
        if let Err(error) = connection.request_name_with_flags(NAME, flags).await {
            tracing::info!(%error, "another program serves this desktop's settings portal");
            return None;
        }
        Some(Self {
            connection: connection.clone(),
            said: Appearance::default(),
        })
    }

    /// Say `appearance` from now on, and tell whoever listens about each key
    /// that changed.
    pub async fn show(&mut self, appearance: Appearance) {
        if appearance == self.said {
            return;
        }
        let interface = match self
            .connection
            .object_server()
            .interface::<_, Settings>(PATH)
            .await
        {
            Ok(interface) => interface,
            Err(error) => {
                tracing::warn!(%error, "the settings portal is gone");
                return;
            }
        };
        interface.get_mut().await.appearance = appearance;
        for key in KEYS {
            let (before, now) = (setting(&self.said, key), setting(&appearance, key));
            if before == now {
                continue;
            }
            let value = now.unwrap_or_else(|| unsaid(key));
            if let Err(error) =
                Settings::setting_changed(interface.signal_emitter(), APPEARANCE, key, value).await
            {
                tracing::warn!(%error, key, "could not say a setting changed");
            }
        }
        self.said = appearance;
    }
}

#[cfg(test)]
mod tests {
    use perspicax_config::{Builtin, ColorScheme};

    use super::*;

    #[test]
    fn the_default_theme_says_nothing_and_a_breeze_says_its_scheme_and_accent() {
        let said = |appearance: &Appearance| {
            KEYS.iter()
                .filter(|key| setting(appearance, key).is_some())
                .count()
        };
        assert_eq!(said(&Builtin::Perspicax.appearance()), 0);

        let dark = Builtin::BreezeDark.appearance();
        assert_eq!(
            setting(&dark, "color-scheme"),
            Some(Value::from(ColorScheme::Dark as u32))
        );
        let Some(Value::Structure(accent)) = setting(&dark, "accent-color") else {
            panic!("the accent is a structure of three doubles");
        };
        assert_eq!(accent.fields().len(), 3);
        assert_eq!(setting(&dark, "contrast"), None);
        assert_eq!(setting(&dark, "font-name"), None);
    }

    #[test]
    fn read_all_globs_trailing_sections_only() {
        let all = |patterns: &[&str]| {
            let patterns: Vec<String> = patterns.iter().map(|&p| p.to_owned()).collect();
            wanted(APPEARANCE, &patterns)
        };
        assert!(all(&[]));
        assert!(all(&[""]));
        assert!(all(&["org.freedesktop.appearance"]));
        assert!(all(&["org.gnome.*", "org.freedesktop.*"]));
        assert!(!all(&["org.gnome.desktop.interface"]));
        assert!(!all(&["org.freedesktop.appear*"]));
    }
}
