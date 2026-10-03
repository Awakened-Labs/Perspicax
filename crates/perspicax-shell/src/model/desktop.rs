//! A desktop entry: the `.desktop` file an application installs to say what
//! it is called, what it looks like, and how to start it.
//!
//! Read to the Desktop Entry Specification, 1.5, as far as a menu needs it:
//! the `[Desktop Entry]` group's keys, localised for the person's language,
//! with the spec's escapes undone; and `Exec` split into a program and its
//! arguments the way the spec quotes them, so it can be run without a shell.
//!
//! `Exec`'s field codes stand for what an application is opened *with*: the
//! files or links dropped on it. A menu opens it with nothing, so each code
//! is dropped, an argument that was only a code with it, and `%%` is a `%`.
//!
//! An entry that breaks the spec's grammar is refused whole, as GLib refuses
//! it: a line that is not a group, a key or a comment, a quote left open, or
//! no `Name` or `Type`. A menu without it is better than a menu item that
//! runs something half-read.

/// What a menu needs of a desktop entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Entry {
    /// `Type=Application`. The other types, links and directories, are not
    /// programs to run.
    pub(crate) application: bool,
    pub(crate) name: String,
    pub(crate) comment: Option<String>,
    /// An icon's name in the theme, or a path to an image.
    pub(crate) icon: Option<String>,
    /// The program and its arguments, field codes dropped. `None` for an
    /// application started only over D-Bus.
    pub(crate) exec: Option<Vec<String>>,
    /// A program that must be installed for the entry to be shown.
    pub(crate) try_exec: Option<String>,
    /// The folder to run it in.
    pub(crate) path: Option<String>,
    /// Run in a terminal.
    pub(crate) terminal: bool,
    pub(crate) categories: Vec<String>,
    pub(crate) keywords: Vec<String>,
    /// Shown only on these desktops, if any are named.
    pub(crate) only_show_in: Vec<String>,
    /// Never shown on these desktops.
    pub(crate) not_show_in: Vec<String>,
    /// Deleted: as if it were not installed.
    pub(crate) hidden: bool,
    /// Installed, and not for a menu: a helper, or a handler for a file type.
    pub(crate) no_display: bool,
    /// The class its windows carry, for finding the entry from a window.
    pub(crate) wm_class: Option<String>,
}

/// Why an entry was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Malformed {
    #[error("it does not start with a [Desktop Entry] group")]
    NoGroup,
    #[error("line {0} is not a group, a key or a comment")]
    Line(usize),
    #[error("it has no Name")]
    NoName,
    #[error("it has no Type")]
    NoType,
    #[error("{0} is neither true nor false")]
    Boolean(&'static str),
    #[error("its Exec {0}")]
    Exec(&'static str),
}

/// The person's language, as the locale suffixes of a key to try in turn,
/// best first: `sr_YU@Latn` tries `sr_YU@Latn`, `sr_YU`, `sr@Latn` and `sr`,
/// then the key with no suffix.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Locale {
    tried: Vec<String>,
}

impl Locale {
    /// The locale messages are shown in: `LC_ALL`, else `LC_MESSAGES`, else
    /// `LANG`, the first one set.
    pub(crate) fn from_env() -> Self {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
            .map_or_else(Self::default, |value| Self::parse(&value))
    }

    /// A locale written `lang_COUNTRY.ENCODING@MODIFIER`, any part but the
    /// language left out. The encoding never matters.
    pub(crate) fn parse(written: &str) -> Self {
        let (rest, modifier) = match written.split_once('@') {
            Some((rest, modifier)) => (rest, Some(modifier)),
            None => (written, None),
        };
        let rest = rest.split_once('.').map_or(rest, |(before, _)| before);
        let (lang, country) = match rest.split_once('_') {
            Some((lang, country)) => (lang, Some(country)),
            None => (rest, None),
        };
        if lang.is_empty() || lang == "C" || lang == "POSIX" {
            return Self::default();
        }
        let mut tried = Vec::new();
        if let (Some(country), Some(modifier)) = (country, modifier) {
            tried.push(format!("{lang}_{country}@{modifier}"));
        }
        if let Some(country) = country {
            tried.push(format!("{lang}_{country}"));
        }
        if let Some(modifier) = modifier {
            tried.push(format!("{lang}@{modifier}"));
        }
        tried.push(lang.to_owned());
        Self { tried }
    }
}

/// One key's values in the group, by the locale each is written for.
#[derive(Default)]
struct Values<'a> {
    plain: Option<&'a str>,
    localised: Vec<(&'a str, &'a str)>,
}

impl<'a> Values<'a> {
    /// The value for `locale`, or the plain one.
    fn best(&self, locale: &Locale) -> Option<&'a str> {
        locale
            .tried
            .iter()
            .find_map(|wanted| {
                self.localised
                    .iter()
                    .find(|(written, _)| without_encoding(written) == *wanted)
                    .map(|&(_, value)| value)
            })
            .or(self.plain)
    }
}

/// `de_DE.UTF-8@euro` as `de_DE@euro`.
fn without_encoding(locale: &str) -> String {
    match locale.split_once('.') {
        Some((before, after)) => match after.split_once('@') {
            Some((_, modifier)) => format!("{before}@{modifier}"),
            None => before.to_owned(),
        },
        None => locale.to_owned(),
    }
}

/// Read a desktop entry, in `locale`'s language where it has one.
pub(crate) fn parse(text: &str, locale: &Locale) -> Result<Entry, Malformed> {
    let keys = group(text)?;
    let get = |key: &str| keys.get(key);
    let plain = |key: &str| get(key).and_then(|values| values.plain);
    let local = |key: &str| get(key).and_then(|values| values.best(locale));
    let boolean = |key: &'static str| match plain(key) {
        None | Some("false" | "0") => Ok(false),
        Some("true" | "1") => Ok(true),
        Some(_) => Err(Malformed::Boolean(key)),
    };

    let name = local("Name").map(unescape).ok_or(Malformed::NoName)?;
    let kind = plain("Type").ok_or(Malformed::NoType)?;
    Ok(Entry {
        application: kind == "Application",
        name,
        comment: local("Comment").map(unescape),
        icon: local("Icon").map(unescape).filter(|icon| !icon.is_empty()),
        exec: plain("Exec").map(exec).transpose()?,
        try_exec: plain("TryExec").map(unescape).filter(|it| !it.is_empty()),
        path: plain("Path").map(unescape).filter(|it| !it.is_empty()),
        terminal: boolean("Terminal")?,
        categories: plain("Categories").map(list).unwrap_or_default(),
        keywords: local("Keywords").map(list).unwrap_or_default(),
        only_show_in: plain("OnlyShowIn").map(list).unwrap_or_default(),
        not_show_in: plain("NotShowIn").map(list).unwrap_or_default(),
        hidden: boolean("Hidden")?,
        no_display: boolean("NoDisplay")?,
        wm_class: plain("StartupWMClass").map(unescape),
    })
}

/// The `[Desktop Entry]` group's keys, each with its raw values. The file
/// must open with that group, after any comments; later groups, an
/// application's actions, are checked for shape and otherwise skipped.
fn group(text: &str) -> Result<std::collections::HashMap<&str, Values<'_>>, Malformed> {
    let mut keys: std::collections::HashMap<&str, Values<'_>> = std::collections::HashMap::new();
    // `None` before the first group; then whether the lines are the entry's.
    let mut inside = None;
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            if inside.is_none() && name != "Desktop Entry" {
                return Err(Malformed::NoGroup);
            }
            inside = Some(inside.is_none());
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(Malformed::Line(number + 1));
        };
        match inside {
            None => return Err(Malformed::NoGroup),
            Some(false) => continue,
            Some(true) => {}
        }
        let (key, value) = (key.trim_end(), value.trim_start());
        match key.split_once('[') {
            Some((key, locale)) => {
                let Some(locale) = locale.strip_suffix(']') else {
                    return Err(Malformed::Line(number + 1));
                };
                keys.entry(key).or_default().localised.push((locale, value));
            }
            // The first of a key written twice, as GLib takes it.
            None => {
                keys.entry(key).or_default().plain.get_or_insert(value);
            }
        }
    }
    if inside.is_none() {
        return Err(Malformed::NoGroup);
    }
    Ok(keys)
}

/// A string value with the spec's escapes undone: `\s`, `\n`, `\t`, `\r` and
/// `\\`. A backslash before anything else is kept as written.
fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A `;`-separated list, with `\;` a semicolon in an element.
fn list(raw: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(';') => current.push(';'),
                Some(other) => {
                    current.push('\\');
                    current.push(other);
                }
                None => current.push('\\'),
            },
            ';' => items.push(unescape(&std::mem::take(&mut current))),
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        items.push(unescape(&current));
    }
    items.retain(|item| !item.is_empty());
    items
}

/// `Exec` as a program and its arguments: unescaped as a string, split on
/// spaces outside double quotes, quotes undone, and field codes dropped.
fn exec(raw: &str) -> Result<Vec<String>, Malformed> {
    let line = unescape(raw);
    let mut words = Vec::new();
    let mut word: Option<Word> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => {
                if let Some(done) = word.take() {
                    words.extend(done.finish());
                }
            }
            '"' => {
                let word = word.get_or_insert_with(Word::default);
                word.quoted = true;
                loop {
                    match chars.next() {
                        None => return Err(Malformed::Exec("leaves a quote open")),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped @ ('"' | '`' | '$' | '\\')) => word.text.push(escaped),
                            Some(other) => {
                                word.text.push('\\');
                                word.text.push(other);
                            }
                            None => return Err(Malformed::Exec("leaves a quote open")),
                        },
                        Some(other) => word.text.push(other),
                    }
                }
            }
            '\\' => {
                let word = word.get_or_insert_with(Word::default);
                if let Some(escaped) = chars.next() {
                    word.text.push(escaped);
                }
            }
            _ => word.get_or_insert_with(Word::default).text.push(c),
        }
    }
    if let Some(done) = word {
        words.extend(done.finish());
    }
    if words.is_empty() {
        return Err(Malformed::Exec("names no program"));
    }
    Ok(words)
}

/// One argument of `Exec`, as read so far.
#[derive(Default)]
struct Word {
    text: String,
    /// Written in quotes, so an empty one is an empty argument rather than
    /// nothing.
    quoted: bool,
}

impl Word {
    /// The argument with its field codes dropped, or nothing if it was only
    /// codes.
    fn finish(self) -> Option<String> {
        let mut out = String::with_capacity(self.text.len());
        let mut coded = false;
        let mut chars = self.text.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => out.push('%'),
                // A code, or a stray `%` the spec does not allow: dropped
                // either way.
                Some(_) | None => coded = true,
            }
        }
        (!out.is_empty() || (self.quoted && !coded)).then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Entry {
        parse(text, &Locale::default()).expect("a well-formed entry")
    }

    fn exec_of(line: &str) -> Vec<String> {
        read(&format!(
            "[Desktop Entry]\nType=Application\nName=X\nExec={line}\n"
        ))
        .exec
        .expect("an Exec")
    }

    #[test]
    fn a_desktop_entry_reads_name_exec_icon_and_categories() {
        let entry = read(
            "# A comment before the group\n\
             [Desktop Entry]\n\
             Type=Application\n\
             Name = Text Editor\n\
             Comment=Edit text files\n\
             Exec=gedit %U\n\
             Icon=org.gnome.gedit\n\
             Categories=GNOME;GTK;Utility;TextEditor;\n\
             Keywords=text;plaintext;\n\
             StartupWMClass=gedit\n\
             \n\
             [Desktop Action new-window]\n\
             Name=New Window\n\
             Exec=gedit --new-window\n",
        );
        assert_eq!(
            entry,
            Entry {
                application: true,
                name: "Text Editor".to_owned(),
                comment: Some("Edit text files".to_owned()),
                icon: Some("org.gnome.gedit".to_owned()),
                exec: Some(vec!["gedit".to_owned()]),
                categories: ["GNOME", "GTK", "Utility", "TextEditor"]
                    .map(str::to_owned)
                    .to_vec(),
                keywords: vec!["text".to_owned(), "plaintext".to_owned()],
                wm_class: Some("gedit".to_owned()),
                ..Entry::default()
            },
            "and the action's Name and Exec are not the entry's"
        );
    }

    #[test]
    fn a_localised_name_wins_for_the_current_locale() {
        let text = "[Desktop Entry]\nType=Application\nExec=files\n\
                    Name=Files\nName[de]=Dateien\nName[pt_BR]=Arquivos\n\
                    Name[sr@latin]=Datoteke\n";
        let name = |locale: &str| {
            parse(text, &Locale::parse(locale))
                .expect("well-formed")
                .name
        };
        assert_eq!(name("de_AT.UTF-8"), "Dateien", "the language alone");
        assert_eq!(name("pt_BR.UTF-8"), "Arquivos", "language and country");
        assert_eq!(name("pt_PT.UTF-8"), "Files", "not another country's");
        assert_eq!(name("sr_RS@latin"), "Datoteke", "language and modifier");
        assert_eq!(name("C.UTF-8"), "Files");
        assert_eq!(name(""), "Files");
    }

    #[test]
    fn exec_field_codes_are_dropped_and_double_percent_is_a_percent() {
        assert_eq!(exec_of("firefox %u"), ["firefox"]);
        assert_eq!(
            exec_of("app --name=%c %i %k -x %F"),
            ["app", "--name=", "-x"],
            "a code inside an argument leaves the rest of it"
        );
        assert_eq!(exec_of("printf 100%%"), ["printf", "100%"]);
    }

    #[test]
    fn a_quoted_exec_argument_keeps_its_spaces() {
        assert_eq!(
            exec_of(r#""/opt/My App/run" --title "a b" """#),
            ["/opt/My App/run", "--title", "a b", ""],
            "and an empty pair of quotes is an empty argument"
        );
        // Escaped twice: once as a string (`\\` is a backslash), and once
        // inside the quotes (`\"` is a quote).
        assert_eq!(
            exec_of(r#"sh -c "echo \\"hi\\" \\$HOME""#),
            ["sh", "-c", r#"echo "hi" $HOME"#]
        );
        assert_eq!(exec_of(r"echo a\sb"), ["echo", "a", "b"], "\\s is a space");
    }

    #[test]
    fn lists_and_strings_undo_their_escapes() {
        assert_eq!(list(r"a\;b;c;;"), ["a;b", "c"]);
        assert_eq!(unescape(r"tab\there\\ \q"), "tab\there\\ \\q");
    }

    #[test]
    fn a_malformed_entry_is_refused_and_says_why() {
        let refused = |text: &str| parse(text, &Locale::default()).unwrap_err();
        assert_eq!(
            refused("[Desktop Action x]\nName=X\n[Desktop Entry]\nType=Application\nName=Y\n"),
            Malformed::NoGroup
        );
        assert_eq!(refused("Name=X\n"), Malformed::NoGroup);
        assert_eq!(
            refused("[Desktop Entry]\nType=Application\nName=X\nnot a key\n"),
            Malformed::Line(4)
        );
        assert_eq!(
            refused("[Desktop Entry]\nType=Application\nExec=x\n"),
            Malformed::NoName
        );
        assert_eq!(refused("[Desktop Entry]\nName=X\n"), Malformed::NoType);
        assert_eq!(
            refused("[Desktop Entry]\nType=Application\nName=X\nNoDisplay=yes\n"),
            Malformed::Boolean("NoDisplay")
        );
        assert_eq!(
            refused("[Desktop Entry]\nType=Application\nName=X\nExec=\"open\n"),
            Malformed::Exec("leaves a quote open")
        );
        assert_eq!(
            refused("[Desktop Entry]\nType=Application\nName=X\nExec=%U\n"),
            Malformed::Exec("names no program")
        );
    }

    #[test]
    fn the_flags_a_menu_filters_on_are_read() {
        let entry = read(
            "[Desktop Entry]\nType=Application\nName=X\nExec=x\nTerminal=true\n\
             Hidden=false\nNoDisplay=1\nOnlyShowIn=KDE;GNOME;\nNotShowIn=XFCE\n\
             TryExec=/usr/bin/x\nPath=/srv\n",
        );
        assert!(entry.terminal && entry.no_display && !entry.hidden);
        assert_eq!(entry.only_show_in, ["KDE", "GNOME"]);
        assert_eq!(entry.not_show_in, ["XFCE"]);
        assert_eq!(entry.try_exec.as_deref(), Some("/usr/bin/x"));
        assert_eq!(entry.path.as_deref(), Some("/srv"));
        assert!(
            !read("[Desktop Entry]\nType=Link\nName=Docs\nURL=https://example.org\n").application
        );
    }
}
