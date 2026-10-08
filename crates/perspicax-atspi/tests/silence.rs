//! What a read does with an application that stops answering (#51).
//!
//! A Flutter Linux application answers `GetRoleName`, `Name`, `GetState` and
//! `GetChildren` for every node it has, and never answers
//! `Component.GetExtents` for its view or any node under it. Before #51 a read
//! waited on that one call until `PER_APP` gave up the whole application, and
//! the application was `nodes: 0` for the rest of the session.
//!
//! These tests serve a small application of their own over a private,
//! peer-to-peer D-Bus connection: no bus daemon, no desktop, nothing to
//! `#[ignore]`. Its view and its squares never answer `GetExtents`, the way
//! Flutter's don't. The application counts how often it is asked, so what the
//! tests assert is how many calls a read made, not how long it took.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use atspi::Role;
use perspicax_atspi::{
    AppRef, ObjectKey,
    read::{self, GaveUp, Patience, RawNode},
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

/// The fake application's unique name, as its nodes name it. Nothing checks
/// it on a peer-to-peer connection, but AT-SPI's references carry one.
const BUS: &str = ":1.42";

/// One node of the fake application.
struct Fake {
    path: String,
    role: Role,
    name: String,
    children: Vec<String>,
    /// Its bounds. `None` serves no `Component` at all, as an application
    /// root does.
    extents: Option<Extents>,
    /// Whether every `Accessible` call on it goes unanswered.
    mute: bool,
}

#[derive(Clone, Copy)]
enum Extents {
    Answered(i32, i32, i32, i32),
    /// `GetExtents` is called and never answered, as Flutter's view and
    /// semantics nodes leave it.
    Unanswered,
}

impl Fake {
    fn new(path: &str, role: Role, name: &str) -> Self {
        Self {
            path: path.to_owned(),
            role,
            name: name.to_owned(),
            children: Vec::new(),
            extents: None,
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

    fn unplaced(mut self) -> Self {
        self.extents = Some(Extents::Unanswered);
        self
    }

    fn mute(mut self) -> Self {
        self.mute = true;
        self
    }
}

struct Accessible {
    role: Role,
    name: String,
    children: Vec<(String, OwnedObjectPath)>,
    mute: bool,
    delay: Duration,
}

impl Accessible {
    /// Answer after the application's delay, or never.
    async fn answer<T>(&self, value: T) -> T {
        if self.mute {
            std::future::pending::<()>().await;
        }
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
        self.answer(vec!["org.a11y.atspi.Accessible".to_owned()])
            .await
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
    extents: Extents,
    /// How many `GetExtents` calls went to a node that will never answer.
    unanswered: Arc<AtomicUsize>,
}

#[zbus::interface(name = "org.a11y.atspi.Component")]
impl Component {
    async fn get_extents(&self, _coord_type: u32) -> (i32, i32, i32, i32) {
        match self.extents {
            Extents::Answered(x, y, width, height) => (x, y, width, height),
            Extents::Unanswered => {
                self.unanswered.fetch_add(1, Ordering::SeqCst);
                std::future::pending().await
            }
        }
    }
}

/// The fake application, served, and a connection reading it.
struct Served {
    reader: Connection,
    app: AppRef,
    /// `GetExtents` calls that were never answered.
    unanswered: Arc<AtomicUsize>,
    _server: Connection,
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

    let unanswered = Arc::new(AtomicUsize::new(0));
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
        let accessible = Accessible {
            role: fake.role,
            name: fake.name,
            children,
            mute: fake.mute,
            delay,
        };
        objects.at(fake.path.as_str(), accessible).await.unwrap();
        if let Some(extents) = fake.extents {
            let component = Component {
                extents,
                unanswered: Arc::clone(&unanswered),
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
        unanswered,
        _server: server,
    }
}

/// ParadoxShift's shape: a GTK frame and button that answer, around a Flutter
/// view whose every node leaves `GetExtents` unanswered.
fn flutter(squares: usize) -> Vec<Fake> {
    let square_paths: Vec<String> = (0..squares).map(|n| format!("/square/{n}")).collect();
    let square_refs: Vec<&str> = square_paths.iter().map(String::as_str).collect();
    let mut nodes = vec![
        Fake::new("/root", Role::Application, "paradoxshift").holding(&["/frame"]),
        Fake::new("/frame", Role::Frame, "Paradox Shift")
            .holding(&["/ok", "/view"])
            .at(0, 0, 1280, 767),
        Fake::new("/ok", Role::Button, "OK").at(10, 10, 80, 30),
        Fake::new("/view", Role::Panel, "")
            .holding(&square_refs)
            .unplaced(),
    ];
    for (n, path) in square_paths.iter().enumerate() {
        nodes.push(Fake::new(path, Role::Button, &format!("square {n}")).unplaced());
    }
    nodes
}

fn named<'a>(nodes: &'a [RawNode], label: &str) -> &'a RawNode {
    nodes
        .iter()
        .find(|node| node.label == label)
        .unwrap_or_else(|| panic!("no node {label:?}"))
}

/// Read the way the ingest does: the tree, then every node's bounds.
async fn read_with_geometry(
    served: &Served,
    walking: &mut Patience,
    placing: &mut Patience,
) -> Vec<RawNode> {
    walking.begin(None);
    placing.begin(None);
    let (_, mut nodes) = read::cold_read(&served.reader, &served.app, walking)
        .await
        .unwrap();
    read::geometry(&served.reader, &mut nodes, placing).await;
    nodes
}

/// The issue's own case. Every node comes back with its label, the nodes that
/// answer are placed, and the twenty-one that never will cost three
/// unanswered calls rather than twenty-one.
///
/// Counted rather than timed: each unanswered call costs exactly one deadline,
/// so the count *is* the wait, and a count does not flake on a loaded runner.
#[tokio::test]
async fn an_application_that_never_answers_get_extents_is_still_read_whole() {
    let served = serve(flutter(20), Duration::ZERO).await;
    let (mut walking, mut placing) = (Patience::new(), Patience::new());

    let nodes = read_with_geometry(&served, &mut walking, &mut placing).await;

    assert_eq!(nodes.len(), 24, "root, frame, OK, view and 20 squares");
    assert_eq!(named(&nodes, "square 19").role, Role::Button);
    assert!(named(&nodes, "Paradox Shift").bounds.is_some());
    assert!(named(&nodes, "OK").bounds.is_some());
    assert!(named(&nodes, "square 0").bounds.is_none());
    assert!(named(&nodes, "square 19").bounds.is_none());

    assert_eq!(
        served.unanswered.load(Ordering::SeqCst),
        usize::from(read::STRIKES),
        "a read stops asking after its strikes, not one call per silent node"
    );
    assert_eq!(walking.gave_up(), None, "every walk call was answered");
    assert_eq!(
        placing.gave_up(),
        Some(GaveUp::Unanswered {
            calls: read::STRIKES
        })
    );
}

/// A re-read of an application that ran a read's patience out is given one
/// unanswered call, not three: the keeper re-reads inline, and every second
/// it waits is a second no other application's signals are drained.
#[tokio::test]
async fn a_later_read_of_an_application_that_went_silent_asks_once() {
    let served = serve(flutter(20), Duration::ZERO).await;
    let (mut walking, mut placing) = (Patience::new(), Patience::new());

    read_with_geometry(&served, &mut walking, &mut placing).await;
    let first = served.unanswered.load(Ordering::SeqCst);
    let nodes = read_with_geometry(&served, &mut walking, &mut placing).await;

    assert_eq!(served.unanswered.load(Ordering::SeqCst) - first, 1);
    assert_eq!(placing.gave_up(), Some(GaveUp::Unanswered { calls: 1 }));
    assert!(
        named(&nodes, "OK").bounds.is_some(),
        "what answers is still placed"
    );
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
    assert!(nodes.len() < 44, "and not the ones after: {}", nodes.len());
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
