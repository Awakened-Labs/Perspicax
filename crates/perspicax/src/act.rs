//! Acting on a node, from a selector to a receipt.
//!
//! This is the composition root of the act path: it is the only place that
//! knows the index, the host and the clock at once, and each of those belongs
//! to a different layer for a reason.
//!
//! # Why the waiting happens here and not in `perspicax-index`
//!
//! Answering "did anything change in the next 200 ms" means sleeping, and
//! `perspicax-index` is the crate that imports neither Wayland nor D-Bus and does no
//! I/O -- the property that makes a GNOME or KWin host a port rather than a
//! rewrite. Teaching it to sleep would be the first crack in that, and for no
//! gain: the gate, the selector resolution and the receipt's shape all live
//! there, and only the clock lives here.
//!
//! So `perspicax-index` decides *whether* an act is allowed and what a receipt is;
//! this module supplies the wall clock and the patience.
//!
//! # The order of operations, and why each step is where it is
//!
//! ```text
//!   resolve  ── selector -> node        perspicax-index   (NotFound / Ambiguous)
//!   gate     ── may we act on it?       perspicax-index   (Stale / Occluded / ...)
//!   mark     ── damage generation now   host       (before, never after)
//!   dispatch ── verb + rect -> action   host       (the compositor's thread)
//!   wait     ── the damage window       here
//!   witness  ── what changed, and where perspicax-index   (region-scoped)
//! ```
//!
//! The damage mark is taken **before** dispatching, for the same reason
//! [`Index::reconcile`] takes the generation observed before a read: crediting
//! an act with the counter it finished at would silently swallow exactly the
//! frames that are the evidence.
//!
//! [`Index::reconcile`]: perspicax_index::Index::reconcile

use std::{thread, time::Duration, time::Instant};

use perspicax_compositor::{ActError, Facts, Host};
use perspicax_index::{
    DamageWitness, Index, Receipt, Refusal, Selector, Verb, WindowReceipt, WindowVerb,
    WindowWitness, check_window,
};
use perspicax_node::SurfaceId;

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
/// report it -- except a keyboard held elsewhere, which the compositor alone
/// can see in time and which arrives here as the refusal it is.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Failure {
    /// The gate said no, and said why.
    #[error("refused: {0}")]
    Refused(#[from] Refusal),
    /// The compositor could not carry it out.
    #[error("not dispatched: {0}")]
    Dispatch(#[source] ActError),
}

impl From<ActError> for Failure {
    fn from(error: ActError) -> Self {
        match error {
            // Decided at dispatch because only the loop can check it in the
            // same turn as the keys, but a statement about the target all the
            // same, with a remedy: `focus` it.
            ActError::FocusElsewhere { focused } => {
                Self::Refused(Refusal::FocusElsewhere { focused })
            }
            error => Self::Dispatch(error),
        }
    }
}

/// Resolve a selector, act on what it names, and report what happened.
///
/// # Errors
///
/// [`Failure::Refused`] if the selector matches nothing, matches several
/// things, or names a node the gate will not act on, or if typing finds the
/// keyboard in another window; [`Failure::Dispatch`] if the compositor did not
/// carry the action out.
pub fn act(
    index: &Index,
    host: &Host,
    facts: &Facts,
    selector: &Selector,
    verb: &Verb,
    window: Duration,
) -> Result<Receipt, Failure> {
    // Resolution and the gate, in that order and both in `perspicax-index`.
    // Nothing here re-decides either: a host that could talk itself past the
    // gate would make the gate's location pointless.
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

/// Act on a whole window -- close it, or bring a tab forward -- and report
/// what became of it.
///
/// The same order as [`act`], with a window in place of a node: the gate
/// (`check_window`, in `perspicax-index`), the dispatch, the wait, and then
/// what the facts say happened. No damage is watched, because what a window
/// verb does is not a change of pixels inside it: a closed window is gone,
/// or a dialog asking about unsaved work has appeared beside it.
///
/// # Errors
///
/// [`Failure::Refused`] when the gate says no, [`Failure::Dispatch`] when the
/// compositor did not carry it out.
pub fn act_window(
    host: &Host,
    facts: &Facts,
    surface: SurfaceId,
    verb: WindowVerb,
    window: Duration,
) -> Result<WindowReceipt, Failure> {
    let before = facts.read();
    let origin = check_window(&before, surface, verb)?.origin.clone();
    let known: Vec<SurfaceId> = before.surfaces().iter().map(|facts| facts.id).collect();

    let started = Instant::now();
    let dispatched = host.act(surface, &verb.action())?;
    let dispatch = started.elapsed();

    thread::sleep(window);

    let after = facts.read();
    let witness = match after.surface(surface) {
        None => WindowWitness::Gone,
        Some(_) if verb == WindowVerb::Close => WindowWitness::StillOpen {
            appeared: after
                .surfaces()
                .iter()
                .filter(|facts| !known.contains(&facts.id) && facts.origin == origin)
                .map(|facts| facts.id)
                .collect(),
        },
        Some(facts) => match facts.behind_tab {
            None => WindowWitness::InFront,
            Some(shown) => WindowWitness::StillBehind { shown },
        },
    };

    Ok(WindowReceipt {
        surface,
        origin,
        verb,
        dispatch,
        focus_before: dispatched.focus_before,
        focus_after: dispatched.focus_after,
        witness,
        window,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use perspicax_compositor::Requests;
    use perspicax_index::{PointerButton, Selector};
    use perspicax_node::{Node, NodeId, ObservedNode, Rect, Role};

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
    fn a_keyboard_elsewhere_is_a_refusal_not_a_dispatch_failure() {
        for focused in [Some(SurfaceId(3)), None] {
            assert_eq!(
                Failure::from(ActError::FocusElsewhere { focused }),
                Failure::Refused(Refusal::FocusElsewhere { focused })
            );
        }
        assert_eq!(
            Failure::from(ActError::Unreachable),
            Failure::Dispatch(ActError::Unreachable)
        );
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
    fn a_window_the_gate_refuses_is_never_dispatched() {
        // No facts at all: the window does not exist as far as the gate can
        // tell, and the host -- which has no compositor behind it -- is
        // never asked, or this would be `Dispatch(Unreachable)`.
        assert_eq!(
            act_window(
                &host(),
                &Facts::new(),
                SurfaceId(1),
                WindowVerb::Close,
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
            by: perspicax_node::SurfaceId(3),
        });
        let broken = Failure::from(ActError::Unreachable);
        assert_ne!(refused, broken);
        assert!(refused.to_string().starts_with("refused:"));
        assert!(broken.to_string().starts_with("not dispatched:"));
    }
}
