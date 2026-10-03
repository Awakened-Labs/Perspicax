//! Keeping the desktop read, for as long as the compositor runs: what each
//! application volunteers, the applications that arrive after the first
//! read, and the ones that go.
//!
//! A desktop does not stay as it was first read. A person starts an
//! application, the shell is restarted, a monitor is plugged in and the
//! shell puts a desktop on it. So every [`REFRESH`], [`keep_current`]:
//!
//! 1. forgets an application whose process no longer draws anything on the
//!    host, taking its tree out of the index;
//! 2. folds in what each application volunteered, and re-reads a subtree
//!    that changed shape;
//! 3. binds each application's windows to surfaces again when its tree or
//!    the host's facts changed, so a window that mapped since is attributed;
//! 4. reads each process that draws on the host and has not been read, once
//!    it has been drawing for a settle -- a toolkit maps its window and
//!    *then* fills in its tree. A process with nothing on the bus yet is
//!    asked about again later, and less often each time: plenty of programs
//!    never join the bus at all.
//!
//! The first read is the fourth step with every process new.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use perspicax_atspi::Ids;
use perspicax_compositor::{Facts, Stop};
use perspicax_index::{Change, HostFacts};

use crate::{
    desk::Desk,
    observe::{self, App},
};

/// How often the index takes what the accessibility bus has volunteered.
///
/// A drain reads a queue that already arrived and asks no application
/// anything, so this is not the polling this project objects to -- polling a
/// whole *tree* was. Four times a second is well inside the latency an agent
/// already accepts from an act, which waits 200 ms for damage.
pub const REFRESH: Duration = Duration::from_millis(250);

/// How long after a miss a process is next asked about: doubled for each
/// miss, up to [`LONGEST_RETRY_MS`].
const FIRST_RETRY_MS: u64 = 1_000;
const LONGEST_RETRY_MS: u64 = 60_000;

/// Keep `desk` current on a thread of its own, until `stop` is asked for.
///
/// A thread with its own runtime, because the accessibility bus is D-Bus and
/// spends its life waiting, while the compositor's loop must never wait.
pub fn keep_current(desk: Arc<Desk>, facts: Facts, stop: Stop, settle: Duration) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(%error, "no runtime for the accessibility read");
                return;
            }
        };
        runtime.block_on(async move {
            let mut keeper = Keeper::new(desk, facts, settle);
            while !stop.requested() {
                keeper.tick().await;
                tokio::time::sleep(REFRESH).await;
            }
        });
    });
}

/// What [`keep_current`] holds between ticks.
struct Keeper {
    desk: Arc<Desk>,
    facts: Facts,
    /// The desktop's one id map, shared by every application read into it.
    ids: Ids,
    apps: Vec<App>,
    arrivals: Arrivals,
    /// The facts the windows were last bound against.
    bound_at: Option<u64>,
    epoch: Instant,
}

impl Keeper {
    fn new(desk: Arc<Desk>, facts: Facts, settle: Duration) -> Self {
        Self {
            desk,
            facts,
            ids: Ids::default(),
            apps: Vec::new(),
            arrivals: Arrivals::new(millis(settle)),
            bound_at: None,
            epoch: Instant::now(),
        }
    }

    async fn tick(&mut self) {
        let now = self.desk.facts();
        self.forget_gone(&now);
        refresh(&mut self.apps, &self.desk).await;
        if self.bound_at != Some(now.generation()) {
            self.bound_at = Some(now.generation());
            let apps = &mut self.apps;
            self.desk.update(|index, facts| {
                for app in apps {
                    app.relink(index, facts);
                }
            });
        }
        self.read_arrivals(&now).await;
    }

    /// Forget every application whose process no longer draws on the host.
    /// If it draws again, it is read again, as a newcomer.
    fn forget_gone(&mut self, now: &HostFacts) {
        let drawing = observe::drawing(now);
        let (gone, kept) = std::mem::take(&mut self.apps)
            .into_iter()
            .partition(|app| !drawing.contains(&app.pid));
        self.apps = kept;
        for app in gone {
            tracing::info!(
                app = app.name,
                pid = app.pid,
                "draws nothing now; forgotten"
            );
            self.desk
                .update(|index, _| index.apply(Change::Removed { id: app.root }));
        }
    }

    /// Read whichever newcomers are due.
    async fn read_arrivals(&mut self, now: &HostFacts) {
        let read: HashSet<u32> = self.apps.iter().map(|app| app.pid).collect();
        let due: HashSet<u32> = self
            .arrivals
            .due(&unread(now, &read), millis(self.epoch.elapsed()))
            .into_iter()
            .collect();
        if due.is_empty() {
            return;
        }
        let snapshots = match observe::read(&due, &self.ids, &self.facts).await {
            Ok(snapshots) => snapshots,
            Err(error) => {
                tracing::warn!("{error:#}");
                Vec::new()
            }
        };
        let mut found = HashSet::new();
        for snapshot in snapshots {
            found.insert(snapshot.pid());
            self.desk.update(|index, _| {
                let app = snapshot.admit(index);
                tracing::info!(
                    app = app.name,
                    pid = app.pid,
                    windows = app.joins.len(),
                    "read"
                );
                self.apps.push(app);
            });
        }
        let at = millis(self.epoch.elapsed());
        for pid in due.difference(&found) {
            tracing::debug!(
                pid,
                "draws on the host, and is not on the accessibility bus yet"
            );
            self.arrivals.missed(*pid, at);
        }
    }
}

/// Take what each application has volunteered and fold it into the index.
async fn refresh(apps: &mut [App], desk: &Desk) {
    for app in apps {
        let changes = match app.changes().await {
            Ok(changes) if changes.is_empty() => continue,
            Ok(changes) => changes,
            Err(error) => {
                tracing::warn!("{error:#}");
                continue;
            }
        };

        // A shape change is the one thing a signal cannot describe: it says the
        // subtree is no longer what we hold, not what it now is. Noted here and
        // answered below, after the cheap half has been applied.
        let invalidated = changes
            .iter()
            .any(|change| matches!(change, Change::SubtreeInvalidated { .. }));

        desk.update(|index, facts| {
            for change in changes {
                index.apply(change);
            }
            // A node the bus has just volunteered arrives unjoined, and an
            // unjoined node is refused. Re-binding is a tree walk with no I/O
            // in it, so it happens on every change rather than being something
            // the next read gets round to -- and a window the change added is
            // bound to its surface here too.
            app.relink(index, facts);
        });

        if !invalidated {
            continue;
        }
        // Snapshot the damage counters BEFORE the read, for the same reason
        // `observe` does: crediting a read with the generation it finished at
        // would silently swallow the frames that arrived during it.
        let before: Vec<_> = {
            let facts = desk.facts();
            app.joins
                .iter()
                .map(|join| {
                    let generation = facts
                        .surface(join.surface)
                        .map_or(0, |facts| facts.damage_generation);
                    (join.surface, generation)
                })
                .collect()
        };
        match app.reread().await {
            Ok(nodes) => desk.update(|index, facts| {
                index.ingest_snapshot(nodes);
                app.relink(index, facts);
                for (surface, generation) in before {
                    index.reconcile(surface, generation);
                }
            }),
            Err(error) => tracing::warn!("{error:#}"),
        }
    }
}

/// The processes drawing on the host that are not among those `read`.
#[must_use]
pub fn unread(facts: &HostFacts, read: &HashSet<u32>) -> BTreeSet<u32> {
    observe::drawing(facts)
        .into_iter()
        .filter(|pid| !read.contains(pid))
        .collect()
}

/// The processes drawing on the host that have not been read, and when each
/// is next to be. Fed times from outside, in milliseconds from any fixed
/// origin, so it is tested without a clock.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Arrivals {
    settle: u64,
    waiting: HashMap<u32, Wait>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wait {
    /// When to read it.
    due: u64,
    /// How long to wait after the next miss.
    retry: u64,
}

impl Arrivals {
    /// Read each newcomer `settle` milliseconds after it is first seen.
    #[must_use]
    pub fn new(settle: u64) -> Self {
        Self {
            settle,
            waiting: HashMap::new(),
        }
    }

    /// Of the processes `unread` now, the ones due to be read at `now`, in
    /// order. One seen for the first time is due a settle from now; one no
    /// longer unread is forgotten.
    pub fn due(&mut self, unread: &BTreeSet<u32>, now: u64) -> Vec<u32> {
        self.waiting.retain(|pid, _| unread.contains(pid));
        unread
            .iter()
            .filter(|pid| {
                let wait = self.waiting.entry(**pid).or_insert(Wait {
                    due: now.saturating_add(self.settle),
                    retry: FIRST_RETRY_MS,
                });
                wait.due <= now
            })
            .copied()
            .collect()
    }

    /// Reading `pid` at `now` found nothing of it on the bus: ask again
    /// later, and later each time.
    pub fn missed(&mut self, pid: u32, now: u64) {
        if let Some(wait) = self.waiting.get_mut(&pid) {
            wait.due = now.saturating_add(wait.retry);
            wait.retry = (wait.retry * 2).min(LONGEST_RETRY_MS);
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use perspicax_index::{Layer, SurfaceFacts};
    use perspicax_node::{Origin, ProcessOrigin, Rect, SurfaceId};

    use super::*;

    /// A surface drawn by process `pid`.
    fn drawn_by(id: u64, pid: u32) -> SurfaceFacts {
        SurfaceFacts::new(SurfaceId(id), Rect::new(0.0, 0.0, 100.0, 100.0)).owned_by(
            Origin::Process(Box::new(ProcessOrigin {
                pid,
                exe: None,
                cgroup: None,
                sandbox: None,
            })),
        )
    }

    #[test]
    fn pids_owning_surfaces_but_not_yet_read_are_the_ones_to_read() {
        let facts = HostFacts::bottom_to_top(
            vec![
                drawn_by(1, 100).layered(Layer::Background, "perspicax-desktop-DP-1"),
                drawn_by(2, 200),
                drawn_by(3, 200),
                drawn_by(4, 300),
                SurfaceFacts::new(SurfaceId(5), Rect::new(0.0, 0.0, 10.0, 10.0)),
            ],
            1,
        );
        let read = HashSet::from([200]);
        assert_eq!(
            unread(&facts, &read),
            BTreeSet::from([100, 300]),
            "each once, not the one read, and nothing for a surface nobody owns"
        );
    }

    #[test]
    fn a_newcomer_is_read_a_settle_after_it_is_seen_and_asked_again_less_often() {
        let mut arrivals = Arrivals::new(2_000);
        let shell = BTreeSet::from([100]);
        assert!(arrivals.due(&shell, 0).is_empty(), "just seen");
        assert!(arrivals.due(&shell, 1_999).is_empty());
        assert_eq!(arrivals.due(&shell, 2_000), [100], "settled");

        arrivals.missed(100, 2_000);
        assert!(arrivals.due(&shell, 2_999).is_empty());
        assert_eq!(arrivals.due(&shell, 3_000), [100], "a second after a miss");
        arrivals.missed(100, 3_000);
        assert!(arrivals.due(&shell, 4_999).is_empty(), "then two");
        assert_eq!(arrivals.due(&shell, 5_000), [100]);

        assert!(
            arrivals.due(&BTreeSet::new(), 6_000).is_empty(),
            "read, or gone"
        );
        assert!(
            arrivals.due(&shell, 6_000).is_empty(),
            "and back again is new again, with a settle of its own"
        );
        assert_eq!(arrivals.due(&shell, 8_000), [100]);
    }
}
