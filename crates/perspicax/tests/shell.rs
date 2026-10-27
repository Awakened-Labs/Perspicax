//! perspicax-shell, as an agent reads it: the real binary, hosted headless,
//! read off the accessibility bus and joined to its surfaces as any
//! application is -- and read again when it comes and goes. And as an agent
//! uses it: a right-click on the wallpaper by selector, and the menu that
//! opens read and clicked through to an application; the panel's start
//! button found by name, and the start menu it opens; a window found on the
//! taskbar by its title, and brought forward by a click on its tab; a
//! window that opened without the keys, with a person at the seat, said on
//! its tab to want attention until it is brought forward (#259); the
//! desktop folder's icons read as a list, and one selected by a click.
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
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use perspicax::{desk::Desk, keep::keep_current, session};
use perspicax_compositor::{Backend, Config, Facts, Host, Requests, Stop};
use perspicax_index::{
    HostFacts, Index, Layer, PointerButton, Refusal, Selector, SurfaceKind, Verb,
};
use perspicax_mcp::{Denied, Desktop};
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
                node.node_space_bounds().map(|b| (b.x1 - b.x0, b.y1 - b.y0)),
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

        // With the menu open, the button is covered to an agent: the menus'
        // surface is drawn over the whole monitor, the panel included, and a
        // surface covers everything it draws (#103). A person's click still
        // reaches it, because the shell leaves its panel out of where that
        // surface takes clicks; #146 would leave the panel undrawn over, and
        // in reach of both.
        let selector = Selector::parse("button:Start").expect("a selector");
        let deadline = Instant::now() + PATIENCE;
        let by = loop {
            match hosted
                .desk
                .act(&selector, &Verb::Click(PointerButton::Left))
            {
                Err(Denied::Refused(Refusal::Occluded { by })) => break by,
                // Stale while the panel is read again after the menu opened.
                outcome => {
                    assert!(
                        Instant::now() < deadline,
                        "the start button with the menu open: {outcome:?}"
                    );
                    thread::sleep(Duration::from_millis(100));
                }
            }
        };
        hosted.wait_until("the menus' surface named as covering it", |_, facts| {
            facts
                .surface(by)
                .is_some_and(|surface| {
                    matches!(
                        &surface.kind,
                        SurfaceKind::Layer { layer: Layer::Overlay, namespace } if namespace == MENUS
                    )
                })
                .then_some(())
        });
    });
}

#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn the_taskbar_lists_a_window_by_title() {
    host("taskbar", CLASSIC, |hosted| {
        let _shell = hosted.shell();
        let _window = hosted.window("Field notes");
        hosted.wait_until(
            "the window's tab read, joined and selected",
            |index, facts| forward(index, facts, "Field notes"),
        );
    });
}

#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn an_agent_brings_a_window_forward_by_its_tab() {
    host("tabs", CLASSIC, |hosted| {
        let _shell = hosted.shell();
        let _first = hosted.window("First");
        hosted.wait_until("the first window forward", |index, facts| {
            forward(index, facts, "First")
        });
        let _second = hosted.window("Second");
        hosted.wait_until("the second window forward", |index, facts| {
            forward(index, facts, "Second")
        });

        hosted.click("tablist:Taskbar>tab:First", PointerButton::Left);
        hosted.wait_until("the first window forward again", |index, facts| {
            forward(index, facts, "First")
        });
    });
}

/// Issue #259: with a person at the seat, a window of another program that
/// opens while they are at one opens without the keys, and its tab says it
/// wants attention, to an agent as to a screen reader. Brought forward by a
/// click on its tab, it wants it no longer.
#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn a_window_that_opened_without_the_keys_says_so_on_its_tab_until_brought_forward() {
    let seated = Backend::headless((1280, 800)).with_person();
    host_on("wanting", seated, CLASSIC, |hosted| {
        let _shell = hosted.shell();
        let _first = hosted.window("First");
        hosted.wait_until("the first window forward", |index, facts| {
            forward(index, facts, "First")
        });
        let _second = hosted.window("Second");
        hosted.wait_until(
            "the second window's tab wanting attention",
            |index, facts| described(index, facts, "Second", Some("Wants attention")),
        );

        hosted.click("tablist:Taskbar>tab:Second", PointerButton::Left);
        hosted.wait_until(
            "the second window forward, wanting nothing",
            |index, facts| {
                forward(index, facts, "Second")
                    .and_then(|_| described(index, facts, "Second", None))
            },
        );
    });
}

/// And a window whose program asks to bring it forward with a token made
/// before the person moved on is refused, and says so on its tab: nothing
/// but that changes, for the panel to be drawn again by.
#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn a_window_refused_the_keys_it_asked_for_says_so_on_its_tab() {
    let seated = Backend::headless((1280, 800)).with_person();
    host_on("refused", seated, CLASSIC, |hosted| {
        let _shell = hosted.shell();
        let first = hosted.window_asking("First");
        hosted.wait_until(
            "the first window forward, its token made",
            |index, facts| forward(index, facts, "First").filter(|_| first.made()),
        );
        let _second = hosted.window("Second");
        hosted.click("tablist:Taskbar>tab:Second", PointerButton::Left);
        hosted.wait_until("the second window forward", |index, facts| {
            forward(index, facts, "Second")
        });
        hosted.wait_until("the first window's tab wanting nothing", |index, facts| {
            described(index, facts, "First", None)
        });
        // Whatever the keys moving drew is drawn, and the panel is drawn
        // again as each minute turns: spent with time to spare in this one,
        // the mark is all there is to draw it again for while it is waited.
        thread::sleep(SETTLE);
        while seconds_into_the_minute() > 45 {
            thread::sleep(Duration::from_millis(200));
        }

        first.spend();
        hosted.wait_within(
            Duration::from_secs(10),
            "the first window's tab wanting attention",
            |index, facts| described(index, facts, "First", Some("Wants attention")),
        );
    });
}

#[test]
#[ignore = "needs a live accessibility bus, and PERSPICAX_SHELL"]
fn the_desktop_icons_are_a_list_an_agent_can_read() {
    host("icons", CLASSIC, |hosted| {
        // A desktop folder of the test's own, where user-dirs.dirs says it
        // is, holding an application's entry and a file.
        let base =
            std::env::temp_dir().join(format!("perspicax-shell-icons-{}", std::process::id()));
        let (config, desk) = (base.join("config"), base.join("desk"));
        std::fs::create_dir_all(&config).expect("a config folder");
        std::fs::create_dir_all(&desk).expect("a desktop folder");
        std::fs::write(
            config.join("user-dirs.dirs"),
            format!("XDG_DESKTOP_DIR=\"{}\"\n", desk.display()),
        )
        .expect("user-dirs.dirs");
        let entry = desk.join("notes.desktop");
        std::fs::write(
            &entry,
            "[Desktop Entry]\nType=Application\nName=Field notes\nExec=true\n",
        )
        .expect("a desktop entry");
        // One that may be run, as an application's on the desktop must be.
        std::fs::set_permissions(&entry, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("made runnable");
        std::fs::write(desk.join("plan.txt"), "").expect("a file");

        let _shell = hosted.shell_with(&[("XDG_CONFIG_HOME", config.as_os_str())]);
        for name in ["Field notes", "plan.txt"] {
            hosted.wait_until(&format!("{name}'s icon read and joined"), |index, facts| {
                icon(index, facts, name)
            });
        }

        // An agent's click selects one: there is no double-click to open it.
        hosted.click("list:Desktop>listitem:plan.txt", PointerButton::Left);
        hosted.wait_until("the icon selected", |index, facts| {
            icon(index, facts, "plan.txt").filter(|&item| {
                index
                    .get(item)
                    .is_some_and(|node| node.node.is_selected() == Some(true))
            })
        });

        // An entry saved to the folder later is shown once it is seen there:
        // as the file it is, while it may not be run, and as its
        // application once it may, though the folder itself is unchanged.
        let later = desk.join("tool.desktop");
        std::fs::write(
            &later,
            "[Desktop Entry]\nType=Application\nName=Tool\nExec=true\n",
        )
        .expect("another entry");
        hosted.wait_until("the new entry's icon read, as a file", |index, facts| {
            icon(index, facts, "tool.desktop")
        });
        std::fs::set_permissions(&later, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("made runnable");
        hosted.wait_until(
            "the entry read again, as its application",
            |index, facts| icon(index, facts, "Tool"),
        );
        std::fs::remove_dir_all(&base).ok();
    });
}

/// The desktop folder's icon named `name`, if it is read and joined to the
/// desktop on the background layer.
fn icon(index: &Index, facts: &HostFacts, name: &str) -> Option<NodeId> {
    on_layer(
        index,
        facts,
        (Role::ListItem, name),
        (Layer::Background, DESKTOP),
    )
}

/// The taskbar's tab for the window titled `title`, if it is read, joined
/// to the panel, and selected: its window is the one with the keyboard.
fn forward(index: &Index, facts: &HostFacts, title: &str) -> Option<NodeId> {
    on_layer(index, facts, (Role::Tab, title), (Layer::Top, PANEL)).filter(|&tab| {
        index
            .get(tab)
            .is_some_and(|node| node.node.is_selected() == Some(true))
    })
}

/// The taskbar's tab for the window titled `title`, if it is read, joined
/// to the panel, and described as `description` says.
fn described(
    index: &Index,
    facts: &HostFacts,
    title: &str,
    description: Option<&str>,
) -> Option<NodeId> {
    on_layer(index, facts, (Role::Tab, title), (Layer::Top, PANEL)).filter(|&tab| {
        index
            .get(tab)
            .is_some_and(|node| node.node.description() == description)
    })
}

/// How far into the minute the clock is: a panel's clock turns over with it.
fn seconds_into_the_minute() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_secs()
        % 60
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

    /// Open a window titled `title`, as an application of the test's own
    /// would, until what this returns is dropped.
    fn window(&self, title: &str) -> window::Opened {
        let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
        let stream =
            UnixStream::connect(runtime.join(&self.socket)).expect("the compositor's socket");
        window::open(stream, title)
    }

    /// A window titled `title` that asks to be brought forward: see
    /// `window::open_asking`.
    fn window_asking(&self, title: &str) -> window::Opened {
        let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
        let stream =
            UnixStream::connect(runtime.join(&self.socket)).expect("the compositor's socket");
        window::open_asking(stream, title)
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
        self.wait_within(PATIENCE, what, find)
    }

    /// The same, for no longer than `patience`.
    fn wait_within<T>(
        &self,
        patience: Duration,
        what: &str,
        find: impl Fn(&Index, &HostFacts) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + patience;
        loop {
            let mut found = None;
            self.desk
                .read(&mut |index, facts| found = find(index, facts));
            if let Some(found) = found {
                return found;
            }
            assert!(Instant::now() < deadline, "{what}, within {patience:?}");
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
    host_on(name, Backend::headless((1280, 800)), shell_config, body);
}

/// As [`host`], on `backend`: one with a person at it, say.
fn host_on(
    name: &str,
    backend: Backend,
    shell_config: &str,
    body: impl FnOnce(&Hosted) + Send + 'static,
) {
    let _registry = session::Registry::ensure(Path::new(env!("CARGO_BIN_EXE_perspicax")))
        .expect("an accessibility registry");
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
        backend,
        spawn: Vec::new(),
        env: session::accessibility_env(),
        run_for: Some(Duration::from_secs(120)),
        config: None,
        places: None,
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

/// A window of the test's own: titled, white, and open until dropped. As
/// small as a Wayland application can be, so a test of the taskbar needs no
/// toolkit installed.
mod window {
    use std::{
        os::unix::net::UnixStream,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    use smithay_client_toolkit::{
        compositor::{CompositorHandler, CompositorState},
        delegate_compositor, delegate_output, delegate_registry, delegate_shm, delegate_xdg_shell,
        delegate_xdg_window,
        output::{OutputHandler, OutputState},
        reexports::protocols::xdg::activation::v1::client::{
            xdg_activation_token_v1::{self, XdgActivationTokenV1},
            xdg_activation_v1::XdgActivationV1,
        },
        registry::{ProvidesRegistryState, RegistryState},
        registry_handlers,
        shell::{
            WaylandSurface,
            xdg::{
                XdgShell,
                window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            },
        },
        shm::{Shm, ShmHandler, slot::SlotPool},
    };
    use wayland_client::{
        Connection, Dispatch, QueueHandle,
        globals::registry_queue_init,
        protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
    };

    /// The size it draws itself, whatever it is offered.
    const SIZE: (i32, i32) = (320, 200);

    /// The window, open until this is dropped.
    pub(super) struct Opened {
        open: Arc<AtomicBool>,
        asking: Arc<Asking>,
        thread: Option<thread::JoinHandle<()>>,
    }

    /// What a window that asks to be brought forward has done, and is told
    /// to do: as an application does when a second copy of it is started, it
    /// makes a token as it takes the keys, and spends it later.
    #[derive(Default)]
    struct Asking {
        /// Its token is made, while it had the keys.
        made: AtomicBool,
        /// Spend it now.
        spend: AtomicBool,
    }

    impl Opened {
        /// Whether it has made its token, if it asks.
        pub(super) fn made(&self) -> bool {
            self.asking.made.load(Ordering::Relaxed)
        }

        /// Spend its token, asking to be brought forward.
        pub(super) fn spend(&self) {
            self.asking.spend.store(true, Ordering::Relaxed);
        }
    }

    impl Drop for Opened {
        fn drop(&mut self) {
            self.open.store(false, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Open a window titled `title` on the compositor at the end of `stream`.
    pub(super) fn open(stream: UnixStream, title: &str) -> Opened {
        open_as(stream, title, false)
    }

    /// The same, a window that asks to be brought forward: see [`Asking`].
    pub(super) fn open_asking(stream: UnixStream, title: &str) -> Opened {
        open_as(stream, title, true)
    }

    fn open_as(stream: UnixStream, title: &str, asks: bool) -> Opened {
        let open = Arc::new(AtomicBool::new(true));
        let asking = Arc::new(Asking::default());
        let title = title.to_owned();
        let thread = {
            let (open, asking) = (Arc::clone(&open), Arc::clone(&asking));
            thread::spawn(move || run(stream, &title, &open, asks.then_some(&*asking)))
        };
        Opened {
            open,
            asking,
            thread: Some(thread),
        }
    }

    fn run(stream: UnixStream, title: &str, open: &AtomicBool, asking: Option<&Asking>) {
        let connection = Connection::from_socket(stream).expect("a connection");
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).expect("the globals");
        let qh = queue.handle();
        let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
        let xdg = XdgShell::bind(&globals, &qh).expect("xdg_wm_base");
        let shm = Shm::bind(&globals, &qh).expect("wl_shm");
        let window =
            xdg.create_window(compositor.create_surface(&qh), WindowDecorations::None, &qh);
        window.set_title(title);
        window.set_app_id("perspicax-test");
        window.commit();
        // One that asks has a keyboard, for the serial it makes its token
        // with, and the activation global to make it with.
        let seat = asking.map(|_| {
            let seat = globals
                .bind::<wl_seat::WlSeat, _, _>(&qh, 1..=7, ())
                .expect("wl_seat");
            seat.get_keyboard(&qh, ());
            seat
        });
        let activation = asking.map(|_| {
            globals
                .bind::<XdgActivationV1, _, _>(&qh, 1..=1, ())
                .expect("xdg_activation_v1")
        });
        let mut client = Client {
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &qh),
            pool: SlotPool::new((SIZE.0 * SIZE.1 * 4) as usize, &shm).expect("memory"),
            shm,
            surface: window.wl_surface().clone(),
            seat,
            activation,
            token: None,
            asked: false,
        };
        // Until dropped, or until the compositor goes.
        let mut spent = false;
        while open.load(Ordering::Relaxed) && queue.roundtrip(&mut client).is_ok() {
            if let Some(asking) = asking {
                asking.made.store(client.token.is_some(), Ordering::Relaxed);
                if let (false, true, Some(token), Some(activation)) = (
                    spent,
                    asking.spend.load(Ordering::Relaxed),
                    &client.token,
                    &client.activation,
                ) {
                    activation.activate(token.clone(), &client.surface);
                    spent = true;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        drop(window);
        let _ = connection.flush();
    }

    struct Client {
        registry: RegistryState,
        outputs: OutputState,
        shm: Shm,
        pool: SlotPool,
        surface: wl_surface::WlSurface,
        /// For one that asks: see [`Asking`].
        seat: Option<wl_seat::WlSeat>,
        activation: Option<XdgActivationV1>,
        token: Option<String>,
        /// Whether it has asked for its token.
        asked: bool,
    }

    impl Dispatch<wl_seat::WlSeat, ()> for Client {
        fn event(
            _: &mut Self,
            _: &wl_seat::WlSeat,
            _: wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
        /// The keys came to its window: it makes its token, with that serial.
        fn event(
            client: &mut Self,
            _: &wl_keyboard::WlKeyboard,
            event: wl_keyboard::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            let wl_keyboard::Event::Enter {
                serial, surface, ..
            } = event
            else {
                return;
            };
            let (Some(seat), Some(activation)) = (&client.seat, &client.activation) else {
                return;
            };
            if client.asked || surface != client.surface {
                return;
            }
            let token = activation.get_activation_token(qh, ());
            token.set_serial(serial, seat);
            token.set_surface(&surface);
            token.commit();
            client.asked = true;
        }
    }

    impl Dispatch<XdgActivationV1, ()> for Client {
        fn event(
            _: &mut Self,
            _: &XdgActivationV1,
            _: <XdgActivationV1 as wayland_client::Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<XdgActivationTokenV1, ()> for Client {
        fn event(
            client: &mut Self,
            _: &XdgActivationTokenV1,
            event: xdg_activation_token_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_activation_token_v1::Event::Done { token } = event {
                client.token = Some(token);
            }
        }
    }

    impl WindowHandler for Client {
        fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {}

        fn configure(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            window: &Window,
            _: WindowConfigure,
            _: u32,
        ) {
            let (width, height) = SIZE;
            let (buffer, canvas) = self
                .pool
                .create_buffer(width, height, width * 4, wl_shm::Format::Argb8888)
                .expect("a buffer");
            canvas.fill(0xff);
            let surface = window.wl_surface();
            buffer.attach_to(surface).expect("attached");
            surface.damage_buffer(0, 0, width, height);
            window.commit();
        }
    }

    impl CompositorHandler for Client {
        fn scale_factor_changed(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &wl_surface::WlSurface,
            _: i32,
        ) {
        }

        fn transform_changed(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &wl_surface::WlSurface,
            _: wl_output::Transform,
        ) {
        }

        fn frame(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &wl_surface::WlSurface,
            _: u32,
        ) {
        }

        fn surface_enter(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &wl_surface::WlSurface,
            _: &wl_output::WlOutput,
        ) {
        }

        fn surface_leave(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &wl_surface::WlSurface,
            _: &wl_output::WlOutput,
        ) {
        }
    }

    impl ShmHandler for Client {
        fn shm_state(&mut self) -> &mut Shm {
            &mut self.shm
        }
    }

    impl OutputHandler for Client {
        fn output_state(&mut self) -> &mut OutputState {
            &mut self.outputs
        }

        fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

        fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        }

        fn output_destroyed(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: wl_output::WlOutput,
        ) {
        }
    }

    impl ProvidesRegistryState for Client {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry
        }

        registry_handlers![OutputState];
    }

    delegate_compositor!(Client);
    delegate_output!(Client);
    delegate_shm!(Client);
    delegate_xdg_shell!(Client);
    delegate_xdg_window!(Client);
    delegate_registry!(Client);
}
