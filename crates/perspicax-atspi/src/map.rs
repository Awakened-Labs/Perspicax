//! AT-SPI's vocabulary projected onto AccessKit's.
//!
//! Two vocabularies designed a decade apart by different people to describe
//! the same widgets. AT-SPI2 has 130 roles inherited from Java's Accessibility
//! API by way of GNOME; AccessKit has 182, shaped by ARIA and Chromium's
//! accessibility tree. They overlap heavily and agree exactly nowhere.
//!
//! So this is a **lossy projection**, and the rule that governs every doubtful
//! entry is: *generalise rather than guess*. A GTK `Panel` becoming
//! [`Role::Group`] loses information; a GTK `Panel` becoming `Role::Pane`
//! because the names rhyme invents it. Losing information leaves a selector
//! merely less specific -- `button:Cancel` still finds the button -- whereas
//! inventing it makes a selector match the wrong node, which is indetectable
//! from above. When there is no honest counterpart at all the answer is
//! [`Role::Unknown`], which is a statement, not a failure.
//!
//! The other rule is that nothing here reads the wire twice. Every function
//! takes what a single cache item or a single node read already carried --
//! role, states, interfaces, extents -- and returns a projection of it. An
//! entry that needed one more D-Bus call to be accurate does not belong in
//! this module; it belongs in the latency table as a cost someone decided to
//! pay.

use atspi::{Interface, InterfaceSet, Role as AtspiRole, State, StateSet};
use perspicax_node::{Action, Node, Orientation, Rect, Role, Toggled};

/// AT-SPI's role, plus the handful of states that change what a role *means*.
///
/// Most of the table is a plain one-to-one rename. Four entries are not, and
/// they are the ones where AT-SPI encodes in a state bit what AccessKit
/// encodes in the role itself:
///
/// - `Text` is AT-SPI's catch-all for anything with characters in it. Editable
///   makes it a text input, multi-line makes it a multi-line one, and neither
///   makes it a label. Mapping it unconditionally to a text input would make
///   every static string in a window look typeable.
/// - `Entry` is always an input; only its line count is in question.
/// - `Button` carrying `IsDefault` is AccessKit's [`Role::DefaultButton`] --
///   the Return-key button of a dialog, which an agent has good reason to
///   distinguish from the other three.
///
/// Everything else ignores `states` entirely, which is why they are passed by
/// value: [`StateSet`] is a `u64` of flags, and threading a reference through
/// 130 match arms to read it four times would be its own small absurdity.
#[must_use]
pub fn role(role: AtspiRole, states: StateSet) -> Role {
    let editable = states.contains(State::Editable);
    let multi_line = states.contains(State::MultiLine);

    match role {
        // -- state-refined ------------------------------------------------
        AtspiRole::Text => match (editable, multi_line) {
            (true, true) => Role::MultilineTextInput,
            (true, false) => Role::TextInput,
            (false, _) => Role::Label,
        },
        AtspiRole::Entry | AtspiRole::Editbar => {
            if multi_line {
                Role::MultilineTextInput
            } else {
                Role::TextInput
            }
        }
        AtspiRole::Button => {
            if states.contains(State::IsDefault) {
                Role::DefaultButton
            } else {
                Role::Button
            }
        }

        // -- exact counterparts -------------------------------------------
        AtspiRole::Alert => Role::Alert,
        AtspiRole::Application => Role::Application,
        AtspiRole::Article => Role::Article,
        AtspiRole::Audio => Role::Audio,
        AtspiRole::BlockQuote => Role::Blockquote,
        AtspiRole::Canvas | AtspiRole::DrawingArea => Role::Canvas,
        AtspiRole::Caption => Role::Caption,
        AtspiRole::CheckBox => Role::CheckBox,
        AtspiRole::CheckMenuItem => Role::MenuItemCheckBox,
        AtspiRole::ColumnHeader | AtspiRole::TableColumnHeader => Role::ColumnHeader,
        AtspiRole::ComboBox => Role::ComboBox,
        AtspiRole::Comment => Role::Comment,
        AtspiRole::ContentDeletion => Role::ContentDeletion,
        AtspiRole::ContentInsertion => Role::ContentInsertion,
        AtspiRole::DateEditor => Role::DateInput,
        AtspiRole::Definition | AtspiRole::DescriptionValue => Role::Definition,
        AtspiRole::DescriptionList => Role::DescriptionList,
        AtspiRole::DescriptionTerm => Role::Term,
        AtspiRole::Dialog => Role::Dialog,
        AtspiRole::Embedded => Role::EmbeddedObject,
        AtspiRole::Footer => Role::Footer,
        AtspiRole::Footnote => Role::DocFootnote,
        AtspiRole::Form => Role::Form,
        AtspiRole::Header => Role::Header,
        AtspiRole::Heading => Role::Heading,
        AtspiRole::Icon | AtspiRole::Image | AtspiRole::ImageMap => Role::Image,
        AtspiRole::Label | AtspiRole::AcceleratorLabel | AtspiRole::Static => Role::Label,
        AtspiRole::LevelBar | AtspiRole::Rating => Role::Meter,
        AtspiRole::Link => Role::Link,
        AtspiRole::List => Role::List,
        AtspiRole::ListBox => Role::ListBox,
        AtspiRole::ListItem => Role::ListItem,
        AtspiRole::Log => Role::Log,
        AtspiRole::Mark => Role::Mark,
        AtspiRole::Marquee => Role::Marquee,
        AtspiRole::Math | AtspiRole::MathFraction | AtspiRole::MathRoot => Role::Math,
        AtspiRole::Menu | AtspiRole::PopupMenu => Role::Menu,
        AtspiRole::MenuBar => Role::MenuBar,
        AtspiRole::MenuItem | AtspiRole::TearoffMenuItem => Role::MenuItem,
        AtspiRole::PageTab => Role::Tab,
        AtspiRole::PageTabList => Role::TabList,
        AtspiRole::Paragraph => Role::Paragraph,
        AtspiRole::PasswordText => Role::PasswordInput,
        AtspiRole::ProgressBar => Role::ProgressIndicator,
        AtspiRole::PushButtonMenu | AtspiRole::ToggleButton => Role::Button,
        AtspiRole::RadioButton => Role::RadioButton,
        AtspiRole::RadioMenuItem => Role::MenuItemRadio,
        AtspiRole::RowHeader | AtspiRole::TableRowHeader => Role::RowHeader,
        AtspiRole::ScrollBar => Role::ScrollBar,
        AtspiRole::ScrollPane => Role::ScrollView,
        AtspiRole::Section => Role::Section,
        AtspiRole::Separator => Role::Splitter,
        AtspiRole::Slider | AtspiRole::Dial => Role::Slider,
        AtspiRole::SpinButton => Role::SpinButton,
        AtspiRole::StatusBar | AtspiRole::InfoBar => Role::Status,
        AtspiRole::Suggestion => Role::Suggestion,
        AtspiRole::Table => Role::Table,
        AtspiRole::TableCell => Role::Cell,
        AtspiRole::TableRow => Role::Row,
        AtspiRole::Terminal => Role::Terminal,
        AtspiRole::Timer => Role::Timer,
        AtspiRole::TitleBar => Role::TitleBar,
        AtspiRole::ToolBar => Role::Toolbar,
        AtspiRole::ToolTip => Role::Tooltip,
        AtspiRole::Tree => Role::Tree,
        AtspiRole::TreeItem => Role::TreeItem,
        AtspiRole::TreeTable => Role::TreeGrid,
        AtspiRole::Video => Role::Video,

        // -- windows -------------------------------------------------------
        // AT-SPI distinguishes four kinds of top-level surface that AccessKit
        // does not. The distinction is real to a toolkit and invisible to an
        // agent, which addresses whatever the compositor gave a surface to.
        AtspiRole::Window
        | AtspiRole::Frame
        | AtspiRole::DesktopFrame
        | AtspiRole::InternalFrame
        | AtspiRole::InputMethodWindow => Role::Window,

        // -- documents -----------------------------------------------------
        // Five document flavours collapse to one. The flavour describes the
        // file the application opened, not the widget on screen.
        AtspiRole::DocumentFrame
        | AtspiRole::DocumentEmail
        | AtspiRole::DocumentPresentation
        | AtspiRole::DocumentSpreadsheet
        | AtspiRole::DocumentText
        | AtspiRole::DocumentWeb => Role::Document,

        // -- dialogs wearing a specialised name ----------------------------
        // ATK defines each of these as "a specialised dialog that lets the
        // user choose ...". They are dialogs, and calling them anything else
        // would lose the one fact an agent acts on.
        AtspiRole::ColorChooser | AtspiRole::FileChooser | AtspiRole::FontChooser => Role::Dialog,

        // -- panes ---------------------------------------------------------
        // AT-SPI's *Pane family are layout surfaces of a Swing-shaped window.
        // `Panel`, despite the name, is not one of them: ATK defines it as a
        // generic grouping container, so it generalises to Group below.
        AtspiRole::RootPane
        | AtspiRole::LayeredPane
        | AtspiRole::GlassPane
        | AtspiRole::OptionPane => Role::Pane,

        // -- honest generalisations ----------------------------------------
        // No counterpart exists, and every candidate would assert something
        // the bus did not say. Group keeps the node addressable and its
        // subtree reachable, which is what a selector needs from a container.
        AtspiRole::Panel
        | AtspiRole::Filler
        | AtspiRole::Grouping
        | AtspiRole::SplitPane
        | AtspiRole::Viewport
        | AtspiRole::DirectoryPane
        | AtspiRole::HTMLContainer
        | AtspiRole::Calendar
        | AtspiRole::DesktopIcon
        | AtspiRole::Page
        | AtspiRole::Ruler
        | AtspiRole::Autocomplete
        | AtspiRole::FocusTraversable
        | AtspiRole::Subscript
        | AtspiRole::Superscript => Role::Group,

        AtspiRole::Landmark => Role::Region,
        AtspiRole::Notification => Role::Alert,
        AtspiRole::CHART => Role::Figure,
        AtspiRole::Animation | AtspiRole::Arrow => Role::Image,

        // -- no answer -----------------------------------------------------
        // `Invalid` is the bus reporting a role it could not decode.
        // `RedundantObject` is the bus asking to be ignored. `Extended` means
        // "a role outside this enumeration", which is precisely unknowable.
        // All three are Unknown, and Unknown is a statement.
        AtspiRole::Invalid
        | AtspiRole::Unknown
        | AtspiRole::Extended
        | AtspiRole::RedundantObject => Role::Unknown,
    }
}

/// Project AT-SPI's state bits onto a node's own properties.
///
/// # Why `Showing` does not become visibility
///
/// AT-SPI has two state bits that look like the answer to "can I see this":
/// `Visible` ("should be rendered") and `Showing` ("is being rendered"). They
/// are the application's claims about its own widgets, made by the process that
/// drew them, over a bus it volunteered to join. A compositor knows better and
/// can be made to say so; an application cannot be checked.
///
/// So the claim lands on [`Node::set_hidden`], which is part of the
/// *application's* schema and reads as "the app says this is not shown" -- and
/// [`Visibility`](perspicax_node::Visibility) stays `Unknown` for every node
/// this crate produces, exactly as it should until a `HostView` exists. The two
/// facts are kept in two fields because they are two different claims with two
/// different warrants, and collapsing them is the mistake this project was
/// started over.
///
/// The gate does consult it, as a refusal of its own,
/// [`Refusal::NotShowing`](perspicax_index::Refusal::NotShowing), and never as
/// a reason to act. An application's "this is not shown" can only take
/// actability away from its own node, so it can be believed without being
/// checked; its "this is shown" could only add it, so it is never asked.
pub fn apply_states(states: StateSet, node: &mut Node) {
    // The app's own claim, in the app's own field. Not visibility.
    //
    // `Showing` alone decides it, because "is being rendered" is the claim.
    // MEASURED 2026-10-07 against Firefox 148 (issue #32): the selected tab's
    // close button and the site icon in the address bar report `Showing`
    // without `Visible`, which the specification says cannot happen, and both
    // are drawn. Requiring `Visible` as well marked them hidden, and the gate
    // would have refused two controls the person can see. Every node Firefox
    // reports without `Visible` that is not drawn also lacks `Showing`.
    if !states.contains(State::Showing) {
        node.set_hidden();
    }

    // AT-SPI splits "greyed out" (`Sensitive`) from "switched off" (`Enabled`)
    // and AccessKit has one flag, so one of the two bits has to decide it.
    //
    // MEASURED 2026-09-05 against GTK 4 under `GTK_A11Y=atspi`: a plainly
    // clickable button reports `Focusable | Focused | Sensitive | Showing |
    // Visible` and **never** `Enabled` -- not on the button, not on its window,
    // not on any node in the tree. Requiring both bits therefore marked every
    // GTK node disabled, and M3 put that on the wire as `state.disabled` for a
    // control that works, which is the confidently-wrong kind of answer this
    // project exists to stop giving. The tests missed it because their fixtures
    // set both bits, which is what the specification suggests and not what a
    // toolkit does.
    //
    // So the negative claim is the absence of `Sensitive`, which is the bit
    // toolkits actually maintain. `Enabled` will earn a rule here on the day a
    // toolkit is measured setting it.
    if !states.contains(State::Sensitive) {
        node.set_disabled();
    }

    if states.contains(State::Indeterminate) {
        // Checked before Indeterminate would be wrong: a tri-state checkbox
        // mid-cycle reports both, and mixed is the more specific truth.
        node.set_toggled(Toggled::Mixed);
    } else if states.contains(State::Checked) || states.contains(State::Pressed) {
        node.set_toggled(Toggled::True);
    } else if states.contains(State::Checkable) {
        node.set_toggled(Toggled::False);
    }

    if states.contains(State::Expandable) {
        node.set_expanded(states.contains(State::Expanded));
    }
    if states.contains(State::Selected) {
        node.set_selected(true);
    } else if states.contains(State::Selectable) {
        node.set_selected(false);
    }

    if states.contains(State::Horizontal) {
        node.set_orientation(Orientation::Horizontal);
    } else if states.contains(State::Vertical) {
        node.set_orientation(Orientation::Vertical);
    }

    if states.contains(State::Modal) {
        node.set_modal();
    }
    if states.contains(State::Busy) {
        node.set_busy();
    }
    if states.contains(State::Required) {
        node.set_required();
    }
    if states.contains(State::ReadOnly) {
        node.set_read_only();
    }
    if states.contains(State::Multiselectable) {
        node.set_multiselectable();
    }
    if states.contains(State::Visited) {
        node.set_visited();
    }
}

/// Declare the actions a node supports, from what one read already told us.
///
/// Deliberately coarse, and deliberately free. AT-SPI's precise action list
/// lives behind `Action.GetActions`, which is **one D-Bus round trip per node**
/// -- on the slow path that would double an already O(n) walk to produce a list
/// nothing in M1 may act on, since every node here is
/// [`Origin::Unattributed`](perspicax_node::Origin::Unattributed) and therefore
/// refused. So this infers what the interface set and the state bits already
/// carry, and the exact list stays a cost M4 can choose to pay.
///
/// Note what is *not* inferred: nothing here promises the action will work.
/// [`check_actable`](perspicax_index::check_actable) is what decides that, and
/// it will refuse all of these until a compositor exists.
pub fn apply_actions(states: StateSet, interfaces: InterfaceSet, node: &mut Node) {
    // The Action interface is AT-SPI's "this widget does something when you
    // poke it". Its first action is `click` or `activate` on both GTK and Qt.
    if interfaces.contains(Interface::Action) {
        node.add_action(Action::Click);
    }
    if states.contains(State::Focusable) {
        node.add_action(Action::Focus);
    }
    if states.contains(State::Expandable) {
        node.add_action(if states.contains(State::Expanded) {
            Action::Collapse
        } else {
            Action::Expand
        });
    }
    if interfaces.contains(Interface::Value) {
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
        node.add_action(Action::SetValue);
    }
}

/// `Component.GetExtents` as a [`Rect`], or `None` if the answer was not one.
///
/// AT-SPI reports `(x, y, width, height)`; [`Rect`] is kurbo-shaped and is two
/// corners. The conversion is trivial and is written down once here so that no
/// call site has to remember which of the two shapes it is holding.
///
/// # Why this returns an `Option`
///
/// Because applications answer this call with nonsense, and measurably so.
/// Sweeping every node of `gtk4-widget-factory` produces, among thirteen
/// sensible boxes, exactly one reading `(0, 0, -605997344, 22099)` -- GTK
/// answering for a widget it has not realised, with whatever was in the struct.
/// It is not an error; the call succeeds.
///
/// A box with negative area is not a fact about the screen, and letting one
/// into the index would put geometry there that no compositor could later
/// reconcile and that any consumer would reasonably believe. So it is refused
/// here, at the boundary, which is the only place that still knows the number
/// came from an application rather than from a measurement.
///
/// A negative *origin* is left alone: `(-6, -6, 1340, 46)` is a real answer
/// from a widget whose shadow extends past its window, and rejecting it would
/// discard good data to catch a different bug.
///
/// The coordinates are whatever [`CoordType`](atspi::CoordType) was asked for,
/// and this crate only ever asks for `Window`. A Wayland client cannot know
/// where it is on screen, so `Screen` extents are a fiction the bus will
/// nonetheless answer with. See
/// [`ObservedNode::node_space_bounds`](perspicax_node::ObservedNode::node_space_bounds).
#[must_use]
pub fn extents_to_rect((x, y, width, height): (i32, i32, i32, i32)) -> Option<Rect> {
    if width < 0 || height < 0 {
        return None;
    }
    Some(Rect {
        x0: f64::from(x),
        y0: f64::from(y),
        x1: f64::from(x) + f64::from(width),
        y1: f64::from(y) + f64::from(height),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(bits: &[State]) -> StateSet {
        bits.iter().copied().collect()
    }

    #[test]
    fn plain_roles_rename() {
        let none = StateSet::empty();
        assert_eq!(role(AtspiRole::PageTab, none), Role::Tab);
        assert_eq!(role(AtspiRole::ToolBar, none), Role::Toolbar);
        assert_eq!(role(AtspiRole::BlockQuote, none), Role::Blockquote);
    }

    /// The four entries where a state bit changes the role's meaning.
    #[test]
    fn text_without_editable_is_a_label_not_an_input() {
        assert_eq!(role(AtspiRole::Text, states(&[])), Role::Label);
        assert_eq!(
            role(AtspiRole::Text, states(&[State::Editable])),
            Role::TextInput
        );
        assert_eq!(
            role(
                AtspiRole::Text,
                states(&[State::Editable, State::MultiLine])
            ),
            Role::MultilineTextInput
        );
    }

    #[test]
    fn a_default_button_is_distinguishable_from_the_other_three() {
        assert_eq!(role(AtspiRole::Button, states(&[])), Role::Button);
        assert_eq!(
            role(AtspiRole::Button, states(&[State::IsDefault])),
            Role::DefaultButton
        );
    }

    /// The generalise-rather-than-guess rule, stated as a test so that a later
    /// edit "improving" one of these has to argue with it first.
    #[test]
    fn containers_without_a_counterpart_generalise_to_group() {
        let none = StateSet::empty();
        for atspi in [
            AtspiRole::Panel,
            AtspiRole::Filler,
            AtspiRole::Grouping,
            AtspiRole::SplitPane,
            AtspiRole::Viewport,
        ] {
            assert_eq!(role(atspi, none), Role::Group, "{atspi:?}");
        }
    }

    #[test]
    fn undecodable_roles_say_unknown_rather_than_pick_something() {
        let none = StateSet::empty();
        assert_eq!(role(AtspiRole::Invalid, none), Role::Unknown);
        assert_eq!(role(AtspiRole::Extended, none), Role::Unknown);
        assert_eq!(role(AtspiRole::RedundantObject, none), Role::Unknown);
    }

    /// The load-bearing one. `Showing` is the application talking about
    /// itself; it must land on the node and never on visibility.
    #[test]
    fn showing_becomes_a_node_property_never_a_visibility_verdict() {
        let mut shown = Node::new(Role::Button);
        apply_states(
            states(&[
                State::Showing,
                State::Visible,
                State::Sensitive,
                State::Enabled,
            ]),
            &mut shown,
        );
        assert!(!shown.is_hidden());

        let mut not_shown = Node::new(Role::Button);
        apply_states(states(&[State::Sensitive, State::Enabled]), &mut not_shown);
        assert!(not_shown.is_hidden());
    }

    /// The states Firefox 148 reported, measured. Its hidden menu bar is
    /// `Visible` without `Showing`, and is not drawn; its selected tab's close
    /// button is `Showing` without `Visible`, and is. `Showing` is the bit that
    /// tells them apart, because the gate refuses whatever is hidden.
    #[test]
    fn showing_alone_decides_hidden_because_firefox_drops_visible_on_drawn_controls() {
        let mut menu_bar = Node::new(Role::MenuBar);
        apply_states(states(&[State::Visible, State::Sensitive]), &mut menu_bar);
        assert!(menu_bar.is_hidden());

        let mut close_tab = Node::new(Role::Button);
        apply_states(states(&[State::Showing, State::Sensitive]), &mut close_tab);
        assert!(!close_tab.is_hidden());
    }

    #[test]
    fn a_tri_state_checkbox_is_mixed_not_true() {
        let mut node = Node::new(Role::CheckBox);
        apply_states(states(&[State::Checked, State::Indeterminate]), &mut node);
        assert_eq!(node.toggled(), Some(Toggled::Mixed));
    }

    #[test]
    fn an_insensitive_node_is_disabled() {
        let mut node = Node::new(Role::Button);
        apply_states(states(&[State::Showing, State::Visible]), &mut node);
        assert!(node.is_disabled());
    }

    /// The state set a real GTK 4 button actually reports, copied from a
    /// measurement rather than from the specification -- which is the whole
    /// point of the test. Written from the spec, this fixture would carry
    /// `Enabled` as well, and carrying it is what hid the bug: GTK sets
    /// `Sensitive` and never `Enabled`, so a rule requiring both marked every
    /// node in every GTK application unavailable for interaction.
    #[test]
    fn a_real_gtk_button_is_not_disabled() {
        let mut node = Node::new(Role::Button);
        apply_states(
            states(&[
                State::Focusable,
                State::Focused,
                State::Sensitive,
                State::Showing,
                State::Visible,
            ]),
            &mut node,
        );
        assert!(
            !node.is_disabled(),
            "a sensitive GTK button is not disabled"
        );
        assert!(!node.is_hidden());
    }

    #[test]
    fn actions_come_from_interfaces_and_states_not_a_round_trip() {
        let mut node = Node::new(Role::Button);
        apply_actions(
            states(&[State::Focusable]),
            InterfaceSet::new(Interface::Action),
            &mut node,
        );
        assert!(node.supports_action(Action::Click));
        assert!(node.supports_action(Action::Focus));
    }

    #[test]
    fn an_expandable_node_offers_the_move_it_can_actually_make() {
        let mut collapsed = Node::new(Role::TreeItem);
        apply_actions(
            states(&[State::Expandable]),
            InterfaceSet::empty(),
            &mut collapsed,
        );
        assert!(collapsed.supports_action(Action::Expand));
        assert!(!collapsed.supports_action(Action::Collapse));

        let mut expanded = Node::new(Role::TreeItem);
        apply_actions(
            states(&[State::Expandable, State::Expanded]),
            InterfaceSet::empty(),
            &mut expanded,
        );
        assert!(expanded.supports_action(Action::Collapse));
    }

    /// AT-SPI hands over an origin and a size; `Rect` is two corners. Getting
    /// this backwards produces boxes that are plausible and wrong.
    #[test]
    fn extents_become_two_corners_not_an_origin_and_a_size() {
        let rect = extents_to_rect((10, 20, 100, 50)).expect("a real box");
        assert_eq!(
            (rect.x0, rect.y0, rect.x1, rect.y1),
            (10.0, 20.0, 110.0, 70.0)
        );
    }

    /// A widget whose shadow overhangs its window reports a negative origin,
    /// and that is a real answer worth keeping.
    #[test]
    fn a_negative_origin_is_a_real_box() {
        let rect = extents_to_rect((-6, -6, 1340, 46)).expect("a real box");
        assert_eq!((rect.x0, rect.x1), (-6.0, 1334.0));
    }

    /// Observed from `gtk4-widget-factory`: one node in thirteen answers this
    /// call successfully with an uninitialised struct. A negative-area box is
    /// not a fact about the screen, and it stops here.
    #[test]
    fn a_negative_area_box_is_refused_rather_than_believed() {
        assert_eq!(extents_to_rect((0, 0, -605_997_344, 22_099)), None);
        assert_eq!(extents_to_rect((0, 0, 10, -1)), None);
    }
}
