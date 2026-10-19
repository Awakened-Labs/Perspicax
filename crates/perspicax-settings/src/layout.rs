//! Which lines a write touches.
//!
//! toml_edit decides what a value *is*; this module decides the line it lives
//! on. toml_edit is good at the first and has no opinion worth having on the
//! second: it renders a table it had to make *before* whatever text trailed
//! the last one, so a table the file already has an example of would land
//! above the file's own comments, with the example further down, one `#` away
//! from defining it twice. So a write goes where a person would put it:
//!
//! - **A set fills the file's own example.** `# [splash]` becomes `[splash]`
//!   and `# border = 2` becomes `border = …`, where they stand, so a table and
//!   a key each have one spelling. A table the file has no example of is
//!   appended at the end, never above the file's leading comments.
//! - **A reset deletes the key's own lines and nothing else.** A header left
//!   with nothing under it goes too, unless commented-out keys still sit in
//!   its section: then it goes back to being a comment, so they keep their
//!   header and stay safe to uncomment.
//! - **An entry of an array of tables is a table found by its `name`.**
//!   `[[output]]` holding `name = "DP-1"` is `output.DP-1`. A new one goes
//!   after the last entry, or at the end of the file; one left holding only
//!   its name is empty, and goes, or goes back to being a comment, header
//!   and name together, as an emptied table does.
//!
//! Two rules keep a line edit from changing what the file means beyond what
//! the write asked for. A line is classified by parsing it ([`declares`]),
//! never by matching brackets, so prose shaped like a header stays prose.
//! And every uncommenting is checked by [`adds_only`]: the file after must
//! mean exactly the file before plus the one table or key being made. The
//! scan proposes; the check decides, so a wrong guess can cost a tidy layout
//! but never the file's meaning.
//!
//! The rules, and most of the code, are ennius-config's, where the template
//! `ennius init` scaffolds taught them. Text here is LF: a file is read as LF
//! on the way in, as toml_edit already writes it on the way out.

use std::ops::Range;

use toml_edit::{ArrayOfTables, Document as Parsed, DocumentMut, Item, Key, Table, TableLike};

/// The arrays of tables a config holds, each entry found by its [`NAME`].
/// Known here, not read from the file, because a file with no entry yet
/// cannot say that `output` is an array.
const ARRAYS: &[&[&str]] = &[&["output"]];

/// The key an entry of an array of tables is found by.
pub(crate) const NAME: &str = "name";

/// `path` as the entry it names, if it names one: the array's path, and the
/// entry's name.
fn entry_of<'a>(path: &'a [&'a str]) -> Option<(&'a [&'a str], &'a str)> {
    let (name, array) = path.split_last()?;
    ARRAYS.contains(&array).then_some((array, *name))
}

/// The entry of `entries` named `name`.
fn named<'a>(entries: &'a ArrayOfTables, name: &str) -> Option<&'a Table> {
    entries
        .iter()
        .find(|entry| entry.get(NAME).and_then(Item::as_str) == Some(name))
}

/// The entry of `entries` named `name`, in plain TOML.
fn named_plain<'a>(entries: &'a [toml::Value], name: &str) -> Option<&'a toml::Table> {
    entries
        .iter()
        .filter_map(toml::Value::as_table)
        .find(|entry| entry.get(NAME).and_then(toml::Value::as_str) == Some(name))
}

/// The table `path` leads to in `root`, stepping into an array's entry by
/// its name; `None` where there is none yet.
///
/// # Errors
///
/// The segment that is a value, an inline table or an array of tables
/// this module does not know, and so has no table to step into.
fn walk<'a>(root: &'a Table, path: &[&str]) -> Result<Option<&'a Table>, String> {
    let mut table = root;
    let mut at = 0;
    while let Some(seg) = path.get(at) {
        at += 1;
        let next = match table.get(seg) {
            None => return Ok(None),
            Some(Item::Table(next)) => next,
            Some(Item::ArrayOfTables(entries)) if ARRAYS.contains(&&path[..at]) => {
                let Some(name) = path.get(at) else {
                    return Err((*seg).to_owned());
                };
                at += 1;
                match named(entries, name) {
                    Some(entry) => entry,
                    None => return Ok(None),
                }
            }
            Some(_) => return Err((*seg).to_owned()),
        };
        table = next;
    }
    Ok(Some(table))
}

/// Make room in `text` for `leaf` in `table` before a set writes it: give
/// the table a header of its own, then uncomment the key's example if its
/// section has one. Returns `text` unchanged where the table cannot take a
/// header -- a table made of dotted keys, or a path through a value -- and
/// the caller's toml_edit insert does what it always did.
pub(crate) fn make_room(text: &str, table: &[&str], leaf: &str) -> String {
    if table.is_empty() {
        return room_at_the_top(text, leaf);
    }
    let text = match header_of(text, table) {
        Header::Line(_) => text.to_owned(),
        Header::Absent => give_header(text, table),
        Header::Unreachable => return text.to_owned(),
    };
    fill_key(&text, table, leaf)
}

/// Make room for `leaf` at the top of the file, above every table: its
/// commented-out example there filled, or else a line made for it after the
/// file's opening paragraph of comments. toml_edit would put a top-level key
/// above that paragraph in a file that has none yet, because it hangs a
/// file's opening comments on the first table's header. Where the file has a
/// top-level key already, toml_edit adds the new one after it, and that is
/// left to it.
fn room_at_the_top(text: &str, leaf: &str) -> String {
    let Ok(before) = text.parse::<toml::Table>() else {
        return text.to_owned();
    };
    let Ok(parsed) = Parsed::parse(text) else {
        return text.to_owned();
    };
    if before.contains_key(leaf) || parsed.as_table().iter().any(|(_, item)| item.is_value()) {
        return text.to_owned();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let first_header = (0..lines.len())
        .find(|&i| is_header(lines[i]))
        .unwrap_or(lines.len());
    for (i, line) in lines.iter().enumerate().take(first_header) {
        let Some(live) = uncomment(line) else {
            continue;
        };
        if declares(&live) != Some(Declares::Key(vec![leaf.to_owned()])) {
            continue;
        }
        let candidate = splice(text, i..i + 1, Some(&live));
        if adds_only(&before, &candidate, &[], Some(leaf)) {
            return candidate;
        }
    }
    let blank = |i: usize| lines.get(i).is_none_or(|line| line.trim().is_empty());
    let comments = lines[..first_header]
        .iter()
        .take_while(|line| uncomment(line).is_some())
        .count();
    let at = if comments > 0 && comments < first_header && blank(comments) {
        comments + 1
    } else {
        0
    };
    // A value of any kind holds the place: the set replaces it, keeping the
    // spacing the line was made with.
    let mut made = format!("{} = \"\"\n", Key::new(leaf));
    if !blank(at) {
        made.push('\n');
    }
    let candidate = splice(text, at..at, Some(&made));
    if adds_only(&before, &candidate, &[], Some(leaf)) {
        candidate
    } else {
        text.to_owned()
    }
}

/// Remove everything at `path` from `text` -- a key, or a whole table --
/// then retire the headers that left empty: each table under `path`,
/// deepest first, then each ancestor on the way back up. A key inside a
/// table written inline (`flip = { edge = true }`) is taken out of it, and
/// the table, once empty, goes with its line.
///
/// # Errors
///
/// A path through a value, named by the segment that is not a table.
pub(crate) fn remove(text: &str, path: &[&str]) -> Result<String, String> {
    if through_inline(text, path) {
        return remove_inline(text, path);
    }
    let mut text = text.to_owned();
    // One value at a time: a span is good for the text it was parsed from
    // and no other, so every deletion re-parses.
    while let Some(lines) = next_value(&text, path)? {
        text = splice(&text, lines, None);
    }
    let mut emptied = tables_under(&text, path);
    emptied.extend((1..path.len()).rev().map(|n| owned(&path[..n])));
    for table in emptied {
        let table: Vec<&str> = table.iter().map(String::as_str).collect();
        text = retire_if_empty(&text, &table);
    }
    Ok(text)
}

/// Whether `path` leads into a table written inline.
fn through_inline(text: &str, path: &[&str]) -> bool {
    let Ok(doc) = Parsed::parse(text) else {
        return false;
    };
    let Some((_, parents)) = path.split_last() else {
        return false;
    };
    let mut table: &dyn TableLike = doc.as_table();
    for seg in parents {
        match table.get(seg) {
            Some(Item::Value(toml_edit::Value::InlineTable(_))) => return true,
            Some(item) => match item.as_table_like() {
                Some(next) => table = next,
                None => return false,
            },
            None => return false,
        }
    }
    false
}

/// [`remove`] for a key in a table written inline: toml_edit takes it out,
/// and a table it leaves empty is removed in turn.
fn remove_inline(text: &str, path: &[&str]) -> Result<String, String> {
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|error| error.to_string())?;
    let Some((leaf, parents)) = path.split_last() else {
        return Ok(text.to_owned());
    };
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for seg in parents {
        let Some(next) = table.get_mut(seg) else {
            return Ok(text.to_owned());
        };
        table = next
            .as_table_like_mut()
            .ok_or_else(|| format!("`{seg}` in `{}` is not a table", path.join(".")))?;
    }
    table.remove(leaf);
    let emptied = table.is_empty();
    let text = doc.to_string();
    if emptied {
        remove(&text, parents)
    } else {
        Ok(text)
    }
}

/// What one line of TOML, read on its own, declares.
#[derive(Debug, PartialEq)]
enum Declares {
    /// A `[a.b]` header, as its path.
    Table(Vec<String>),
    /// A `[[a.b]]` header: an entry of the array at its path.
    Entry(Vec<String>),
    /// A `key = value` line, as its path: one segment, or a dotted key's.
    Key(Vec<String>),
}

/// Parse `line` alone and say what it declares, if anything: a comment, a
/// blank, prose and half a multi-line value all declare nothing.
fn declares(line: &str) -> Option<Declares> {
    let doc: DocumentMut = line.parse().ok()?;
    let (key, mut item) = only_entry(doc.as_table())?;
    let mut path = vec![key.to_owned()];
    loop {
        match item {
            Item::Value(_) => return Some(Declares::Key(path)),
            // `[a.b]` leaves `a` implicit and `a.b = 1` leaves it dotted:
            // either way, the declaration is one level down.
            Item::Table(t) if t.is_implicit() || t.is_dotted() => {
                let (key, next) = only_entry(t)?;
                path.push(key.to_owned());
                item = next;
            }
            Item::Table(t) if t.is_empty() => return Some(Declares::Table(path)),
            Item::ArrayOfTables(entries) if entries.iter().all(Table::is_empty) => {
                return Some(Declares::Entry(path));
            }
            _ => return None,
        }
    }
}

fn only_entry(table: &Table) -> Option<(&str, &Item)> {
    let mut entries = table.iter();
    let entry = entries.next()?;
    entries.next().is_none().then_some(entry)
}

/// `line` with its comment marker taken off -- the indent kept, `#` and at
/// most one space after it dropped -- or `None` if it is not a comment.
fn uncomment(line: &str) -> Option<String> {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    let rest = body.strip_prefix('#')?;
    Some(format!(
        "{indent}{}",
        rest.strip_prefix(' ').unwrap_or(rest)
    ))
}

/// The exact inverse of [`uncomment`].
fn comment_out(line: &str) -> String {
    let body = line.trim_start();
    format!("{}# {body}", &line[..line.len() - body.len()])
}

fn declares_table(line: &str, table: &[&str]) -> bool {
    matches!(declares(line), Some(Declares::Table(path)) if path == table)
}

/// A header, live or commented out: where a section ends.
fn is_header(line: &str) -> bool {
    let live = |l: &str| matches!(declares(l), Some(Declares::Table(_) | Declares::Entry(_)));
    live(line) || uncomment(line).is_some_and(|l| live(&l))
}

/// The lines of the section `header` opens: up to the next header, live or
/// commented out.
fn section(lines: &[&str], header: usize) -> Range<usize> {
    let start = header + 1;
    let end = (start..lines.len())
        .find(|&i| is_header(lines[i]))
        .unwrap_or(lines.len());
    start..end
}

/// Where `table`'s header is, if it has one of its own.
enum Header {
    /// On this line.
    Line(usize),
    /// Absent, or implicit: the table exists only through its subtables.
    Absent,
    /// Made of dotted keys, or behind a value: it cannot take a header.
    Unreachable,
}

fn header_of(text: &str, table: &[&str]) -> Header {
    let Ok(doc) = Parsed::parse(text) else {
        return Header::Unreachable;
    };
    let current = match walk(doc.as_table(), table) {
        Ok(Some(current)) => current,
        Ok(None) => return Header::Absent,
        Err(_) => return Header::Unreachable,
    };
    if current.is_dotted() {
        return Header::Unreachable;
    }
    match current.span() {
        Some(span) if !current.is_implicit() => Header::Line(line_of(text, span.start)),
        _ => Header::Absent,
    }
}

/// Give `table` a header: its commented-out example uncommented, or failing
/// that one appended at the end of the file.
fn give_header(text: &str, table: &[&str]) -> String {
    let Ok(before) = text.parse::<toml::Table>() else {
        return text.to_owned();
    };
    if let Some((array, name)) = entry_of(table) {
        return give_entry(text, &before, array, name);
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let Some(live) = uncomment(line) else {
            continue;
        };
        if !declares_table(&live, table) {
            continue;
        }
        let candidate = splice(text, i..i + 1, Some(&live));
        if adds_only(&before, &candidate, table, None) {
            return candidate;
        }
    }
    let appended = append_header(text, table);
    if adds_only(&before, &appended, table, None) {
        appended
    } else {
        text.to_owned()
    }
}

/// `text` with `[table]` after its last line, a blank line between.
fn append_header(text: &str, table: &[&str]) -> String {
    let path: Vec<String> = table.iter().map(|seg| Key::new(*seg).to_string()).collect();
    append(text, &format!("[{}]\n", path.join(".")))
}

/// `text` with `lines` after its last line, a blank line between.
fn append(text: &str, lines: &str) -> String {
    let mut out = text.to_owned();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
    }
    out.push_str(lines);
    out
}

/// Give the array at `array` an entry named `name`: its commented-out
/// example uncommented, header and name together, or else a new one after
/// the last entry, or at the end of the file where there is none.
fn give_entry(text: &str, before: &toml::Table, array: &[&str], name: &str) -> String {
    let mut table = array.to_vec();
    table.push(name);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let names_it = |line: &str| {
        uncomment(line)
            .and_then(|live| live.parse::<toml::Table>().ok())
            .is_some_and(|live| live.get(NAME).and_then(toml::Value::as_str) == Some(name))
    };
    for (i, line) in lines.iter().enumerate() {
        let declares_entry = uncomment(line)
            .is_some_and(|live| declares(&live) == Some(Declares::Entry(owned(array))));
        if !declares_entry {
            continue;
        }
        let Some(at) = section(&lines, i).find(|&j| names_it(lines[j])) else {
            continue;
        };
        let mut candidate = lines
            .iter()
            .map(|line| (*line).to_owned())
            .collect::<Vec<_>>();
        for j in [i, at] {
            candidate[j] = uncomment(lines[j]).unwrap_or_default();
        }
        let candidate = candidate.concat();
        if adds_only(before, &candidate, &table, None) {
            return candidate;
        }
    }
    let header: Vec<String> = array.iter().map(|seg| Key::new(*seg).to_string()).collect();
    let made = format!(
        "[[{}]]\n{NAME} = {}\n",
        header.join("."),
        toml_edit::Value::from(name)
    );
    let candidate = match last_entry_ends(text, array) {
        Some(end) => {
            let blank = |i: usize| lines.get(i).is_none_or(|line| line.trim().is_empty());
            let mut made = format!("\n{made}");
            if !blank(end) {
                made.push('\n');
            }
            splice(text, end..end, Some(&made))
        }
        None => append(text, &made),
    };
    if adds_only(before, &candidate, &table, None) {
        candidate
    } else {
        text.to_owned()
    }
}

/// The line after the last entry of the array at `array`: after its header,
/// or its last value. Prose under it is the next table's, as often as not,
/// so a new entry goes above that.
fn last_entry_ends(text: &str, array: &[&str]) -> Option<usize> {
    let doc = Parsed::parse(text).ok()?;
    let (leaf, parents) = array.split_last()?;
    let Item::ArrayOfTables(entries) = walk(doc.as_table(), parents).ok()??.get(leaf)? else {
        return None;
    };
    let last = entries.iter().last()?;
    let header = line_of(text, last.span()?.start);
    let values = last
        .iter()
        .filter_map(|(key, _)| value_lines(text, last, key))
        .map(|lines| lines.end);
    Some(values.fold(header + 1, usize::max))
}

/// Uncomment `leaf`'s example in `table`'s section, if the table has no live
/// `leaf` and the section holds one.
fn fill_key(text: &str, table: &[&str], leaf: &str) -> String {
    let Header::Line(header) = header_of(text, table) else {
        return text.to_owned();
    };
    let Ok(before) = text.parse::<toml::Table>() else {
        return text.to_owned();
    };
    if table_in(&before, table).is_some_and(|t| t.contains_key(leaf)) {
        return text.to_owned();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for i in section(&lines, header) {
        let Some(live) = uncomment(lines[i]) else {
            continue;
        };
        if declares(&live) != Some(Declares::Key(vec![leaf.to_owned()])) {
            continue;
        }
        let candidate = splice(text, i..i + 1, Some(&live));
        if adds_only(&before, &candidate, table, Some(leaf)) {
            return candidate;
        }
    }
    text.to_owned()
}

/// Whether `after` means exactly `before` plus a table at `table` (made if
/// absent) and, when `leaf` is given, that table's `leaf` (absent before).
///
/// This is what makes a line edit safe to make blind. An uncommented header
/// that would capture live keys below it moves them, and the schema cannot
/// see the move when the keys exist in both tables (`focused`, `unfocused`
/// …); a `# [x]` inside a multi-line string changes the string; an example
/// that is not valid TOML does not parse. Each of those means something
/// other than the write asked for, and is refused here.
fn adds_only(before: &toml::Table, after: &str, table: &[&str], leaf: Option<&str>) -> bool {
    let Ok(after) = after.parse::<toml::Table>() else {
        return false;
    };
    let mut expected = before.clone();
    let Some(made) = make_table(&mut expected, table) else {
        return false;
    };
    if let Some(leaf) = leaf {
        let Some(value) = table_in(&after, table).and_then(|t| t.get(leaf)) else {
            return false;
        };
        if made.insert(leaf.to_owned(), value.clone()).is_some() {
            return false;
        }
    }
    expected == after
}

/// The table at `path` in plain TOML, stepping into an array's entry by its
/// name, as [`walk`] does in the person's.
pub(crate) fn table_in<'a>(root: &'a toml::Table, path: &[&str]) -> Option<&'a toml::Table> {
    let mut table = root;
    let mut at = 0;
    while let Some(seg) = path.get(at) {
        at += 1;
        let value = table.get(*seg)?;
        table = if ARRAYS.contains(&&path[..at]) {
            let name = path.get(at)?;
            at += 1;
            named_plain(value.as_array()?, name)?
        } else {
            value.as_table()?
        };
    }
    Some(table)
}

/// [`table_in`], making each table and entry that is not there.
fn make_table<'a>(root: &'a mut toml::Table, path: &[&str]) -> Option<&'a mut toml::Table> {
    let mut table = root;
    let mut at = 0;
    while let Some(seg) = path.get(at) {
        at += 1;
        if ARRAYS.contains(&&path[..at]) {
            let name = *path.get(at)?;
            at += 1;
            let entries = table
                .entry(*seg)
                .or_insert_with(|| toml::Value::Array(Vec::new()))
                .as_array_mut()?;
            let named =
                |entry: &toml::Value| entry.get(NAME).and_then(toml::Value::as_str) == Some(name);
            let found = match entries.iter().position(named) {
                Some(found) => found,
                None => {
                    let mut entry = toml::Table::new();
                    entry.insert(NAME.to_owned(), name.into());
                    entries.push(toml::Value::Table(entry));
                    entries.len() - 1
                }
            };
            table = entries[found].as_table_mut()?;
        } else {
            table = table
                .entry(*seg)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()?;
        }
    }
    Some(table)
}

/// The lines of the first live value at or under `path`, depth first.
/// An entry's [`NAME`] is not among its values: it goes with the header.
fn next_value(text: &str, path: &[&str]) -> Result<Option<Range<usize>>, String> {
    let doc = Parsed::parse(text).map_err(|error| error.to_string())?;
    if entry_of(path).is_some() {
        return Ok(match walk(doc.as_table(), path) {
            Ok(Some(entry)) => entry
                .iter()
                .filter(|(key, _)| *key != NAME)
                .find_map(|(key, _)| value_lines(text, entry, key)),
            _ => None,
        });
    }
    let Some((leaf, parents)) = path.split_last() else {
        return Ok(None);
    };
    match walk(doc.as_table(), parents) {
        Ok(Some(table)) => Ok(value_lines(text, table, leaf)),
        Ok(None) => Ok(None),
        Err(seg) => Err(format!("`{seg}` in `{}` is not a table", path.join("."))),
    }
}

fn value_lines(text: &str, table: &Table, key: &str) -> Option<Range<usize>> {
    match table.get(key)? {
        Item::Table(t) => t.iter().find_map(|(k, _)| value_lines(text, t, k)),
        Item::Value(value) => {
            let start = table.key(key)?.span()?.start;
            let end = value.span()?.end;
            Some(line_of(text, start)..line_of(text, end) + 1)
        }
        _ => None,
    }
}

/// Every table at or under `path`, deepest first.
fn tables_under(text: &str, path: &[&str]) -> Vec<Vec<String>> {
    fn under(table: &Table, path: Vec<String>, out: &mut Vec<Vec<String>>) {
        for (key, item) in table {
            if let Item::Table(t) = item {
                let mut child = path.clone();
                child.push(key.to_owned());
                under(t, child, out);
            }
        }
        out.push(path);
    }
    let Ok(doc) = Parsed::parse(text) else {
        return Vec::new();
    };
    let Ok(Some(current)) = walk(doc.as_table(), path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    under(current, owned(path), &mut out);
    out
}

fn owned(path: &[&str]) -> Vec<String> {
    path.iter().map(|seg| (*seg).to_owned()).collect()
}

/// Retire `table`'s header if the table is empty: comment it out while
/// commented-out keys remain in its section, else delete it with one blank
/// line beside it. An entry holding only its name is empty, and its name
/// line goes with its header, or is commented out with it.
fn retire_if_empty(text: &str, table: &[&str]) -> String {
    let Header::Line(header) = header_of(text, table) else {
        return text.to_owned();
    };
    let Ok(doc) = text.parse::<toml::Table>() else {
        return text.to_owned();
    };
    let entry = entry_of(table).is_some();
    let empty = |t: &toml::Table| t.keys().all(|key| entry && key == NAME);
    if !table_in(&doc, table).is_some_and(empty) {
        return text.to_owned();
    }
    let name = if entry {
        Parsed::parse(text)
            .ok()
            .and_then(|doc| value_lines(text, walk(doc.as_table(), table).ok()??, NAME))
    } else {
        None
    };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let documented = section(&lines, header).any(|i| {
        uncomment(lines[i]).is_some_and(|l| matches!(declares(&l), Some(Declares::Key(_))))
    });
    let mut doomed: Vec<usize> = std::iter::once(header)
        .chain(name.into_iter().flatten())
        .collect();
    if documented {
        let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
        for i in doomed {
            out[i] = comment_out(lines[i]);
        }
        return out.concat();
    }
    // The header and its name, with one blank line beside them where
    // there is one on either side.
    doomed.sort_unstable();
    let blank = |i: usize| lines.get(i).is_none_or(|l| l.trim().is_empty());
    let (first, last) = (doomed[0], doomed[doomed.len() - 1]);
    let joined = last - first + 1 == doomed.len();
    if joined && first > 0 && blank(first - 1) && blank(last + 1) {
        doomed.push(first - 1);
    } else if joined && first == 0 && last + 1 < lines.len() && blank(last + 1) {
        doomed.push(last + 1);
    }
    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !doomed.contains(i))
        .map(|(_, line)| *line)
        .collect()
}

/// `text` with the lines in `range` replaced by `with`, or deleted.
fn splice(text: &str, range: Range<usize>, with: Option<&str>) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    for line in &lines[..range.start] {
        out.push_str(line);
    }
    if let Some(with) = with {
        out.push_str(with);
    }
    for line in &lines[range.end..] {
        out.push_str(line);
    }
    out
}

/// The 0-based line holding byte `offset`.
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prose_line_shaped_like_a_header_is_never_uncommented() {
        // Parsed on its own it is not a header, so the table is appended.
        let text = "[theme]\nname = \"x\"\n# [splash] is drawn until the desktop is (seat only)\n";
        assert_eq!(
            make_room(text, &["splash"], "enabled"),
            format!("{text}\n[splash]\n")
        );
    }

    #[test]
    fn a_commented_header_and_its_example_are_filled_where_they_stand() {
        let text = "[theme]\nname = \"x\"\n\n# [splash]\n# enabled = true\n\n[agent]\n";
        assert_eq!(
            make_room(text, &["splash"], "enabled"),
            "[theme]\nname = \"x\"\n\n[splash]\nenabled = true\n\n[agent]\n"
        );
    }

    #[test]
    fn a_key_example_is_looked_for_only_up_to_the_next_header() {
        let text = "[a]\nx = 1\n\n# [b]\n# y = 2\n";
        assert_eq!(make_room(text, &["a"], "y"), text);
    }

    #[test]
    fn a_commented_header_that_would_capture_live_keys_stays_commented() {
        // Uncommenting `# [decorations.agent]` would move `focused` out of
        // `[decorations]`, and `decorations.agent.focused` is a real key, so
        // nothing downstream would object.
        let text =
            "[decorations]\nmode = \"server\"\n# [decorations.agent]\nfocused = \"#440000\"\n";
        assert_eq!(
            make_room(text, &["decorations", "agent"], "outline"),
            format!("{text}\n[decorations.agent]\n")
        );
    }

    #[test]
    fn a_commented_header_inside_a_multiline_string_stays_commented() {
        let text = "[theme]\nname = \"\"\"\n# [splash]\n\"\"\"\n";
        assert_eq!(
            make_room(text, &["splash"], "enabled"),
            format!("{text}\n[splash]\n")
        );
    }

    #[test]
    fn a_super_table_is_appended_after_its_subtable() {
        let text = "[a.b]\nx = 1\n";
        assert_eq!(make_room(text, &["a"], "y"), "[a.b]\nx = 1\n\n[a]\n");
    }

    #[test]
    fn a_table_made_of_dotted_keys_gets_no_header() {
        // `[shell.panel]` would redefine the dotted table, so the text is
        // left alone and toml_edit adds a dotted key beside the first.
        let text = "[shell]\npanel.height = 30\n";
        assert_eq!(make_room(text, &["shell", "panel"], "edge"), text);
    }

    #[test]
    fn removing_a_key_takes_every_line_of_its_value_and_nothing_else() {
        let text = "[shell]\n# above\nleave = [\n  \"lock\",\n  \"log-out\",\n]  # beside\n\
                    desktop-icons = false\n";
        assert_eq!(
            remove(text, &["shell", "leave"]).unwrap(),
            "[shell]\n# above\ndesktop-icons = false\n"
        );
    }

    #[test]
    fn removing_the_last_key_comments_the_header_out_while_examples_remain() {
        let text = "[focus]\nautoraise = true\n# model = \"click\"\n";
        assert_eq!(
            remove(text, &["focus", "autoraise"]).unwrap(),
            "# [focus]\n# model = \"click\"\n"
        );
    }

    #[test]
    fn removing_the_last_key_deletes_a_bare_header_and_one_blank_beside_it() {
        // At the end of the file, the blank before it.
        assert_eq!(
            remove("# note\n\n[a]\nx = 1\n", &["a", "x"]).unwrap(),
            "# note\n"
        );
        // At the top, the blank after it.
        assert_eq!(
            remove("[a]\nx = 1\n\n[b]\ny = 2\n", &["a", "x"]).unwrap(),
            "[b]\ny = 2\n"
        );
        // Between two sections, one of the two blanks.
        assert_eq!(
            remove("[a]\nx = 1\n\n[b]\ny = 2\n\n[c]\nz = 3\n", &["b", "y"]).unwrap(),
            "[a]\nx = 1\n\n[c]\nz = 3\n"
        );
        // Against a comment, none: the comment keeps its spacing.
        assert_eq!(
            remove("[a]\nx = 1\n\n[b]\ny = 2\n# after\n", &["b", "y"]).unwrap(),
            "[a]\nx = 1\n\n# after\n"
        );
    }

    #[test]
    fn removing_a_table_removes_its_keys_and_headers_but_not_the_comments_above() {
        let text = "# keep me\n[shell]\nleave = []\n\n[shell.panel]\nheight = 30\n";
        assert_eq!(remove(text, &["shell"]).unwrap(), "# keep me\n");
    }

    #[test]
    fn removing_a_dotted_key_retires_the_header_it_emptied() {
        assert_eq!(
            remove(
                "[shell]\npanel.height = 30\n",
                &["shell", "panel", "height"]
            )
            .unwrap(),
            ""
        );
    }

    #[test]
    fn a_key_holding_a_dot_is_one_segment() {
        let text = "[keys]\n\"Logo+.\" = \"close\"\n\"Logo+a\" = \"close\"\n";
        assert_eq!(
            remove(text, &["keys", "Logo+."]).unwrap(),
            "[keys]\n\"Logo+a\" = \"close\"\n"
        );
    }

    #[test]
    fn removing_through_a_value_names_the_segment() {
        let error = remove("profile = \"minimal\"\n", &["profile", "x"]).unwrap_err();
        assert_eq!(error, "`profile` in `profile.x` is not a table");
    }

    #[test]
    fn uncomment_and_comment_out_are_inverses() {
        for line in [
            "[splash]\n",
            "  border = 2   # doc\n",
            "[opacity]   # defaults\n",
        ] {
            assert_eq!(uncomment(&comment_out(line)).as_deref(), Some(line));
        }
        assert_eq!(uncomment("#[a]").as_deref(), Some("[a]"));
        assert_eq!(uncomment("border = 1"), None);
    }
}
