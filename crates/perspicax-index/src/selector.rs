//! Selectors -- the addressing scheme an agent can actually write down.
//!
//! A [`NodeId`] is cheap and exact and useless to a human or to a model: it is
//! minted at runtime, means nothing across a restart, and cannot be guessed
//! from looking at a screen. A [`Selector`] is the other half of the pair --
//! slower to resolve, but writable in advance, stable across an application
//! restart, and legible in a transcript.
//!
//! # Grammar
//!
//! ```text
//! selector := segment ('>' segment)*
//! segment  := [role] [':' name] ['[' index ']']
//! ```
//!
//! | Written | Means |
//! |---|---|
//! | `Cancel` | any role, label `Cancel` |
//! | `button:Cancel` | role `Button`, label `Cancel` |
//! | `button:` | role `Button`, any label |
//! | `menu:File>Open` | a `Menu` labelled `File`, with a descendant labelled `Open` |
//! | `button:Close[1]` | the second matching button, in tree order |
//!
//! A bare word is a **label**, not a role -- labels are what a person reads off
//! a screen, so they get the short spelling. To constrain the role alone, write
//! the trailing colon.
//!
//! # `>` is descendant, not child
//!
//! This is the load-bearing decision in the grammar. GTK and Qt both insert
//! layout containers that no user ever sees, and they insert *different* ones:
//! the same dialog is a different number of levels deep in each toolkit. A
//! child-selector would therefore need to be rewritten per toolkit, and one
//! selector working against both is most of the point of having selectors at
//! all. Descendant matching skips the scaffolding.
//!
//! [`NodeId`]: perspicax_node::NodeId

use core::fmt;

use perspicax_node::ObservedNode;

/// A parsed selector. Build one with [`Selector::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    segments: Vec<Segment>,
}

/// One `role:name[index]` step of a selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Compared case-insensitively against the node's role name.
    role: Option<String>,
    /// Compared exactly against the node's label.
    label: Option<String>,
    /// Which match to take, zero-based, when this segment matches several.
    nth: Option<usize>,
}

/// Why a selector string could not be parsed.
///
/// Distinct from [`Refusal`](crate::Refusal) on purpose: these mean the agent
/// wrote something that is not a selector, whereas a refusal means it wrote a
/// perfectly good one that the screen did not satisfy. Conflating the two
/// sends an agent hunting for a window when it has actually typed a bracket
/// wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorParseError {
    /// The selector was empty or entirely whitespace.
    Empty,
    /// A `>`-separated segment was empty, as in `a>>b` or a trailing `>`.
    EmptySegment,
    /// An index was present but not a number, as in `button:Close[x]`.
    BadIndex { got: String },
    /// A `[` with no closing `]`.
    UnclosedIndex,
}

impl fmt::Display for SelectorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "selector is empty"),
            Self::EmptySegment => write!(f, "selector has an empty segment"),
            Self::BadIndex { got } => write!(f, "selector index `{got}` is not a number"),
            Self::UnclosedIndex => write!(f, "selector has a `[` with no closing `]`"),
        }
    }
}

impl core::error::Error for SelectorParseError {}

impl Selector {
    /// Parse a selector string.
    ///
    /// # Errors
    ///
    /// Returns [`SelectorParseError`] if `input` does not match the grammar in
    /// the module documentation.
    pub fn parse(input: &str) -> Result<Self, SelectorParseError> {
        if input.trim().is_empty() {
            return Err(SelectorParseError::Empty);
        }
        let segments = input
            .split('>')
            .map(Segment::parse)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { segments })
    }

    /// The segments, outermost first.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.segments.iter().enumerate() {
            if i > 0 {
                write!(f, ">")?;
            }
            write!(f, "{segment}")?;
        }
        Ok(())
    }
}

impl Segment {
    fn parse(raw: &str) -> Result<Self, SelectorParseError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(SelectorParseError::EmptySegment);
        }

        let (head, nth) = match raw.find('[') {
            Some(open) => {
                let rest = &raw[open + 1..];
                let close = rest.find(']').ok_or(SelectorParseError::UnclosedIndex)?;
                let index = rest[..close].trim();
                let parsed = index
                    .parse::<usize>()
                    .map_err(|_| SelectorParseError::BadIndex {
                        got: index.to_owned(),
                    })?;
                (raw[..open].trim(), Some(parsed))
            }
            None => (raw, None),
        };

        // A bare word is a label. Only a colon introduces a role, which is why
        // `button:` and `button` mean different things.
        let (role, label) = match head.split_once(':') {
            Some((role, label)) => (non_empty(role), non_empty(label)),
            None => (None, non_empty(head)),
        };

        if role.is_none() && label.is_none() && nth.is_none() {
            return Err(SelectorParseError::EmptySegment);
        }

        Ok(Self { role, label, nth })
    }

    /// Whether this segment describes `node`.
    ///
    /// The role comparison formats the node's [`Role`] through `Debug` rather
    /// than consulting a name table. The enum carries 182 unit variants and
    /// gains more with each upstream release, so a hand-written table would be
    /// a permanent source of silent gaps -- a role we forgot to list would
    /// simply never match, and nothing would say so. Formatting costs an
    /// allocation per candidate and is the first thing to optimise if a profile
    /// ever objects.
    ///
    /// [`Role`]: perspicax_node::Role
    #[must_use]
    pub fn matches(&self, node: &ObservedNode) -> bool {
        if let Some(role) = &self.role {
            let actual = format!("{:?}", node.node.role());
            if !actual.eq_ignore_ascii_case(role) {
                return false;
            }
        }
        if let Some(label) = &self.label
            && node.node.label() != Some(label.as_str())
        {
            return false;
        }
        true
    }

    /// Which match to take when this segment matches several, zero-based.
    #[must_use]
    pub fn nth(&self) -> Option<usize> {
        self.nth
    }
}

impl fmt::Display for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.role, &self.label) {
            (Some(role), Some(label)) => write!(f, "{role}:{label}")?,
            (Some(role), None) => write!(f, "{role}:")?,
            (None, Some(label)) => write!(f, "{label}")?,
            (None, None) => {}
        }
        if let Some(nth) = self.nth {
            write!(f, "[{nth}]")?;
        }
        Ok(())
    }
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use perspicax_node::{Node, NodeId, Role};

    fn node(role: Role, label: Option<&str>) -> ObservedNode {
        let mut n = Node::new(role);
        if let Some(label) = label {
            n.set_label(label);
        }
        ObservedNode::unjoined(NodeId(1), n)
    }

    #[test]
    fn a_bare_word_is_a_label_not_a_role() {
        let selector = Selector::parse("Cancel").unwrap();
        let segment = &selector.segments()[0];
        assert_eq!(segment.role, None);
        assert_eq!(segment.label.as_deref(), Some("Cancel"));
    }

    #[test]
    fn a_trailing_colon_constrains_the_role_alone() {
        let selector = Selector::parse("button:").unwrap();
        let segment = &selector.segments()[0];
        assert_eq!(segment.role.as_deref(), Some("button"));
        assert_eq!(segment.label, None);

        assert!(segment.matches(&node(Role::Button, Some("anything"))));
        assert!(!segment.matches(&node(Role::CheckBox, Some("anything"))));
    }

    #[test]
    fn role_and_label_together() {
        let selector = Selector::parse("button:Cancel").unwrap();
        let segment = &selector.segments()[0];
        assert!(segment.matches(&node(Role::Button, Some("Cancel"))));
        assert!(!segment.matches(&node(Role::Button, Some("OK"))));
        assert!(!segment.matches(&node(Role::Link, Some("Cancel"))));
    }

    #[test]
    fn role_matching_is_case_insensitive() {
        for spelling in ["button:", "Button:", "BUTTON:"] {
            let selector = Selector::parse(spelling).unwrap();
            assert!(selector.segments()[0].matches(&node(Role::Button, None)));
        }
    }

    /// Multi-word roles are spelled as AccessKit spells them, minus the case.
    #[test]
    fn multi_word_roles_match_their_variant_name() {
        let selector = Selector::parse("menuitem:Open").unwrap();
        assert!(selector.segments()[0].matches(&node(Role::MenuItem, Some("Open"))));
    }

    #[test]
    fn the_m3_demo_selectors_parse() {
        let menu = Selector::parse("menu:File>Open").unwrap();
        assert_eq!(menu.segments().len(), 2);
        assert_eq!(menu.segments()[0].role.as_deref(), Some("menu"));
        assert_eq!(menu.segments()[0].label.as_deref(), Some("File"));
        assert_eq!(menu.segments()[1].role, None);
        assert_eq!(menu.segments()[1].label.as_deref(), Some("Open"));

        let button = Selector::parse("button:Cancel").unwrap();
        assert_eq!(button.segments().len(), 1);
    }

    #[test]
    fn an_index_selects_among_equals() {
        let selector = Selector::parse("button:Close[1]").unwrap();
        let segment = &selector.segments()[0];
        assert_eq!(segment.nth(), Some(1));
        assert_eq!(segment.label.as_deref(), Some("Close"));
    }

    #[test]
    fn whitespace_around_segments_is_insignificant() {
        assert_eq!(
            Selector::parse(" menu:File > Open ").unwrap(),
            Selector::parse("menu:File>Open").unwrap()
        );
    }

    #[test]
    fn display_round_trips_a_parsed_selector() {
        for input in [
            "Cancel",
            "button:Cancel",
            "button:",
            "menu:File>Open",
            "x[2]",
        ] {
            let parsed = Selector::parse(input).unwrap();
            assert_eq!(
                Selector::parse(&parsed.to_string()).unwrap(),
                parsed,
                "round trip failed for {input}"
            );
        }
    }

    #[test]
    fn malformed_selectors_say_which_way_they_are_malformed() {
        assert_eq!(Selector::parse(""), Err(SelectorParseError::Empty));
        assert_eq!(Selector::parse("   "), Err(SelectorParseError::Empty));
        assert_eq!(
            Selector::parse("a>>b"),
            Err(SelectorParseError::EmptySegment)
        );
        assert_eq!(
            Selector::parse("button:Close["),
            Err(SelectorParseError::UnclosedIndex)
        );
        assert_eq!(
            Selector::parse("button:Close[x]"),
            Err(SelectorParseError::BadIndex { got: "x".into() })
        );
    }

    /// A node with no label must not be matched by a label constraint. The
    /// alternative -- treating absent as wildcard -- would make `Cancel` match
    /// every unlabelled container in the tree.
    #[test]
    fn an_absent_label_does_not_satisfy_a_label_constraint() {
        let selector = Selector::parse("Cancel").unwrap();
        assert!(!selector.segments()[0].matches(&node(Role::Button, None)));
    }
}
