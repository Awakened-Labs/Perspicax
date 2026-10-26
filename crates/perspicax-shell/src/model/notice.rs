//! What the shell says when something the person asked for was refused:
//! the one place its sentences sit together.
//!
//! perspicax sends what kind of thing it refused, and only the words of it
//! that came from the person or the config: the program as `[agent] never`
//! writes it, the config's own reason in a line, the layout as written.
//! The rest of each sentence is the shell's.

/// Something the person asked for, refused, with what its words need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refused {
    /// A window's grant to an agent: it is an X11 window.
    GrantX11,
    /// A window's grant to an agent: nobody knows which program drew it.
    GrantUnattributed,
    /// A window's grant to an agent: its program could not be held.
    GrantUnheld,
    /// A window's grant to an agent: its program, as `[agent] never`
    /// writes it, is one no agent is ever given.
    GrantNever(String),
    /// A save of the config perspicax refused whole: why, in a line.
    Config(String),
    /// A save of the config the shell refused: why, in a line.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the shell's own refused save, next")
    )]
    ShellConfig(String),
    /// A keyboard layout xkb could not compile, as the config writes it.
    Keymap(String),
}

/// What is said of a refusal: a headline, and why under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub(crate) headline: String,
    pub(crate) why: String,
}

/// What the shell says of `refused`.
pub(crate) fn notice(refused: &Refused) -> Notice {
    const NOT_GIVEN: &str = "Not given to an agent";
    let (headline, why) = match refused {
        Refused::GrantX11 => (
            NOT_GIVEN,
            "X11 windows share one server: granting one would grant them all".to_owned(),
        ),
        Refused::GrantUnattributed => (
            NOT_GIVEN,
            "Nobody knows which program drew this window".to_owned(),
        ),
        Refused::GrantUnheld => (
            NOT_GIVEN,
            "Its program can't be held, so its grant couldn't end with it".to_owned(),
        ),
        Refused::GrantNever(program) => (
            NOT_GIVEN,
            format!(
                "No agent is ever given {}",
                if program.is_empty() {
                    "this program"
                } else {
                    program
                }
            ),
        ),
        Refused::Config(why) => ("Config not applied: the one in use is kept", why.clone()),
        Refused::ShellConfig(why) => ("Shell settings not applied", why.clone()),
        Refused::Keymap(layout) => (
            "Keyboard layout not applied",
            if layout.is_empty() {
                "xkb could not compile the layout; the keyboard keeps its old one".to_owned()
            } else {
                format!("xkb could not compile “{layout}”; the keyboard keeps its old one")
            },
        ),
    };
    Notice {
        headline: headline.to_owned(),
        why,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(refused: &Refused) -> (String, String) {
        let Notice { headline, why } = notice(refused);
        (headline, why)
    }

    #[test]
    fn a_refused_grant_says_it_was_not_given_and_which_reason() {
        let whys: Vec<_> = [
            Refused::GrantX11,
            Refused::GrantUnattributed,
            Refused::GrantUnheld,
            Refused::GrantNever("ennius".to_owned()),
        ]
        .iter()
        .map(|refused| {
            let (headline, why) = said(refused);
            assert_eq!(headline, "Not given to an agent", "{refused:?}");
            why
        })
        .collect();
        assert_eq!(
            whys,
            [
                "X11 windows share one server: granting one would grant them all",
                "Nobody knows which program drew this window",
                "Its program can't be held, so its grant couldn't end with it",
                "No agent is ever given ennius",
            ]
        );
        assert_eq!(
            said(&Refused::GrantNever(String::new())).1,
            "No agent is ever given this program",
            "a program not named"
        );
    }

    #[test]
    fn a_refused_save_says_what_is_kept_and_the_reason_as_it_came() {
        let reason = "line 3, column 7: expected `=`";
        assert_eq!(
            said(&Refused::Config(reason.to_owned())),
            (
                "Config not applied: the one in use is kept".to_owned(),
                reason.to_owned()
            )
        );
        assert_eq!(
            said(&Refused::ShellConfig(reason.to_owned())),
            ("Shell settings not applied".to_owned(), reason.to_owned())
        );
    }

    #[test]
    fn a_refused_layout_is_named_as_written() {
        assert_eq!(
            said(&Refused::Keymap("us,nowhere".to_owned())),
            (
                "Keyboard layout not applied".to_owned(),
                "xkb could not compile “us,nowhere”; the keyboard keeps its old one".to_owned()
            )
        );
        assert_eq!(
            said(&Refused::Keymap(String::new())).1,
            "xkb could not compile the layout; the keyboard keeps its old one",
            "no quotes round nothing"
        );
    }
}
