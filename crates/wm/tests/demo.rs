//! The M2 demo, asserted rather than described.
//!
//! One compositor, two real toolkits, no pixels read anywhere: observe both
//! windows, confirm every node is attributed to the process that actually drew
//! it, and confirm that a node under another window is refused with the
//! covering surface named.
//!
//! `#[ignore]`d because it needs a live accessibility bus and two installed
//! sample applications. `ci/live-tests.sh` runs it with `--include-ignored`.

use std::{
    collections::HashSet, path::PathBuf, process::Command, sync::mpsc, thread, time::Duration,
};

use wm::{
    observe::{self, App, Reading},
    session,
};
use wm_compositor::{Config, Facts, Requests, Stop};
use wm_index::{Index, Refusal};
use wm_node::{NodeId, Origin, SurfaceId, Visibility};

/// How long the applications get to map their windows and populate their
/// accessibility trees before anything is read.
///
/// A toolkit maps a window and *then* fills in its tree, so reading too early
/// finds a window with nothing in it. Generous rather than tuned: the failure
/// this guards against is a flaky test that looks like a broken join.
const SETTLE: Duration = Duration::from_secs(8);

#[test]
#[ignore = "needs a live accessibility bus, gtk-4-examples and qt6-base-examples"]
fn a_node_under_another_window_is_refused_and_names_the_surface() {
    let reading = run_demo();
    let index = &reading.index;
    assert_eq!(
        reading.apps.len(),
        2,
        "expected both toolkits to be hosted and read, got {:?}",
        reading.apps.iter().map(|app| &app.name).collect::<Vec<_>>()
    );

    // Every application bound exactly one window to exactly one surface.
    for app in &reading.apps {
        assert_eq!(
            app.joins.len(),
            1,
            "{} did not bind exactly one window: {:?}",
            app.name,
            app.findings
        );
        assert!(
            app.findings.is_empty(),
            "{} produced findings: {:?}",
            app.name,
            app.findings
        );
    }

    // One index, one id space. Until M3 slice 5 each application minted from
    // its own interner and both started at 1, so the two trees collided --
    // invisibly, because nothing had ever put them in the same index to notice.
    let sets: Vec<HashSet<NodeId>> = reading
        .apps
        .iter()
        .map(|app| app.nodes(index).into_iter().collect())
        .collect();
    assert_eq!(
        sets[0].intersection(&sets[1]).count(),
        0,
        "two applications shared node ids"
    );
    assert_eq!(
        index.len(),
        sets[0].len() + sets[1].len(),
        "the index holds nodes belonging to neither application's tree"
    );

    // Provenance reached the leaves, which is the claim no library outside a
    // compositor can make.
    for app in &reading.apps {
        let sample = first_visible(index, app)
            .unwrap_or_else(|| panic!("{} has no visible attributed node at all", app.name));
        let node = index.get(sample).expect("just found");
        assert!(
            matches!(node.origin, Origin::Process(_)),
            "{} node {} is {:?}, not attributed",
            app.name,
            sample.0,
            node.origin
        );
    }

    // The windows are cascaded, so one covers part of the other. Which one
    // ends up on top depends on which toolkit maps first, and that is not
    // deterministic -- so the test finds the covered one rather than assuming.
    let (covered, occluded_node, by) = reading
        .apps
        .iter()
        .find_map(|app| first_occluded(index, app).map(|(node, by)| (app, node, by)))
        .expect("with two overlapping windows, something must be covered");

    let top = reading
        .apps
        .iter()
        .find(|app| app.name != covered.name)
        .expect("two applications");
    let above = top
        .joins
        .first()
        .map(|join| join.surface)
        .expect("the other application is hosted too");
    assert_eq!(
        by, above,
        "{} is covered by surface {}, which is not the other window's surface {}",
        covered.name, by.0, above.0
    );

    // And the gate refuses it, naming what is in the way -- which is the whole
    // sentence this milestone exists to be able to say.
    assert_eq!(
        index.actable(occluded_node).unwrap_err(),
        Refusal::Occluded { by },
        "a covered node must be refused, and the refusal must name the surface"
    );

    // Counted over the top application's own nodes rather than read off a
    // per-application tally, which one desktop-wide index no longer has: the
    // question is about this window, and the index is about the screen.
    let (visible, occluded) = seen(index, top);
    assert_eq!(
        occluded, 0,
        "nothing is above {}, so nothing of it can be occluded",
        top.name
    );
    assert!(
        visible > 0,
        "{} is on top and should have visible nodes",
        top.name
    );
}

/// Host both toolkits, read them, and give back what was found.
fn run_demo() -> Reading {
    let gallery = qt_gallery().expect("qt6-base-examples must be installed");
    let _registry = session::Registry::ensure().expect("an accessibility registry");

    let facts = Facts::new();
    let stop = Stop::new();
    let config = Config {
        size: (1280, 800),
        spawn: vec![
            vec![gallery.to_string_lossy().into_owned()],
            vec!["gtk4-widget-factory".to_owned()],
        ],
        env: session::accessibility_env(),
        // A backstop only. The read below asks the compositor to stop when it
        // is finished, because a Qt tree takes seconds nobody can predict.
        run_for: Some(Duration::from_secs(120)),
    };

    let (sender, receiver) = mpsc::channel();
    let reader = {
        let (facts, stop) = (facts.clone(), stop.clone());
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            let reports = runtime.block_on(async {
                tokio::time::sleep(SETTLE).await;
                let _ = wm_atspi::enable().await;
                observe::observe(&facts).await
            });
            let _ = sender.send(reports);
            stop.request();
        })
    };

    wm_compositor::run(&config, &facts, &Requests::new(), &stop).expect("the compositor runs");
    reader.join().expect("the reader finishes");
    receiver
        .recv()
        .expect("the reader reports")
        .expect("the accessibility bus is readable")
}

/// The first node this application shows as visible.
fn first_visible(index: &Index, app: &App) -> Option<NodeId> {
    app.nodes(index).into_iter().find(|id| {
        index
            .get(*id)
            .is_some_and(|node| node.visibility == Visibility::Visible)
    })
}

/// The first node this application shows as covered, and what covers it.
fn first_occluded(index: &Index, app: &App) -> Option<(NodeId, SurfaceId)> {
    app.nodes(index)
        .into_iter()
        .find_map(|id| match index.get(id)?.visibility {
            Visibility::Occluded { by } => Some((id, by)),
            _ => None,
        })
}

/// How many of this application's nodes are visible, and how many are covered.
fn seen(index: &Index, app: &App) -> (usize, usize) {
    app.nodes(index)
        .into_iter()
        .filter_map(|id| index.get(id))
        .fold((0, 0), |(visible, occluded), node| match node.visibility {
            Visibility::Visible => (visible + 1, occluded),
            Visibility::Occluded { .. } => (visible, occluded + 1),
            _ => (visible, occluded),
        })
}

/// Where Debian keeps Qt's widget gallery.
///
/// Asked of `dpkg` rather than hard-coded, because the path is
/// architecture-qualified and would be right on exactly one runner.
/// `WM_QT_GALLERY` overrides it for anyone whose distribution puts it
/// elsewhere.
fn qt_gallery() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("WM_QT_GALLERY") {
        return Some(PathBuf::from(path));
    }
    let listing = Command::new("dpkg")
        .args(["-L", "qt6-base-examples"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&listing.stdout)
        .lines()
        .find(|line| line.ends_with("/widgets/gallery/bin/gallery"))
        .map(PathBuf::from)
}
