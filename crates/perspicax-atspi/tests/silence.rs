//! What a read does with an application that stops answering (#51), and with
//! one that being asked freezes (#80).
//!
//! A Flutter Linux application embeds an `AtkPlug` in an `AtkSocket` of its
//! own process. at-spi2-core's bridge answers `GetExtents` on that plug by
//! asking the socket, over D-Bus and blocking -- which is asking itself, so
//! its main thread waits on a call only it could answer until libdbus gives
//! up, 25 s later, and the answer is `(-1, -1, -1, -1)`. Every Flutter node
//! takes its extents from its parent's, up to the plug, so asking where any
//! of them is freezes the whole application. A call sent while it is frozen
//! waits in its queue and freezes it again when its turn comes, however long
//! ago the reader stopped waiting for it.
//!
//! These tests serve a small application of their own over a private,
//! peer-to-peer D-Bus connection: no bus daemon, no desktop, nothing to
//! `#[ignore]`. It has one main thread, as a toolkit does, and asking where
//! one of its Flutter nodes is holds that thread for [`FREEZE`]. It counts
//! what it is asked and how often it froze, so what the tests assert is how
//! many calls a read made and what they cost the application, not how long
//! the read took.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use atspi::Role;
use perspicax_atspi::{
    AppRef, ObjectKey,
    read::{self, GaveUp, Patience, Placed, RawNode, Spared},
};
use zbus::{Connection, Guid, connection::Builder, zvariant::OwnedObjectPath};

/// How long the reading connection waits for any one answer.
///
/// Half of production's, so a test that sits through four of them still runs
/// in a couple of seconds -- and no shorter, because the deadline cannot tell
/// an application that will never answer from one the scheduler has not run
/// yet. At 100 ms, on a laptop compiling something else, prompt calls missed
/// it in most runs.
const ANSWER: Duration = Duration::from_millis(500);

/// How long asking where a Flutter node is holds the application's main
/// thread: libdbus's 25 s, scaled to the test's deadline. Longer than
/// [`ANSWER`], so the reader stops waiting first, as perspicax does.
const FREEZE: Duration = Duration::from_secs(1);

/// The fake application's unique name, as its nodes name it. Nothing checks
/// it on a peer-to-peer connection, but AT-SPI's references carry one.
const BUS: &str = ":1.42";

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";

/// One node of the fake application.
struct Fake {
    path: String,
    role: Role,
    name: String,
    children: Vec<String>,
    /// Its bounds. `None` serves no `Component` at all, as an application
    /// root does.
    extents: Option<Extents>,
    /// Whether `GetInterfaces` lists the `Component` it serves.
    listed: bool,
    /// Whether every `Accessible` call on it goes unanswered, until
    /// [`Served::unmute`].
    mute: bool,
}

#[derive(Clone, Copy)]
enum Extents {
    Answered(i32, i32, i32, i32),
    /// Asking freezes the application for [`FREEZE`], and the answer, when it
    /// comes, has no geometry in it: a node beneath Flutter's plug.
    Freezes,
}

impl Fake {
    fn new(path: &str, role: Role, name: &str) -> Self {
        Self {
            path: path.to_owned(),
            role,
            name: name.to_owned(),
            children: Vec::new(),
            extents: None,
            listed: true,
            mute: false,
        }
    }

    fn holding(mut self, children: &[&str]) -> Self {
        self.children = children.iter().map(|&child| child.to_owned()).collect();
        self
    }

    fn at(mut self, x: i32, y: i32, width: i32, height: i32) -> Self {
        self.extents = Some(Extents::Answered(x, y, width, height));
        self
    }

    fn freezes(mut self) -> Self {
        self.extents = Some(Extents::Freezes);
        self
    }

    /// Serve its `Component` without listing it in `GetInterfaces`.
    fn unlisted(mut self) -> Self {
        self.listed = false;
        self
    }

    fn mute(mut self) -> Self {
        self.mute = true;
        self
    }
}

/// What every object of the fake application shares: one main thread, and
/// the tallies the tests read.
struct Process {
    /// The application's main thread. Every answer is given holding it, so
    /// while something holds it nothing is answered, and calls wait their
    /// turn in the order they arrived.
    main: tokio::sync::Mutex<()>,
    /// `GetExtents` calls received, by object path.
    asked: Mutex<HashMap<String, usize>>,
    /// `GetExtents` calls received by nodes that being asked freezes.
    asked_freezing: AtomicUsize,
    /// How often being asked froze the application.
    freezes: AtomicUsize,
    /// Whether the nodes made [`Fake::mute`] are silent still.
    muted: AtomicBool,
}

struct Accessible {
    role: Role,
    name: String,
    children: Vec<(String, OwnedObjectPath)>,
    interfaces: Vec<String>,
    mute: bool,
    delay: Duration,
    process: Arc<Process>,
}

impl Accessible {
    /// Answer on the main thread after the application's delay, or never.
    async fn answer<T>(&self, value: T) -> T {
        if self.mute && self.process.muted.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let _main = self.process.main.lock().await;
        // Not `sleep(ZERO)`: tokio still parks that on its timer, which made
        // every call of a prompt application cost about ten milliseconds.
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        value
    }
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    async fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.answer(self.children.clone()).await
    }

    async fn get_role(&self) -> u32 {
        self.answer(self.role as u32).await
    }

    async fn get_state(&self) -> Vec<u32> {
        self.answer(vec![0, 0]).await
    }

    async fn get_interfaces(&self) -> Vec<String> {
        self.answer(self.interfaces.clone()).await
    }

    #[zbus(property)]
    async fn name(&self) -> String {
        self.answer(self.name.clone()).await
    }

    #[zbus(property)]
    async fn description(&self) -> String {
        self.answer(String::new()).await
    }
}

struct Component {
    path: String,
    extents: Extents,
    process: Arc<Process>,
}

#[zbus::interface(name = "org.a11y.atspi.Component")]
impl Component {
    async fn get_extents(&self, _coord_type: u32) -> (i32, i32, i32, i32) {
        *self
            .process
            .asked
            .lock()
            .unwrap()
            .entry(self.path.clone())
            .or_default() += 1;
        if matches!(self.extents, Extents::Freezes) {
            self.process.asked_freezing.fetch_add(1, Ordering::SeqCst);
        }

        let _main = self.process.main.lock().await;
        match self.extents {
            Extents::Answered(x, y, width, height) => (x, y, width, height),
            Extents::Freezes => {
                self.process.freezes.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(FREEZE).await;
                (-1, -1, -1, -1)
            }
        }
    }
}

/// The fake application, served, and a connection reading it.
struct Served {
    reader: Connection,
    app: AppRef,
    process: Arc<Process>,
    _server: Connection,
}

impl Served {
    /// `GetExtents` calls the node at `path` received.
    fn asked(&self, path: &str) -> usize {
        self.process
            .asked
            .lock()
            .unwrap()
            .get(path)
            .copied()
            .unwrap_or(0)
    }

    /// `GetExtents` calls received by nodes that being asked freezes.
    fn asked_freezing(&self) -> usize {
        self.process.asked_freezing.load(Ordering::SeqCst)
    }

    /// How often being asked froze the application.
    fn freezes(&self) -> usize {
        self.process.freezes.load(Ordering::SeqCst)
    }

    /// Wait until every call the application has been sent has had its turn
    /// on the main thread. This stands in for an application waking from a
    /// freeze and signalling what changed meanwhile, which is what brings the
    /// keeper's next re-read.
    async fn idle(&self) {
        drop(self.process.main.lock().await);
    }

    /// Let the nodes made [`Fake::mute`] answer from now on.
    fn unmute(&self) {
        self.process.muted.store(false, Ordering::SeqCst);
    }

    /// The key of the node at `path`.
    fn key(&self, path: &str) -> ObjectKey {
        ObjectKey::new(BUS, path)
    }
}

/// Serve `nodes` -- the first is the application root -- answering every
/// call after `delay`.
async fn serve(nodes: Vec<Fake>, delay: Duration) -> Served {
    let (server_end, reader_end) = tokio::net::UnixStream::pair().unwrap();
    let server = Builder::unix_stream(server_end)
        .server(Guid::generate())
        .unwrap()
        .p2p()
        .build();
    let reader = Builder::unix_stream(reader_end)
        .p2p()
        .method_timeout(ANSWER)
        .build();
    let (server, reader) = tokio::try_join!(server, reader).unwrap();

    let process = Arc::new(Process {
        main: tokio::sync::Mutex::new(()),
        asked: Mutex::new(HashMap::new()),
        asked_freezing: AtomicUsize::new(0),
        freezes: AtomicUsize::new(0),
        muted: AtomicBool::new(true),
    });
    let root = nodes[0].path.clone();
    for fake in nodes {
        let objects = server.object_server();
        let children = fake
            .children
            .iter()
            .map(|child| {
                (
                    BUS.to_owned(),
                    OwnedObjectPath::try_from(child.as_str()).unwrap(),
                )
            })
            .collect();
        let mut interfaces = vec![ACCESSIBLE.to_owned()];
        if fake.extents.is_some() && fake.listed {
            interfaces.push(COMPONENT.to_owned());
        }
        let accessible = Accessible {
            role: fake.role,
            name: fake.name,
            children,
            interfaces,
            mute: fake.mute,
            delay,
            process: Arc::clone(&process),
        };
        objects.at(fake.path.as_str(), accessible).await.unwrap();
        if let Some(extents) = fake.extents {
            let component = Component {
                path: fake.path.clone(),
                extents,
                process: Arc::clone(&process),
            };
            objects.at(fake.path.as_str(), component).await.unwrap();
        }
    }

    Served {
        reader,
        app: AppRef::new(
            "paradoxshift".to_owned(),
            "GTK".to_owned(),
            ObjectKey::new(BUS, &root),
            None,
        ),
        process,
        _server: server,
    }
}

/// ParadoxShift's shape. A GTK frame and button, and Flutter's view: a GTK
/// container holding a bare socket, in which Flutter's plug is embedded by
/// the same process. The socket answers at once, with no geometry; asking
/// where the plug or any node beneath it is freezes the application.
fn flutter(squares: usize) -> Vec<Fake> {
    let square_paths: Vec<String> = (0..squares).map(|n| format!("/square/{n}")).collect();
    let square_refs: Vec<&str> = square_paths.iter().map(String::as_str).collect();
    let mut nodes = vec![
        Fake::new("/root", Role::Application, "paradoxshift").holding(&["/frame"]),
        Fake::new("/frame", Role::Frame, "Paradox Shift")
            .holding(&["/ok", "/view"])
            .at(0, 0, 1280, 767),
        Fake::new("/ok", Role::Button, "OK").at(10, 10, 80, 30),
        Fake::new("/view", Role::Panel, "view")
            .holding(&["/socket"])
            .at(0, 40, 1280, 727),
        Fake::new("/socket", Role::Filler, "socket")
            .holding(&["/plug"])
            .at(-1, -1, -1, -1),
        Fake::new("/plug", Role::Panel, "plug")
            .holding(&square_refs)
            .freezes(),
    ];
    for (n, path) in square_paths.iter().enumerate() {
        nodes.push(Fake::new(path, Role::Button, &format!("square {n}")).freezes());
    }
    nodes
}

fn named<'a>(nodes: &'a [RawNode], label: &str) -> &'a RawNode {
    nodes
        .iter()
        .find(|node| node.label == label)
        .unwrap_or_else(|| panic!("no node {label:?}"))
}

/// Read the way the ingest does: the tree, then where each node is.
async fn read_placing(
    served: &Served,
    walking: &mut Patience,
    spared: &Spared,
) -> (Vec<RawNode>, Placed) {
    walking.begin(None);
    let (_, mut nodes) = read::cold_read(&served.reader, &served.app, walking)
        .await
        .unwrap();
    let placed = read::place(
        &served.reader,
        &mut nodes,
        served.app.root(),
        walking,
        spared,
        None,
    )
    .await;
    (nodes, placed)
}

/// The issue's own case. Every node comes back with its label, and the nodes
/// that answer are placed. The plug is asked where it is once, which freezes
/// the application once -- not three times, as three calls into a frozen
/// application did when each froze it again in its turn.
#[tokio::test]
async fn a_flutter_application_is_read_whole_and_frozen_once() {
    let served = serve(flutter(20), Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (nodes, placed) = read_placing(&served, &mut walking, &spared).await;

    assert_eq!(
        nodes.len(),
        26,
        "root, frame, OK, view, socket, plug and 20 squares"
    );
    assert_eq!(named(&nodes, "square 19").role, Role::Button);
    assert!(named(&nodes, "Paradox Shift").bounds.is_some());
    assert!(named(&nodes, "OK").bounds.is_some());
    assert!(named(&nodes, "view").bounds.is_some());
    assert!(
        named(&nodes, "socket").bounds.is_none(),
        "a bare socket answers, with no geometry"
    );
    assert!(named(&nodes, "plug").bounds.is_none());
    assert!(named(&nodes, "square 19").bounds.is_none());

    assert_eq!(placed, Placed::Spared(served.key("/plug")));
    assert_eq!(served.asked_freezing(), 1);
    served.idle().await;
    assert_eq!(served.freezes(), 1);
    assert_eq!(walking.gave_up(), None, "every walk call was answered");
}

/// The issue's deterministic reproduction. Each re-read stands for a signal
/// the application sends when it wakes, which is what brought the next freeze
/// every 25 s. Once the plug froze it straight after a walk it answered in
/// full, neither the plug nor anything beneath it is asked again -- and what
/// answers is still asked, and still placed.
#[tokio::test]
async fn a_node_that_froze_its_application_is_never_asked_again_nor_anything_beneath_it() {
    let served = serve(flutter(20), Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    read_placing(&served, &mut walking, &spared).await;
    served.idle().await;
    for _ in 0..5 {
        let (nodes, placed) = read_placing(&served, &mut walking, &spared).await;
        served.idle().await;
        assert_eq!(placed, Placed::All);
        assert!(named(&nodes, "Paradox Shift").bounds.is_some());
        assert!(named(&nodes, "OK").bounds.is_some());
        assert!(named(&nodes, "plug").bounds.is_none());
    }

    assert_eq!(served.asked_freezing(), 1, "the plug, once");
    assert_eq!(served.freezes(), 1);
    assert_eq!(served.asked("/ok"), 6, "and OK at every read");
}

/// A cache delivers a tree in whatever order it holds it. Asked bottom-up, a
/// square would freeze the application first, and sparing it would spare
/// nothing else -- so a read asks top-down, and the plug the squares' own
/// extents climb to is the node that is spared.
#[tokio::test]
async fn nodes_are_asked_top_down_whatever_order_the_read_gave() {
    let served = serve(flutter(20), Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());
    walking.begin(None);
    let (_, mut nodes) = read::cold_read(&served.reader, &served.app, &mut walking)
        .await
        .unwrap();
    nodes.reverse();

    let root = served.app.root();
    let placed = read::place(&served.reader, &mut nodes, root, &walking, &spared, None).await;
    served.idle().await;
    assert_eq!(placed, Placed::Spared(served.key("/plug")));

    let placed = read::place(&served.reader, &mut nodes, root, &walking, &spared, None).await;
    served.idle().await;
    assert_eq!(placed, Placed::All);
    assert_eq!(served.freezes(), 1);
}

/// An application already silent during the walk is busy, or frozen by
/// something else. A call it then leaves unanswered says nothing about the
/// node it was asked of, so nothing is held against it -- and once a walk is
/// answered in full, a silence that begins at `GetExtents` is.
#[tokio::test]
async fn silence_after_a_walk_that_went_unanswered_is_not_held_against_the_node() {
    let mut nodes = flutter(2);
    // A label beside OK that answers nothing, until it is unmuted.
    nodes[1].children.insert(1, "/busy".to_owned());
    nodes.push(Fake::new("/busy", Role::Label, "busy").mute());
    let served = serve(nodes, Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;
    assert_eq!(placed, Placed::Unanswered(served.key("/plug")));
    assert!(spared.lock().unwrap().is_empty());

    served.unmute();
    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;
    assert_eq!(placed, Placed::Spared(served.key("/plug")));
    assert_eq!(served.freezes(), 2);
}

/// An application root offers no `Component`, and atk-bridge prints a
/// CRITICAL for every `GetExtents` it is sent anyway. A node is asked where
/// it is only if it says it can answer -- here, one that would.
#[tokio::test]
async fn a_node_that_offers_no_component_is_never_asked_where_it_is() {
    let served = serve(
        vec![
            Fake::new("/root", Role::Application, "app")
                .holding(&["/frame"])
                .at(0, 0, 1280, 767)
                .unlisted(),
            Fake::new("/frame", Role::Frame, "frame").at(0, 0, 1280, 767),
        ],
        Duration::ZERO,
    )
    .await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (nodes, placed) = read_placing(&served, &mut walking, &spared).await;

    assert_eq!(placed, Placed::All);
    assert_eq!(served.asked("/root"), 0);
    assert!(named(&nodes, "app").bounds.is_none());
    assert!(named(&nodes, "frame").bounds.is_some());
}

/// An application that answers, slowly, is read until the read's budget runs
/// out -- and what was read by then is kept rather than thrown away.
#[tokio::test]
async fn a_read_that_runs_out_of_time_keeps_what_it_read() {
    let served = serve(flutter(40), Duration::from_millis(10)).await;
    let mut walking = Patience::new();
    let budget = Duration::from_millis(300);

    let started = Instant::now();
    walking.begin(Some(started + budget));
    let nodes = read::walk(&served.reader, served.app.root(), &mut walking)
        .await
        .unwrap();

    assert!(
        !nodes.is_empty(),
        "the nodes read before the budget ran out"
    );
    assert!(nodes.len() < 46, "and not the ones after: {}", nodes.len());
    assert_eq!(walking.gave_up(), Some(GaveUp::OutOfTime));
    assert!(started.elapsed() < budget * 3, "{:?}", started.elapsed());
}

/// A root that does not answer leaves nothing to read, and the caller hears
/// why: a deadline, not a hang.
#[tokio::test]
async fn a_root_that_never_answers_fails_the_read_at_the_deadline() {
    let served = serve(
        vec![Fake::new("/root", Role::Application, "paradoxshift").mute()],
        Duration::ZERO,
    )
    .await;
    let mut walking = Patience::new();
    walking.begin(None);

    let error = read::cold_read(&served.reader, &served.app, &mut walking)
        .await
        .unwrap_err();
    assert!(error.is_unanswered(), "{error}");
}
