//! Basic formatting for a day's notes, kept as Markdown in the text itself
//! so it is stored as typed and reaches the model as Markdown
//! (`fortnight_core::notes::to_markdown`).
//!
//! - `**bold**`, `*italic*` and `<u>underline</u>` (`_italic_` and
//!   `__bold__` are understood too). The editor draws bold and italic in
//!   Inter's bold and italic faces and the markers faint; underlined text
//!   is drawn in the accent colour, since iced's editor cannot underline.
//! - Lists: `• ` bullets, `- ` dashes and `1. ` numbers, at any
//!   indentation. Typing `* ` at the start of a line makes a `•` bullet.
//!   `Enter` in a list item starts the next item (`2.` after `1.`), on an
//!   empty item it ends the list (or steps out one level); `Backspace`
//!   right after a marker removes it; `Tab` / `Shift+Tab` indent and
//!   outdent the line.
//!
//! - Links: `[text](url)` (`⌘K`), and bare web addresses. Drawn in the
//!   accent colour; in the formatted view underlined, and a click opens
//!   them.
//!
//! While the notes are not being edited they are shown by [`preview`]:
//! markers hidden, underline drawn as underline.
//!
//! The toolbar buttons and the shortcuts (`⌘B`, `⌘I`, `⌘U`, `⌘⇧8` for
//! bullets, `⌘⇧7` for numbers) send a [`Command`]: bold, italic and
//! underline wrap the selection (or the word at the cursor, or start an
//! empty pair) and unwrap it when it is already formatted; the list
//! commands mark each selected line, or unmark them when all already are
//! that kind of list.
//!
//! The editing itself is plain text functions (testable without a
//! window) returning a [`Change`]: lines to replace and where the cursor
//! goes. [`apply`] makes it in the editor by selecting those lines and
//! pasting over them.

use std::ops::Range;
use std::sync::Arc;

use iced::advanced::text::highlighter::{self, Highlighter};
use iced::font::{Style, Weight};
use iced::keyboard::{self, key::Named};
use iced::widget::text_editor::{
    Action, Binding, Content, Cursor, Edit, KeyPress, Position, Status,
};
use iced::widget::{Column, Space, button, container, rich_text, row, span, text, tooltip};
use iced::{Center, Element, Font, Theme};

use crate::theme;

/// A formatting command from the toolbar or a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Bold,
    Italic,
    Underline,
    Bullets,
    Dashes,
    Numbers,
    Indent,
    Outdent,
    /// Make the selection (or the word at the cursor) a link, or take a
    /// link away.
    Link,
}

// ------------------------------------------------------------------ lists

/// The three kinds of list line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `• text`
    Bullet,
    /// `- text` (also `* ` and `+ `)
    Dash,
    /// `1. text` (also `1) `)
    Number,
}

/// A list line's marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Item {
    kind: Kind,
    /// Where the marker starts (the indentation's length), in bytes.
    indent: usize,
    /// Where the text after the marker and its space starts, in bytes.
    end: usize,
    /// The number of a numbered item.
    number: u32,
}

fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// The list marker at the start of `line`, if it is a list line.
fn item(line: &str) -> Option<Item> {
    let indent = indent_len(line);
    let rest = &line[indent..];
    let at = |kind, len, number| {
        Some(Item {
            kind,
            indent,
            end: indent + len,
            number,
        })
    };
    if rest.starts_with("• ") {
        return at(Kind::Bullet, "• ".len(), 0);
    }
    if ["- ", "* ", "+ "].iter().any(|m| rest.starts_with(m)) {
        return at(Kind::Dash, 2, 0);
    }
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if (1..=9).contains(&digits) {
        let after = &rest[digits..];
        if after.starts_with(". ") || after.starts_with(") ") {
            return at(Kind::Number, digits + 2, rest[..digits].parse().ok()?);
        }
    }
    None
}

fn marker(kind: Kind, number: u32) -> String {
    match kind {
        Kind::Bullet => "• ".into(),
        Kind::Dash => "- ".into(),
        Kind::Number => format!("{number}. "),
    }
}

// ------------------------------------------------------------------ changes

/// An edit to the notes: lines `first..=last` become `text` (which may
/// hold more or fewer lines), then the cursor moves to `cursor`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    first: usize,
    last: usize,
    text: String,
    cursor: Cursor,
}

/// Replaces `del` bytes at `at` in a line with `ins`. A cursor exactly at
/// `at` of an insertion stays before it when `stick_left`, else moves
/// past it.
#[derive(Debug, Clone)]
struct Splice {
    at: usize,
    del: usize,
    ins: String,
    stick_left: bool,
}

impl Splice {
    fn insert(at: usize, ins: impl Into<String>, stick_left: bool) -> Self {
        Splice {
            at,
            del: 0,
            ins: ins.into(),
            stick_left,
        }
    }

    fn delete(at: usize, del: usize) -> Self {
        Splice {
            at,
            del,
            ins: String::new(),
            stick_left: true,
        }
    }

    fn map(&self, col: usize) -> usize {
        let past = if self.stick_left { 0 } else { self.ins.len() };
        if col < self.at || (col == self.at && self.del == 0 && self.stick_left) {
            col
        } else if col < self.at + self.del || col == self.at {
            self.at + past
        } else {
            col - self.del + self.ins.len()
        }
    }
}

/// The change made by `splices[i]` on line `first + i`. Each line's
/// splices are in text order and don't overlap; two at the same place
/// end up in that order.
fn splice_lines(
    lines: &[String],
    first: usize,
    splices: &[Vec<Splice>],
    cursor: Cursor,
) -> Option<Change> {
    if splices.iter().all(Vec::is_empty) {
        return None;
    }
    let last = first + splices.len() - 1;
    let mut out = Vec::new();
    for (i, line) in lines[first..=last].iter().enumerate() {
        let mut line = line.clone();
        for s in splices[i].iter().rev() {
            line.replace_range(s.at..s.at + s.del, &s.ins);
        }
        out.push(line);
    }
    let map = |p: Position| {
        let Some(list) = p.line.checked_sub(first).and_then(|i| splices.get(i)) else {
            return p;
        };
        let column = list.iter().rev().fold(p.column, |c, s| s.map(c));
        Position { column, ..p }
    };
    Some(Change {
        first,
        last,
        text: out.join("\n"),
        cursor: Cursor {
            position: map(cursor.position),
            selection: cursor.selection.map(map),
        },
    })
}

/// The selection's ends in text order (both the cursor when there is
/// none).
fn ends(cursor: Cursor) -> (Position, Position) {
    let a = cursor.position;
    let b = cursor.selection.unwrap_or(a);
    if (a.line, a.column) <= (b.line, b.column) {
        (a, b)
    } else {
        (b, a)
    }
}

fn at(line: usize, column: usize) -> Cursor {
    Cursor {
        position: Position { line, column },
        selection: None,
    }
}

/// What a command changes in `lines` with the given cursor.
pub fn command(lines: &[String], cursor: Cursor, cmd: Command) -> Option<Change> {
    match cmd {
        Command::Bold => toggle_inline(lines, cursor, Inline::Bold),
        Command::Italic => toggle_inline(lines, cursor, Inline::Italic),
        Command::Underline => toggle_inline(lines, cursor, Inline::Underline),
        Command::Bullets => toggle_list(lines, cursor, Kind::Bullet),
        Command::Dashes => toggle_list(lines, cursor, Kind::Dash),
        Command::Numbers => toggle_list(lines, cursor, Kind::Number),
        Command::Link => toggle_link(lines, cursor),
        Command::Indent => indent(lines, cursor, true),
        Command::Outdent => indent(lines, cursor, false),
    }
}

/// The selected lines: those with text, or the cursor's line alone.
fn selected_lines(lines: &[String], cursor: Cursor) -> (usize, usize, Vec<bool>) {
    let (start, end) = ends(cursor);
    let last = end.line.min(lines.len().saturating_sub(1));
    let first = start.line.min(last);
    let picked = (first..=last)
        .map(|i| first == last || !lines[i].trim().is_empty())
        .collect();
    (first, last, picked)
}

fn toggle_list(lines: &[String], cursor: Cursor, kind: Kind) -> Option<Change> {
    let (first, _, picked) = selected_lines(lines, cursor);
    let all_already = picked
        .iter()
        .enumerate()
        .filter(|(_, p)| **p)
        .all(|(i, _)| item(&lines[first + i]).is_some_and(|it| it.kind == kind));
    // Numbers carry on from a numbered item just above.
    let mut number = first
        .checked_sub(1)
        .and_then(|i| item(&lines[i]))
        .filter(|it| it.kind == Kind::Number && it.indent == indent_len(&lines[first]))
        .map_or(1, |it| it.number + 1);
    let splices: Vec<Vec<Splice>> = picked
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if !p {
                return vec![];
            }
            let line = &lines[first + i];
            let (at, del) =
                item(line).map_or((indent_len(line), 0), |it| (it.indent, it.end - it.indent));
            let ins = if all_already {
                String::new()
            } else {
                number += 1;
                marker(kind, number - 1)
            };
            vec![Splice {
                at,
                del,
                ins,
                stick_left: false,
            }]
        })
        .collect();
    splice_lines(lines, first, &splices, cursor)
}

fn indent(lines: &[String], cursor: Cursor, more: bool) -> Option<Change> {
    let (first, _, picked) = selected_lines(lines, cursor);
    let splices: Vec<Vec<Splice>> = picked
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let line = &lines[first + i];
            if !p {
                vec![]
            } else if more {
                vec![Splice::insert(0, "  ", false)]
            } else if line.starts_with('\t') {
                vec![Splice::delete(0, 1)]
            } else {
                let spaces = line.len() - line.trim_start_matches(' ').len();
                match spaces.min(2) {
                    0 => vec![],
                    n => vec![Splice::delete(0, n)],
                }
            }
        })
        .collect();
    splice_lines(lines, first, &splices, cursor)
}

// ----------------------------------------------------------------- inline

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inline {
    Bold,
    Italic,
    Underline,
}

impl Inline {
    fn open(self) -> &'static str {
        match self {
            Inline::Bold => "**",
            Inline::Italic => "*",
            Inline::Underline => "<u>",
        }
    }

    fn close(self) -> &'static str {
        match self {
            Inline::Underline => "</u>",
            other => other.open(),
        }
    }

    /// Whether runs of `before` and `after` emphasis characters around
    /// some text make it this style: bold needs two or more each side,
    /// italic an odd count (1, or 3 for bold italic).
    fn emphasis(self, before: usize, after: usize) -> bool {
        match self {
            Inline::Bold => before >= 2 && after >= 2,
            Inline::Italic => before % 2 == 1 && after % 2 == 1,
            Inline::Underline => false,
        }
    }

    /// How many characters of an emphasis run make this style.
    fn width(self) -> usize {
        match self {
            Inline::Bold => 2,
            _ => 1,
        }
    }
}

/// The markers that put `line[a..b]` in a style, to remove: `len_a`
/// bytes at `at_a` (around `a`) and `len_b` at `at_b` (around `b`). Each
/// may sit just outside the range or just inside it.
#[derive(Debug, Clone, Copy)]
struct Wrapped {
    at_a: usize,
    len_a: usize,
    at_b: usize,
    len_b: usize,
}

fn run_before(s: &str, c: char) -> usize {
    s.chars().rev().take_while(|x| *x == c).count()
}

fn run_after(s: &str, c: char) -> usize {
    s.chars().take_while(|x| *x == c).count()
}

fn wrapped(line: &str, a: usize, b: usize, style: Inline) -> Option<Wrapped> {
    let (before, inside, after) = (&line[..a], &line[a..b], &line[b..]);
    if style == Inline::Underline {
        let (open, close) = (style.open(), style.close());
        let at_a = if before.ends_with(open) {
            a - open.len()
        } else if inside.starts_with(open) {
            a
        } else {
            return None;
        };
        let at_b = if after.starts_with(close) {
            b
        } else if inside.ends_with(close)
            && inside.len() >= close.len() + (a - at_a).max(open.len())
        {
            b - close.len()
        } else {
            return None;
        };
        return Some(Wrapped {
            at_a,
            len_a: open.len(),
            at_b,
            len_b: close.len(),
        });
    }
    // The emphasis runs at each end, outside and inside the range together.
    let w = style.width();
    ['*', '_'].into_iter().find_map(|c| {
        let (out_a, in_a) = (run_before(before, c), run_after(inside, c));
        let (in_b, out_b) = (run_before(inside, c), run_after(after, c));
        (in_a + in_b < inside.len() && style.emphasis(out_a + in_a, in_b + out_b)).then_some(
            Wrapped {
                at_a: a - out_a,
                len_a: w,
                at_b: b + out_b - w,
                len_b: w,
            },
        )
    })
}

/// The word around byte `col` of `line` (letters, digits and
/// apostrophes), as a byte range; empty when there is none.
fn word_at(line: &str, col: usize) -> (usize, usize) {
    let is_word = |c: char| c.is_alphanumeric() || c == '\'';
    let start = line[..col]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(col, |(i, _)| i);
    let end = line[col..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(line.len(), |(i, _)| col + i);
    (start, end)
}

fn toggle_inline(lines: &[String], cursor: Cursor, style: Inline) -> Option<Change> {
    let (start, end) = ends(cursor);
    if start.line >= lines.len() {
        return None;
    }
    let first = start.line;
    // Byte ranges of text to format, one per line (by line index).
    let mut ranges: Vec<Option<(usize, usize)>> = Vec::new();
    if start == end {
        let line = &lines[first];
        let col = start.column;
        let (open, close) = (style.open(), style.close());
        // An empty pair around the cursor: take it away again.
        let (before, after) = (&line[..col], &line[col..]);
        let empty_pair = match style {
            Inline::Underline => before.ends_with(open) && after.starts_with(close),
            _ => style.emphasis(run_before(before, '*'), run_after(after, '*')),
        };
        if empty_pair {
            let (b, a) = match style {
                Inline::Underline => (open.len(), close.len()),
                _ => (style.width(), style.width()),
            };
            let splices = vec![vec![Splice::delete(col - b, b), Splice::delete(col, a)]];
            return splice_lines(lines, first, &splices, cursor);
        }
        let (a, b) = word_at(line, col);
        if a == b {
            let splices = vec![vec![
                Splice::insert(col, open, false),
                Splice::insert(col, close, true),
            ]];
            return splice_lines(lines, first, &splices, cursor);
        }
        ranges.push(Some((a, b)));
    } else {
        for (i, line) in lines.iter().enumerate().take(end.line + 1).skip(first) {
            let content = item(line).map_or(indent_len(line), |it| it.end);
            let a = if i == first { start.column } else { 0 }.max(content);
            let b = if i == end.line {
                end.column
            } else {
                line.len()
            };
            if a >= b {
                ranges.push(None);
                continue;
            }
            let seg = &line[a..b];
            let a = a + (seg.len() - seg.trim_start().len());
            let b = b - (seg.len() - seg.trim_end().len());
            ranges.push((a < b).then_some((a, b)));
        }
    }
    let found: Vec<Option<Wrapped>> = ranges
        .iter()
        .enumerate()
        .map(|(i, r)| r.and_then(|(a, b)| wrapped(&lines[first + i], a, b, style)))
        .collect();
    let any = ranges.iter().any(Option::is_some);
    let remove = any
        && ranges
            .iter()
            .zip(&found)
            .all(|(r, f)| r.is_none() || f.is_some());
    let splices: Vec<Vec<Splice>> = ranges
        .iter()
        .zip(&found)
        .map(|(r, f)| match (*r, *f) {
            (None, _) => vec![],
            (Some(_), Some(w)) if remove => {
                vec![
                    Splice::delete(w.at_a, w.len_a),
                    Splice::delete(w.at_b, w.len_b),
                ]
            }
            (Some(_), Some(_)) => vec![],
            (Some((a, b)), None) => vec![
                Splice::insert(a, style.open(), false),
                Splice::insert(b, style.close(), true),
            ],
        })
        .collect();
    splice_lines(lines, first, &splices, cursor)
}

// ------------------------------------------------------------------ links

/// A link in a line: Markdown's `[text](url)`, or a bare web address
/// (`https://…`, `http://…`, `www.…`), whose text is its address.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    whole: Range<usize>,
    text: Range<usize>,
    url: Range<usize>,
    bare: bool,
}

fn is_address(s: &str) -> bool {
    ["https://", "http://", "www.", "mailto:"]
        .iter()
        .any(|p| s.starts_with(p))
        && !s.contains(char::is_whitespace)
}

/// The links in `line`, in order.
fn links(line: &str) -> Vec<Link> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if let Some(link) = markdown_link(line, i) {
            i = link.whole.end;
            out.push(link);
            continue;
        }
        let word_start = line[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if word_start
            && ["https://", "http://", "www."]
                .iter()
                .any(|p| rest.starts_with(p))
        {
            let mut end = i + rest.find(char::is_whitespace).unwrap_or(rest.len());
            // Trailing punctuation belongs to the sentence (or is a
            // formatting marker).
            while end > i
                && line[..end].ends_with(['.', ',', ';', ':', '!', '?', ')', '"', '\'', '*', '_'])
            {
                end -= 1;
            }
            if end > i {
                out.push(Link {
                    whole: i..end,
                    text: i..end,
                    url: i..end,
                    bare: true,
                });
                i = end;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// A `[text](url)` link starting at byte `i` (the URL without spaces).
fn markdown_link(line: &str, i: usize) -> Option<Link> {
    let rest = line[i..].strip_prefix('[')?;
    let close = i + 1 + rest.find(']')?;
    if line[i + 1..close].contains('[') {
        return None;
    }
    let url_start = close + 2;
    if !line[close + 1..].starts_with('(') {
        return None;
    }
    let url_end = url_start + line[url_start..].find(|c: char| c == ')' || c.is_whitespace())?;
    if url_end == url_start || !line[url_end..].starts_with(')') {
        return None;
    }
    Some(Link {
        whole: i..url_end + 1,
        text: i + 1..close,
        url: url_start..url_end,
        bare: false,
    })
}

/// Where a link's address takes you: a scheme is added to `www.…` and
/// other addresses without one.
pub fn href(url: &str) -> String {
    if url.contains("://") || url.starts_with("mailto:") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

/// `⌘K`: inside a `[text](url)` link, take the link away (keeping the
/// text). Otherwise the selection, a bare address at the cursor, or the
/// word at the cursor becomes `[text](|)`, the cursor where the address
/// goes; an address becomes `[|](address)`, the cursor where its text
/// goes. Nothing selected and no word: `[|]()`.
fn toggle_link(lines: &[String], cursor: Cursor) -> Option<Change> {
    let (start, end) = ends(cursor);
    if start.line != end.line {
        return None;
    }
    let n = start.line;
    let line = lines.get(n)?;
    let found = links(line);
    let change = |text: String, column: usize| Change {
        first: n,
        last: n,
        text,
        cursor: at(n, column),
    };
    let inside = |l: &&Link| l.whole.start <= start.column && end.column <= l.whole.end;
    if let Some(l) = found.iter().filter(|l| !l.bare).find(inside) {
        let text = if l.text.is_empty() { &l.url } else { &l.text };
        let text = &line[text.clone()];
        return Some(change(
            format!("{}{text}{}", &line[..l.whole.start], &line[l.whole.end..]),
            l.whole.start + text.len(),
        ));
    }
    let (a, b) = if let Some(l) = found.iter().filter(|l| l.bare).find(inside) {
        (l.whole.start, l.whole.end)
    } else if start == end {
        word_at(line, start.column)
    } else {
        let seg = &line[start.column..end.column];
        (
            start.column + (seg.len() - seg.trim_start().len()),
            end.column - (seg.len() - seg.trim_end().len()),
        )
    };
    let (before, sel, after) = (&line[..a], &line[a..b], &line[b..]);
    Some(if is_address(sel) {
        change(format!("{before}[]({sel}){after}"), a + 1)
    } else {
        change(format!("{before}[{sel}](){after}"), a + sel.len() + 3)
    })
}

// ----------------------------------------------------------------- typing

/// `Enter` in a list item: the next item, or the end of the list on an
/// empty one (one level out when it is indented).
fn on_enter(line: &str, n: usize, col: usize) -> Option<Change> {
    let it = item(line)?;
    if col < it.end {
        return None;
    }
    let mark = &line[it.indent..it.end];
    let mut rest = &line[it.end..];
    let check =
        it.kind == Kind::Dash && ["[ ] ", "[x] ", "[X] "].iter().any(|c| rest.starts_with(c));
    if check {
        rest = &rest[4..];
    }
    if rest.trim().is_empty() {
        let (text, column) = if it.indent > 0 {
            let out = if line.starts_with('\t') {
                1
            } else {
                it.indent.min(2)
            };
            (line[out..].to_string(), col - out)
        } else {
            (String::new(), 0)
        };
        return Some(Change {
            first: n,
            last: n,
            text,
            cursor: at(n, column),
        });
    }
    let next = match it.kind {
        Kind::Number => {
            let delim = &mark[mark.len() - 2..];
            format!("{}{delim}", it.number + 1)
        }
        _ if check => format!("{}[ ] ", &mark[..2]),
        _ => mark.to_string(),
    };
    let indent = &line[..it.indent];
    Some(Change {
        first: n,
        last: n,
        text: format!(
            "{}\n{indent}{next}{}",
            line[..col].trim_end(),
            line[col..].trim_start()
        ),
        cursor: at(n + 1, indent.len() + next.len()),
    })
}

/// `Backspace` right after a list marker: remove the marker.
fn on_backspace(line: &str, n: usize, col: usize) -> Option<Change> {
    let it = item(line).filter(|it| it.end == col)?;
    Some(Change {
        first: n,
        last: n,
        text: format!("{}{}", &line[..it.indent], &line[it.end..]),
        cursor: at(n, it.indent),
    })
}

/// A space after a lone `*` at the start of a line: a `•` bullet.
fn on_space(line: &str, n: usize, col: usize) -> Option<Change> {
    let before = &line[..col];
    let indent = indent_len(before);
    if &before[indent..] != "*" {
        return None;
    }
    Some(Change {
        first: n,
        last: n,
        text: format!("{}• {}", &before[..indent], &line[col..]),
        cursor: at(n, indent + "• ".len()),
    })
}

// ----------------------------------------------------------------- editor

/// Makes a change in the editor: selects its lines, pastes the new text
/// over them and places the cursor.
pub fn apply(content: &mut Content, change: Change) {
    let end = content.line(change.last).map_or(0, |l| l.text.len());
    content.move_to(Cursor {
        position: Position {
            line: change.last,
            column: end,
        },
        selection: Some(Position {
            line: change.first,
            column: 0,
        }),
    });
    content.perform(Action::Edit(Edit::Paste(Arc::new(change.text))));
    content.move_to(change.cursor);
}

fn lines(content: &Content) -> Vec<String> {
    content.lines().map(|l| l.text.into_owned()).collect()
}

/// Runs a command on the editor. Whether it changed anything.
pub fn run(content: &mut Content, cmd: Command) -> bool {
    match command(&lines(content), content.cursor(), cmd) {
        Some(change) => {
            apply(content, change);
            true
        }
        None => false,
    }
}

/// Handles the list keys (`Enter`, `Backspace`, a space after `*`) before
/// the editor does. Whether it did; if not, perform `action` as usual.
pub fn intercept(content: &mut Content, action: &Action) -> bool {
    let cursor = content.cursor();
    if cursor.selection.is_some_and(|s| s != cursor.position) {
        return false;
    }
    let Position { line: n, column } = cursor.position;
    let Some(line) = content.line(n).map(|l| l.text.into_owned()) else {
        return false;
    };
    let change = match action {
        Action::Edit(Edit::Enter) => on_enter(&line, n, column),
        Action::Edit(Edit::Backspace) => on_backspace(&line, n, column),
        Action::Edit(Edit::Insert(' ')) => on_space(&line, n, column),
        _ => None,
    };
    match change {
        Some(change) => {
            apply(content, change);
            true
        }
        None => false,
    }
}

/// The editor's keys: the formatting shortcuts, `Tab` / `Shift+Tab`, and
/// `Escape`, which leaves the editor and sends `leave`; then iced's usual
/// bindings.
pub fn key_binding<M>(press: KeyPress, on: fn(Command) -> M, leave: M) -> Option<Binding<M>> {
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    if press.key == keyboard::Key::Named(Named::Escape) {
        return Some(Binding::Sequence(vec![
            Binding::Unfocus,
            Binding::Custom(leave),
        ]));
    }
    let m = press.modifiers;
    let cmd = if m.command() && !m.alt() {
        match (press.key.to_latin(press.physical_key), m.shift()) {
            (Some('b'), false) => Some(Command::Bold),
            (Some('i'), false) => Some(Command::Italic),
            (Some('u'), false) => Some(Command::Underline),
            (Some('k'), false) => Some(Command::Link),
            (Some('8'), true) => Some(Command::Bullets),
            (Some('7'), true) => Some(Command::Numbers),
            _ => None,
        }
    } else if !m.command() && !m.alt() && press.key == keyboard::Key::Named(Named::Tab) {
        Some(if m.shift() {
            Command::Outdent
        } else {
            Command::Indent
        })
    } else {
        None
    };
    match cmd {
        Some(c) => Some(Binding::Custom(on(c))),
        None => Binding::from_key_press(press),
    }
}

// ------------------------------------------------------------ highlighting

/// How a stretch of a line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Look {
    bold: bool,
    italic: bool,
    underline: bool,
    /// A formatting marker (`**`, `<u>`): faint.
    faint: bool,
    /// A list marker: muted.
    list: bool,
    /// Link text: the index of its link in the line.
    link: Option<u16>,
}

/// The looks of a line's stretches. Unstyled text is left out.
pub fn looks(line: &str) -> Vec<(Range<usize>, Look)> {
    stretches(line)
        .0
        .into_iter()
        .filter(|(_, look)| *look != Look::default())
        .collect()
}

/// The looks of all of a line's stretches, in order, and the line's
/// links (which [`Look::link`] points into).
fn stretches(line: &str) -> (Vec<(Range<usize>, Look)>, Vec<Link>) {
    let mut each = vec![Look::default(); line.len()];
    let mut start = 0;
    if let Some(it) = item(line) {
        for l in &mut each[it.indent..it.end - 1] {
            l.list = true;
        }
        start = it.end;
    }
    fn mark(each: &mut [Look], range: Range<usize>, f: impl Fn(&mut Look)) {
        each[range].iter_mut().for_each(f);
    }
    // Links: `[` and `](url)` faint, the text a link. Addresses are left
    // out of the emphasis below (`_` and `*` are common in them).
    let found: Vec<Link> = links(&line[start..])
        .into_iter()
        .map(|l| Link {
            whole: l.whole.start + start..l.whole.end + start,
            text: l.text.start + start..l.text.end + start,
            url: l.url.start + start..l.url.end + start,
            bare: l.bare,
        })
        .collect();
    let mut plain = vec![true; line.len()];
    for (k, l) in found.iter().enumerate() {
        let id = Some(k as u16);
        // `[](url)` shows its address as its text.
        let text = if l.text.is_empty() { &l.url } else { &l.text };
        mark(&mut each, l.whole.clone(), |l| l.faint = true);
        mark(&mut each, text.clone(), |l| {
            l.faint = false;
            l.link = id;
        });
        if l.bare {
            plain[l.whole.clone()].fill(false);
        } else {
            plain[l.url.clone()].fill(false);
        }
    }
    // Underline: each `<u>` with the next `</u>`.
    let mut from = start;
    while let Some(o) = line[from..].find("<u>").map(|i| from + i) {
        let Some(c) = line[o + 3..].find("</u>").map(|i| o + 3 + i) else {
            break;
        };
        mark(&mut each, o..o + 3, |l| l.faint = true);
        mark(&mut each, o + 3..c, |l| l.underline = true);
        mark(&mut each, c..c + 4, |l| l.faint = true);
        from = c + 4;
    }
    // Emphasis: runs of `*` or `_`, each closing the nearest open run of
    // the same character and length.
    struct Run {
        c: char,
        at: usize,
        len: usize,
    }
    let mut open: Vec<Run> = Vec::new();
    let bytes = line.as_bytes();
    let mut i = start;
    while i < line.len() {
        let c = bytes[i] as char;
        if (c != '*' && c != '_') || !plain[i] {
            i += 1;
            continue;
        }
        let len = run_after(&line[i..], c);
        let prev = line[..i].chars().next_back();
        let next = line[i + len..].chars().next();
        let mut can_open = next.is_some_and(|n| !n.is_whitespace());
        let mut can_close = prev.is_some_and(|p| !p.is_whitespace());
        if c == '_'
            && prev.is_some_and(char::is_alphanumeric)
            && next.is_some_and(char::is_alphanumeric)
        {
            can_open = false;
            can_close = false;
        }
        let closes = can_close
            .then(|| open.iter().rposition(|r| r.c == c && r.len == len))
            .flatten();
        if let Some(k) = closes {
            let o = &open[k];
            let (bold, italic) = (len >= 2, len % 2 == 1);
            if len <= 3 {
                // The markers take the text's face too: cosmic-text picks
                // one face for a whole ASCII word, from its first
                // character, so `**Yes**` is only bold if `**` is.
                let style = |l: &mut Look| {
                    l.bold |= bold;
                    l.italic |= italic;
                };
                mark(&mut each, o.at..o.at + len, |l| {
                    l.faint = true;
                    style(l);
                });
                mark(&mut each, o.at + len..i, style);
                mark(&mut each, i..i + len, |l| {
                    l.faint = true;
                    style(l);
                });
            }
            open.truncate(k);
        } else if can_open {
            open.push(Run { c, at: i, len });
        }
        i += len;
    }
    let mut out: Vec<(Range<usize>, Look)> = Vec::new();
    let mut i = 0;
    while i < each.len() {
        let look = each[i];
        let mut j = i + 1;
        while j < each.len() && each[j] == look {
            j += 1;
        }
        out.push((i..j, look));
        i = j;
    }
    (out, found)
}

/// The editor's highlighter: each line on its own, by [`looks`].
pub struct Notes {
    line: usize,
}

impl Highlighter for Notes {
    type Settings = ();
    type Highlight = Look;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Look)>;

    fn new(_settings: &()) -> Self {
        Notes { line: 0 }
    }

    fn update(&mut self, _settings: &()) {
        self.line = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.line = self.line.min(line);
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        self.line += 1;
        looks(line).into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

/// The face for bold and italic text (`None`: the regular face).
fn face(look: &Look) -> Option<Font> {
    (look.bold || look.italic).then_some(Font {
        weight: if look.bold {
            Weight::Bold
        } else {
            Weight::Normal
        },
        style: if look.italic {
            Style::Italic
        } else {
            Style::Normal
        },
        ..theme::FONT
    })
}

/// How a [`Look`] is drawn.
pub fn format(look: &Look, theme: &Theme) -> highlighter::Format<Font> {
    let font = face(look);
    let color = if look.faint {
        Some(theme.palette().text.scale_alpha(0.35))
    } else if look.list {
        Some(theme::muted_color(theme))
    } else if look.underline || look.link.is_some() {
        Some(theme::accent_color())
    } else {
        None
    };
    highlighter::Format { color, font }
}

// ----------------------------------------------------------------- preview

/// The notes drawn as they read, for when they are not being edited:
/// markers hidden, bold, italic and underline drawn, list markers as `•`,
/// `–` or the number with the item's text hanging beside it.
/// Links are underlined in the accent colour; a click on one sends
/// `on_link` with its address ([`href`]).
pub fn preview<'a, M: 'a>(notes: &str, on_link: fn(String) -> M) -> Element<'a, M> {
    Column::with_children(notes.lines().map(|line| preview_line(line, on_link))).into()
}

fn preview_line<'a, M: 'a>(line: &str, on_link: fn(String) -> M) -> Element<'a, M> {
    let Some(it) = item(line) else {
        return rich_line(line, 0, on_link);
    };
    let mark = match it.kind {
        Kind::Bullet => "•",
        Kind::Dash => "–",
        Kind::Number => line[it.indent..it.end].trim_end(),
    };
    let depth: f32 = line[..it.indent]
        .chars()
        .map(|c| if c == '\t' { 4.0 } else { 1.0 })
        .sum();
    row![
        Space::new().width(depth * 9.0),
        container(
            text(mark.to_string())
                .size(theme::TEXT_BODY)
                .style(theme::muted)
        )
        .width(24),
        rich_line(line, it.end, on_link),
    ]
    .into()
}

/// `line[start..]` as rich text, without its formatting markers.
fn rich_line<'a, M: 'a>(line: &str, start: usize, on_link: fn(String) -> M) -> Element<'a, M> {
    let (all, found) = stretches(line);
    let spans: Vec<_> = all
        .into_iter()
        .filter(|(r, look)| r.end > start && !look.faint)
        .map(|(r, look)| {
            let s = span::<String, Font>(line[r.start.max(start)..r.end].to_string())
                .font_maybe(face(&look))
                .underline(look.underline || look.link.is_some());
            match look.link.and_then(|k| found.get(usize::from(k))) {
                Some(l) => s
                    .link(href(&line[l.url.clone()]))
                    .color(theme::accent_color()),
                None => s,
            }
        })
        .collect();
    if spans.is_empty() {
        // A blank line keeps its height.
        return text(" ").size(theme::TEXT_BODY).into();
    }
    rich_text(spans)
        .size(theme::TEXT_BODY)
        .on_link_click(on_link)
        .into()
}

// ----------------------------------------------------------------- toolbar

fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// The formatting buttons: bold, italic, underline, then the three lists.
pub fn toolbar<'a, M: Clone + 'a>(on: fn(Command) -> M) -> Element<'a, M> {
    let tool = |label: Element<'a, M>, tip: String, cmd: Command| -> Element<'a, M> {
        tooltip(
            button(container(label).center(22))
                .padding(0)
                .style(theme::icon_button)
                .on_press(on(cmd)),
            theme::tip(tip),
            tooltip::Position::Bottom,
        )
        .into()
    };
    let letter = |s: &'a str, font: Font| text(s).size(theme::TEXT_BODY).font(font).into();
    let underline: Element<'a, M> = rich_text([span::<(), _>("U").underline(true)])
        .size(theme::TEXT_BODY)
        .into();
    let list = |which: theme::Icon| theme::icon(which).width(16).height(16).into();
    let italic = Font {
        style: Style::Italic,
        ..theme::FONT
    };
    row![
        tool(
            letter("B", theme::FONT_BOLD),
            format!("Bold  {}", shortcut("B")),
            Command::Bold
        ),
        tool(
            letter("I", italic),
            format!("Italic  {}", shortcut("I")),
            Command::Italic
        ),
        tool(
            underline,
            format!("Underline  {}", shortcut("U")),
            Command::Underline
        ),
        tool(
            list(theme::Icon::Link),
            format!("Link  {}", shortcut("K")),
            Command::Link
        ),
        container(text("")).width(6),
        tool(
            list(theme::Icon::ListBullet),
            format!("Bullet list  {}", shortcut("⇧8")),
            Command::Bullets
        ),
        tool(
            list(theme::Icon::ListDash),
            "Dash list".into(),
            Command::Dashes
        ),
        tool(
            list(theme::Icon::ListNumber),
            format!("Numbered list  {}", shortcut("⇧7")),
            Command::Numbers
        ),
    ]
    .spacing(2)
    .align_y(Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(String::from).collect()
    }

    /// Applies a command to `text`, where `|` marks the cursor and `{` `}`
    /// a selection (cursor at `]`), and returns the result marked the
    /// same way.
    fn run_on(text: &str, cmd: Command) -> String {
        let (ls, cursor) = parse(text);
        let change = command(&ls, cursor, cmd).expect("a change");
        show(&ls, &change)
    }

    fn parse(text: &str) -> (Vec<String>, Cursor) {
        let mut pos = None;
        let mut sel = None;
        let mut out = Vec::new();
        for (n, line) in text.split('\n').enumerate() {
            let mut l = String::new();
            for ch in line.chars() {
                match ch {
                    '|' | '}' => {
                        pos = Some(Position {
                            line: n,
                            column: l.len(),
                        })
                    }
                    '{' => {
                        sel = Some(Position {
                            line: n,
                            column: l.len(),
                        })
                    }
                    _ => l.push(ch),
                }
            }
            out.push(l);
        }
        let cursor = Cursor {
            position: pos.expect("a cursor"),
            selection: sel,
        };
        (out, cursor)
    }

    fn show(ls: &[String], change: &Change) -> String {
        let mut all: Vec<String> = ls[..change.first].to_vec();
        all.extend(lines(&change.text));
        all.extend(ls[change.last + 1..].iter().cloned());
        let mut marks = vec![(
            change.cursor.position,
            if change.cursor.selection.is_some() {
                '}'
            } else {
                '|'
            },
        )];
        if let Some(s) = change.cursor.selection {
            marks.push((s, '{'));
        }
        marks.sort_by_key(|(p, _)| std::cmp::Reverse((p.line, p.column)));
        for (p, m) in marks {
            all[p.line].insert(p.column, m);
        }
        all.join("\n")
    }

    #[test]
    fn bold_wraps_and_unwraps() {
        assert_eq!(run_on("a {big} day", Command::Bold), "a **{big}** day");
        assert_eq!(run_on("a **{big}** day", Command::Bold), "a {big} day");
        // The markers inside the selection count too.
        assert_eq!(run_on("a {**big**} day", Command::Bold), "a {big} day");
        // Spaces at the selection's ends stay outside the markers.
        assert_eq!(run_on("a{ big }day", Command::Bold), "a{ **big** }day");
        // The word at the cursor.
        assert_eq!(run_on("a bi|g day", Command::Bold), "a **bi|g** day");
        assert_eq!(run_on("a **big|** day", Command::Bold), "a big| day");
        // No word: an empty pair, and again takes it away.
        assert_eq!(run_on("a |", Command::Bold), "a **|**");
        assert_eq!(run_on("a **|**", Command::Bold), "a |");
    }

    #[test]
    fn italic_and_bold_combine() {
        assert_eq!(run_on("**{big}**", Command::Italic), "***{big}***");
        assert_eq!(run_on("***{big}***", Command::Italic), "**{big}**");
        assert_eq!(run_on("***{big}***", Command::Bold), "*{big}*");
        assert_eq!(run_on("*{big}*", Command::Italic), "{big}");
        assert_eq!(run_on("_{big}_", Command::Italic), "{big}");
    }

    #[test]
    fn underline_uses_tags() {
        assert_eq!(run_on("{big}", Command::Underline), "<u>{big}</u>");
        assert_eq!(run_on("<u>{big}</u>", Command::Underline), "{big}");
        assert_eq!(run_on("{<u>big</u>}", Command::Underline), "{big}");
        assert_eq!(run_on("|", Command::Underline), "<u>|</u>");
    }

    #[test]
    fn inline_over_several_lines_skips_markers_and_blanks() {
        assert_eq!(
            run_on("• {one\n\n• two}", Command::Bold),
            "• **{one**\n\n• **two}**"
        );
        assert_eq!(
            run_on("• **{one**\n\n• **two}**", Command::Bold),
            "• {one\n\n• two}"
        );
        // Mixed: format the lines not yet formatted.
        assert_eq!(
            run_on("{**one**\ntwo}", Command::Bold),
            "{**one**\n**two}**"
        );
    }

    #[test]
    fn lists_mark_and_unmark_lines() {
        assert_eq!(run_on("buy |milk", Command::Bullets), "• buy |milk");
        assert_eq!(run_on("• buy |milk", Command::Bullets), "buy |milk");
        assert_eq!(run_on("- buy |milk", Command::Bullets), "• buy |milk");
        assert_eq!(run_on("|", Command::Dashes), "- |");
        assert_eq!(
            run_on("{a\n\n  b\nc}", Command::Numbers),
            "1. {a\n\n  2. b\n3. c}"
        );
        assert_eq!(run_on("1. a\n|b", Command::Numbers), "1. a\n2. |b");
        assert_eq!(run_on("{1. a\n2. b}", Command::Numbers), "{a\nb}");
        assert_eq!(run_on("{1. a\nb}", Command::Dashes), "- {a\n- b}");
    }

    #[test]
    fn indent_and_outdent() {
        assert_eq!(run_on("• a|", Command::Indent), "  • a|");
        assert_eq!(run_on("   • a|", Command::Outdent), " • a|");
        assert_eq!(run_on("\t• a|", Command::Outdent), "• a|");
        assert!(command(&lines("a"), at(0, 1), Command::Outdent).is_none());
    }

    fn typed(f: fn(&str, usize, usize) -> Option<Change>, text: &str) -> Option<String> {
        let (ls, cursor) = parse(text);
        let n = cursor.position.line;
        f(&ls[n], n, cursor.position.column).map(|c| show(&ls, &c))
    }

    #[test]
    fn enter_continues_and_ends_lists() {
        assert_eq!(typed(on_enter, "• milk|").unwrap(), "• milk\n• |");
        assert_eq!(typed(on_enter, "  9) a|").unwrap(), "  9) a\n  10) |");
        assert_eq!(
            typed(on_enter, "- [x] done|").unwrap(),
            "- [x] done\n- [ ] |"
        );
        // In the middle of the text: the rest moves to the new item.
        assert_eq!(typed(on_enter, "1. buy| milk").unwrap(), "1. buy\n2. |milk");
        // An empty item ends the list, or steps out one level.
        assert_eq!(typed(on_enter, "a\n• |").unwrap(), "a\n|");
        assert_eq!(typed(on_enter, "    - |").unwrap(), "  - |");
        // Not a list, or before the marker: Enter as usual.
        assert_eq!(typed(on_enter, "plain|"), None);
        assert_eq!(typed(on_enter, "|• a"), None);
    }

    #[test]
    fn backspace_and_space_shortcuts() {
        assert_eq!(typed(on_backspace, "  • |milk").unwrap(), "  |milk");
        assert_eq!(typed(on_backspace, "  • m|ilk"), None);
        assert_eq!(typed(on_space, "  *|milk").unwrap(), "  • |milk");
        assert_eq!(typed(on_space, "a*|"), None);
    }

    #[test]
    fn list_lines() {
        assert_eq!(
            item("• a").map(|i| (i.kind, i.end)),
            Some((Kind::Bullet, 4))
        );
        assert_eq!(
            item("  12. a").map(|i| (i.kind, i.number, i.end)),
            Some((Kind::Number, 12, 6))
        );
        for not in ["-a", "1.a", "•a", "", "1234567890. a", "**bold**"] {
            assert_eq!(item(not), None, "{not:?}");
        }
    }

    fn styled(line: &str) -> Vec<(&str, &'static str)> {
        looks(line)
            .into_iter()
            .map(|(r, l)| {
                let name = match (l.bold, l.italic, l.underline, l.faint, l.list) {
                    (_, _, _, true, _) => "faint",
                    (_, _, _, _, true) => "list",
                    (true, true, _, _, _) => "bold italic",
                    (true, _, _, _, _) => "bold",
                    (_, true, _, _, _) => "italic",
                    (_, _, true, _, _) => "underline",
                    _ => "?",
                };
                (&line[r], name)
            })
            .collect()
    }

    #[test]
    fn highlighting() {
        assert_eq!(
            styled("• a **b** *c* <u>d</u>"),
            vec![
                ("•", "list"),
                ("**", "faint"),
                ("b", "bold"),
                ("**", "faint"),
                ("*", "faint"),
                ("c", "italic"),
                ("*", "faint"),
                ("<u>", "faint"),
                ("d", "underline"),
                ("</u>", "faint"),
            ]
        );
        assert_eq!(
            styled("***x*** __y__ snake_case_name"),
            vec![
                ("***", "faint"),
                ("x", "bold italic"),
                ("***", "faint"),
                ("__", "faint"),
                ("y", "bold"),
                ("__", "faint"),
            ]
        );
        // Links: the text accent, the rest faint; no emphasis in addresses.
        let linked = |line: &str| -> Vec<(String, bool, bool)> {
            looks(line)
                .into_iter()
                .map(|(r, l)| (line[r].to_string(), l.link.is_some(), l.faint))
                .collect()
        };
        assert_eq!(
            linked("[**go**](https://a_b_c) x https://d_e_f"),
            vec![
                ("[".into(), false, true),
                ("**".into(), true, true),
                ("go".into(), true, false),
                ("**".into(), true, true),
                ("](https://a_b_c)".into(), false, true),
                ("https://d_e_f".into(), true, false),
            ]
        );
        // Unclosed or spaced markers are plain text.
        assert!(looks("2 * 3 * 4 and **open").is_empty());
        assert_eq!(styled("12. x"), vec![("12.", "list")]);
    }

    #[test]
    fn cmd_k_makes_and_removes_links() {
        // Selection or word: the cursor goes where the address is typed.
        assert_eq!(
            run_on("see {the docs} now", Command::Link),
            "see [the docs](|) now"
        );
        assert_eq!(run_on("see do|cs", Command::Link), "see [docs](|)");
        assert_eq!(run_on("a |", Command::Link), "a [](|)");
        // An address: the cursor goes where its text is typed.
        assert_eq!(
            run_on("at https://ex.am/p|le_x.", Command::Link),
            "at [|](https://ex.am/ple_x)."
        );
        assert_eq!(run_on("{www.ex.am}", Command::Link), "[|](www.ex.am)");
        // Inside a link: take it away, keeping the text.
        assert_eq!(
            run_on("see [the d|ocs](https://x.y) now", Command::Link),
            "see the docs| now"
        );
        assert_eq!(run_on("[](https://x.y|)", Command::Link), "https://x.y|");
        // Over several lines: nothing.
        assert!(
            command(
                &lines("a\nb"),
                Cursor {
                    position: Position { line: 1, column: 1 },
                    selection: Some(Position { line: 0, column: 0 })
                },
                Command::Link
            )
            .is_none()
        );
    }

    #[test]
    fn links_in_a_line() {
        let found = |line: &str| -> Vec<(String, String, bool)> {
            links(line)
                .into_iter()
                .map(|l| (line[l.text].to_string(), line[l.url].to_string(), l.bare))
                .collect()
        };
        assert_eq!(
            found("[a b](https://x.y/z_1) and (https://q.r/s), www.t.u!"),
            vec![
                ("a b".into(), "https://x.y/z_1".into(), false),
                ("https://q.r/s".into(), "https://q.r/s".into(), true),
                ("www.t.u".into(), "www.t.u".into(), true),
            ]
        );
        // Not links: a checkbox, spaces in the address, mid-word.
        assert!(found("- [ ] task [a](b c) xhttps://no").is_empty());
        assert_eq!(href("www.x.y"), "https://www.x.y");
        assert_eq!(href("http://x.y"), "http://x.y");
        assert_eq!(href("mailto:a@b.c"), "mailto:a@b.c");
    }

    #[test]
    fn changes_apply_to_the_editor() {
        let mut content = Content::with_text("buy milk\nand eggs");
        content.move_to(Cursor {
            position: Position { line: 1, column: 8 },
            selection: Some(Position { line: 0, column: 0 }),
        });
        assert!(run(&mut content, Command::Bullets));
        assert_eq!(content.text(), "• buy milk\n• and eggs");
        // (A plain move: `move_to` keeps an existing selection.)
        content.perform(Action::Move(iced::widget::text_editor::Motion::DocumentEnd));
        assert!(intercept(&mut content, &Action::Edit(Edit::Enter)));
        assert_eq!(content.text(), "• buy milk\n• and eggs\n• ");
        assert!(intercept(&mut content, &Action::Edit(Edit::Enter)));
        assert_eq!(content.text(), "• buy milk\n• and eggs\n");
        content.perform(Action::Edit(Edit::Insert('x')));
        assert_eq!(content.text(), "• buy milk\n• and eggs\nx");
        content.move_to(at(0, 6));
        assert!(run(&mut content, Command::Bold));
        assert_eq!(content.text(), "• **buy** milk\n• and eggs\nx");
        assert_eq!(content.cursor().position, Position { line: 0, column: 8 });
    }
}
