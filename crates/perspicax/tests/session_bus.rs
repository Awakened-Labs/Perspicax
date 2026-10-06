//! The session bus is told where the session's displays are (issue #27).
//!
//! Each test starts a `dbus-daemon` of its own, with a service directory the
//! test writes, so what D-Bus starts on request is a script the test can read
//! back rather than whatever this machine has installed. They are `#[ignore]`d
//! with the other live tests, since a bus daemon is a program the four gates do
//! not assume; `ci/live-tests.sh` runs them.
//!
//! One thing about dbus-daemon shapes them: a service that exits without
//! taking its name is not reported as gone (1.16.2 waits out its 25 second
//! start timeout), so nothing here waits for an activation's reply. A
//! program that cannot be run at all is reported at once.

use std::{
    io::BufRead as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use perspicax::bus::{self, Keyring, Options};
use perspicax_compositor::SessionFacts;
use zbus::Connection;

/// A bus of the test's own, starting only the services in its directory.
struct Bus {
    daemon: Child,
    address: String,
    dir: PathBuf,
}

impl Bus {
    /// A bus whose activatable services are `services`: each a name and the
    /// command D-Bus runs to start it.
    fn start(services: &[(&str, &str)]) -> Self {
        let dir = scratch("bus");
        std::fs::create_dir_all(dir.join("services")).expect("a directory for the bus");
        for (name, exec) in services {
            std::fs::write(
                dir.join("services").join(format!("{name}.service")),
                format!("[D-BUS Service]\nName={name}\nExec={exec}\n"),
            )
            .expect("a service file");
        }
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={dir}</listen>
  <servicedir>{services}</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                dir = dir.display(),
                services = dir.join("services").display(),
            ),
        )
        .expect("a bus configuration");

        let mut daemon = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon, which a session with a bus has");
        let mut address = String::new();
        std::io::BufReader::new(daemon.stdout.take().expect("its output"))
            .read_line(&mut address)
            .expect("its address");
        Self {
            daemon,
            address: address.trim().to_owned(),
            dir,
        }
    }

    fn connect(&self, runtime: &tokio::runtime::Runtime) -> Connection {
        runtime
            .block_on(async {
                zbus::connection::Builder::address(self.address.as_str())?
                    .build()
                    .await
            })
            .expect("a connection to the test's bus")
    }

    fn options(&self) -> Options {
        Options {
            address: Some(self.address.clone()),
            desktop: vec![
                ("XDG_CURRENT_DESKTOP".to_owned(), "perspicax".to_owned()),
                ("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned()),
            ],
            keyring_deadline: Duration::from_millis(300),
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.daemon.kill().ok();
        self.daemon.wait().ok();
        // File by file, then the two directories: nothing here deletes a
        // tree it did not just list.
        empty(&self.dir.join("services"));
        empty(&self.dir);
    }
}

/// Remove the files directly in `dir`, then `dir` itself if that left it empty.
fn empty(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| !kind.is_dir()) {
                std::fs::remove_file(entry.path()).ok();
            }
        }
    }
    std::fs::remove_dir(dir).ok();
}

/// A path in the temporary directory no other test or run will use.
fn scratch(what: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "perspicax-session-{what}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

/// Until `check` says yes or ten seconds pass.
fn eventually(mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// The case the issue could only find by reading: a program D-Bus starts --
/// the keyring's unlock prompt, a portal -- has to be given the displays and
/// the desktop's name, or it has nowhere to draw and the wrong portals. A
/// systemd user manager keeps an environment of its own for the services it
/// starts, which dbus-daemon does not pass on, so it is told too.
#[test]
#[ignore = "starts a dbus-daemon of its own"]
fn a_program_the_bus_starts_and_systemd_are_told_the_displays_and_the_desktop() {
    #[derive(Clone, Default)]
    struct Manager(Arc<Mutex<Vec<String>>>);

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl Manager {
        fn set_environment(&self, assignments: Vec<String>) {
            self.0
                .lock()
                .expect("the record of what was set")
                .extend(assignments);
        }
    }

    let written = scratch("environment");
    let bus = Bus::start(&[(
        "org.perspicax.Test.Environment",
        &format!("/bin/sh -c \"env > {}\"", written.display()),
    )]);
    let runtime = runtime();
    let manager = Manager::default();
    let _systemd = runtime
        .block_on(async {
            zbus::connection::Builder::address(bus.address.as_str())?
                .name("org.freedesktop.systemd1")?
                .serve_at("/org/freedesktop/systemd1", manager.clone())?
                .build()
                .await
        })
        .expect("a stand-in systemd served");

    let (tell, watch) = mpsc::channel();
    bus::start(bus.options(), watch);
    tell.send(SessionFacts {
        wayland_display: Some("wayland-test".to_owned()),
        x11_display: Some(7),
        ..SessionFacts::default()
    })
    .expect("the bus thread is listening");

    // systemd is told after the bus, so once it has the display the bus has
    // too. The stand-in only answers while its runtime is driven.
    let set = || manager.0.lock().expect("the record").clone();
    let told = eventually(|| {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
        set()
            .iter()
            .any(|set| set == "WAYLAND_DISPLAY=wayland-test")
    });
    assert!(told, "systemd was never told the display: {:?}", set());
    let set = set();
    for expected in [
        "DISPLAY=:7",
        "XDG_CURRENT_DESKTOP=perspicax",
        "XDG_SESSION_TYPE=wayland",
    ] {
        assert!(
            set.iter().any(|set| set == expected),
            "no {expected} in {set:?}"
        );
    }

    // Ask for the service as a program wanting it would, without waiting on
    // a reply the script never earns.
    let connection = bus.connect(&runtime);
    runtime.block_on(async {
        let _ = tokio::time::timeout(
            Duration::from_millis(200),
            connection.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "StartServiceByName",
                &("org.perspicax.Test.Environment", 0u32),
            ),
        )
        .await;
    });
    let environment = || std::fs::read_to_string(&written).unwrap_or_default();
    let started = eventually(|| environment().contains("WAYLAND_DISPLAY="));
    let environment = environment();
    std::fs::remove_file(&written).ok();
    assert!(started, "the bus never started the program");
    for expected in [
        "WAYLAND_DISPLAY=wayland-test",
        "DISPLAY=:7",
        "XDG_CURRENT_DESKTOP=perspicax",
        "XDG_SESSION_TYPE=wayland",
    ] {
        assert!(
            environment.lines().any(|line| line == expected),
            "the program the bus started had no {expected}:\n{environment}"
        );
    }
}

/// The settings portal says what the theme tells applications -- a Breeze's
/// scheme and accent -- answers "not found" for what it does not say, so
/// that xdg-desktop-portal asks the next backend, and says so when it
/// changes.
#[test]
#[ignore = "starts a dbus-daemon of its own"]
fn the_settings_portal_says_the_themes_look_and_when_it_changes() {
    use std::collections::HashMap;

    use futures_util::StreamExt as _;
    use perspicax::portal::{APPEARANCE, NAME, PATH};
    use perspicax_config::Builtin;
    use zbus::zvariant::OwnedValue;

    let bus = Bus::start(&[]);
    let (tell, watch) = mpsc::channel();
    bus::start(bus.options(), watch);
    let runtime = runtime();
    // In the runtime's context for the whole test: a deadline is made, and
    // the signal stream let go, outside `block_on`.
    let _inside = runtime.enter();
    let connection = bus.connect(&runtime);
    let settings = runtime
        .block_on(zbus::Proxy::new(
            &connection,
            NAME,
            PATH,
            "org.freedesktop.impl.portal.Settings",
        ))
        .expect("a proxy for the portal");
    let mut changes = runtime
        .block_on(settings.receive_signal("SettingChanged"))
        .expect("its changes");
    let read =
        |key: &str| runtime.block_on(settings.call::<_, _, OwnedValue>("Read", &(APPEARANCE, key)));
    let scheme = || {
        read("color-scheme")
            .ok()
            .and_then(|value| u32::try_from(value).ok())
    };

    // Each change, in the order said: a key and its value.
    let mut next_change = || {
        let changed = runtime
            .block_on(tokio::time::timeout(
                Duration::from_secs(10),
                changes.next(),
            ))
            .expect("a change said within ten seconds")
            .expect("the signal stream open");
        let (namespace, key, value): (String, String, OwnedValue) =
            changed.body().deserialize().expect("a change's arguments");
        assert_eq!(namespace, APPEARANCE);
        (key, value)
    };

    tell.send(SessionFacts {
        appearance: Builtin::BreezeDark.appearance(),
        ..SessionFacts::default()
    })
    .expect("the bus thread is listening");
    let (key, value) = next_change();
    assert_eq!(
        (key.as_str(), u32::try_from(value)),
        ("color-scheme", Ok(1)),
        "dark"
    );
    assert_eq!(next_change().0, "accent-color");
    assert_eq!(scheme(), Some(1));

    let unsaid = read("contrast").expect_err("contrast is not said");
    assert!(
        unsaid
            .to_string()
            .contains("org.freedesktop.portal.Error.NotFound"),
        "{unsaid}"
    );
    let all: HashMap<String, HashMap<String, OwnedValue>> = runtime
        .block_on(settings.call("ReadAll", &(vec!["org.freedesktop.*"],)))
        .expect("everything it says");
    let mut keys: Vec<&String> = all[APPEARANCE].keys().collect();
    keys.sort();
    assert_eq!(keys, ["accent-color", "color-scheme"]);

    // Light, with the same accent: one change, the scheme.
    tell.send(SessionFacts {
        appearance: Builtin::BreezeLight.appearance(),
        ..SessionFacts::default()
    })
    .expect("the bus thread is listening");
    let (key, value) = next_change();
    assert_eq!(
        (key.as_str(), u32::try_from(value)),
        ("color-scheme", Ok(2)),
        "light"
    );
    assert_eq!(scheme(), Some(2));
}

fn keyring(bus: &Bus) -> Keyring {
    let runtime = runtime();
    let connection = bus.connect(&runtime);
    runtime.block_on(bus::keyring(&connection, Duration::from_millis(300)))
}

/// Each way the Secret Service can answer, with a stand-in for it: none
/// installed, one that cannot be run, one that never takes its name -- as
/// another session's gnome-keyring does not, the case seen on 2026-10-05 --
/// and one already serving.
#[test]
#[ignore = "starts a dbus-daemon of its own"]
fn the_secret_service_is_told_apart_in_each_way_it_can_answer() {
    assert_eq!(keyring(&Bus::start(&[])), Keyring::NotInstalled);

    let missing = keyring(&Bus::start(&[(
        "org.freedesktop.secrets",
        "/nonexistent/perspicax-keyring",
    )]));
    assert!(
        matches!(&missing, Keyring::Failed(error) if error.contains("ExecFailed")),
        "{missing:?}"
    );

    let hangs = Bus::start(&[("org.freedesktop.secrets", "/bin/sleep 5")]);
    let started = Instant::now();
    assert_eq!(keyring(&hangs), Keyring::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the deadline was not kept: {:?}",
        started.elapsed()
    );

    let serving = Bus::start(&[]);
    let runtime = runtime();
    let _secrets = runtime
        .block_on(async {
            zbus::connection::Builder::address(serving.address.as_str())?
                .name("org.freedesktop.secrets")?
                .build()
                .await
        })
        .expect("a stand-in Secret Service");
    assert_eq!(keyring(&serving), Keyring::Running);
}

/// A session with no bus to reach still comes up: the session is the
/// person's windows, and a bus is a courtesy to the programs it starts.
#[test]
fn no_bus_to_tell_is_no_obstacle() {
    let (_tell, watch) = mpsc::channel();
    let started = Instant::now();
    bus::start(
        Options {
            address: Some("unix:path=/nonexistent/perspicax-bus".to_owned()),
            desktop: Vec::new(),
            keyring_deadline: Duration::from_millis(300),
        },
        watch,
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "waited {:?} on a bus that is not there",
        started.elapsed()
    );
}
