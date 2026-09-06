//! Binding what a bridge describes to what a host drew.
//!
//! An accessibility bridge offers a tree of windows. A compositor holds a list
//! of surfaces. Nothing in either connects them: the bridge does not know what
//! a surface is, and the compositor cannot read a widget tree. Everything this
//! project claims -- provenance, occlusion, a refusal that names the window in
//! the way -- rests on a correspondence that neither side reports and both
//! sides only supply evidence for.
//!
//! # Why this is a correlation and not a lookup
//!
//! The obvious answer is the pid, and it is necessary without being sufficient.
//! The compositor's pid comes from the credentials of a Wayland socket; the
//! bridge's comes from `GetConnectionUnixProcessID` on the accessibility bus.
//! Both are kernel-attested, and they attest to *different sockets*: one says
//! which process drew, the other says which process is serving accessibility.
//! Usually the same program. Not always -- AT-SPI's `Socket` interface exists
//! precisely so one process can serve trees it did not draw -- and one program
//! with four windows produces one pid and four surfaces, which a pid cannot
//! separate at all.
//!
//! So the join weighs evidence, and the rule when the evidence does not settle
//! it is the rule this whole project runs on: **a disagreement is a finding,
//! not a tie-break.** An unjoined window stays [`Origin::Unattributed`] and
//! every node beneath it is refused, which is a recoverable state that says so.
//! A guessed join is an agent clicking in the wrong application's window while
//! being told the receipt was fine.
//!
//! # Windows are structural, not a role
//!
//! Callers build [`WindowClaim`]s from the *children of an application's root*,
//! not by looking for [`Role::Window`](perspicax_node::Role::Window). GTK's
//! toplevel is an AT-SPI `frame` and maps to `Role::Window`; the Qt widget
//! gallery's is a `dialog` and maps to `Role::Dialog`. A role-shaped search
//! finds one toolkit, misses the other entirely, and reports an empty desktop
//! rather than an error.
//!
//! [`Origin::Unattributed`]: perspicax_node::Origin::Unattributed

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use perspicax_node::{NodeId, SurfaceId};

/// How far apart a host's focus and a bridge's activation may be and still
/// describe the same event.
///
/// They are two observations of one thing travelling by different routes: the
/// compositor focuses a toplevel, the toolkit notices, and its bridge emits
/// `state-changed:active` over D-Bus. A quarter of a second is generous for
/// that path and far shorter than the interval between two deliberate window
/// switches, which is the only thing this has to avoid confusing.
pub const FOCUS_CORRELATION: Duration = Duration::from_millis(250);

/// A toplevel, as an accessibility bridge describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowClaim {
    /// The node the bridge offered as a child of its application root.
    pub node: NodeId,
    /// The window's name, which for both toolkits is its title.
    pub title: Option<String>,
    /// The pid the accessibility bus daemon observed for the connection
    /// serving this window. Attested, and about a bus connection rather than
    /// about whoever drew anything.
    pub bus_pid: Option<u32>,
    /// When the bridge last reported this window active.
    pub active_at: Option<Instant>,
}

/// A surface, as its host knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceClaim {
    /// The host's handle for it.
    pub surface: SurfaceId,
    /// The title the client set on its toplevel.
    pub title: Option<String>,
    /// The pid from the client's own connection credentials.
    pub pid: Option<u32>,
    /// When the host last gave this surface keyboard focus.
    pub focused_at: Option<Instant>,
}

/// One reason to believe a window and a surface are the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// Two sockets, attested separately, name one process.
    Pid(u32),
    /// The title the client set and the name the bridge reports are the same
    /// string.
    Title,
    /// The host focused this surface and the bridge reported this window
    /// active, close enough together to be one event.
    Focus {
        /// How far apart the two observations were.
        skew: Duration,
    },
}

/// A window bound to a surface, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    /// The bridge's window node.
    pub node: NodeId,
    /// The host's surface.
    pub surface: SurfaceId,
    /// Everything that pointed this way, strongest first.
    pub evidence: Vec<Evidence>,
}

/// A window that could not be bound, and what stopped it.
///
/// Findings are the output that matters most when something is wrong, so they
/// are returned rather than logged: a caller can print them, count them, or
/// assert on them, and none of those work on a `tracing::warn!`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// Several surfaces fit and nothing separates them.
    Ambiguous {
        /// The window that could not be placed.
        node: NodeId,
        /// Everything it might have been.
        candidates: Vec<SurfaceId>,
    },
    /// One axis of evidence pointed at a surface and another ruled it out --
    /// the titles matched and the processes did not. Worth surfacing rather
    /// than discarding, because it is what a bridge serving another process's
    /// tree looks like from here.
    Contradiction {
        /// The window that could not be placed.
        node: NodeId,
        /// The surface it agreed with on one axis.
        surface: SurfaceId,
        /// The process the accessibility bus named.
        bridge_pid: u32,
        /// The process the host named.
        host_pid: u32,
    },
    /// Nothing on the host could be this window.
    Unmatched {
        /// The window that could not be placed.
        node: NodeId,
    },
    /// Two windows both resolved to one surface, so at most one is right and
    /// this join cannot say which. Reported for every claimant.
    Contested {
        /// The window that could not be placed.
        node: NodeId,
        /// The surface more than one window claimed.
        surface: SurfaceId,
    },
}

impl Finding {
    /// The window this finding is about.
    #[must_use]
    pub fn node(&self) -> NodeId {
        match self {
            Self::Ambiguous { node, .. }
            | Self::Contradiction { node, .. }
            | Self::Unmatched { node }
            | Self::Contested { node, .. } => *node,
        }
    }
}

impl core::fmt::Display for Finding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ambiguous { node, candidates } => write!(
                f,
                "window {} matches {} surfaces equally well",
                node.0,
                candidates.len()
            ),
            Self::Contradiction {
                node,
                surface,
                bridge_pid,
                host_pid,
            } => write!(
                f,
                "window {} shares a title with surface {} but the bridge names \
                 process {bridge_pid} where the host names {host_pid}",
                node.0, surface.0
            ),
            Self::Unmatched { node } => {
                write!(f, "window {} matches no surface this host drew", node.0)
            }
            Self::Contested { node, surface } => write!(
                f,
                "window {} and another both resolve to surface {}",
                node.0, surface.0
            ),
        }
    }
}

/// Bind every window that can be bound, and say why the rest could not.
///
/// The process is the gate: a surface is a candidate only when both sides know
/// a pid and the two agree. Everything after that separates candidates rather
/// than admitting new ones, which is what keeps a title -- a string an
/// application chooses for itself and can change to anything -- from ever being
/// sufficient on its own.
#[must_use]
pub fn join(windows: &[WindowClaim], surfaces: &[SurfaceClaim]) -> (Vec<Join>, Vec<Finding>) {
    let mut joins = Vec::new();
    let mut findings = Vec::new();

    for window in windows {
        match bind(window, surfaces) {
            Ok(join) => joins.push(join),
            Err(finding) => findings.push(finding),
        }
    }

    // A surface can have drawn only one window. Two claimants means at least
    // one is wrong, and nothing here knows which -- so both are demoted, which
    // costs a refusal and never spends a wrong attribution.
    let mut claimants: HashMap<SurfaceId, usize> = HashMap::new();
    for join in &joins {
        *claimants.entry(join.surface).or_default() += 1;
    }
    let (kept, contested): (Vec<Join>, Vec<Join>) = joins
        .into_iter()
        .partition(|join| claimants.get(&join.surface) == Some(&1));
    findings.extend(contested.into_iter().map(|join| Finding::Contested {
        node: join.node,
        surface: join.surface,
    }));

    (kept, findings)
}

/// Bind one window, or say what stopped it.
fn bind(window: &WindowClaim, surfaces: &[SurfaceClaim]) -> Result<Join, Finding> {
    let candidates: Vec<&SurfaceClaim> = surfaces
        .iter()
        .filter(|surface| agree(window.bus_pid, surface.pid))
        .collect();

    let Some(pid) = window.bus_pid.filter(|_| !candidates.is_empty()) else {
        // Nothing shares this window's process. If something nonetheless shares
        // its title, that is a finding rather than an absence -- it is what a
        // bridge serving a tree it did not draw looks like from here.
        let impostor = surfaces.iter().find(|surface| contradicts(window, surface));
        return Err(match impostor {
            // Both pids are known here -- `contradicts` requires it -- so the
            // finding can name them. A refusal that says "the processes differ"
            // without saying which is a refusal nobody can act on, which is the
            // failure mode this project keeps arguing against.
            Some(surface) => Finding::Contradiction {
                node: window.node,
                surface: surface.surface,
                bridge_pid: window.bus_pid.unwrap_or_default(),
                host_pid: surface.pid.unwrap_or_default(),
            },
            None => Finding::Unmatched { node: window.node },
        });
    };

    let chosen = if let [only] = candidates[..] {
        only
    } else {
        // One process, several windows: the pid cannot separate them, which is
        // the case the other two axes exist for.
        let by_title: Vec<&&SurfaceClaim> = candidates
            .iter()
            .filter(|surface| titles_match(window, surface))
            .collect();
        match by_title[..] {
            [only] => only,
            _ => closest_focus(window, &candidates).ok_or(Finding::Ambiguous {
                node: window.node,
                candidates: candidates.iter().map(|s| s.surface).collect(),
            })?,
        }
    };

    let mut evidence = vec![Evidence::Pid(pid)];
    if titles_match(window, chosen) {
        evidence.push(Evidence::Title);
    }
    if let Some(skew) = skew(window, chosen) {
        evidence.push(Evidence::Focus { skew });
    }

    Ok(Join {
        node: window.node,
        surface: chosen.surface,
        evidence,
    })
}

/// Whether two optional pids are both known and equal. Unknown never agrees
/// with anything, including another unknown: this is the gate, and a gate that
/// opens on absence is not one.
fn agree(window: Option<u32>, surface: Option<u32>) -> bool {
    matches!((window, surface), (Some(a), Some(b)) if a == b)
}

/// Whether the two sides name one window the same thing while naming two
/// different processes.
///
/// Both pids have to be *known* and different. Two unknowns sharing a title
/// contradict nothing -- they are an absence of evidence, and reporting that as
/// a conflict would send somebody looking for a bridge impersonating an
/// application when the real answer is that nobody asked the bus for a pid.
fn contradicts(window: &WindowClaim, surface: &SurfaceClaim) -> bool {
    titles_match(window, surface)
        && matches!((window.bus_pid, surface.pid), (Some(a), Some(b)) if a != b)
}

/// Whether both sides name this window the same thing. An absent title on
/// either side is not a match.
fn titles_match(window: &WindowClaim, surface: &SurfaceClaim) -> bool {
    matches!((&window.title, &surface.title), (Some(a), Some(b)) if a == b)
}

/// How far apart the host's focus and the bridge's activation were, if both
/// happened and they are close enough to be one event.
fn skew(window: &WindowClaim, surface: &SurfaceClaim) -> Option<Duration> {
    let (active, focused) = (window.active_at?, surface.focused_at?);
    let skew = if active > focused {
        active - focused
    } else {
        focused - active
    };
    (skew <= FOCUS_CORRELATION).then_some(skew)
}

/// The candidate whose focus best explains this window's activation, when
/// exactly one does.
fn closest_focus<'a>(
    window: &WindowClaim,
    candidates: &'a [&'a SurfaceClaim],
) -> Option<&'a SurfaceClaim> {
    let mut correlated: Vec<(Duration, &SurfaceClaim)> = candidates
        .iter()
        .filter_map(|surface| skew(window, surface).map(|skew| (skew, *surface)))
        .collect();
    correlated.sort_by_key(|(skew, _)| *skew);

    match correlated[..] {
        [(_, only)] => Some(only),
        // A tie is not a winner. Two surfaces focused within microseconds of
        // one activation is exactly the situation where picking the nearer one
        // would be a coin toss wearing a measurement's clothes.
        [(first, surface), (second, _), ..] if first != second => Some(surface),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(node: u64, title: Option<&str>, pid: Option<u32>) -> WindowClaim {
        WindowClaim {
            node: NodeId(node),
            title: title.map(str::to_owned),
            bus_pid: pid,
            active_at: None,
        }
    }

    fn surface(id: u64, title: Option<&str>, pid: Option<u32>) -> SurfaceClaim {
        SurfaceClaim {
            surface: SurfaceId(id),
            title: title.map(str::to_owned),
            pid,
            focused_at: None,
        }
    }

    #[test]
    fn one_process_with_one_window_binds_on_the_pid_alone() {
        let (joins, findings) = join(
            &[window(1, Some("GTK Widget Factory"), Some(9182))],
            &[surface(7, Some("GTK Widget Factory"), Some(9182))],
        );
        assert!(findings.is_empty());
        assert_eq!(
            joins,
            vec![Join {
                node: NodeId(1),
                surface: SurfaceId(7),
                evidence: vec![Evidence::Pid(9182), Evidence::Title],
            }]
        );
    }

    /// The two toolkits in the M2 demo: two processes, one window each, and
    /// nothing to confuse.
    #[test]
    fn two_applications_bind_to_their_own_surfaces() {
        let (joins, findings) = join(
            &[
                window(1, Some("GTK Widget Factory"), Some(100)),
                window(2, Some("Widget Gallery Qt 6.8.2"), Some(200)),
            ],
            &[
                surface(7, Some("Widget Gallery Qt 6.8.2"), Some(200)),
                surface(8, Some("GTK Widget Factory"), Some(100)),
            ],
        );
        assert!(findings.is_empty());
        assert_eq!(joins[0].surface, SurfaceId(8));
        assert_eq!(joins[1].surface, SurfaceId(7));
    }

    /// An unknown pid on either side is not a match, and specifically is not a
    /// match with another unknown. A gate that opens on absence is not a gate.
    #[test]
    fn an_unknown_process_never_agrees_with_anything() {
        let (joins, findings) = join(
            &[window(1, Some("Same"), None)],
            &[surface(7, Some("Same"), None)],
        );
        assert!(joins.is_empty());
        assert_eq!(findings, vec![Finding::Unmatched { node: NodeId(1) }]);
    }

    /// The title agrees and the process does not. That is what a bridge
    /// serving a tree it did not draw looks like from here, and it is reported
    /// rather than resolved in the title's favour.
    #[test]
    fn a_matching_title_over_a_different_process_is_a_finding() {
        let (joins, findings) = join(
            &[window(1, Some("Text Editor"), Some(100))],
            &[surface(7, Some("Text Editor"), Some(200))],
        );
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            vec![Finding::Contradiction {
                node: NodeId(1),
                surface: SurfaceId(7),
                bridge_pid: 100,
                host_pid: 200,
            }]
        );
        assert_eq!(
            findings[0].to_string(),
            "window 1 shares a title with surface 7 but the bridge names \
             process 100 where the host names 200"
        );
    }

    /// One application, two windows: the pid cannot separate them, and the
    /// title can.
    #[test]
    fn two_windows_of_one_application_are_separated_by_their_titles() {
        let (joins, findings) = join(
            &[
                window(1, Some("notes.txt"), Some(100)),
                window(2, Some("draft.txt"), Some(100)),
            ],
            &[
                surface(7, Some("draft.txt"), Some(100)),
                surface(8, Some("notes.txt"), Some(100)),
            ],
        );
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(joins[0].surface, SurfaceId(8));
        assert_eq!(joins[1].surface, SurfaceId(7));
    }

    /// Two identically titled windows of one process, told apart by the
    /// compositor having focused one of them just as the bridge reported it
    /// active.
    #[test]
    fn focus_correlation_separates_what_a_title_cannot() {
        let now = Instant::now();
        let mut claim = window(1, Some("Untitled"), Some(100));
        claim.active_at = Some(now);

        let mut recent = surface(7, Some("Untitled"), Some(100));
        recent.focused_at = Some(now - Duration::from_millis(20));
        let mut stale = surface(8, Some("Untitled"), Some(100));
        stale.focused_at = Some(now - Duration::from_secs(30));

        let (joins, findings) = join(&[claim], &[recent, stale]);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(joins[0].surface, SurfaceId(7));
        assert_eq!(
            joins[0].evidence,
            vec![
                Evidence::Pid(100),
                Evidence::Title,
                Evidence::Focus {
                    skew: Duration::from_millis(20)
                },
            ]
        );
    }

    /// And when nothing separates them, the answer is a finding. This is the
    /// case a system that wanted to look capable would guess at.
    #[test]
    fn two_indistinguishable_windows_are_ambiguous_rather_than_guessed() {
        let (joins, findings) = join(
            &[window(1, Some("Untitled"), Some(100))],
            &[
                surface(7, Some("Untitled"), Some(100)),
                surface(8, Some("Untitled"), Some(100)),
            ],
        );
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            vec![Finding::Ambiguous {
                node: NodeId(1),
                candidates: vec![SurfaceId(7), SurfaceId(8)],
            }]
        );
        assert_eq!(
            findings[0].to_string(),
            "window 1 matches 2 surfaces equally well"
        );
    }

    /// A focus that arrived a minute before the activation explains nothing.
    #[test]
    fn focus_outside_the_correlation_window_is_not_evidence() {
        let now = Instant::now();
        let mut claim = window(1, None, Some(100));
        claim.active_at = Some(now);
        let mut old = surface(7, None, Some(100));
        old.focused_at = Some(now - Duration::from_secs(60));

        let (joins, _) = join(&[claim], &[old]);
        assert_eq!(
            joins[0].evidence,
            vec![Evidence::Pid(100)],
            "the pid still binds it; the stale focus is not offered as a reason"
        );
    }

    /// Two windows resolving to one surface means at least one is wrong, and
    /// nothing here knows which. Both are demoted.
    #[test]
    fn a_surface_two_windows_both_claim_binds_to_neither() {
        let now = Instant::now();
        let mut first = window(1, Some("Untitled"), Some(100));
        first.active_at = Some(now);
        let mut second = window(2, Some("Untitled"), Some(100));
        second.active_at = Some(now);

        let mut only = surface(7, Some("Untitled"), Some(100));
        only.focused_at = Some(now);

        let (joins, findings) = join(&[first, second], &[only]);
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            vec![
                Finding::Contested {
                    node: NodeId(1),
                    surface: SurfaceId(7),
                },
                Finding::Contested {
                    node: NodeId(2),
                    surface: SurfaceId(7),
                },
            ]
        );
    }
}
