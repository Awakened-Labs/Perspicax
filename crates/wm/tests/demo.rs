//! The M2 demo, asserted rather than described.
//!
//! One compositor, two real toolkits, no pixels read anywhere: observe both
//! windows, confirm every node is attributed to the process that actually drew
//! it, and confirm that a node under another window is refused with the
//! covering surface named.
//!
//! `#[ignore]`d because it needs a live accessibility bus and two installed
//! sample applications. `ci/live-tests.sh` runs it with `--include-ignored`.

use std::{path::PathBuf, process::Command, sync::mpsc, thread, time::Duration};

use wm::{
    observe::{self, AppReport},
    session,
};
use wm_compositor::{Config, Facts, Requests, Stop};
use wm_index::Refusal;
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
    let reports = run_demo();
    assert_eq!(
        reports.len(),
        2,
        "expected both toolkits to be hosted and read, got {:?}",
        reports.iter().map(|r| &r.name).collect::<Vec<_>>()
    );

    // Every application bound exactly one window to exactly one surface.
    for report in &reports {
        assert_eq!(
            report.joins.len(),
            1,
            "{} did not bind exactly one window: {:?}",
            report.name,
            report.findings
        );
        assert!(
            report.findings.is_empty(),
            "{} produced findings: {:?}",
            report.name,
            report.findings
        );
    }

    // Provenance reached the leaves, which is the claim no library outside a
    // compositor can make.
    for report in &reports {
        let sample = first_visible(report)
            .unwrap_or_else(|| panic!("{} has no visible attributed node at all", report.name));
        let node = report.index.get(sample).expect("just found");
        assert!(
            matches!(node.origin, Origin::Process(_)),
            "{} node {} is {:?}, not attributed",
            report.name,
            sample.0,
            node.origin
        );
    }

    // The windows are cascaded, so one covers part of the other. Which one
    // ends up on top depends on which toolkit maps first, and that is not
    // deterministic -- so the test finds the covered one rather than assuming.
    let (covered, occluded_node, by) = reports
        .iter()
        .find_map(|report| first_occluded(report).map(|(node, by)| (report, node, by)))
        .expect("with two overlapping windows, something must be covered");

    let above = reports
        .iter()
        .find(|report| report.name != covered.name)
        .and_then(|report| report.joins.first())
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
        covered.index.actable(occluded_node).unwrap_err(),
        Refusal::Occluded { by },
        "a covered node must be refused, and the refusal must name the surface"
    );

    let top = reports
        .iter()
        .find(|report| report.name != covered.name)
        .expect("two reports");
    assert_eq!(
        top.tally.occluded, 0,
        "nothing is above {}, so nothing of it can be occluded",
        top.name
    );
    assert!(
        top.tally.visible > 0,
        "{} is on top and should have visible nodes",
        top.name
    );
}

/// Host both toolkits, read them, and give back what was found.
fn run_demo() -> Vec<AppReport> {
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
fn first_visible(report: &AppReport) -> Option<NodeId> {
    report.index.preorder().into_iter().find(|id| {
        report
            .index
            .get(*id)
            .is_some_and(|node| node.visibility == Visibility::Visible)
    })
}

/// The first node this application shows as covered, and what covers it.
fn first_occluded(report: &AppReport) -> Option<(NodeId, SurfaceId)> {
    report
        .index
        .preorder()
        .into_iter()
        .find_map(|id| match report.index.get(id)?.visibility {
            Visibility::Occluded { by } => Some((id, by)),
            _ => None,
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
