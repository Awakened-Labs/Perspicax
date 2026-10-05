//! What perspicax draws looks like: a palette and a font, from a theme with a
//! name and whatever colours a person wrote over it.
//!
//! The palette is a set of *roles*, not of widgets: the panel's background,
//! the ink written on it, the colour of whatever is chosen. The compositor
//! draws its titlebars from it and perspicax-shell its panel, menus and
//! desktop labels, so one table recolours the whole desk.
//!
//! A person who writes one colour should not have to write six. Each role
//! that is not written but follows from one that is -- the ink on a panel
//! from the panel, the colour of an open item from the panel and the accent --
//! follows it, by the rules in [`follows`]. A role nobody touched keeps the
//! theme's own value, which is how the default theme stays exactly what
//! perspicax drew before there were themes.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{Index, IndexMut},
};

use crate::Colour;

/// A colour and how opaque it is, as `config.toml` writes it: `"#rrggbb"`, or
/// `"#rrggbbaa"` for the few roles drawn see-through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub const fn opaque(colour: Colour) -> Self {
        Self::new(colour.r, colour.g, colour.b, 0xff)
    }

    /// `"#rrggbb"` (opaque) or `"#rrggbbaa"`, or `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        if !hex.is_ascii() {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
        match hex.len() {
            6 => Some(Self::new(byte(0)?, byte(2)?, byte(4)?, 0xff)),
            8 => Some(Self::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    /// The colour, whatever its opacity.
    #[must_use]
    pub const fn colour(self) -> Colour {
        Colour::rgb(self.r, self.g, self.b)
    }

    #[must_use]
    pub const fn is_opaque(self) -> bool {
        self.a == 0xff
    }
}

impl Colour {
    /// `thousandths` of the way from this colour to `toward`: 0 is this one,
    /// 1000 is `toward`. In whole numbers, so the same colours mix the same on
    /// every machine.
    #[must_use]
    pub fn mix(self, toward: Self, thousandths: u32) -> Self {
        let share = thousandths.min(1000);
        let channel = |from: u8, to: u8| {
            let mixed = (u32::from(from) * (1000 - share) + u32::from(to) * share + 500) / 1000;
            u8::try_from(mixed).unwrap_or(u8::MAX)
        };
        Self::rgb(
            channel(self.r, toward.r),
            channel(self.g, toward.g),
            channel(self.b, toward.b),
        )
    }
}

macro_rules! roles {
    ($($(#[$doc:meta])* $role:ident = $key:literal,)*) => {
        /// A colour's job in what perspicax draws, and its key in
        /// `[theme.palette]`.
        ///
        /// In the order [`follows`]'s rules need: every role comes after the
        /// roles it follows from.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Role {
            $($(#[$doc])* $role,)*
        }

        impl Role {
            /// Every role, in order.
            pub const ALL: &'static [Self] = &[$(Self::$role,)*];

            /// The role's key in `[theme.palette]`.
            #[must_use]
            pub const fn key(self) -> &'static str {
                match self {
                    $(Self::$role => $key,)*
                }
            }
        }
    };
}

roles! {
    /// Whatever is chosen or in use: the menu line under the keyboard, the
    /// task in use, the workspace showing.
    Accent = "accent",
    /// Text written on the accent.
    OnAccent = "on-accent",
    /// The titlebar and border of the window with the keyboard.
    TitleFocused = "title-focused",
    /// Its title and buttons.
    TitleFocusedInk = "title-focused-ink",
    /// Every other window's titlebar and border.
    TitleUnfocused = "title-unfocused",
    /// Their titles and buttons.
    TitleUnfocusedInk = "title-unfocused-ink",
    /// Where a window dragged to an edge would snap to, drawn a quarter
    /// opaque over whatever is there.
    SnapPreview = "snap-preview",
    /// The panel's background.
    Panel = "panel",
    /// Text and symbols on the panel.
    PanelInk = "panel-ink",
    /// The rule along the panel's edge, and the edge of a workspace not
    /// showing.
    PanelRule = "panel-rule",
    /// A task's button, and a workspace not showing.
    PanelFace = "panel-face",
    /// The start button with its menu open, the task in use, the workspace
    /// showing.
    PanelOpen = "panel-open",
    /// The title of a minimized task.
    PanelFaint = "panel-faint",
    /// A menu's background.
    Menu = "menu",
    /// A menu's text.
    MenuInk = "menu-ink",
    /// A menu's border.
    MenuEdge = "menu-edge",
    /// The line between groups of a menu's items.
    MenuRule = "menu-rule",
    /// The line whose submenu is open.
    MenuOpened = "menu-opened",
    /// The start menu's search line.
    MenuTyped = "menu-typed",
    /// The search line's hint, and an item that cannot be chosen.
    MenuHint = "menu-hint",
    /// The desktop icon chosen, drawn see-through over the wallpaper.
    Selected = "selected",
    /// A desktop icon's name.
    LabelInk = "label-ink",
    /// The shadow a desktop icon's name casts, so it reads on any wallpaper.
    LabelShadow = "label-shadow",
}

impl Role {
    /// The role whose key is `key`.
    #[must_use]
    pub fn keyed(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|role| role.key() == key)
    }

    /// Whether this role may be drawn see-through. Everything else is drawn
    /// opaque, and has to be: an opaque menu or panel is what lets the
    /// compositor prove to an agent what it covers.
    #[must_use]
    pub const fn takes_alpha(self) -> bool {
        matches!(self, Self::Selected | Self::LabelShadow)
    }
}

/// How many roles there are.
const ROLES: usize = Role::ALL.len();

/// A colour for every [`Role`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette([Rgba; ROLES]);

impl Palette {
    /// A palette with `colour` for each role.
    #[must_use]
    pub fn from_fn(colour: impl Fn(Role) -> Rgba) -> Self {
        Self(std::array::from_fn(|at| colour(Role::ALL[at])))
    }

    /// This palette with `written` over it, and each role nobody wrote that
    /// follows from one that was written -- or from one that followed in its
    /// turn -- following.
    #[must_use]
    pub fn written_over(mut self, written: &BTreeMap<Role, Rgba>) -> Self {
        let mut moved = BTreeSet::new();
        for &role in Role::ALL {
            if let Some(&colour) = written.get(&role) {
                self[role] = colour;
                moved.insert(role);
            } else if let Some((sources, rule)) = follows(role)
                && sources.iter().any(|source| moved.contains(source))
            {
                self[role] = rule(&self);
                moved.insert(role);
            }
        }
        self
    }
}

impl Index<Role> for Palette {
    type Output = Rgba;

    fn index(&self, role: Role) -> &Rgba {
        &self.0[role as usize]
    }
}

impl IndexMut<Role> for Palette {
    fn index_mut(&mut self, role: Role) -> &mut Rgba {
        &mut self.0[role as usize]
    }
}

/// A rule for a role nobody wrote: what it follows from, and how.
type Rule = (&'static [Role], fn(&Palette) -> Rgba);

/// How `role` follows from others when nobody wrote it, if it does.
///
/// The shares were read off the `perspicax` theme's own hand-picked colours,
/// so that a person's panel or menu colour gets the same relations between
/// its parts that the default has. A test holds the two together.
#[must_use]
pub fn follows(role: Role) -> Option<Rule> {
    use Role::*;

    fn ink(palette: &Palette, on: Role) -> Rgba {
        Rgba::opaque(palette[on].colour().ink())
    }
    fn mix(palette: &Palette, from: Role, toward: Role, thousandths: u32) -> Rgba {
        Rgba::opaque(
            palette[from]
                .colour()
                .mix(palette[toward].colour(), thousandths),
        )
    }

    let rule: Rule = match role {
        OnAccent => (&[Accent], |p| ink(p, Accent)),
        TitleFocusedInk => (&[TitleFocused], |p| ink(p, TitleFocused)),
        TitleUnfocusedInk => (&[TitleUnfocused], |p| ink(p, TitleUnfocused)),
        PanelInk => (&[Panel], |p| ink(p, Panel)),
        PanelRule => (&[Panel, PanelInk], |p| mix(p, Panel, PanelInk, 120)),
        PanelFace => (&[Panel, PanelInk], |p| mix(p, Panel, PanelInk, 75)),
        PanelOpen => (&[Panel, Accent], |p| mix(p, Panel, Accent, 300)),
        PanelFaint => (&[Panel, PanelInk], |p| mix(p, Panel, PanelInk, 570)),
        MenuInk => (&[Menu], |p| ink(p, Menu)),
        MenuEdge => (&[Menu, MenuInk], |p| mix(p, Menu, MenuInk, 420)),
        MenuRule => (&[Menu, MenuInk], |p| mix(p, Menu, MenuInk, 150)),
        MenuOpened => (&[Menu, Accent], |p| mix(p, Menu, Accent, 300)),
        MenuTyped => (&[Menu, MenuInk], |p| mix(p, Menu, MenuInk, 60)),
        MenuHint => (&[Menu, MenuInk], |p| mix(p, Menu, MenuInk, 550)),
        Selected => (&[Accent], |p| Rgba {
            a: 0x66,
            ..p[Accent]
        }),
        Accent | TitleFocused | TitleUnfocused | SnapPreview | Panel | Menu | LabelInk
        | LabelShadow => return None,
    };
    Some(rule)
}

/// The themes perspicax knows by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Builtin {
    /// What perspicax drew before there were themes: blue titlebars, a dark
    /// panel, light menus.
    #[default]
    Perspicax,
    /// After Plasma's Breeze, light.
    BreezeLight,
    /// After Plasma's Breeze, dark.
    BreezeDark,
}

impl Builtin {
    pub const ALL: [Self; 3] = [Self::Perspicax, Self::BreezeLight, Self::BreezeDark];

    /// Its name in `[theme] name`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Perspicax => "perspicax",
            Self::BreezeLight => "breeze-light",
            Self::BreezeDark => "breeze-dark",
        }
    }

    /// The theme called `name`.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.name() == name)
    }

    /// Its palette.
    ///
    /// The two Breezes write their window, view, header and selection
    /// colours, from Plasma's colour schemes, over this theme's and let the
    /// rest follow; the desktop's labels and the snap preview are the same in
    /// all three.
    #[must_use]
    pub fn palette(self) -> Palette {
        use Role::*;
        let written = |pairs: &[(Role, u32)]| -> BTreeMap<Role, Rgba> {
            pairs
                .iter()
                .map(|&(role, hex)| (role, opaque(hex)))
                .collect()
        };
        match self {
            Self::Perspicax => Palette::from_fn(perspicax),
            Self::BreezeLight => Palette::from_fn(perspicax).written_over(&written(&[
                (Accent, 0x3daee9),
                (TitleFocused, 0xdee0e2),
                (TitleFocusedInk, 0x232629),
                (TitleUnfocused, 0xeff0f1),
                (TitleUnfocusedInk, 0x707d8a),
                (Panel, 0xeff0f1),
                (PanelInk, 0x232629),
                (Menu, 0xffffff),
                (MenuInk, 0x232629),
            ])),
            Self::BreezeDark => Palette::from_fn(perspicax).written_over(&written(&[
                (Accent, 0x3daee9),
                (TitleFocused, 0x292c30),
                (TitleFocusedInk, 0xfcfcfc),
                (TitleUnfocused, 0x202326),
                (TitleUnfocusedInk, 0xa1a9b1),
                (Panel, 0x2a2e32),
                (PanelInk, 0xfcfcfc),
                (Menu, 0x202326),
                (MenuInk, 0xfcfcfc),
            ])),
        }
    }
}

/// `0xrrggbb`, opaque.
const fn opaque(hex: u32) -> Rgba {
    let [_, r, g, b] = hex.to_be_bytes();
    Rgba::new(r, g, b, 0xff)
}

/// The `perspicax` theme: every colour perspicax drew before there were
/// themes, so that the default changes nothing anyone can see.
const fn perspicax(role: Role) -> Rgba {
    use Role::*;
    match role {
        Accent => opaque(0x3daee9),
        OnAccent | TitleFocusedInk | TitleUnfocusedInk | LabelInk => opaque(0xffffff),
        TitleFocused => opaque(0x2d6fa3),
        TitleUnfocused => opaque(0x475057),
        SnapPreview => opaque(0x8cb3f2),
        Panel => opaque(0x232629),
        PanelInk => opaque(0xfcfcfc),
        PanelRule => opaque(0x3b4045),
        PanelFace => opaque(0x31363b),
        PanelOpen => opaque(0x2b4f63),
        PanelFaint => opaque(0x9aa0a6),
        Menu => opaque(0xfcfcfc),
        MenuInk => opaque(0x232629),
        MenuEdge => opaque(0xa0a4a8),
        MenuRule => opaque(0xdcdee0),
        MenuOpened => opaque(0xc4e5f7),
        MenuTyped => opaque(0xeff0f1),
        MenuHint => opaque(0x7f8c8d),
        Selected => Rgba::new(0x3d, 0xae, 0xe9, 0x66),
        LabelShadow => Rgba::new(0, 0, 0, 0xc0),
    }
}

/// A font family: one of the three every system has a choice for, or one
/// named.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    SansSerif,
    Serif,
    Monospace,
    Named(String),
}

impl Family {
    /// `"sans-serif"`, `"serif"` and `"monospace"` are the system's choices;
    /// anything else names a family.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "sans-serif" => Self::SansSerif,
            "serif" => Self::Serif,
            "monospace" => Self::Monospace,
            named => Self::Named(named.to_owned()),
        }
    }
}

/// The font the shell writes in, and the titles' family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Font {
    pub family: Family,
    /// The shell's text, in logical pixels. A title's size follows its bar's
    /// height instead.
    pub size: u16,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: Family::SansSerif,
            size: 14,
        }
    }
}

/// How perspicax draws: the `[theme]` table, decided.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Theme {
    /// The theme the palette started from.
    pub builtin: Builtin,
    pub palette: Palette,
    pub font: Font,
}

impl Default for Palette {
    fn default() -> Self {
        Builtin::Perspicax.palette()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_written_with_or_without_their_opacity() {
        assert_eq!(
            Rgba::parse("#3daee9"),
            Some(Rgba::new(0x3d, 0xae, 0xe9, 0xff))
        );
        assert_eq!(Rgba::parse("#000000c0"), Some(Rgba::new(0, 0, 0, 0xc0)));
        for wrong in ["3daee9", "#3dae", "#3daee9c", "#gggggg", "#3daeé9", "blue"] {
            assert_eq!(Rgba::parse(wrong), None, "{wrong}");
        }
    }

    /// The default theme is a promise that nothing changed: these are the
    /// colours the compositor and the shell drew before there were themes.
    #[test]
    fn the_default_theme_is_what_perspicax_drew_before_themes() {
        let palette = Palette::default();
        let hex = |role| {
            let Rgba { r, g, b, a } = palette[role];
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        };
        assert_eq!(hex(Role::TitleFocused), "#2d6fa3ff");
        assert_eq!(hex(Role::TitleUnfocused), "#475057ff");
        assert_eq!(hex(Role::Panel), "#232629ff");
        assert_eq!(hex(Role::PanelOpen), "#2b4f63ff");
        assert_eq!(hex(Role::PanelFace), "#31363bff");
        assert_eq!(hex(Role::Menu), "#fcfcfcff");
        assert_eq!(hex(Role::Selected), "#3daee966");
        // The titles' ink was always whichever of black and white read.
        assert_eq!(
            palette[Role::TitleFocusedInk].colour(),
            palette[Role::TitleFocused].colour().ink()
        );
        assert_eq!(
            palette[Role::TitleUnfocusedInk].colour(),
            palette[Role::TitleUnfocused].colour().ink()
        );
    }

    #[test]
    fn every_role_follows_only_from_roles_before_it() {
        for &role in Role::ALL {
            if let Some((sources, _)) = follows(role) {
                for source in sources {
                    assert!(
                        source < &role,
                        "{role:?} follows {source:?}, which comes after it"
                    );
                }
            }
        }
    }

    /// The rules were read off the default theme's own colours, and are
    /// held to them: a panel or menu a person colours gets the relations
    /// between its parts the default has, to within a few levels.
    ///
    /// The inks are given rather than followed here. An ink is a choice
    /// between black and white, and Breeze, which the default follows, made
    /// some by eye: white on its blue accent, where the rule says black
    /// reads better.
    #[test]
    fn the_rules_give_back_the_default_themes_own_choices() {
        let default = Palette::default();
        let sources: BTreeMap<Role, Rgba> = [
            Role::Accent,
            Role::OnAccent,
            Role::TitleFocused,
            Role::TitleUnfocused,
            Role::Panel,
            Role::PanelInk,
            Role::Menu,
            Role::MenuInk,
        ]
        .into_iter()
        .map(|role| (role, default[role]))
        .collect();
        let followed = Palette::from_fn(|_| Rgba::new(0, 0, 0, 0)).written_over(&sources);
        for &role in Role::ALL {
            if sources.contains_key(&role) || follows(role).is_none() {
                continue;
            }
            let (got, want) = (followed[role], default[role]);
            let off = [
                got.r.abs_diff(want.r),
                got.g.abs_diff(want.g),
                got.b.abs_diff(want.b),
                got.a.abs_diff(want.a),
            ];
            assert!(
                off.iter().all(|&off| off <= 6),
                "{role:?}: followed {got:?}, the default has {want:?}"
            );
        }
    }

    #[test]
    fn a_written_colour_carries_what_follows_from_it_and_nothing_else() {
        let accent = Rgba::new(0xe9, 0x3d, 0x5a, 0xff);
        let palette = Palette::default().written_over(&BTreeMap::from([(Role::Accent, accent)]));
        assert_eq!(palette[Role::Accent], accent);
        assert_eq!(palette[Role::Selected], Rgba { a: 0x66, ..accent });
        assert_ne!(
            palette[Role::PanelOpen],
            Palette::default()[Role::PanelOpen]
        );
        assert_ne!(
            palette[Role::MenuOpened],
            Palette::default()[Role::MenuOpened]
        );
        // The panel itself was not written and follows from nothing written.
        assert_eq!(palette[Role::Panel], Palette::default()[Role::Panel]);
        assert_eq!(palette[Role::PanelInk], Palette::default()[Role::PanelInk]);
    }

    #[test]
    fn a_colour_written_wins_over_one_that_would_follow() {
        let ink = Rgba::new(0xff, 0, 0, 0xff);
        let palette = Palette::default().written_over(&BTreeMap::from([
            (Role::Panel, Rgba::new(0xff, 0xff, 0xff, 0xff)),
            (Role::PanelInk, ink),
        ]));
        assert_eq!(palette[Role::PanelInk], ink);
        // And what follows from both follows from the written ink.
        assert_eq!(
            palette[Role::PanelFaint].colour(),
            Colour::rgb(0xff, 0xff, 0xff).mix(ink.colour(), 570)
        );
    }

    #[test]
    fn the_breezes_are_light_and_dark_and_keep_the_desktops_labels() {
        let light = Builtin::BreezeLight.palette();
        let dark = Builtin::BreezeDark.palette();
        assert_eq!(
            light[Role::PanelInk].colour().ink(),
            Colour::rgb(0xff, 0xff, 0xff)
        );
        assert_eq!(dark[Role::PanelInk].colour().ink(), Colour::rgb(0, 0, 0));
        assert_eq!(light[Role::MenuInk].colour(), Colour::rgb(0x23, 0x26, 0x29));
        for palette in [light, dark] {
            assert_eq!(palette[Role::LabelInk], Palette::default()[Role::LabelInk]);
            assert_eq!(
                palette[Role::LabelShadow],
                Palette::default()[Role::LabelShadow]
            );
            for &role in Role::ALL {
                assert!(
                    role.takes_alpha() || palette[role].is_opaque(),
                    "{role:?} is see-through"
                );
            }
        }
    }

    #[test]
    fn every_theme_is_found_by_its_name_and_every_role_by_its_key() {
        for theme in Builtin::ALL {
            assert_eq!(Builtin::named(theme.name()), Some(theme));
        }
        assert_eq!(Builtin::named("Breeze"), None);
        for &role in Role::ALL {
            assert_eq!(Role::keyed(role.key()), Some(role));
        }
        assert_eq!(Role::keyed("background"), None);
    }

    #[test]
    fn the_three_generic_families_are_the_systems_and_the_rest_are_names() {
        assert_eq!(Family::parse("monospace"), Family::Monospace);
        assert_eq!(
            Family::parse("Noto Sans"),
            Family::Named("Noto Sans".to_owned())
        );
    }
}
