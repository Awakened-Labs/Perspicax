//! Which connector each CRTC drives.
//!
//! A GPU has a fixed set of CRTCs -- the scanout engines -- and each connector
//! a monitor can plug into can be driven by only some of them. Every connector
//! scan (at start, and on every udev hotplug event) comes back as "these
//! connectors are connected, and each could use these CRTCs", and has to be
//! turned into what changes: which outputs go away, and which new ones come up
//! on which CRTC.
//!
//! Kept free of DRM types so the rule can be tested as data. The rule is the
//! conservative one: an output that is still connected keeps its CRTC, so a
//! monitor plugged in beside a working one never makes the working one blink.

/// What a scan changes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Changes<C, K> {
    /// Driven before and no longer connected: tear these down first, so their
    /// CRTCs are free for anything new in the same scan.
    pub gone: Vec<(C, K)>,
    /// Connected and not yet driven, each with the CRTC it should take.
    pub new: Vec<(C, K)>,
    /// Connected, and no CRTC left that can drive them. More monitors than
    /// scanout engines; they stay dark and the caller says so.
    pub dark: Vec<C>,
}

/// Reconcile what is driven with what is connected.
///
/// `driving` is every (connector, CRTC) pair currently up; `connected` is every
/// connected connector with the CRTCs its encoders can reach, in the order the
/// hardware listed them. The first free CRTC in that order wins.
pub(crate) fn reconcile<C, K>(driving: &[(C, K)], connected: &[(C, Vec<K>)]) -> Changes<C, K>
where
    C: Copy + PartialEq,
    K: Copy + PartialEq,
{
    let still = |connector: &C| connected.iter().any(|(c, _)| c == connector);
    let (kept, gone): (Vec<_>, Vec<_>) = driving.iter().copied().partition(|(c, _)| still(c));

    let mut taken: Vec<K> = kept.iter().map(|&(_, crtc)| crtc).collect();
    let mut new = Vec::new();
    let mut dark = Vec::new();
    for (connector, crtcs) in connected {
        if kept.iter().any(|(c, _)| c == connector) {
            continue;
        }
        match crtcs.iter().copied().find(|crtc| !taken.contains(crtc)) {
            Some(crtc) => {
                taken.push(crtc);
                new.push((*connector, crtc));
            }
            None => dark.push(*connector),
        }
    }
    Changes { gone, new, dark }
}

/// One mode a monitor offers: its size, refresh in Hz, and whether the
/// monitor calls it preferred.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Offered {
    pub width: u16,
    pub height: u16,
    pub refresh: f64,
    pub preferred: bool,
}

/// Which offered mode to use, by index.
///
/// With no request, the monitor's preferred mode (or its first, for one that
/// prefers none). With a request, the offered mode of that size whose refresh
/// is nearest the one asked for, or the fastest at that size if no refresh was
/// given: someone who writes `2560x1440` for a 144 Hz monitor means 144 Hz.
/// A size the monitor does not offer falls back to preferred, and the caller
/// says so, so a typo cannot leave a screen dark.
pub(crate) fn pick_mode(
    wanted: Option<(u16, u16, Option<f64>)>,
    offered: &[Offered],
) -> Option<(usize, bool)> {
    let preferred = || {
        offered
            .iter()
            .position(|mode| mode.preferred)
            .or((!offered.is_empty()).then_some(0))
    };
    let Some((width, height, refresh)) = wanted else {
        return preferred().map(|at| (at, true));
    };
    let sized = offered
        .iter()
        .enumerate()
        .filter(|(_, mode)| mode.width == width && mode.height == height);
    let chosen = match refresh {
        Some(hz) => {
            sized.min_by(|(_, a), (_, b)| (a.refresh - hz).abs().total_cmp(&(b.refresh - hz).abs()))
        }
        None => sized.max_by(|(_, a), (_, b)| a.refresh.total_cmp(&b.refresh)),
    };
    match chosen {
        Some((at, _)) => Some((at, true)),
        None => preferred().map(|at| (at, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_scan_gives_each_connector_its_first_free_crtc() {
        let changes = reconcile::<u32, u32>(&[], &[(1, vec![10, 11]), (2, vec![10, 11])]);
        assert_eq!(changes.new, vec![(1, 10), (2, 11)]);
        assert!(changes.gone.is_empty() && changes.dark.is_empty());
    }

    #[test]
    fn an_unchanged_scan_changes_nothing() {
        let changes = reconcile(&[(1, 10)], &[(1, vec![10, 11])]);
        assert_eq!(
            changes,
            Changes {
                gone: vec![],
                new: vec![],
                dark: vec![]
            }
        );
    }

    #[test]
    fn a_working_output_keeps_its_crtc_when_another_is_plugged_in() {
        // Connector 2 would prefer CRTC 10, but 1 already has it.
        let changes = reconcile(&[(1, 10)], &[(2, vec![10, 11]), (1, vec![10, 11])]);
        assert_eq!(changes.new, vec![(2, 11)]);
        assert!(changes.gone.is_empty());
    }

    #[test]
    fn an_unplugged_connector_releases_its_crtc_to_the_same_scan() {
        let changes = reconcile(&[(1, 10)], &[(2, vec![10])]);
        assert_eq!(changes.gone, vec![(1, 10)]);
        assert_eq!(changes.new, vec![(2, 10)]);
    }

    #[test]
    fn more_monitors_than_crtcs_leaves_the_last_one_dark() {
        let changes = reconcile::<u32, u32>(&[], &[(1, vec![10]), (2, vec![10])]);
        assert_eq!(changes.new, vec![(1, 10)]);
        assert_eq!(changes.dark, vec![2]);
    }

    fn mode(width: u16, height: u16, refresh: f64, preferred: bool) -> Offered {
        Offered {
            width,
            height,
            refresh,
            preferred,
        }
    }

    fn monitor() -> [Offered; 4] {
        [
            mode(1920, 1080, 60.0, false),
            mode(2560, 1440, 59.95, true),
            mode(2560, 1440, 143.91, false),
            mode(2560, 1440, 120.0, false),
        ]
    }

    #[test]
    fn with_no_request_the_monitors_preferred_mode_is_used() {
        assert_eq!(pick_mode(None, &monitor()), Some((1, true)));
    }

    #[test]
    fn a_size_alone_takes_the_fastest_refresh_at_that_size() {
        assert_eq!(
            pick_mode(Some((2560, 1440, None)), &monitor()),
            Some((2, true))
        );
    }

    #[test]
    fn a_refresh_takes_the_nearest_offered() {
        assert_eq!(
            pick_mode(Some((2560, 1440, Some(120.0))), &monitor()),
            Some((3, true))
        );
        assert_eq!(
            pick_mode(Some((2560, 1440, Some(144.0))), &monitor()),
            Some((2, true))
        );
    }

    #[test]
    fn a_size_the_monitor_lacks_falls_back_to_preferred_and_says_so() {
        assert_eq!(
            pick_mode(Some((800, 600, None)), &monitor()),
            Some((1, false))
        );
    }

    #[test]
    fn a_monitor_with_no_modes_has_nothing_to_pick() {
        assert_eq!(pick_mode(None, &[]), None);
    }
}
