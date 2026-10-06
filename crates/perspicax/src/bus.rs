//! Telling the session bus where this session's displays are (issue #27).
//!
//! A program this compositor starts is told `WAYLAND_DISPLAY` and `DISPLAY` in
//! its environment. A program D-Bus starts on somebody's request is not: it
//! gets the bus's *activation environment*, which is whatever the bus was
//! started with -- and a bus `dbus-run-session` started from a text console
//! was started with nothing graphical at all. gnome-keyring's unlock prompt, a
//! notification daemon and every portal are started that way, and each one
//! started without a display has nowhere to draw.
//!
//! `UpdateActivationEnvironment` is the bus's own way to be told, and what
//! `dbus-update-activation-environment` calls. A systemd user manager keeps an
//! environment of its own for the services it starts, and is told too when
//! there is one.
//!
//! The same thread asks, once, whether the Secret Service will start, because
//! when it will not the symptom turns up minutes later somewhere else entirely:
//! every keyring lookup in every program waits out D-Bus's 25 second timeout,
//! and says nothing about why. One line here saying so is the difference
//! between that and a program that looks as if it will not open.
//!
//! The settings portal's backend is served from here too, on the same
//! connection: see [`crate::portal`].
//!
//! All of it on a thread of its own with its own runtime, as the agent
//! interface has, and for the reason the facts boundary exists: the
//! compositor's thread must never wait on D-Bus, and this one spends its life
//! doing exactly that.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use perspicax_compositor::SessionFacts;
use tracing::Level;
use zbus::{Connection, fdo::DBusProxy, names::WellKnownName};

/// The Secret Service's name on the bus: gnome-keyring's, KWallet's, or
/// whichever program a person keeps their secrets in.
const SECRETS: &str = "org.freedesktop.secrets";

/// How long the Secret Service has to start before a warning says it did not.
///
/// gnome-keyring is up in a few milliseconds. Three seconds is enough for a
/// slow disk and short enough that the warning lands near the start of the log
/// rather than after the person has given up on whatever was waiting.
pub const KEYRING_DEADLINE: Duration = Duration::from_secs(3);

/// What to tell, and where.
#[derive(Debug, Clone)]
pub struct Options {
    /// The bus, by address. `None` is the session's own.
    pub address: Option<String>,
    /// What the session calls itself, told before the compositor exists, so
    /// that nothing it starts can ask for a portal before the bus knows which
    /// desktop's portals to give it.
    pub desktop: Vec<(String, String)>,
    /// How long the Secret Service has to start; [`KEYRING_DEADLINE`] but in
    /// a test.
    pub keyring_deadline: Duration,
}

impl Options {
    /// The session's own bus, told `desktop`.
    #[must_use]
    pub fn session(desktop: Vec<(String, String)>) -> Self {
        Self {
            address: None,
            desktop,
            keyring_deadline: KEYRING_DEADLINE,
        }
    }
}

/// Tell the bus what the session calls itself, then where its displays are
/// each time `changes` says, on a thread of its own.
///
/// Returns once the names are told, or after a second if the bus is slow to
/// answer: a session that cannot reach its bus still has windows to show.
/// With no bus at all this says so at debug level and does nothing else, which
/// is what a session started without one is owed.
pub fn start(options: Options, changes: mpsc::Receiver<SessionFacts>) {
    let (ready, told) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("session-bus".to_owned())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(tell(options, changes, ready)),
                Err(error) => {
                    tracing::warn!(%error, "no runtime to tell the session bus with");
                    let _ = ready.send(());
                }
            }
        });
    match spawned {
        Ok(_) => {
            let _ = told.recv_timeout(Duration::from_secs(1));
        }
        Err(error) => tracing::warn!(%error, "no thread to tell the session bus with"),
    }
}

/// The displays as the activation environment says them: nothing before
/// there is a socket, then `WAYLAND_DISPLAY`, and `DISPLAY`.
///
/// `DISPLAY` is said empty when there is no Xwayland, rather than left out.
/// A bus may be shared -- a systemd user manager's is one per user, not per
/// session -- and a value another session left there would send this session's
/// X11 programs to that one's screen.
#[must_use]
pub fn activation(facts: &SessionFacts) -> Vec<(&'static str, String)> {
    let Some(wayland) = &facts.wayland_display else {
        return Vec::new();
    };
    vec![
        ("WAYLAND_DISPLAY", wayland.clone()),
        (
            "DISPLAY",
            facts
                .x11_display
                .map(|display| format!(":{display}"))
                .unwrap_or_default(),
        ),
    ]
}

/// What became of the Secret Service when this session asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keyring {
    /// Something already serves it on this bus.
    Running,
    /// It started when asked.
    Started,
    /// Nothing on this machine says it can serve it.
    NotInstalled,
    /// It was started and went away without serving; D-Bus's own words.
    Failed(String),
    /// It was started and was not serving by the deadline.
    TimedOut,
}

/// Ask the bus whether the Secret Service is there, and to start it if it is
/// not, giving it `deadline` to come up.
///
/// Starting it is what any program wanting a secret would do first anyway, so
/// asking early costs a daemon this session would have had soon, and it hands
/// a keyring the login unlocked to this session's bus before anything waits on
/// it.
pub async fn keyring(connection: &Connection, deadline: Duration) -> Keyring {
    let bus = match DBusProxy::new(connection).await {
        Ok(bus) => bus,
        Err(error) => return Keyring::Failed(error.to_string()),
    };
    let name = WellKnownName::from_static_str_unchecked(SECRETS);
    if bus
        .name_has_owner(name.clone().into())
        .await
        .unwrap_or(false)
    {
        return Keyring::Running;
    }
    // A bus that will not say what it can start is asked anyway: the answer
    // to starting it says the same thing, and more.
    if let Ok(names) = bus.list_activatable_names().await
        && !names
            .iter()
            .any(|activatable| activatable.as_str() == SECRETS)
    {
        return Keyring::NotInstalled;
    }
    match tokio::time::timeout(deadline, bus.start_service_by_name(name, 0)).await {
        Ok(Ok(_)) => Keyring::Started,
        Ok(Err(error)) => Keyring::Failed(error.to_string()),
        Err(_) => Keyring::TimedOut,
    }
}

/// What to say about the Secret Service, and how loudly; `None` for nothing
/// worth a line.
///
/// `holder` is gnome-keyring's control socket, when one exists in this user's
/// runtime directory. It is shared by every session of the user, and its being
/// there while the service would not start on this bus is the one cause this
/// session can name: another session's gnome-keyring, serving that session's
/// bus and no other, took the request and kept it.
#[must_use]
pub fn keyring_advice(outcome: &Keyring, holder: Option<&Path>) -> Option<(Level, String)> {
    let what = match outcome {
        Keyring::Running => return None,
        Keyring::Started => {
            return Some((
                Level::INFO,
                "the Secret Service started on this session's bus".to_owned(),
            ));
        }
        Keyring::NotInstalled => {
            return Some((
                Level::INFO,
                "no Secret Service is installed: programs that keep secrets in a keyring \
                 will find none"
                    .to_owned(),
            ));
        }
        Keyring::Failed(error) => format!("it went away without serving ({error})"),
        Keyring::TimedOut => "it was not serving within the deadline".to_owned(),
    };
    let why = match holder {
        Some(holder) => format!(
            "another session of yours probably holds the keyring ({} exists), and \
             gnome-keyring serves one session's bus at a time. Programs here that keep \
             secrets in it will wait and then fail until that session is logged out",
            holder.display()
        ),
        None => "programs here that keep secrets in a keyring will find none".to_owned(),
    };
    Some((
        Level::WARN,
        format!("the Secret Service did not start on this session's bus: {what}; {why}"),
    ))
}

/// The thread's whole life: the names, then the displays as they change.
async fn tell(options: Options, changes: mpsc::Receiver<SessionFacts>, ready: mpsc::Sender<()>) {
    let connection = match connect(options.address.as_deref()).await {
        Ok(connection) => connection,
        Err(error) => {
            tracing::debug!(%error, "no session bus to tell where this session's displays are");
            let _ = ready.send(());
            return;
        }
    };
    let desktop: Vec<(&str, &str)> = options
        .desktop
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    publish(&connection, &desktop).await;
    // Before the compositor exists, so the backend is there to be found by
    // the first application that asks how to look.
    let mut portal = crate::portal::Portal::serve(&connection).await;
    let _ = ready.send(());

    // The facts arrive on a plain channel, from a thread that must never
    // wait. Read here on a blocking task and passed on, so that waiting for
    // the next one does not stop the keyring check from running meanwhile.
    let (forward, mut news) = tokio::sync::mpsc::unbounded_channel();
    tokio::task::spawn_blocking(move || {
        while let Ok(facts) = changes.recv() {
            if forward.send(facts).is_err() {
                break;
            }
        }
    });

    let mut checked = false;
    let mut published = Vec::new();
    while let Some(facts) = news.recv().await {
        if let Some(portal) = portal.as_mut() {
            portal.show(facts.appearance).await;
        }
        let displays = activation(&facts);
        if displays.is_empty() || displays == published {
            continue;
        }
        let pairs: Vec<(&str, &str)> = displays
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        publish(&connection, &pairs).await;
        published = displays;
        // Once there is a display to publish, so a keyring that has to ask
        // for its password has somewhere to ask.
        if !checked {
            checked = true;
            tokio::spawn(report_keyring(connection.clone(), options.keyring_deadline));
        }
    }
}

async fn connect(address: Option<&str>) -> zbus::Result<Connection> {
    match address {
        Some(address) => zbus::connection::Builder::address(address)?.build().await,
        None => Connection::session().await,
    }
}

/// Tell the bus, and a systemd user manager when there is one, to give `vars`
/// to every program they start from now on.
async fn publish(connection: &Connection, vars: &[(&str, &str)]) {
    let bus = match DBusProxy::new(connection).await {
        Ok(bus) => bus,
        Err(error) => {
            tracing::warn!(%error, "could not tell the session bus where this session is");
            return;
        }
    };
    let environment: HashMap<&str, &str> = vars.iter().copied().collect();
    match bus.update_activation_environment(environment).await {
        Ok(()) => tracing::debug!(?vars, "told the session bus"),
        Err(error) => {
            tracing::warn!(%error, ?vars, "the session bus would not take this session's environment");
        }
    }

    // Under systemd the services D-Bus starts are often systemd's to start,
    // with systemd's environment. dbus-broker passes the update on by itself
    // and dbus-daemon does not, so it is told directly, as
    // `dbus-update-activation-environment --systemd` does.
    let systemd = WellKnownName::from_static_str_unchecked("org.freedesktop.systemd1");
    if !bus.name_has_owner(systemd.into()).await.unwrap_or(false) {
        return;
    }
    let assignments: Vec<String> = vars
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if let Err(error) = connection
        .call_method(
            Some("org.freedesktop.systemd1"),
            "/org/freedesktop/systemd1",
            Some("org.freedesktop.systemd1.Manager"),
            "SetEnvironment",
            &assignments,
        )
        .await
    {
        tracing::warn!(%error, ?vars, "the systemd user manager would not take this session's environment");
    }
}

async fn report_keyring(connection: Connection, deadline: Duration) {
    let outcome = keyring(&connection, deadline).await;
    let holder = std::env::var_os("XDG_RUNTIME_DIR")
        .map(|runtime| PathBuf::from(runtime).join("keyring/control"))
        .filter(|control| control.exists());
    match keyring_advice(&outcome, holder.as_deref()) {
        Some((Level::WARN, advice)) => tracing::warn!("{advice}"),
        Some((_, advice)) => tracing::info!("{advice}"),
        None => tracing::debug!("the Secret Service is already serving this session's bus"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_published_before_there_is_a_socket() {
        assert!(activation(&SessionFacts::default()).is_empty());
    }

    #[test]
    fn display_is_said_empty_without_an_xwayland_and_numbered_with_one() {
        let mut facts = SessionFacts {
            wayland_display: Some("wayland-1".to_owned()),
            ..SessionFacts::default()
        };
        assert_eq!(
            activation(&facts),
            [
                ("WAYLAND_DISPLAY", "wayland-1".to_owned()),
                ("DISPLAY", String::new()),
            ]
        );

        facts.x11_display = Some(2);
        assert_eq!(activation(&facts)[1], ("DISPLAY", ":2".to_owned()));
    }

    #[test]
    fn a_keyring_already_serving_or_started_is_not_a_warning() {
        assert_eq!(keyring_advice(&Keyring::Running, None), None);
        assert!(matches!(
            keyring_advice(&Keyring::Started, None),
            Some((Level::INFO, _))
        ));
        assert!(matches!(
            keyring_advice(&Keyring::NotInstalled, None),
            Some((Level::INFO, _))
        ));
    }

    /// The case seen on 2026-10-05: another session's gnome-keyring took the
    /// request. The warning has to name the socket, since that is the one
    /// thing a person can check and the one cause this session can know.
    #[test]
    fn a_keyring_that_will_not_start_beside_another_sessions_names_its_socket() {
        let holder = Path::new("/run/user/1000/keyring/control");
        let Some((level, advice)) = keyring_advice(&Keyring::TimedOut, Some(holder)) else {
            panic!("a keyring that did not start is worth a line");
        };
        assert_eq!(level, Level::WARN);
        assert!(
            advice.contains("/run/user/1000/keyring/control"),
            "{advice}"
        );
        assert!(advice.contains("another session"), "{advice}");

        let Some((level, advice)) =
            keyring_advice(&Keyring::Failed("ChildExited".to_owned()), None)
        else {
            panic!("a keyring that went away is worth a line");
        };
        assert_eq!(level, Level::WARN);
        assert!(advice.contains("ChildExited"), "{advice}");
        assert!(!advice.contains("another session"), "{advice}");
    }
}
