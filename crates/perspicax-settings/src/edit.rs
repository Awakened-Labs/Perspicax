//! The edits, made on text: the file as a person would have left it, had
//! they made the change in an editor.

use perspicax_config::{Built, Spelled, Trigger};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike};

use crate::{Edit, Reason, Value, layout};

/// Every feature on. The check made here is of the file's shape and its key
/// names; what this build of perspicax lacks is for the installed one to
/// say, through the judge.
pub(crate) const EVERYTHING: Built = Built {
    seat: true,
    xwayland: true,
    capture: true,
    svg: true,
};

/// `text` with `edits` made, in order. An edit that changes nothing the file
/// says is not made, and nor is one that adds a key perspicax already reads
/// as that value: either would only churn the person's text.
///
/// # Errors
///
/// [`Reason::NotToml`] for a `text` that does not parse, and
/// [`Reason::Unwritable`] for an edit with no place in it.
pub(crate) fn edited(text: &str, edits: &[Edit]) -> Result<String, Reason> {
    text.parse::<DocumentMut>()
        .map_err(|error| Reason::NotToml(error.to_string()))?;
    let mut text = text.to_owned();
    for edit in edits {
        let (next, added) = made(&text, edit).map_err(Reason::Unwritable)?;
        if says(&next) == says(&text) {
            continue;
        }
        if added && same_config(&text, &next) {
            continue;
        }
        text = next;
    }
    Ok(text)
}

/// `text` with `edit` made, and whether it added a key the text did not
/// have.
fn made(text: &str, edit: &Edit) -> Result<(String, bool), String> {
    Ok(match edit {
        Edit::Set { key, value } => {
            let added = !written(text, &key.path());
            let table: Vec<&str> = key.table.iter().map(String::as_str).collect();
            (set(text, &table, &key.leaf, toml(value))?, added)
        }
        Edit::Reset { key } => (layout::remove(text, &key.path())?, false),
        Edit::Bind { trigger, action } => {
            let (leaf, added) = match binding(text, *trigger) {
                Some(written) => (written, false),
                None => (
                    perspicax_config::spell_trigger(*trigger)
                        .ok_or("that chord cannot be written in a config yet")?,
                    true,
                ),
            };
            (set(text, &["keys"], &leaf, bound(action.as_ref()))?, added)
        }
        Edit::ResetBinding { trigger } => match binding(text, *trigger) {
            Some(written) => (layout::remove(text, &["keys", &written])?, false),
            None => (text.to_owned(), false),
        },
    })
}

/// What `text` says, as a plain table: its meaning, without its layout.
fn says(text: &str) -> Option<toml::Table> {
    text.parse().ok()
}

/// Whether perspicax reads `before` and `after` as the same config. Not if
/// either is refused: a refused file reads as nothing, and nothing is not
/// the same as anything.
fn same_config(before: &str, after: &str) -> bool {
    let read = |text: &str| perspicax_config::parse(text, EVERYTHING).ok();
    matches!((read(before), read(after)), (Some(before), Some(after)) if before == after)
}

/// Whether `text` holds a value, or a table, at `path`.
fn written(text: &str, path: &[&str]) -> bool {
    let Some(root) = says(text) else {
        return false;
    };
    let Some((leaf, tables)) = path.split_last() else {
        return false;
    };
    layout::table_in(&root, tables).is_some_and(|table| table.contains_key(*leaf))
}

/// The `[keys]` entry for `trigger`, as the file spells it. Found by the
/// chord each entry parses as, so `"Super+a"` is Logo+a's. perspicax refuses
/// two spellings of one chord, so a file it takes has at most one.
fn binding(text: &str, trigger: Trigger) -> Option<String> {
    let root = says(text)?;
    let keys = root.get("keys")?.as_table()?;
    keys.keys()
        .find(|written| perspicax_config::trigger(written) == Ok(trigger))
        .cloned()
}

/// Set `leaf` in `table` to `value`, filling the file's own example for it
/// where there is one.
fn set(text: &str, table: &[&str], leaf: &str, value: toml_edit::Value) -> Result<String, String> {
    let text = layout::make_room(text, table, leaf);
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|error| error.to_string())?;
    let (table, inline) = descend(&mut doc, table)?;
    let added = !table.contains_key(leaf);
    replace_value(table, leaf, value);
    // toml_edit leaves the space before an inline table's `}` on the value
    // that had been last, so a key added there is laid out afresh, as
    // toml_edit lays out a table it writes inline itself.
    if inline && added {
        table.fmt();
    }
    Ok(doc.to_string())
}

/// Walk (making as needed) the tables of `path`, returning the innermost.
/// [`layout::make_room`] has usually given the file their headers already;
/// making one here is the fallback for a table it could not: one made of
/// dotted keys, or one written inline (`flip = { edge = true }`), which
/// stays inline. A table made here is implicit, so it prints no header
/// until it holds a key. Also whether the innermost is written inline.
///
/// In an array of tables, the segment after the array names an entry, as
/// the layout finds one: `output.DP-1` is the `[[output]]` whose `name` is
/// `DP-1`.
fn descend<'a>(
    doc: &'a mut DocumentMut,
    path: &[&str],
) -> Result<(&'a mut dyn TableLike, bool), String> {
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    let mut inline = false;
    let mut segs = path.iter();
    while let Some(seg) = segs.next() {
        if !table.contains_key(seg) {
            let mut implicit = Table::new();
            implicit.set_implicit(true);
            table.insert(seg, Item::Table(implicit));
        }
        let not_a_table = || format!("`{seg}` in `{}` is not a table", path.join("."));
        let item = table.get_mut(seg).ok_or_else(not_a_table)?;
        inline = inline || item.is_inline_table();
        table = match item {
            Item::ArrayOfTables(entries) => {
                let name = segs.next().ok_or_else(not_a_table)?;
                entry(entries, name).ok_or_else(not_a_table)?
            }
            item => item.as_table_like_mut().ok_or_else(not_a_table)?,
        };
    }
    Ok((table, inline))
}

/// The entry of `entries` named `name`, made at the end where there is
/// none.
fn entry<'a>(entries: &'a mut ArrayOfTables, name: &str) -> Option<&'a mut Table> {
    let found = entries
        .iter()
        .position(|entry| entry.get(layout::NAME).and_then(Item::as_str) == Some(name));
    let at = found.unwrap_or_else(|| {
        let mut made = Table::new();
        made.insert(layout::NAME, toml_edit::value(name));
        entries.push(made);
        entries.len() - 1
    });
    entries.get_mut(at)
}

/// Put `value` at `leaf`, changing the value and nothing else on its line.
///
/// `Table::insert` will not do for a key that is already there: the new
/// value brings default decor, which drops the comment beside the old one.
/// So an existing value hands its decor -- the spacing around it and the
/// comment after it -- to its replacement. The spacing before the `=` is the
/// key's, which stays as it was, and so does an alignment.
fn replace_value(table: &mut dyn TableLike, leaf: &str, mut value: toml_edit::Value) {
    match table.get_mut(leaf) {
        Some(Item::Value(old)) => {
            *value.decor_mut() = old.decor().clone();
            *old = value;
        }
        _ => {
            table.insert(leaf, Item::Value(value));
        }
    }
}

fn toml(value: &Value) -> toml_edit::Value {
    match value {
        Value::Bool(value) => (*value).into(),
        Value::Integer(value) => (*value).into(),
        Value::Float(value) => (*value).into(),
        Value::String(value) => value.as_str().into(),
        Value::Strings(values) => strings(values),
        Value::Integers(values) => toml_edit::Value::Array(values.iter().copied().collect()),
    }
}

fn strings(values: &[String]) -> toml_edit::Value {
    toml_edit::Value::Array(values.iter().map(String::as_str).collect())
}

/// A binding's value as a person writes it: a name, `"none"`, or one of the
/// two tables, inline: `{ spawn = ["foot"] }`, with `agent = true` only when
/// it is true.
fn bound(action: Option<&Spelled>) -> toml_edit::Value {
    let table = |entries: Vec<(&str, toml_edit::Value)>| {
        let mut table: InlineTable = entries.into_iter().collect();
        table.fmt();
        toml_edit::Value::InlineTable(table)
    };
    match action {
        None => "none".into(),
        Some(Spelled::Name(name)) => name.as_str().into(),
        Some(Spelled::Spawn { command, agent }) => {
            let mut entries = vec![("spawn", strings(command))];
            if *agent {
                entries.push(("agent", true.into()));
            }
            table(entries)
        }
        Some(Spelled::Pie(pie)) => table(vec![("pie", pie.as_str().into())]),
    }
}
