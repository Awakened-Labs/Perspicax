//! What an agent gets back for acting, and the verb it asked for.
//!
//! # Why a receipt rather than a boolean
//!
//! An agent that gets `true` back has learned nothing. It knows a function
//! returned; it does not know whether the application noticed, whether focus
//! moved, whether it clicked the thing it meant, or whether the window it acted
//! on is the one it was looking at a moment ago. Every one of those is
//! answerable here and nowhere else -- they are facts about a dispatch that only
//! the component doing the dispatching holds, and they are gone by the time
//! anybody could ask.
//!
//! So an act returns what happened, and the agent decides what that was worth.
//! That is the same instinct as [`Refusal`](crate::Refusal): carry back what the
//! caller would need to recover on its own, rather than a verdict it has to
//! trust.
//!
//! # The one thing a receipt must not do is overclaim
//!
//! The tempting field is "did it work", and it cannot be honestly filled.
//! Damage is the only evidence available on this timescale, and damage means
//! different things on different toolkits: an idle `gtk4-widget-factory`
//! repaints its whole window about forty times a second, so *any* act on it will
//! see damage whether or not it did anything, while Qt's widget gallery manages
//! about one repaint every two seconds and so answers the question rather well.
//! A receipt that flattened that into a boolean would be confidently wrong on
//! one of the two toolkits this project tests against.
//!
//! [`DamageWitness`] therefore reports what was seen and where, and says how
//! many frames it took to see it, so a reader can weigh it against what that
//! surface does when nothing is happening. The strong answer is
//! [`DamageWitness::Quiet`]: nothing changed here at all.

use std::time::Duration;

use perspicax_node::{NodeId, Origin, Rect, SurfaceId};

use crate::{Action, PointerButton};

/// What an agent asks for, before anyone knows where the target is.
///
/// The distinction from [`Action`] is the coordinates, and it is load-bearing
/// rather than cosmetic. An agent names a control -- "click the Cancel button"
/// -- and does not know, and must not be asked to know, where that button is:
/// its rectangle comes from the index, and turning that rectangle into global
/// space is the compositor's job. A `Verb` is the request; an `Action` is the
/// request with the geometry filled in by the only components entitled to
/// supply it.
///
/// This is also what keeps coordinates out of the agent-facing API entirely,
/// which is the mitigation for risk #1 expressed as a type: an agent that cannot
/// name a pixel cannot name the wrong one.
#[derive(Debug, Clone, PartialEq)]
pub enum Verb {
    /// Press a mouse button on the target.
    Click(PointerButton),
    /// Type text at whatever holds keyboard focus.
    Type(String),
    /// Scroll over the target.
    Scroll {
        /// Horizontal steps.
        dx: f64,
        /// Vertical steps.
        dy: f64,
    },
    /// Give the target's surface keyboard focus.
    Focus,
}

impl Verb {
    /// Fill in the geometry, producing the action a host can dispatch.
    #[must_use]
    pub fn at(&self, rect: Rect) -> Action {
        match self {
            Self::Click(button) => Action::Click {
                at: rect,
                button: *button,
            },
            Self::Type(text) => Action::Type { text: text.clone() },
            Self::Scroll { dx, dy } => Action::Scroll {
                at: rect,
                dx: *dx,
                dy: *dy,
            },
            Self::Focus => Action::Focus,
        }
    }

    /// A short name, for a receipt an agent will read.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Click(_) => "click",
            Self::Type(_) => "type",
            Self::Scroll { .. } => "scroll",
            Self::Focus => "focus",
        }
    }
}

/// What an agent asks of a whole window, rather than of a control in it.
///
/// Addressed by surface, not by selector: a window is what `window_list`
/// names, and the controls a person would use to do the same (a titlebar's
/// close button, a tab) are drawn by the compositor and are in no
/// accessible tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowVerb {
    /// Ask the window to close. A request its client may decline -- to ask
    /// about unsaved work, say -- and never a kill.
    Close,
    /// Bring a tab behind another to the front of its group, in the group's
    /// place, as clicking its tab does.
    Forward,
}

impl WindowVerb {
    /// The action a host dispatches for it.
    #[must_use]
    pub fn action(self) -> Action {
        match self {
            Self::Close => Action::Close,
            Self::Forward => Action::Forward,
        }
    }

    /// A short name, for a receipt an agent will read.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Forward => "forward",
        }
    }
}

/// What became of a window an agent closed or brought forward, once it had
/// been given time to react.
///
/// As with [`DamageWitness`], what was seen rather than whether it worked.
/// A window still open after a close has not necessarily refused: it may be
/// asking the person whether to save, and `appeared` names the window that
/// is asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowWitness {
    /// The window is gone.
    Gone,
    /// The window is still open. `appeared` lists windows of the same
    /// process that were not there before: a dialog asking about unsaved
    /// work, typically.
    StillOpen { appeared: Vec<SurfaceId> },
    /// The tab is in front of its group.
    InFront,
    /// The tab is still behind `shown`.
    StillBehind { shown: SurfaceId },
}

/// What happened when an agent acted on a window.
///
/// No success field, for the reason [`Receipt`] has none.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowReceipt {
    /// The window acted on.
    pub surface: SurfaceId,
    /// Who owns it, from its connection credentials.
    pub origin: Origin,
    /// What was asked for.
    pub verb: WindowVerb,
    /// How long the dispatch itself took.
    pub dispatch: Duration,
    /// Which surface held keyboard focus before.
    pub focus_before: Option<SurfaceId>,
    /// And after.
    pub focus_after: Option<SurfaceId>,
    /// What became of the window.
    pub witness: WindowWitness,
    /// How long it was given to react.
    pub window: Duration,
}

/// What the pixels did, in the window an act was given to have an effect.
///
/// Region-scoped on purpose: "the surface changed" is nearly always true and
/// therefore nearly always uninformative, whereas "pixels changed on the
/// rectangle I just clicked" is a claim about this act. See the module docs for
/// why this is not a boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageWitness {
    /// Nothing damaged this surface at all. The strong answer, and the only one
    /// that is equally trustworthy on a busy toolkit and a quiet one.
    Quiet,
    /// Pixels changed on this surface, but not on the target's own rectangle.
    Elsewhere {
        /// How many frames of damage arrived while waiting.
        frames: u64,
    },
    /// Pixels changed on the target's own rectangle.
    ///
    /// Weigh this against what the surface does when idle: on a toolkit that
    /// repaints wholesale it will be true regardless, and `frames` is the number
    /// that tells you which kind of surface you are looking at.
    OnTarget {
        /// How many frames of damage arrived while waiting.
        frames: u64,
    },
}

impl DamageWitness {
    /// How many frames of damage arrived while waiting.
    #[must_use]
    pub fn frames(&self) -> u64 {
        match self {
            Self::Quiet => 0,
            Self::Elsewhere { frames } | Self::OnTarget { frames } => *frames,
        }
    }

    /// Whether anything at all changed on the surface.
    #[must_use]
    pub fn saw_anything(&self) -> bool {
        !matches!(self, Self::Quiet)
    }
}

/// What happened when an agent acted.
///
/// Every field is something only the acting path knew and nobody can recover
/// afterwards. Nothing here is a judgement about success: the receipt is
/// evidence, and weighing it is the reader's.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    /// The selector the agent named, as it wrote it.
    pub selector: String,
    /// The node it resolved to. Stable, and never reused for another widget, so
    /// an agent can act on it again without re-resolving.
    pub node: NodeId,
    /// The surface that node was drawn on.
    pub surface: SurfaceId,
    /// Who owns that surface, from its connection credentials.
    pub origin: Origin,
    /// What was asked for.
    pub verb: Verb,
    /// How long the dispatch itself took -- the round trip to the compositor's
    /// thread and back, not including the wait for damage.
    pub dispatch: Duration,
    /// Which surface held keyboard focus before the act.
    pub focus_before: Option<SurfaceId>,
    /// And after. For [`Verb::Focus`] this is the claim being made; for a click
    /// it is the cheapest evidence that something took the input.
    pub focus_after: Option<SurfaceId>,
    /// What the pixels did, and how long we watched them.
    pub damage: DamageWitness,
    /// How long the act was given to have a visible effect.
    pub damage_window: Duration,
}

impl Receipt {
    /// Whether keyboard focus moved to the surface acted on.
    ///
    /// Not "did it work" -- plenty of controls take a click without taking
    /// focus, and a receipt should not imply otherwise. This answers the
    /// narrower question a caller can act on.
    #[must_use]
    pub fn focus_landed(&self) -> bool {
        self.focus_after == Some(self.surface)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verb_becomes_an_action_by_gaining_the_geometry_it_lacked() {
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        assert_eq!(
            Verb::Click(PointerButton::Left).at(rect),
            Action::Click {
                at: rect,
                button: PointerButton::Left
            }
        );
    }

    #[test]
    fn typing_carries_no_geometry_because_focus_is_the_address() {
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        assert_eq!(
            Verb::Type("hi".to_owned()).at(rect),
            Action::Type {
                text: "hi".to_owned()
            }
        );
        assert_eq!(Verb::Focus.at(rect), Action::Focus);
    }

    #[test]
    fn quiet_is_the_only_witness_that_saw_nothing() {
        assert!(!DamageWitness::Quiet.saw_anything());
        assert_eq!(DamageWitness::Quiet.frames(), 0);
        assert!(DamageWitness::Elsewhere { frames: 3 }.saw_anything());
        assert_eq!(DamageWitness::OnTarget { frames: 8 }.frames(), 8);
    }

    #[test]
    fn focus_landed_compares_against_the_surface_acted_on() {
        let receipt = Receipt {
            selector: "button:Cancel".to_owned(),
            node: NodeId(7),
            surface: SurfaceId(2),
            origin: Origin::Unattributed,
            verb: Verb::Focus,
            dispatch: Duration::from_millis(1),
            focus_before: Some(SurfaceId(1)),
            focus_after: Some(SurfaceId(2)),
            damage: DamageWitness::Quiet,
            damage_window: Duration::from_millis(200),
        };
        assert!(receipt.focus_landed());
        assert!(
            !Receipt {
                focus_after: Some(SurfaceId(1)),
                ..receipt
            }
            .focus_landed()
        );
    }
}
