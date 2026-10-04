//! perspicax-shell, as an agent reads it: the real binary, hosted headless,
//! read off the accessibility bus and joined to its surfaces as any
//! application is -- and read again when it comes and goes. And as an agent
//! uses it: a right-click on the wallpaper by selector, and the menu that
//! opens read and clicked through to an application; the panel's start
//! button found by name, and the start menu it opens.
//!
//! Each test runs a compositor that keeps its desk current the way `--mcp`
//! does, and starts shells against its socket as a person's session would,
//! after the compositor is up. The shell's one desktop window is named
//! after its surface's layer namespace, and that is what joins the two.
//!
//! `#[ignore]`d: it needs a live accessibility bus, and a perspicax-shell
//! built with every component, named by `PERSPICAX_SHELL`.
//! `ci/live-tests.sh` builds one and exports it.

use std::{
    path::PathBuf,
    process::{Child, Command},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use perspicax::{desk::Desk, keep::keep_current, session};
use perspicax_compositor::{Backend, Config, Facts, Host, Requests, Stop};
use perspicax_index::{HostFacts, Index, Layer, PointerButton, Selector, SurfaceKind, Verb};
use perspicax_mcp::Desktop;
use perspicax_node::{NodeId, Origin, Role, SurfaceId};

/// The one monitor's desktop: the surface's namespace and its window's name.
const DESKTOP: &str = "perspicax-desktop-HEADLESS-1";
/// Its panel, and the menus' surface, likewise.
const PANEL: &str = "perspicax-panel-HEADLESS-1";
const MENUS: &str = "perspicax-menu-HEADLESS-1";

/// The profiles' shells: a wallpaper and a root menu, and a panel too.
const MINIMAL: &str = "profile = \"minimal\"\n";
const CLASSIC: &str = "profile = \"classic\"\n";

/// How long a newcomer draws before it is read. Short: the shell's tree is
/// whole before its first frame.
const SETTLE: Duration = Duration::from_secs(1);

/// How long anything here may take to happen.
const PATIENCE: Duration = Duration::from_secs(30);

#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn the_wallpapers_window_joins_its_background_surface_by_namespace() {
    host("joins", MINIMAL, |hosted| {
        let shell = hosted.shell();
        let (window, surface) = hosted.wait_until("the desktop window joined", |index, facts| {
            joined_desktop(index, facts, shell.id())
        });
        hosted.desk.read(&mut |index, facts| {
            let node = index.get(window).expect("the window");
            assert_eq!(node.node.role(), Role::Window);
            let facts = facts.surface(surface).expect("the surface");
            assert_eq!(
                node.bounds().map(|b| (b.x1 - b.x0, b.y1 - b.y0)),
                Some((
                    facts.geometry.x1 - facts.geometry.x0,
                    facts.geometry.y1 - facts.geometry.y0
                )),
                "the window is the size of the surface it is joined to"
            );
        });
    });
}

#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn an_application_started_after_the_first_read_is_read_and_joined() {
    host("late", MINIMAL, |hosted| {
        let first = hosted.shell();
        hosted.wait_until("the first shell read and joined", |index, facts| {
            joined_desktop(index, facts, first.id())
        });

        // Another program, started once the desk has been read: a second
        // shell, with a desktop of its own on the same monitor.
        let late = hosted.shell();
        let (_, surface) = hosted.wait_until("the late one read and joined", |index, facts| {
            joined_desktop(index, facts, late.id())
        });
        hosted.desk.read(&mut |index, facts| {
            let (_, first_surface) =
                joined_desktop(index, facts, first.id()).expect("the first still joined");
            assert_ne!(
                first_surface, surface,
                "each window joined to its own process's surface"
            );
        });
    });
}

#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn a_restarted_shell_is_read_again() {
    host("restart", MINIMAL, |hosted| {
        let mut shell = hosted.shell();
        let (window, _) = hosted.wait_until("the shell read and joined", |index, facts| {
            joined_desktop(index, facts, shell.id())
        });

        let gone = shell.id();
        shell.stop();
        hosted.wait_until("its tree forgotten", |index, _| {
            index.get(window).is_none().then_some(())
        });

        let again = hosted.shell();
        hosted.wait_until("the new one read and joined", |index, facts| {
            joined_desktop(index, facts, again.id())
        });
        hosted.desk.read(&mut |index, _| {
            assert!(
                !index.preorder().into_iter().any(|id| {
                    index.get(id).is_some_and(
                        |node| matches!(&node.origin, Origin::Process(p) if p.pid == gone),
                    )
                }),
                "nothing of the old one is left"
            );
        });
    });
}

#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn choosing_an_application_launches_it() {
    host("launch", MINIMAL, |hosted| {
        // One application installed, and nothing else: data folders of the
        // test's own, the system's left out.
        let base =
            std::env::temp_dir().join(format!("perspicax-shell-launch-{}", std::process::id()));
        let (data, none) = (base.join("data"), base.join("none"));
        let marker = base.join("launched");
        std::fs::create_dir_all(data.join("applications")).expect("a data folder");
        std::fs::create_dir_all(&none).expect("an empty one");
        std::fs::write(
            data.join("applications/marker.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=Marker\nExec=touch {}\nCategories=Utility;\n",
                marker.display()
            ),
        )
        .expect("a desktop entry");
        let shell = hosted.shell_with(&[
            ("XDG_DATA_HOME", data.as_os_str()),
            ("XDG_DATA_DIRS", none.as_os_str()),
        ]);
        hosted.wait_until("the desktop window joined", |index, facts| {
            joined_desktop(index, facts, shell.id())
        });

        hosted.click(&format!("window:{DESKTOP}"), PointerButton::Right);
        hosted.wait_until("the root menu read and joined", |index, facts| {
            menu_item(index, facts, "Accessories")
        });
        hosted.click("menuitem:Accessories", PointerButton::Left);
        hosted.wait_until("its submenu read", |index, facts| {
            menu_item(index, facts, "Marker")
        });
        hosted.click("menuitem:Marker", PointerButton::Left);

        let deadline = Instant::now() + PATIENCE;
        while !marker.exists() {
            assert!(Instant::now() < deadline, "the application started");
            thread::sleep(Duration::from_millis(100));
        }
        hosted.wait_until("the menu gone", |index, facts| {
            menu_item(index, facts, "Accessories")
                .is_none()
                .then_some(())
        });
        std::fs::remove_dir_all(&base).ok();
    });
}

#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn an_agent_finds_the_start_button_by_name_and_it_opens_the_start_menu() {
    host("start", CLASSIC, |hosted| {
        let _shell = hosted.shell();
        hosted.wait_until("the start button read and joined", |index, facts| {
            on_layer(index, facts, (Role::Button, "Start"), (Layer::Top, PANEL))
        });
        let clock = hosted.wait_until("the clock read and joined", |index, facts| {
            on_layer(index, facts, (Role::Status, "Clock"), (Layer::Top, PANEL))
        });
        hosted.desk.read(&mut |index, _| {
            let time = index
                .get(clock)
                .and_then(|node| node.node.description())
                .unwrap_or_default();
            assert!(
                time.contains(':'),
                "the time is read with the clock: {time:?}"
            );
        });

        hosted.click("button:Start", PointerButton::Left);
        hosted.wait_until("the start menu read and joined", |index, facts| {
            on_layer(
                index,
                facts,
                (Role::Menu, "Start menu"),
                (Layer::Overlay, MENUS),
            )
        });

        // The button is in reach with the menu open, and closes it.
        hosted.click("button:Start", PointerButton::Left);
        hosted.wait_until("the start menu gone", |index, facts| {
            on_layer(
                index,
                facts,
                (Role::Menu, "Start menu"),
                (Layer::Overlay, MENUS),
            )
            .is_none()
            .then_some(())
        });
    });
}

/// The menu item labelled `label`, if it is read and joined to the menus'
/// surface on the overlay layer.
fn menu_item(index: &Index, facts: &HostFacts, label: &str) -> Option<NodeId> {
    on_layer(
        index,
        facts,
        (Role::MenuItem, label),
        (Layer::Overlay, MENUS),
    )
}

/// The node of `role` labelled `label`, if it is read and joined to the
/// surface on `layer` named `namespace`.
fn on_layer(
    index: &Index,
    facts: &HostFacts,
    (role, label): (Role, &str),
    (layer, namespace): (Layer, &str),
) -> Option<NodeId> {
    index.preorder().into_iter().find(|&id| {
        index.get(id).is_some_and(|node| {
            node.node.role() == role
                && node.node.label() == Some(label)
                && node
                    .surface
                    .and_then(|surface| facts.surface(surface))
                    .is_some_and(|surface| {
                        matches!(
                            &surface.kind,
                            SurfaceKind::Layer { layer: on, namespace: named }
                                if *on == layer && named == namespace
                        )
                    })
        })
    })
}

/// The desktop window drawn by `pid`, if it is joined: its node, and the
/// surface it is joined to, which must be `pid`'s background surface of the
/// same name.
fn joined_desktop(index: &Index, facts: &HostFacts, pid: u32) -> Option<(NodeId, SurfaceId)> {
    index.preorder().into_iter().find_map(|id| {
        let node = index.get(id)?;
        if node.node.label() != Some(DESKTOP) {
            return None;
        }
        let surface = facts.surface(node.surface?)?;
        let background = matches!(
            &surface.kind,
            SurfaceKind::Layer { layer: Layer::Background, namespace } if namespace == DESKTOP
        );
        let drawn_by_pid = matches!(&node.origin, Origin::Process(process) if process.pid == pid);
        (background && drawn_by_pid && surface.claim().pid == Some(pid)).then_some((id, surface.id))
    })
}

/// A headless perspicax keeping its desk current, as `--mcp` does.
struct Hosted {
    desk: Arc<Desk>,
    socket: String,
    config: PathBuf,
}

impl Hosted {
    /// Start a shell against the compositor.
    fn shell(&self) -> Shell {
        self.shell_with(&[])
    }

    /// Start a shell against the compositor, with `env` set for it.
    fn shell_with(&self, env: &[(&str, &std::ffi::OsStr)]) -> Shell {
        let program = std::env::var_os("PERSPICAX_SHELL")
            .expect("PERSPICAX_SHELL names a perspicax-shell built with --features full");
        let child = Command::new(program)
            .arg("--config")
            .arg(&self.config)
            .env("WAYLAND_DISPLAY", &self.socket)
            .envs(env.iter().copied())
            .spawn()
            .expect("the shell starts");
        Shell(child)
    }

    /// Click what `selector` names with `button`, as an agent does.
    fn click(&self, selector: &str, button: PointerButton) {
        let selector = Selector::parse(selector).expect("a selector");
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.desk.act(&selector, &Verb::Click(button)) {
                Ok(_) => return,
                // Refused while the screen settles: a menu just drawn is
                // stale until it is read again.
                Err(refused) => {
                    assert!(
                        Instant::now() < deadline,
                        "{selector:?} clicked: {refused:?}"
                    );
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    /// Wait for `find` to find something on the desk.
    fn wait_until<T>(&self, what: &str, find: impl Fn(&Index, &HostFacts) -> Option<T>) -> T {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let mut found = None;
            self.desk
                .read(&mut |index, facts| found = find(index, facts));
            if let Some(found) = found {
                return found;
            }
            assert!(Instant::now() < deadline, "{what}, within {PATIENCE:?}");
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// A shell this test started, stopped when the test is done with it.
struct Shell(Child);

impl Shell {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run a compositor, and `body` against it on a thread of its own, with
/// shells reading `shell_config`; then stop it.
///
/// The compositor has the test's own thread, because Wayland state is not
/// `Send`. A panic in `body` stops it and is the test's failure.
fn host(name: &str, shell_config: &str, body: impl FnOnce(&Hosted) + Send + 'static) {
    let _registry = session::Registry::ensure().expect("an accessibility registry");
    let socket = format!("perspicax-shell-{name}-{}", std::process::id());
    let runtime_dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
    let config = std::env::temp_dir().join(format!("{socket}.toml"));
    std::fs::write(&config, shell_config).expect("a shell config");

    let (facts, stop, requests) = (Facts::new(), Stop::new(), Requests::new());
    let desk = Arc::new(Desk::new(&facts, &Host::new(&facts, &requests)));
    keep_current(Arc::clone(&desk), facts.clone(), stop.clone(), SETTLE);
    let hosted = Hosted {
        desk,
        socket: socket.clone(),
        config: config.clone(),
    };

    let worker = {
        let stop = stop.clone();
        let listening = runtime_dir.join(&socket);
        thread::spawn(move || {
            let _stop = StopOnDrop(stop);
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(perspicax_atspi::enable())
                .expect("accessibility turned on");
            let deadline = Instant::now() + PATIENCE;
            while !listening.exists() {
                assert!(Instant::now() < deadline, "the compositor's socket appears");
                thread::sleep(Duration::from_millis(20));
            }
            body(&hosted);
        })
    };

    let config_run = Config {
        backend: Backend::headless((1280, 800)),
        spawn: Vec::new(),
        env: session::accessibility_env(),
        run_for: Some(Duration::from_secs(120)),
        config: None,
        socket: Some(socket),
        xwayland: false,
    };
    perspicax_compositor::run(&config_run, &facts, &requests, &stop).expect("the compositor runs");
    std::fs::remove_file(&config).ok();
    if let Err(panic) = worker.join() {
        std::panic::resume_unwind(panic);
    }
}

/// Asks the compositor to stop when the test body ends, however it ends.
struct StopOnDrop(Stop);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.request();
    }
}
