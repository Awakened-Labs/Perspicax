//! Acting on a node, from a selector to a receipt.
//!
//! This is the composition root of the act path: it is the only place that
//! knows the index, the host and the clock at once, and each of those belongs
//! to a different layer for a reason.
//!
//! # Why the waiting happens here and not in `wm-index`
//!
//! Answering "did anything change in the next 200 ms" means sleeping, and
//! `wm-index` is the crate that imports neither Wayland nor D-Bus and does no
//! I/O -- the property that makes a GNOME or KWin host a port rather than a
//! rewrite. Teaching it to sleep would be the first crack in that, and for no
//! gain: the gate, the selector resolution and the receipt's shape all live
//! there, and only the clock lives here.
//!
//! So `wm-index` decides *whether* an act is allowed and what a receipt is;
//! this module supplies the wall clock and the patience.
//!
//! # The order of operations, and why each step is where it is
//!
//! ```text
//!   resolve  ── selector -> node        wm-index   (NotFound / Ambiguous)
//!   gate     ── may we act on it?       wm-index   (Stale / Occluded / ...)
//!   mark     ── damage generation now   host       (before, never after)
//!   dispatch ── verb + rect -> action   host       (the compositor's thread)
//!   wait     ── the damage window       here
//!   witness  ── what changed, and where wm-index   (region-scoped)
//! ```
//!
//! The damage mark is taken **before** dispatching, for the same reason
//! [`Index::reconcile`] takes the generation observed before a read: crediting
//! an act with the counter it finished at would silently swallow exactly the
//! frames that are the evidence.
//!
//! [`Index::reconcile`]: wm_index::Index::reconcile

use std::{thread, time::Duration, time::Instant};

use wm_compositor::{ActError, Facts, Host};
use wm_index::{DamageWitness, Index, Receipt, Refusal, Selector, Verb};

/// How long an act is given to have a visible effect, by default.
///
/// Above one frame interval by a wide margin and deliberately so: frame
/// callbacks are a 16 ms timer, so a window shorter than that would report
/// "nothing happened" for acts that worked, purely because the client had not
/// been told it could draw yet. 200 ms is long enough for a toolkit to react and
/// short enough to sit inside a latency an agent will accept.
pub const DAMAGE_WINDOW: Duration = Duration::from_millis(200);

/// Why an act did not produce a receipt.
///
/// Two genuinely different kinds of failure, kept apart because an agent can do
/// something about one of them. A [`Refusal`] is a statement about the target --
/// it is covered, it is stale, the selector matched three things -- and every
/// variant carries what would be needed to recover. An [`ActError`] is a
/// statement about this compositor, and there is nothing an agent can do but
/// report it.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Failure {
    /// The gate said no, and said why.
    #[error("refused: {0}")]
    Refused(#[from] Refusal),
    /// The compositor could not carry it out.
    #[error("not dispatched: {0}")]
    Dispatch(#[from] ActError),
}

/// Resolve a selector, act on what it names, and report what happened.
///
/// # Errors
///
/// [`Failure::Refused`] if the selector matches nothing, matches several
/// things, or names a node the gate will not act on; [`Failure::Dispatch`] if
/// the compositor did not carry the action out.
pub fn act(
    index: &Index,
    host: &Host,
    facts: &Facts,
    selector: &Selector,
    verb: &Verb,
    window: Duration,
) -> Result<Receipt, Failure> {
    // Resolution and the gate, in that order and both in `wm-index`. Nothing
    // here re-decides either: a host that could talk itself past the gate would
    // make the gate's location pointless.
    let id = index.resolve(selector)?;
    let node = index.actable(id)?;

    // `actable` has already established an attributed origin and a `Visible`
    // verdict, and a node cannot be judged visible without a surface and a
    // rectangle. These two are therefore unreachable rather than defensive --
    // but they fail closed rather than unwrapping, because "unreachable" is a
    // claim about today's `check_actable` and this is the act path.
    let surface = node.surface.ok_or(Refusal::Unattributed)?;
    let rect = node.bounds().ok_or(Refusal::Unjudged)?;
    let origin = node.origin.clone();

    // Marked before the act, never after. See the module docs.
    let before = facts
        .read()
        .surface(surface)
        .map_or(0, |facts| facts.damage_generation);

    let action = verb.at(rect);
    let started = Instant::now();
    let dispatched = host.act(surface, &action)?;
    let dispatch = started.elapsed();

    thread::sleep(window);

    // Re-read rather than reuse: the whole point of the wait is that the world
    // moved, and the snapshot taken before it is precisely the one that cannot
    // say so.
    let after = facts.read();
    let damage = after
        .surface(surface)
        .map_or(DamageWitness::Quiet, |facts| {
            let frames = facts.damage_generation.saturating_sub(before);
            if frames == 0 {
                DamageWitness::Quiet
            } else if facts.damage_touches(before, rect) {
                DamageWitness::OnTarget { frames }
            } else {
                DamageWitness::Elsewhere { frames }
            }
        });

    Ok(Receipt {
        selector: selector.to_string(),
        node: id,
        surface,
        origin,
        verb: verb.clone(),
        dispatch,
        focus_before: dispatched.focus_before,
        focus_after: dispatched.focus_after,
        damage,
        damage_window: window,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_compositor::Requests;
    use wm_index::{PointerButton, Selector};
    use wm_node::{Node, NodeId, ObservedNode, Rect, Role};

    /// An index holding one unjoined node, which is what M1 produced for
    /// everything and what the gate exists to refuse.
    fn unjoined() -> Index {
        let mut node = Node::new(Role::Button);
        node.set_label("Cancel");
        node.set_bounds(Rect::new(0.0, 0.0, 10.0, 10.0));
        let mut index = Index::new();
        index.ingest_snapshot([ObservedNode::unjoined(NodeId(1), node)]);
        index
    }

    fn host() -> Host {
        Host::new(&Facts::new(), &Requests::new()).waiting(Duration::from_millis(20))
    }

    #[test]
    fn an_unattributed_node_is_refused_before_anything_is_dispatched() {
        let index = unjoined();
        let selector = Selector::parse("button:Cancel").expect("parses");

        let outcome = act(
            &index,
            &host(),
            &Facts::new(),
            &selector,
            &Verb::Click(PointerButton::Left),
            Duration::ZERO,
        );

        // Not `Dispatch(Unreachable)`: the gate refused, so nothing was ever
        // sent to a compositor that is not there. The order is the assertion.
        assert_eq!(outcome, Err(Failure::Refused(Refusal::Unattributed)));
    }

    #[test]
    fn a_selector_that_matches_nothing_says_so_rather_than_acting() {
        let index = unjoined();
        let selector = Selector::parse("button:Nonexistent").expect("parses");

        assert_eq!(
            act(
                &index,
                &host(),
                &Facts::new(),
                &selector,
                &Verb::Focus,
                Duration::ZERO
            ),
            Err(Failure::Refused(Refusal::NotFound))
        );
    }

    #[test]
    fn a_refusal_and_a_dispatch_failure_are_different_failures() {
        // They read differently to an agent, which is the whole reason they are
        // separate variants rather than one string.
        let refused = Failure::from(Refusal::Occluded {
            by: wm_node::SurfaceId(3),
        });
        let broken = Failure::from(ActError::Unreachable);
        assert_ne!(refused, broken);
        assert!(refused.to_string().starts_with("refused:"));
        assert!(broken.to_string().starts_with("not dispatched:"));
    }
}
