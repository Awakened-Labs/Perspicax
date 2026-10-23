//! What `perspicax doctor`, `perspicax claude-code` and `perspicax ennius`
//! answer with: rows of a name, a word on it, and why, as one grid.
//!
//! One kind of row for a check and for a step alike. A check says whether
//! something is as a session needs it; a step says what an install did about
//! it, and its detail says what that was, down to the command it ran. Either
//! way a warning is something one of perspicax's own commands puts right, and
//! says which; a failure is something none of them can.

use std::fmt::Write as _;

/// What a row says of its subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// As it should be, or done.
    Ok,
    /// Not as it should be, and the detail names the command that puts it
    /// right.
    Warn,
    /// Not as it should be, and nothing here puts it right.
    Fail,
    /// Not asked: it does not apply here.
    Skipped,
}

impl Status {
    /// The word the grid shows.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
        }
    }
}

/// One line of an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// What it is about: `config`, `mcp-server`.
    pub name: &'static str,
    /// What it says of it.
    pub status: Status,
    /// Why, in a sentence.
    pub detail: String,
}

impl Row {
    #[must_use]
    pub fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

/// The rows as a grid, each column as wide as its widest word, then a line
/// on the whole.
#[must_use]
pub fn render(rows: &[Row]) -> String {
    let named = rows.iter().map(|row| row.name.len()).max().unwrap_or(0);
    let said = rows
        .iter()
        .map(|row| row.status.word().len())
        .max()
        .unwrap_or(0);
    let mut text = String::new();
    for row in rows {
        let _ = writeln!(
            text,
            "{:named$}  {:said$}  {}",
            row.name,
            row.status.word(),
            row.detail
        );
    }
    let count = |status| rows.iter().filter(|row| row.status == status).count();
    let (failed, warned) = (count(Status::Fail), count(Status::Warn));
    let _ = writeln!(text);
    let _ = match (failed, warned) {
        (0, 0) => writeln!(text, "nothing to put right"),
        (0, 1) => writeln!(text, "no failures; 1 warning"),
        (0, warned) => writeln!(text, "no failures; {warned} warnings"),
        (failed, _) => writeln!(text, "{failed} of {} failed", rows.len()),
    };
    text
}

/// Print the rows on stdout, and return the exit status they come to: 1 if
/// any failed, else 0. A warning is not a failure: a machine that has not
/// been wired yet is not broken.
#[must_use]
pub fn print(rows: &[Row]) -> i32 {
    print!("{}", render(rows));
    i32::from(rows.iter().any(|row| row.status == Status::Fail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_is_as_wide_as_its_widest_words() {
        let rows = [
            Row::new("config", Status::Ok, "taken"),
            Row::new("runtime-dir", Status::Skipped, "not asked"),
        ];
        assert_eq!(
            render(&rows),
            "config       ok       taken\n\
             runtime-dir  skipped  not asked\n\
             \n\
             nothing to put right\n"
        );
    }

    #[test]
    fn the_last_line_counts_what_is_wrong() {
        let ok = Row::new("a", Status::Ok, "");
        let warn = Row::new("b", Status::Warn, "");
        let fail = Row::new("c", Status::Fail, "");
        let last = |rows: &[Row]| render(rows).lines().last().unwrap().to_owned();
        assert_eq!(last(&[ok.clone(), warn.clone()]), "no failures; 1 warning");
        assert_eq!(
            last(&[warn.clone(), warn.clone()]),
            "no failures; 2 warnings"
        );
        assert_eq!(last(&[ok, warn, fail]), "1 of 3 failed");
    }
}
