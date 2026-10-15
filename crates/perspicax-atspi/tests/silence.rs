//! What a read does with an application that stops answering (#51), and with
//! one that being asked freezes (#80, #124).
//!
//! A Flutter Linux application embeds an `AtkPlug` in an `AtkSocket` of its
//! own process. at-spi2-core's bridge answers `GetExtents` on that plug by
//! asking the socket, over D-Bus and blocking -- which is asking itself, so
//! its main thread waits on a call only it could answer until libdbus gives
//! up, 25 s later, and the answer is `(-1, -1, -1, -1)`. Every Flutter node
//! takes its extents from its parent's, up to the plug, so asking where any
//! of them is freezes the whole application. A call sent while it is frozen
//! waits in its queue and freezes it again when its turn comes, however long
//! ago the reader stopped waiting for it. So the plug is known before it is
//! asked, and anything else that freezes it is asked once.
//!
//! These tests serve a small application of their own over a private,
//! peer-to-peer D-Bus connection: no bus daemon, no desktop, nothing to
//! `#[ignore]`. It has one main thread, as a toolkit does, and asking where
//! one of its Flutter nodes is holds that thread for [`FREEZE`]. Each node
//! also says its index among its parent's children, as ATK counts them: a
//! bare socket and Flutter's plug have no ATK parent, so they say -1 though a
//! node of their own process lists them (#124). It counts what it is asked
//! and how often it froze, so what the tests assert is how many calls a read
//! made and what they cost the application, not how long the read took.

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
    read::{self, GaveUp, Patience, Placed, Placing, RawNode, Spared},
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
    /// What it says when asked its index among its parent's children.
    index: Index,
    /// Whether `GetIndexInParent` on it goes unanswered, until
    /// [`Served::unmute`]. Its other calls answer.
    index_mute: bool,
    /// The bus name references to it carry, when not [`BUS`]: a node of
    /// another process, embedded in this one's tree.
    bus: Option<&'static str>,
}

#[derive(Clone, Copy)]
enum Extents {
    Answered(i32, i32, i32, i32),
    /// Asking freezes the application for [`FREEZE`], and the answer, when it
    /// comes, has no geometry in it: a node beneath Flutter's plug.
    Freezes,
}

/// What a node says when asked `GetIndexInParent`.
#[derive(Clone, Copy)]
enum Index {
    /// Its place among the children of the first node that holds it, or -1
    /// for the root, which nothing holds.
    Position,
    /// -1, though a node of its own process lists it: it has no ATK parent.
    /// Measured on ParadoxShift, of both the bare socket and the plug.
    Disowned,
    /// No index at all: the call fails at once, as it would against a bridge
    /// that does not serve it.
    Unknown,
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
            index: Index::Position,
            index_mute: false,
            bus: None,
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

    /// Say -1 when asked its index, as a node with no ATK parent does.
    fn disowned(mut self) -> Self {
        self.index = Index::Disowned;
        self
    }

    /// Be a node of another process, whose references carry `bus`.
    fn elsewhere(mut self, bus: &'static str) -> Self {
        self.bus = Some(bus);
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
    /// `GetIndexInParent` calls received, by object path.
    indexed: Mutex<HashMap<String, usize>>,
    /// How often being asked froze the application.
    freezes: AtomicUsize,
    /// Whether the nodes made [`Fake::mute`], and the indexes made
    /// [`Fake::index_mute`], are silent still.
    muted: AtomicBool,
}

struct Accessible {
    path: String,
    role: Role,
    name: String,
    children: Vec<(String, OwnedObjectPath)>,
    interfaces: Vec<String>,
    /// Its index among its parent's children, or `None` for no index at all.
    index: Option<i32>,
    index_mute: bool,
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

    async fn get_index_in_parent(&self) -> zbus::fdo::Result<i32> {
        *self
            .process
            .indexed
            .lock()
            .unwrap()
            .entry(self.path.clone())
            .or_default() += 1;
        if self.index_mute && self.process.muted.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        self.answer(self.index)
            .await
            .ok_or_else(|| zbus::fdo::Error::UnknownMethod("no GetIndexInParent here".to_owned()))
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
    /// The bus name of each node that is not on [`BUS`], by path.
    buses: HashMap<String, &'static str>,
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

    /// `GetIndexInParent` calls the node at `path` received.
    fn asked_index(&self, path: &str) -> usize {
        self.process
            .indexed
            .lock()
            .unwrap()
            .get(path)
            .copied()
            .unwrap_or(0)
    }

    /// `GetIndexInParent` calls the whole application received.
    fn asked_index_total(&self) -> usize {
        self.process.indexed.lock().unwrap().values().sum()
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

    /// Let the nodes made [`Fake::mute`], and the indexes made
    /// [`Fake::index_mute`], answer from now on.
    fn unmute(&self) {
        self.process.muted.store(false, Ordering::SeqCst);
    }

    /// The key of the node at `path`.
    fn key(&self, path: &str) -> ObjectKey {
        ObjectKey::new(self.buses.get(path).copied().unwrap_or(BUS), path)
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
        indexed: Mutex::new(HashMap::new()),
        freezes: AtomicUsize::new(0),
        muted: AtomicBool::new(true),
    });
    let root = nodes[0].path.clone();
    let buses: HashMap<String, &'static str> = nodes
        .iter()
        .filter_map(|fake| Some((fake.path.clone(), fake.bus?)))
        .collect();
    let mut positions: HashMap<String, i32> = HashMap::new();
    for fake in &nodes {
        for (position, child) in fake.children.iter().enumerate() {
            positions
                .entry(child.clone())
                .or_insert(i32::try_from(position).unwrap());
        }
    }
    for fake in nodes {
        let objects = server.object_server();
        let children = fake
            .children
            .iter()
            .map(|child| {
                (
                    buses.get(child).copied().unwrap_or(BUS).to_owned(),
                    OwnedObjectPath::try_from(child.as_str()).unwrap(),
                )
            })
            .collect();
        let mut interfaces = vec![ACCESSIBLE.to_owned()];
        if fake.extents.is_some() && fake.listed {
            interfaces.push(COMPONENT.to_owned());
        }
        let index = match fake.index {
            Index::Position => Some(positions.get(&fake.path).copied().unwrap_or(-1)),
            Index::Disowned => Some(-1),
            Index::Unknown => None,
        };
        let accessible = Accessible {
            path: fake.path.clone(),
            role: fake.role,
            name: fake.name,
            children,
            interfaces,
            index,
            index_mute: fake.index_mute,
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
        buses,
        _server: server,
    }
}

/// ParadoxShift's shape. A GTK frame and button, and Flutter's view: a GTK
/// container holding a bare socket, in which Flutter's plug is embedded by
/// the same process. The socket answers at once, with no geometry; asking
/// where the plug or any node beneath it is freezes the application. Neither
/// the socket nor the plug has an ATK parent, so each says its index is -1.
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
            .at(-1, -1, -1, -1)
            .disowned(),
        Fake::new("/plug", Role::Panel, "plug")
            .holding(&square_refs)
            .freezes()
            .disowned(),
    ];
    for (n, path) in square_paths.iter().enumerate() {
        nodes.push(Fake::new(path, Role::Button, &format!("square {n}")).freezes());
    }
    nodes
}

/// ParadoxShift's shape once Flutter gives its socket a place
/// (flutter/flutter#153560) while at-spi2-core still answers the plug by
/// asking itself: the plug freezes the application as before, but nothing
/// says where it is first.
fn flutter_whose_socket_has_a_place(squares: usize) -> Vec<Fake> {
    let mut nodes = flutter(squares);
    the(&mut nodes, "/socket").extents = Some(Extents::Answered(0, 40, 1280, 727));
    nodes
}

/// The fake node at `path`, to change one node of a fixture.
fn the<'a>(nodes: &'a mut [Fake], path: &str) -> &'a mut Fake {
    nodes
        .iter_mut()
        .find(|node| node.path == path)
        .unwrap_or_else(|| panic!("no fake node {path:?}"))
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
) -> (Vec<RawNode>, Placing) {
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
/// that answer are placed. The plug is known before it is asked where it is
/// -- listed, on its own bus, by a socket with no place, and with no parent of
/// its own -- so the application is never frozen (#124). Finding it costs
/// one call: the socket is not asked its index, since its own lister has a
/// place.
#[tokio::test]
async fn a_flutter_application_is_read_whole_and_never_frozen() {
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

    assert_eq!(placed.ended, Placed::All);
    assert_eq!(placed.plugs, [served.key("/plug")]);
    assert!(spared.lock().unwrap().contains(&served.key("/plug")));
    assert_eq!(served.asked("/plug"), 0);
    assert_eq!(served.asked_freezing(), 0);
    assert_eq!(served.asked_index("/plug"), 1);
    assert_eq!(served.asked_index_total(), 1, "the plug alone");
    served.idle().await;
    assert_eq!(served.freezes(), 0);
    assert_eq!(walking.gave_up(), None, "every walk call was answered");
}

/// Once known, a plug is spared for as long as its application runs, so no
/// read after the first asks it anything -- not even its index -- and the
/// pass says it found it only the once. What answers is still asked at every
/// read.
#[tokio::test]
async fn a_plug_found_once_is_asked_nothing_at_any_read_after() {
    let served = serve(flutter(20), Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    assert_eq!(placed.plugs, [served.key("/plug")]);
    for _ in 0..5 {
        let (nodes, placed) = read_placing(&served, &mut walking, &spared).await;
        assert_eq!(placed.ended, Placed::All);
        assert!(placed.plugs.is_empty(), "said once");
        assert!(named(&nodes, "OK").bounds.is_some());
    }
    served.idle().await;

    assert_eq!(served.asked_index("/plug"), 1);
    assert_eq!(served.asked("/plug"), 0);
    assert_eq!(served.asked("/socket"), 6);
    assert_eq!(served.asked("/ok"), 6);
    assert_eq!(served.freezes(), 0);
}

/// A cache delivers a tree in its own order. A read asks top-down anyway, so
/// the socket has answered by the time the plug's turn comes.
#[tokio::test]
async fn a_plug_is_found_whatever_order_the_read_gave() {
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

    assert_eq!(placed.ended, Placed::All);
    assert_eq!(placed.plugs, [served.key("/plug")]);
    assert_eq!(served.asked_freezing(), 0);
    assert_eq!(served.freezes(), 0);
}

/// What freezes is everything beneath the plug, so a plug that offered no
/// `Component`, and would not be asked where it is itself, is known all the
/// same, and keeps its squares unasked.
#[tokio::test]
async fn a_plug_that_does_not_list_component_still_keeps_everything_beneath_it_unasked() {
    let mut nodes = flutter(20);
    the(&mut nodes, "/plug").listed = false;
    let served = serve(nodes, Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;

    assert_eq!(placed.ended, Placed::All);
    assert_eq!(placed.plugs, [served.key("/plug")]);
    assert_eq!(served.asked_freezing(), 0, "no square either");
    assert_eq!(served.freezes(), 0);
}

/// Silence during the walk keeps a silent `GetExtents` from being held
/// against a node, since it may not be the asking that silenced it. A plug is
/// known by what its application said instead, so it is known all the same.
#[tokio::test]
async fn a_plug_is_known_even_after_a_walk_that_went_unanswered() {
    let mut nodes = flutter(2);
    nodes[1].children.insert(1, "/busy".to_owned());
    nodes.push(Fake::new("/busy", Role::Label, "busy").mute());
    let served = serve(nodes, Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;

    assert!(walking.heard_silence());
    assert_eq!(placed.ended, Placed::All);
    assert_eq!(placed.plugs, [served.key("/plug")]);
    assert_eq!(served.freezes(), 0);
}

/// A plug another process embedded has its `GetExtents` forwarded to the
/// socket's process, which answers: WebKitGTK's web process is in this
/// shape. It is on a bus of its own, so it is not asked its index, and it and
/// everything beneath it are asked where they are, and placed.
#[tokio::test]
async fn a_plug_another_process_embedded_is_asked_where_it_is() {
    const WEB: &str = ":1.43";
    let mut nodes = flutter(0);
    the(&mut nodes, "/plug").extents = Some(Extents::Answered(-1, -1, -1, -1));
    the(&mut nodes, "/plug").bus = Some(WEB);
    the(&mut nodes, "/plug").children = vec!["/page".to_owned()];
    nodes.push(
        Fake::new("/page", Role::DocumentWeb, "page")
            .at(0, 40, 1280, 727)
            .elsewhere(WEB),
    );
    let served = serve(nodes, Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (nodes, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;

    assert_eq!(placed.ended, Placed::All);
    assert!(placed.plugs.is_empty());
    assert_eq!(served.asked_index("/plug"), 0, "it is on a bus of its own");
    assert_eq!(served.asked("/plug"), 1);
    assert_eq!(
        served.asked_index("/page"),
        1,
        "on its plug's bus, under a plug with no place, so it is asked"
    );
    assert!(named(&nodes, "page").bounds.is_some());
    assert!(spared.lock().unwrap().is_empty());
}

/// GTK leaves a widget it has not laid out yet with extents of no size. A
/// child of one that knows its place among its parent's children is no plug,
/// and is asked where it is as any node is: the one extra call is its whole
/// cost.
#[tokio::test]
async fn a_child_that_knows_its_index_is_asked_where_it_is_though_its_parent_has_no_place() {
    let served = serve(
        vec![
            Fake::new("/root", Role::Application, "app").holding(&["/frame"]),
            Fake::new("/frame", Role::Frame, "frame")
                .holding(&["/unrealized"])
                .at(0, 0, 1280, 767),
            Fake::new("/unrealized", Role::Panel, "unrealized")
                .holding(&["/label"])
                .at(0, 0, -605_997_344, 22099),
            Fake::new("/label", Role::Label, "label").at(10, 10, 80, 20),
        ],
        Duration::ZERO,
    )
    .await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (nodes, placed) = read_placing(&served, &mut walking, &spared).await;

    assert_eq!(placed.ended, Placed::All);
    assert!(placed.plugs.is_empty());
    assert!(named(&nodes, "unrealized").bounds.is_none());
    assert!(named(&nodes, "label").bounds.is_some());
    assert_eq!(served.asked_index("/label"), 1);
    assert_eq!(served.asked_index_total(), 1);
}

/// A node that cannot say its index says nothing about being a plug, so it
/// is asked where it is as before -- and #80's spare is what is left to
/// catch it, at the cost of the one freeze.
#[tokio::test]
async fn a_node_that_cannot_say_its_index_is_asked_where_it_is_as_before() {
    let mut nodes = flutter(20);
    the(&mut nodes, "/plug").index = Index::Unknown;
    let served = serve(nodes, Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;

    assert_eq!(placed.ended, Placed::Spared(served.key("/plug")));
    assert!(placed.plugs.is_empty());
    assert_eq!(served.asked_index("/plug"), 1);
    assert_eq!(served.freezes(), 1);
}

/// Asking a node its index never froze anything, so a node that leaves it
/// unanswered was silenced by something else: the asking ends, as it does at
/// any silence, and nothing is held against the node. Once it answers again,
/// the next read knows it.
#[tokio::test]
async fn a_node_that_does_not_answer_for_its_index_ends_the_asking_and_is_not_held_against_it() {
    let mut nodes = flutter(20);
    the(&mut nodes, "/plug").index_mute = true;
    let served = serve(nodes, Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    assert_eq!(placed.ended, Placed::Unanswered(served.key("/plug")));
    assert!(placed.plugs.is_empty());
    assert!(spared.lock().unwrap().is_empty());
    assert_eq!(served.asked("/plug"), 0);

    served.unmute();
    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;
    assert_eq!(placed.ended, Placed::All);
    assert_eq!(placed.plugs, [served.key("/plug")]);
    assert_eq!(served.freezes(), 0);
}

/// #80's deterministic reproduction. Each re-read stands for a signal the
/// application sends when it wakes, which is what brought the next freeze
/// every 25 s. Once the plug froze it straight after a walk it answered in
/// full, neither the plug nor anything beneath it is asked again -- and what
/// answers is still asked, and still placed. Here the socket has a place, so
/// nothing says the plug will freeze before it does.
#[tokio::test]
async fn a_node_that_froze_its_application_is_never_asked_again_nor_anything_beneath_it() {
    let served = serve(flutter_whose_socket_has_a_place(20), Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;
    assert_eq!(placed.ended, Placed::Spared(served.key("/plug")));
    assert!(placed.plugs.is_empty(), "nothing said it was a plug");
    for _ in 0..5 {
        let (nodes, placed) = read_placing(&served, &mut walking, &spared).await;
        served.idle().await;
        assert_eq!(placed.ended, Placed::All);
        assert!(named(&nodes, "Paradox Shift").bounds.is_some());
        assert!(named(&nodes, "OK").bounds.is_some());
        assert!(named(&nodes, "plug").bounds.is_none());
    }

    assert_eq!(served.asked_freezing(), 1, "the plug, once");
    assert_eq!(served.freezes(), 1);
    assert_eq!(served.asked("/ok"), 6, "and OK at every read");
    assert_eq!(served.asked_index_total(), 0, "no node had no place");
}

/// A cache delivers a tree in whatever order it holds it. Asked bottom-up, a
/// square would freeze the application first, and sparing it would spare
/// nothing else -- so a read asks top-down, and the plug the squares' own
/// extents climb to is the node that is spared.
#[tokio::test]
async fn nodes_are_asked_top_down_whatever_order_the_read_gave() {
    let served = serve(flutter_whose_socket_has_a_place(20), Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());
    walking.begin(None);
    let (_, mut nodes) = read::cold_read(&served.reader, &served.app, &mut walking)
        .await
        .unwrap();
    nodes.reverse();

    let root = served.app.root();
    let placed = read::place(&served.reader, &mut nodes, root, &walking, &spared, None).await;
    served.idle().await;
    assert_eq!(placed.ended, Placed::Spared(served.key("/plug")));

    let placed = read::place(&served.reader, &mut nodes, root, &walking, &spared, None).await;
    served.idle().await;
    assert_eq!(placed.ended, Placed::All);
    assert_eq!(served.freezes(), 1);
}

/// An application already silent during the walk is busy, or frozen by
/// something else. A call it then leaves unanswered says nothing about the
/// node it was asked of, so nothing is held against it -- and once a walk is
/// answered in full, a silence that begins at `GetExtents` is.
#[tokio::test]
async fn silence_after_a_walk_that_went_unanswered_is_not_held_against_the_node() {
    let mut nodes = flutter_whose_socket_has_a_place(2);
    // A label beside OK that answers nothing, until it is unmuted.
    nodes[1].children.insert(1, "/busy".to_owned());
    nodes.push(Fake::new("/busy", Role::Label, "busy").mute());
    let served = serve(nodes, Duration::ZERO).await;
    let (mut walking, spared) = (Patience::new(), Spared::default());

    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;
    assert_eq!(placed.ended, Placed::Unanswered(served.key("/plug")));
    assert!(spared.lock().unwrap().is_empty());

    served.unmute();
    let (_, placed) = read_placing(&served, &mut walking, &spared).await;
    served.idle().await;
    assert_eq!(placed.ended, Placed::Spared(served.key("/plug")));
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

    assert_eq!(placed.ended, Placed::All);
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
