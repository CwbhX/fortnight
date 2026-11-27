//! Formatted notes: the document behind the notes editors (a day's notes
//! and a task's notes), kept as Markdown so it is stored as typed and
//! reaches the model as Markdown (`fortnight_core::notes::to_markdown`).
//!
//! The editor ([`super::rich_editor`]) shows the formatting, not the
//! Markdown: what you see is what you get. Underneath, [`Content`] holds
//! the notes as blocks (paragraphs and list items) of styled characters,
//! read from Markdown ([`Content::with_text`]) and written back
//! ([`Content::text`]):
//!
//! - `**bold**`, `*italic*`, `<u>underline</u>` (`_italic_` and
//!   `__bold__` are read too);
//! - links `[text](url)`, and bare web addresses;
//! - lists: `• ` bullets, `- ` dashes and `1. ` numbers (renumbered from 1
//!   on writing), two spaces per level of indentation.
//!
//! Editing: `⌘B` / `⌘I` / `⌘U` (or `Ctrl`) toggle the style of the
//! selection, or of what is typed next; `⌘K` asks for a link address
//! ([`Content::link_prompt`]); typing `* `, `- ` or `1. ` at the start of
//! a line starts a list; `Enter` carries a list on, and on an empty item
//! ends it (or steps out one level); `Backspace` at the start of an item
//! takes its marker away; `Tab` / `Shift+Tab` indent. The day's notes add
//! a [`toolbar`] with the same commands.

use std::collections::VecDeque;
use std::sync::Arc;

use iced::font::Style as FontStyle;
use iced::widget::{button, container, rich_text, row, span, text, text_input, tooltip};
use iced::{Center, Element, Font};

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
    /// Ask for a link address for the selection (or the word at the
    /// cursor, or the link there).
    Link,
}

// ------------------------------------------------------------------ model

/// How a character is drawn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// The address of the link the character is part of.
    pub link: Option<Arc<str>>,
}

impl Style {
    /// The face for this style's text.
    pub fn font(&self) -> Font {
        Font {
            weight: if self.bold {
                iced::font::Weight::Bold
            } else {
                iced::font::Weight::Normal
            },
            style: if self.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..theme::FONT
        }
    }

    fn marks(&self) -> (bool, bool, bool) {
        (self.underline, self.bold, self.italic)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cell {
    c: char,
    style: Style,
}

/// What kind of line a block is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum List {
    #[default]
    None,
    Bullet,
    Dash,
    Number,
}

/// The deepest indentation.
const MAX_INDENT: u8 = 6;

/// A line of the notes: a paragraph or a list item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Block {
    pub list: List,
    /// Levels of indentation (two spaces each in Markdown).
    pub indent: u8,
    cells: Vec<Cell>,
}

impl Block {
    fn len(&self) -> usize {
        self.cells.len()
    }

    /// The block's characters.
    pub fn plain(&self) -> String {
        self.cells.iter().map(|c| c.c).collect()
    }

    /// Runs of characters with the same style, in order.
    pub fn runs(&self) -> Vec<(String, Style)> {
        let mut out: Vec<(String, Style)> = Vec::new();
        for cell in &self.cells {
            match out.last_mut() {
                Some((s, style)) if *style == cell.style => s.push(cell.c),
                _ => out.push((cell.c.to_string(), cell.style.clone())),
            }
        }
        out
    }
}

/// A place in the notes: a block and a character in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub block: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(block: usize, col: usize) -> Self {
        Pos { block, col }
    }
}

/// A cursor movement within the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    BlockStart,
    BlockEnd,
    DocStart,
    DocEnd,
}

/// Something done in the editor.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Put the cursor at a place (extending the selection when `select`).
    MoveTo {
        at: Pos,
        select: bool,
    },
    Move {
        motion: Motion,
        select: bool,
    },
    /// Select the word at a place.
    SelectWord(Pos),
    /// Select the block at a place.
    SelectBlock(Pos),
    SelectAll,
    /// Typed text.
    Insert(String),
    /// Pasted text, read as Markdown.
    Paste(String),
    Enter,
    Backspace,
    Delete,
    Format(Command),
    /// Typing in the link prompt.
    LinkInput(String),
    /// `Enter` in the link prompt: link (or, empty, unlink) the text.
    LinkSubmit,
    /// Close the link prompt.
    LinkCancel,
    /// The prompt's Remove: take the link away.
    LinkRemove,
    /// Undo the last edit.
    Undo,
    /// Redo the last undone edit.
    Redo,
}

impl Action {
    /// Whether the action can change the notes.
    pub fn is_edit(&self) -> bool {
        match self {
            Action::Insert(_)
            | Action::Paste(_)
            | Action::Enter
            | Action::Backspace
            | Action::Delete
            | Action::LinkSubmit
            | Action::LinkRemove
            | Action::Undo
            | Action::Redo => true,
            Action::Format(cmd) => *cmd != Command::Link,
            _ => false,
        }
    }
}

/// What should get the keyboard focus after an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    LinkPrompt,
}

/// The `⌘K` prompt: the address typed so far, and the text it links.
#[derive(Debug, Clone, PartialEq)]
struct LinkPrompt {
    url: String,
    from: Pos,
    to: Pos,
    /// Whether the text was a link already (the prompt offers Remove).
    existing: bool,
}

/// How many undo (and redo) states are kept.
const MAX_UNDO: usize = 100;

/// A saved state of the notes for undo/redo.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    blocks: Vec<Block>,
    cursor: Pos,
    anchor: Option<Pos>,
}

/// Which kind of consecutive single-character edit is currently being
/// coalesced into a single undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditGroup {
    Insert,
    Backspace,
    Delete,
}

/// The notes being edited: blocks of styled characters, the cursor and
/// selection, and the style for what is typed next.
#[derive(Debug, Clone)]
pub struct Content {
    blocks: Vec<Block>,
    cursor: Pos,
    /// The other end of the selection.
    anchor: Option<Pos>,
    /// A style toggled with nothing selected: for what is typed next.
    pending: Option<Style>,
    link: Option<LinkPrompt>,
    focus: Option<Focus>,
    undo: VecDeque<Snapshot>,
    redo: VecDeque<Snapshot>,
    edit_group: Option<EditGroup>,
    /// Which content this is, and a count bumped by every change of its
    /// blocks (for the editor's layout).
    id: u64,
    version: u64,
}

impl Default for Content {
    fn default() -> Self {
        Content::with_text("")
    }
}

impl PartialEq for Content {
    fn eq(&self, other: &Self) -> bool {
        self.blocks == other.blocks
    }
}

impl Content {
    /// The notes in `markdown`.
    pub fn with_text(markdown: &str) -> Self {
        Content {
            blocks: parse(markdown),
            cursor: Pos::default(),
            anchor: None,
            pending: None,
            link: None,
            focus: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            edit_group: None,
            id: super::rich_editor::content_id(),
            version: 0,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// The notes as Markdown.
    pub fn text(&self) -> String {
        write(&self.blocks)
    }

    /// Whether the notes are blank: one line, with no text, list marker
    /// or indentation (the editor then shows its placeholder).
    pub fn is_empty(&self) -> bool {
        matches!(&self.blocks[..], [b] if b.cells.is_empty() && b.list == List::None && b.indent == 0)
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn cursor(&self) -> Pos {
        self.cursor
    }

    /// The selection, in order, when there is one.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        (a != self.cursor).then(|| (a.min(self.cursor), a.max(self.cursor)))
    }

    /// The number shown for each numbered item (0 for other blocks):
    /// counting from 1 through consecutive items at the same level, with
    /// deeper lines in between.
    pub fn numbers(&self) -> Vec<u32> {
        numbers(&self.blocks)
    }

    /// The address being typed for a link, while `⌘K`'s prompt is open,
    /// and whether the text is a link already.
    pub fn link_prompt(&self) -> Option<(&str, bool)> {
        self.link.as_ref().map(|l| (l.url.as_str(), l.existing))
    }

    /// What should get the focus after the last action, once.
    pub fn take_focus(&mut self) -> Option<Focus> {
        self.focus.take()
    }

    /// The selection as Markdown, for the clipboard.
    pub fn selected_text(&self) -> Option<String> {
        let (from, to) = self.selection()?;
        Some(write(&self.slice(from, to)))
    }

    /// The style at the cursor: of what is typed next.
    pub fn typing_style(&self) -> Style {
        if let Some(p) = &self.pending {
            return p.clone();
        }
        let block = &self.blocks[self.cursor.block];
        let col = self.cursor.col;
        let mut style = if col > 0 {
            block.cells[col - 1].style.clone()
        } else {
            block
                .cells
                .first()
                .map(|c| c.style.clone())
                .unwrap_or_default()
        };
        // A link carries on only inside it.
        let inside = col > 0
            && block
                .cells
                .get(col)
                .is_some_and(|c| c.style.link.is_some() && c.style.link == style.link);
        if !inside {
            style.link = None;
        }
        style
    }

    pub fn perform(&mut self, action: Action) {
        // The editor lays out again when this changes (only the lines
        // that changed).
        if action.is_edit() {
            self.version += 1;
            if !matches!(action, Action::LinkSubmit | Action::LinkRemove) {
                // Any document edit invalidates the link prompt's saved range.
                self.link = None;
            }
        }
        if !matches!(
            action,
            Action::Insert(_) | Action::Backspace | Action::Delete
        ) {
            self.edit_group = None;
        }
        match action {
            Action::MoveTo { at, select } => self.move_to(at, select),
            Action::Move { motion, select } => {
                let at = match (motion, self.selection()) {
                    (Motion::Left, Some((from, _))) if !select => from,
                    (Motion::Right, Some((_, to))) if !select => to,
                    _ => self.moved(self.cursor, motion),
                };
                self.move_to(at, select);
            }
            Action::SelectWord(at) => {
                let at = self.clamp(at);
                let (a, b) = word_at(&self.blocks[at.block].plain(), at.col);
                self.anchor = Some(Pos::new(at.block, a));
                self.cursor = Pos::new(at.block, b);
                self.pending = None;
            }
            Action::SelectBlock(at) => {
                let at = self.clamp(at);
                self.anchor = Some(Pos::new(at.block, 0));
                self.cursor = Pos::new(at.block, self.blocks[at.block].len());
                self.pending = None;
            }
            Action::SelectAll => {
                self.anchor = Some(Pos::default());
                let last = self.blocks.len() - 1;
                self.cursor = Pos::new(last, self.blocks[last].len());
                self.pending = None;
            }
            Action::Insert(s) => {
                let single_char = is_single_word_char(&s);
                let coalescing = single_char
                    && self.selection().is_none()
                    && self.edit_group == Some(EditGroup::Insert);
                if coalescing {
                    self.redo.clear();
                    self.insert(&s);
                } else {
                    let before = self.snapshot();
                    self.insert(&s);
                    self.finish_edit(before, single_char.then_some(EditGroup::Insert));
                }
            }
            Action::Paste(s) => {
                let before = self.snapshot();
                self.delete_selection();
                let blocks = parse(&s.replace("\r\n", "\n"));
                self.insert_blocks(blocks);
                self.finish_edit(before, None);
            }
            Action::Enter => {
                let before = self.snapshot();
                self.enter();
                self.finish_edit(before, None);
            }
            Action::Backspace => {
                let single_char = self.selection().is_none()
                    && self.cursor.col > 0
                    && !self.blocks[self.cursor.block].cells[self.cursor.col - 1]
                        .c
                        .is_whitespace();
                if single_char && self.edit_group == Some(EditGroup::Backspace) {
                    self.redo.clear();
                    self.backspace();
                } else {
                    let before = self.snapshot();
                    self.backspace();
                    self.finish_edit(before, single_char.then_some(EditGroup::Backspace));
                }
            }
            Action::Delete => {
                let single_char = self.selection().is_none()
                    && self.blocks[self.cursor.block]
                        .cells
                        .get(self.cursor.col)
                        .is_some_and(|cell| !cell.c.is_whitespace());
                if single_char && self.edit_group == Some(EditGroup::Delete) {
                    self.redo.clear();
                    self.delete();
                } else {
                    let before = self.snapshot();
                    self.delete();
                    self.finish_edit(before, single_char.then_some(EditGroup::Delete));
                }
            }
            Action::Format(Command::Link) => self.open_link_prompt(),
            Action::Format(cmd) => {
                let before = self.snapshot();
                self.format(cmd);
                self.finish_edit(before, None);
            }
            Action::LinkInput(s) => {
                if let Some(l) = &mut self.link {
                    l.url = s;
                }
            }
            Action::LinkSubmit => {
                let before = self.snapshot();
                self.submit_link();
                self.finish_edit(before, None);
            }
            Action::LinkRemove => {
                let before = self.snapshot();
                if let Some(l) = &mut self.link {
                    l.url.clear();
                }
                self.submit_link();
                self.finish_edit(before, None);
            }
            Action::LinkCancel => {
                self.link = None;
                self.focus = Some(Focus::Editor);
            }
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
        }
    }

    // ---- undo / redo

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            blocks: self.blocks.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo.len() >= MAX_UNDO {
            self.undo.pop_front();
        }
        self.undo.push_back(snapshot);
    }

    fn push_redo(&mut self, snapshot: Snapshot) {
        if self.redo.len() >= MAX_UNDO {
            self.redo.pop_front();
        }
        self.redo.push_back(snapshot);
    }

    fn finish_edit(&mut self, before: Snapshot, group: Option<EditGroup>) {
        if self.blocks != before.blocks {
            self.push_undo(before);
            self.redo.clear();
            self.edit_group = group;
        } else {
            self.edit_group = None;
        }
    }

    fn undo(&mut self) {
        let Some(prev) = self.undo.pop_back() else {
            return;
        };
        let current = self.snapshot();
        self.push_redo(current);
        self.blocks = prev.blocks;
        self.cursor = prev.cursor;
        self.anchor = prev.anchor;
        self.pending = None;
        self.link = None;
    }

    fn redo(&mut self) {
        let Some(next) = self.redo.pop_back() else {
            return;
        };
        let current = self.snapshot();
        self.push_undo(current);
        self.blocks = next.blocks;
        self.cursor = next.cursor;
        self.anchor = next.anchor;
        self.pending = None;
        self.link = None;
    }

    // ---- moving

    fn move_to(&mut self, at: Pos, select: bool) {
        let at = self.clamp(at);
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = at;
        self.pending = None;
    }

    fn clamp(&self, at: Pos) -> Pos {
        let block = at.block.min(self.blocks.len() - 1);
        Pos::new(block, at.col.min(self.blocks[block].len()))
    }

    fn moved(&self, at: Pos, motion: Motion) -> Pos {
        let len = |b: usize| self.blocks[b].len();
        let last = self.blocks.len() - 1;
        let chars = |b: usize| -> Vec<char> { self.blocks[b].cells.iter().map(|c| c.c).collect() };
        match motion {
            Motion::Left if at.col > 0 => Pos::new(at.block, at.col - 1),
            Motion::Left if at.block > 0 => Pos::new(at.block - 1, len(at.block - 1)),
            Motion::Right if at.col < len(at.block) => Pos::new(at.block, at.col + 1),
            Motion::Right if at.block < last => Pos::new(at.block + 1, 0),
            Motion::Left | Motion::Right => at,
            Motion::WordLeft if at.col == 0 => self.moved(at, Motion::Left),
            Motion::WordLeft => {
                let chars = chars(at.block);
                let mut i = at.col;
                while i > 0 && !is_word(chars[i - 1]) {
                    i -= 1;
                }
                while i > 0 && is_word(chars[i - 1]) {
                    i -= 1;
                }
                Pos::new(at.block, i)
            }
            Motion::WordRight if at.col == len(at.block) => self.moved(at, Motion::Right),
            Motion::WordRight => {
                let chars = chars(at.block);
                let mut i = at.col;
                while i < chars.len() && !is_word(chars[i]) {
                    i += 1;
                }
                while i < chars.len() && is_word(chars[i]) {
                    i += 1;
                }
                Pos::new(at.block, i)
            }
            Motion::BlockStart => Pos::new(at.block, 0),
            Motion::BlockEnd => Pos::new(at.block, len(at.block)),
            Motion::DocStart => Pos::default(),
            Motion::DocEnd => Pos::new(last, len(last)),
        }
    }

    // ---- editing

    /// The blocks between two places, cut out as their own notes.
    fn slice(&self, from: Pos, to: Pos) -> Vec<Block> {
        (from.block..=to.block)
            .map(|b| {
                let block = &self.blocks[b];
                let a = if b == from.block { from.col } else { 0 };
                let z = if b == to.block { to.col } else { block.len() };
                Block {
                    cells: block.cells[a..z].to_vec(),
                    ..block.clone()
                }
            })
            .collect()
    }

    /// Removes the selection, if any, leaving the cursor where it was.
    fn delete_selection(&mut self) -> bool {
        let Some((from, to)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.remove(from, to);
        true
    }

    fn remove(&mut self, from: Pos, to: Pos) {
        let tail: Vec<Cell> = self.blocks[to.block].cells[to.col..].to_vec();
        self.blocks[from.block].cells.truncate(from.col);
        self.blocks[from.block].cells.extend(tail);
        self.blocks.drain(from.block + 1..=to.block);
        self.cursor = from;
        self.anchor = None;
    }

    fn insert(&mut self, s: &str) {
        self.delete_selection();
        let style = self.typing_style();
        for (i, line) in s.split('\n').enumerate() {
            if i > 0 {
                self.split();
            }
            for c in line.chars().filter(|c| !c.is_control()) {
                if c == ' ' && self.list_shortcut() {
                    continue;
                }
                let Pos { block, col } = self.cursor;
                self.blocks[block].cells.insert(
                    col,
                    Cell {
                        c,
                        style: style.clone(),
                    },
                );
                self.cursor.col += 1;
            }
        }
        self.pending = None;
    }

    /// A space after `*`, `-`, `+`, `•` or `1.` at the start of a
    /// paragraph: make it a list item instead. Whether it did.
    fn list_shortcut(&mut self) -> bool {
        let Pos { block, col } = self.cursor;
        let b = &self.blocks[block];
        if b.list != List::None {
            return false;
        }
        let typed: String = b.cells[..col].iter().map(|c| c.c).collect();
        let list = match typed.as_str() {
            "*" | "•" => List::Bullet,
            "-" | "+" => List::Dash,
            t if t.len() > 1
                && t.len() <= 4
                && (t.ends_with('.') || t.ends_with(')'))
                && t[..t.len() - 1].chars().all(|c| c.is_ascii_digit()) =>
            {
                List::Number
            }
            _ => return false,
        };
        let b = &mut self.blocks[block];
        b.list = list;
        b.cells.drain(..col);
        self.cursor.col = 0;
        true
    }

    /// Breaks the block at the cursor; the new block takes the list and
    /// indentation.
    fn split(&mut self) {
        let Pos { block, col } = self.cursor;
        let rest = self.blocks[block].cells.split_off(col);
        let b = &self.blocks[block];
        let next = Block {
            list: b.list,
            indent: b.indent,
            cells: rest,
        };
        self.blocks.insert(block + 1, next);
        self.cursor = Pos::new(block + 1, 0);
    }

    fn insert_blocks(&mut self, mut blocks: Vec<Block>) {
        if blocks.is_empty() {
            return;
        }
        let first = blocks.remove(0);
        let Pos { block, col } = self.cursor;
        let tail = self.blocks[block].cells.split_off(col);
        let here = &mut self.blocks[block];
        if here.cells.is_empty() && here.list == List::None {
            here.list = first.list;
            here.indent = first.indent;
        }
        here.cells.extend(first.cells);
        let mut at = Pos::new(block, here.len());
        for (i, b) in blocks.into_iter().enumerate() {
            self.blocks.insert(block + 1 + i, b);
            at = Pos::new(block + 1 + i, self.blocks[block + 1 + i].len());
        }
        self.blocks[at.block].cells.extend(tail);
        self.cursor = at;
        self.anchor = None;
    }

    fn enter(&mut self) {
        self.delete_selection();
        let b = &mut self.blocks[self.cursor.block];
        if b.list != List::None && b.cells.is_empty() {
            // An empty item: out one level, or out of the list.
            if b.indent > 0 {
                b.indent -= 1;
            } else {
                b.list = List::None;
            }
            return;
        }
        self.split();
        self.pending = None;
    }

    fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { block, col } = self.cursor;
        if col > 0 {
            self.blocks[block].cells.remove(col - 1);
            self.cursor.col -= 1;
            return;
        }
        let b = &mut self.blocks[block];
        if b.list != List::None {
            b.list = List::None;
        } else if b.indent > 0 {
            b.indent -= 1;
        } else if block > 0 {
            let at = Pos::new(block - 1, self.blocks[block - 1].len());
            self.remove(at, Pos::new(block, 0));
        }
    }

    fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { block, col } = self.cursor;
        if col < self.blocks[block].len() {
            self.blocks[block].cells.remove(col);
        } else if block + 1 < self.blocks.len() {
            self.remove(self.cursor, Pos::new(block + 1, 0));
        }
    }

    /// The selected blocks, or the cursor's.
    fn selected_blocks(&self) -> std::ops::RangeInclusive<usize> {
        match self.selection() {
            Some((from, to)) => from.block..=to.block,
            None => self.cursor.block..=self.cursor.block,
        }
    }

    fn format(&mut self, cmd: Command) {
        match cmd {
            Command::Bold => self.toggle(|s| &mut s.bold),
            Command::Italic => self.toggle(|s| &mut s.italic),
            Command::Underline => self.toggle(|s| &mut s.underline),
            Command::Bullets => self.toggle_list(List::Bullet),
            Command::Dashes => self.toggle_list(List::Dash),
            Command::Numbers => self.toggle_list(List::Number),
            Command::Indent | Command::Outdent => {
                for b in self.selected_blocks() {
                    let b = &mut self.blocks[b];
                    b.indent = if cmd == Command::Indent {
                        (b.indent + 1).min(MAX_INDENT)
                    } else {
                        b.indent.saturating_sub(1)
                    };
                }
            }
            Command::Link => self.open_link_prompt(),
        }
    }

    /// Bold, italic or underline: the selection's characters all get it,
    /// or (when they all have it) all lose it. With nothing selected it
    /// is for what is typed next.
    fn toggle(&mut self, flag: fn(&mut Style) -> &mut bool) {
        let Some((from, to)) = self.selection() else {
            let mut style = self.typing_style();
            let f = flag(&mut style);
            *f = !*f;
            self.pending = Some(style);
            return;
        };
        let mut cells = Vec::new();
        for b in from.block..=to.block {
            let a = if b == from.block { from.col } else { 0 };
            let z = if b == to.block {
                to.col
            } else {
                self.blocks[b].len()
            };
            cells.extend((a..z).map(|c| (b, c)));
        }
        let all = cells.iter().all(|&(b, c)| {
            let mut s = self.blocks[b].cells[c].style.clone();
            *flag(&mut s)
        });
        for (b, c) in cells {
            *flag(&mut self.blocks[b].cells[c].style) = !all;
        }
    }

    fn toggle_list(&mut self, list: List) {
        let range = self.selected_blocks();
        let all = self.blocks[range.clone()].iter().all(|b| b.list == list);
        for b in range {
            self.blocks[b].list = if all { List::None } else { list };
        }
    }

    /// The link at a place: its start and end in the block.
    fn link_at(&self, at: Pos) -> Option<(usize, usize)> {
        let cells = &self.blocks[at.block].cells;
        let here = cells.get(at.col).filter(|c| c.style.link.is_some());
        let col = if here.is_some() {
            at.col
        } else {
            at.col.checked_sub(1)?
        };
        let link = cells.get(col)?.style.link.clone()?;
        let same = |c: &Cell| c.style.link.as_ref() == Some(&link);
        let start = cells[..col]
            .iter()
            .rposition(|c| !same(c))
            .map_or(0, |i| i + 1);
        let end = cells[col..]
            .iter()
            .position(|c| !same(c))
            .map_or(cells.len(), |i| col + i);
        Some((start, end))
    }

    fn open_link_prompt(&mut self) {
        let (from, to) = match self.selection() {
            Some(sel) => sel,
            None => {
                let at = self.cursor;
                let (a, b) = self
                    .link_at(at)
                    .unwrap_or_else(|| word_at(&self.blocks[at.block].plain(), at.col));
                (Pos::new(at.block, a), Pos::new(at.block, b))
            }
        };
        let url = (from < to)
            .then(|| self.blocks[from.block].cells.get(from.col))
            .flatten()
            .and_then(|c| c.style.link.as_deref().map(str::to_string));
        self.link = Some(LinkPrompt {
            existing: url.is_some(),
            url: url.unwrap_or_default(),
            from,
            to,
        });
        self.focus = Some(Focus::LinkPrompt);
    }

    fn submit_link(&mut self) {
        let Some(prompt) = self.link.take() else {
            return;
        };
        self.focus = Some(Focus::Editor);
        let url = prompt.url.trim();
        let link: Option<Arc<str>> = (!url.is_empty()).then(|| Arc::from(url));
        let (a, b) = (self.clamp(prompt.from), self.clamp(prompt.to));
        let (from, to) = if a <= b { (a, b) } else { (b, a) };
        self.anchor = None;
        if from == to {
            // Nothing to link: the address becomes the text.
            self.cursor = from;
            if let Some(link) = link {
                let style = Style {
                    link: Some(link.clone()),
                    ..self.typing_style()
                };
                let cells: Vec<Cell> = link
                    .chars()
                    .map(|c| Cell {
                        c,
                        style: style.clone(),
                    })
                    .collect();
                self.cursor.col += cells.len();
                self.blocks[from.block]
                    .cells
                    .splice(from.col..from.col, cells);
            }
            return;
        }
        for b in from.block..=to.block {
            let a = if b == from.block { from.col } else { 0 };
            let z = if b == to.block {
                to.col
            } else {
                self.blocks[b].len()
            };
            for c in &mut self.blocks[b].cells[a..z] {
                c.style.link = link.clone();
            }
        }
        self.cursor = to;
    }
}

fn is_single_word_char(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if !c.is_control() && !c.is_whitespace())
        && chars.next().is_none()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '\''
}

/// The word around character `col` of `text` (letters, digits and
/// apostrophes), as a character range; empty when there is none.
fn word_at(text: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let col = col.min(chars.len());
    let mut a = col;
    while a > 0 && is_word(chars[a - 1]) {
        a -= 1;
    }
    let mut b = col;
    while b < chars.len() && is_word(chars[b]) {
        b += 1;
    }
    (a, b)
}

fn numbers(blocks: &[Block]) -> Vec<u32> {
    // The count so far at each indentation.
    let mut counts = [0u32; MAX_INDENT as usize + 1];
    blocks
        .iter()
        .map(|b| {
            let level = usize::from(b.indent.min(MAX_INDENT));
            // Deeper levels start again under a shallower line.
            for c in &mut counts[level + 1..] {
                *c = 0;
            }
            if b.list == List::Number {
                counts[level] += 1;
                counts[level]
            } else {
                counts[level] = 0;
                0
            }
        })
        .collect()
}

// ------------------------------------------------------------- reading

/// A list line's marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Item {
    list: List,
    /// Where the marker starts (the indentation's length), in bytes.
    indent: usize,
    /// Where the text after the marker and its space starts, in bytes.
    end: usize,
}

fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// The list marker at the start of `line`, if it is a list line.
fn item(line: &str) -> Option<Item> {
    let indent = indent_len(line);
    let rest = &line[indent..];
    let at = |list, len| {
        Some(Item {
            list,
            indent,
            end: indent + len,
        })
    };
    if rest.starts_with("• ") {
        return at(List::Bullet, "• ".len());
    }
    if ["- ", "* ", "+ "].iter().any(|m| rest.starts_with(m)) {
        return at(List::Dash, 2);
    }
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if (1..=9).contains(&digits) {
        let after = &rest[digits..];
        if after.starts_with(". ") || after.starts_with(") ") {
            return at(List::Number, digits + 2);
        }
    }
    None
}

/// The notes in `markdown`, one block per line.
fn parse(markdown: &str) -> Vec<Block> {
    markdown
        .split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let (list, indent, start) = match item(line) {
                Some(it) => (it.list, it.indent, it.end),
                None => (List::None, indent_len(line), indent_len(line)),
            };
            let columns: usize = line[..indent]
                .chars()
                .map(|c| if c == '\t' { 2 } else { 1 })
                .sum();
            Block {
                list,
                indent: (columns / 2).min(usize::from(MAX_INDENT)) as u8,
                cells: parse_inline(&line[start..]),
            }
        })
        .collect()
}

/// A link in a line: Markdown's `[text](url)`, or a bare web address
/// (`https://…`, `http://…`, `www.…`), whose text is its address.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    whole: std::ops::Range<usize>,
    text: std::ops::Range<usize>,
    url: std::ops::Range<usize>,
    bare: bool,
}

fn is_address(s: &str) -> bool {
    ["https://", "http://", "www.", "mailto:"]
        .iter()
        .any(|p| s.starts_with(p))
        && !s.contains(char::is_whitespace)
}

/// The link starting at byte `i` of `line`, if one does.
fn link_at(line: &str, i: usize) -> Option<Link> {
    if line[..i].ends_with('\\') {
        return None;
    }
    if let Some(link) = markdown_link(line, i) {
        return Some(link);
    }
    let rest = &line[i..];
    let word_start = line[..i]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric());
    if !word_start
        || !["https://", "http://", "www.", "mailto:"]
            .iter()
            .any(|p| rest.starts_with(p))
    {
        return None;
    }
    let mut end = i + rest.find(char::is_whitespace).unwrap_or(rest.len());
    // Trailing punctuation belongs to the sentence (or is a formatting
    // marker).
    while end > i && line[..end].ends_with(['.', ',', ';', ':', '!', '?', ')', '"', '\'', '*', '_'])
    {
        end -= 1;
    }
    (end > i).then_some(Link {
        whole: i..end,
        text: i..end,
        url: i..end,
        bare: true,
    })
}

/// A `[text](url)` or `[text](<url>)` link starting at byte `i`.
fn markdown_link(line: &str, i: usize) -> Option<Link> {
    let rest = line[i..].strip_prefix('[')?;
    let close = i + 1 + rest.find(']')?;
    if line[i + 1..close].contains('[') || !line[close + 1..].starts_with('(') {
        return None;
    }
    let after_open = close + 2;
    if line[after_open..].starts_with('<') {
        let url_start = after_open + 1;
        let rel_end = line[url_start..].find(['>', '\n', '\r'])?;
        let url_end = url_start + rel_end;
        if url_end == url_start || !line[url_end..].starts_with(">)") {
            return None;
        }
        return Some(Link {
            whole: i..url_end + 2,
            text: i + 1..close,
            url: url_start..url_end,
            bare: false,
        });
    }
    let url_start = after_open;
    let mut depth = 0usize;
    let mut url_end = None;
    for (off, c) in line[url_start..].char_indices() {
        if c.is_whitespace() {
            break;
        }
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => {
                url_end = Some(url_start + off);
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let url_end = url_end?;
    if url_end == url_start {
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
    if url.contains("://")
        || url
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("mailto:"))
    {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

/// A piece of a line while reading it.
#[derive(Debug, Clone)]
enum Piece {
    Char(Cell),
    /// A run of `*` or `_`, `len` long: `n` of them still unused.
    Delim {
        c: char,
        n: usize,
        len: usize,
        open: bool,
        close: bool,
        style: Style,
    },
    /// `<u>` (`true`) or `</u>`, not yet paired.
    Under(bool, Style),
    /// A marker used up.
    Gone,
}

impl Piece {
    fn style(&mut self) -> Option<&mut Style> {
        match self {
            Piece::Char(cell) => Some(&mut cell.style),
            Piece::Delim { style, .. } | Piece::Under(_, style) => Some(style),
            Piece::Gone => None,
        }
    }

    fn delim(&self) -> Option<(char, usize)> {
        match self {
            Piece::Delim { c, n, .. } => Some((*c, *n)),
            _ => None,
        }
    }
}

fn plain(c: char) -> Cell {
    Cell {
        c,
        style: Style::default(),
    }
}

/// One line's text (after any list marker) as styled characters.
fn parse_inline(s: &str) -> Vec<Cell> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap_or(' ');
        if c == '\\'
            && let Some(next) = rest[1..]
                .chars()
                .next()
                .filter(|c| c.is_ascii_punctuation() || *c == '•')
        {
            pieces.push(Piece::Char(plain(next)));
            i += 1 + next.len_utf8();
            continue;
        }
        if let Some(link) = link_at(s, i) {
            let url: Arc<str> = Arc::from(&s[link.url.clone()]);
            let cells = if link.bare {
                s[link.text.clone()].chars().map(plain).collect()
            } else if link.text.is_empty() {
                s[link.url.clone()].chars().map(plain).collect()
            } else {
                parse_inline(&s[link.text.clone()])
            };
            pieces.extend(cells.into_iter().map(|mut cell| {
                cell.style.link = Some(url.clone());
                Piece::Char(cell)
            }));
            i = link.whole.end;
            continue;
        }
        if rest.starts_with("<u>") || rest.starts_with("</u>") {
            let open = rest.starts_with("<u>");
            pieces.push(Piece::Under(open, Style::default()));
            i += if open { 3 } else { 4 };
            continue;
        }
        if c == '*' || c == '_' {
            let n = rest.chars().take_while(|x| *x == c).count();
            let prev = s[..i].chars().next_back();
            let next = s[i + n..].chars().next();
            let mut open = next.is_some_and(|x| !x.is_whitespace());
            let mut close = prev.is_some_and(|x| !x.is_whitespace());
            if c == '_' {
                open &= prev.is_none_or(|x| !x.is_alphanumeric());
                close &= next.is_none_or(|x| !x.is_alphanumeric());
            }
            pieces.push(Piece::Delim {
                c,
                n,
                len: n,
                open,
                close,
                style: Style::default(),
            });
            i += n;
            continue;
        }
        pieces.push(Piece::Char(plain(c)));
        i += c.len_utf8();
    }

    // Underline: each `<u>` with the next `</u>`.
    let mut open_u = None;
    for k in 0..pieces.len() {
        match pieces[k] {
            Piece::Under(true, _) if open_u.is_none() => open_u = Some(k),
            Piece::Under(false, _) => {
                if let Some(o) = open_u.take() {
                    for p in &mut pieces[o + 1..k] {
                        if let Some(s) = p.style() {
                            s.underline = true;
                        }
                    }
                    pieces[o] = Piece::Gone;
                    pieces[k] = Piece::Gone;
                }
            }
            _ => {}
        }
    }

    // Emphasis: a closing run uses up the nearest opening run of the same
    // character, two at a time when both have two or more (bold), else
    // one (italic).
    let mut openers: Vec<usize> = Vec::new();
    for k in 0..pieces.len() {
        let Piece::Delim {
            c,
            open,
            close,
            len,
            ..
        } = pieces[k]
        else {
            continue;
        };
        if close {
            loop {
                let left = pieces[k].delim().map_or(0, |(_, n)| n);
                if left == 0 {
                    break;
                }
                // CommonMark's rule of three: a run that could open and
                // close doesn't pair with one whose lengths add up to a
                // multiple of three (unless both are), so `**a*b***` is
                // bold "a*b*" with italic "b".
                let pairs = |o: usize| match pieces[o] {
                    Piece::Delim {
                        c: oc,
                        len: olen,
                        close: oclose,
                        ..
                    } => {
                        oc == c
                            && !((open || oclose)
                                && (olen + len) % 3 == 0
                                && !(olen % 3 == 0 && len % 3 == 0))
                    }
                    _ => false,
                };
                let Some(oi) = openers.iter().rposition(|&o| pairs(o)) else {
                    break;
                };
                let o = openers[oi];
                let have = pieces[o].delim().map_or(0, |(_, n)| n);
                let used = if have >= 2 && left >= 2 { 2 } else { 1 };
                for p in &mut pieces[o + 1..k] {
                    if let Some(s) = p.style() {
                        if used == 2 {
                            s.bold = true;
                        } else {
                            s.italic = true;
                        }
                    }
                }
                for at in [o, k] {
                    if let Piece::Delim { n, .. } = &mut pieces[at] {
                        *n -= used;
                    }
                }
                // Openers between the two can no longer close anything.
                openers.truncate(oi + 1);
                if have == used {
                    openers.pop();
                }
            }
        }
        if open && pieces[k].delim().is_some_and(|(_, n)| n > 0) {
            openers.push(k);
        }
    }

    let mut out = Vec::new();
    for p in pieces {
        match p {
            Piece::Gone => {}
            Piece::Char(cell) => out.push(cell),
            Piece::Delim { c, n, style, .. } => {
                out.extend((0..n).map(|_| Cell {
                    c,
                    style: style.clone(),
                }));
            }
            Piece::Under(open, style) => {
                let tag = if open { "<u>" } else { "</u>" };
                out.extend(tag.chars().map(|c| Cell {
                    c,
                    style: style.clone(),
                }));
            }
        }
    }
    out
}

// ------------------------------------------------------------- writing

/// The notes as Markdown: a line per block.
fn write(blocks: &[Block]) -> String {
    let numbers = numbers(blocks);
    blocks
        .iter()
        .zip(numbers)
        .map(|(b, n)| {
            let mut line = "  ".repeat(usize::from(b.indent));
            match b.list {
                List::None => {}
                List::Bullet => line.push_str("• "),
                List::Dash => line.push_str("- "),
                List::Number => line.push_str(&format!("{n}. ")),
            }
            let text = write_inline(&b.cells);
            // A paragraph that would read as a list item keeps its text.
            match item(&text) {
                Some(it) if b.list == List::None => match it.list {
                    List::Number => {
                        let dot = it.indent + text[it.indent..].find(['.', ')']).unwrap_or(0);
                        line.push_str(&text[..dot]);
                        line.push('\\');
                        line.push_str(&text[dot..]);
                    }
                    List::Dash | List::Bullet => {
                        line.push_str(&text[..it.indent]);
                        line.push('\\');
                        line.push_str(&text[it.indent..]);
                    }
                    List::None => line.push_str(&text),
                },
                _ => line.push_str(&text),
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Characters as Markdown text, escaped where they would read as
/// formatting.
fn escape(chars: &[char], out: &mut String) {
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1).copied();
        let rest: String = chars[i..chars.len().min(i + 4)].iter().collect();
        let special = match c {
            '\\' | '*' | '[' | ']' => true,
            '_' => {
                !(prev.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric))
            }
            '<' => rest.starts_with("<u>") || rest.starts_with("</u>"),
            _ => false,
        };
        if special {
            out.push('\\');
        }
        out.push(c);
    }
}

/// Closes open markers (innermost first) until `keep` are left.
fn close_to(out: &mut String, open: &mut Vec<char>, keep: usize) {
    while open.len() > keep {
        match open.pop() {
            Some('u') => out.push_str("</u>"),
            Some('b') => out.push_str("**"),
            Some(_) => out.push('*'),
            None => {}
        }
    }
}

/// A line's characters as Markdown, the markers opened and closed where
/// the style changes.
fn write_inline(cells: &[Cell]) -> String {
    // Stretches of one style; spaces lose bold and italic, whose markers
    // may not start or end on a space.
    let mut runs: Vec<(Vec<char>, Style)> = Vec::new();
    for cell in cells {
        let mut style = cell.style.clone();
        if cell.c.is_whitespace() {
            style.bold = false;
            style.italic = false;
        }
        match runs.last_mut() {
            Some((chars, s)) if *s == style => chars.push(cell.c),
            _ => runs.push((vec![cell.c], style)),
        }
    }

    let mut out = String::new();
    // The open markers, outermost first: 'u', 'b', 'i'.
    let mut open: Vec<char> = Vec::new();
    let mut k = 0;
    while k < runs.len() {
        let link = runs[k].1.link.clone();
        // The runs of one link, or of no link.
        let mut end = k;
        while end < runs.len() && runs[end].1.link == link {
            end += 1;
        }
        if let Some(url) = &link {
            close_to(&mut out, &mut open, 0);
            let text: String = runs[k..end].iter().flat_map(|(c, _)| c.iter()).collect();
            let unstyled = runs[k..end]
                .iter()
                .all(|(_, s)| s.marks() == (false, false, false));
            let prev_alnum = out.chars().next_back().is_some_and(char::is_alphanumeric);
            let trailing_punct =
                url.ends_with(['.', ',', ';', ':', '!', '?', ')', '"', '\'', '*', '_']);
            if unstyled && text == **url && is_address(url) && !prev_alnum && !trailing_punct {
                out.push_str(url);
                k = end;
                continue;
            }
            out.push('[');
        }
        for (chars, style) in &runs[k..end] {
            let (u, b, i) = style.marks();
            let want: Vec<char> = [(u, 'u'), (b, 'b'), (i, 'i')]
                .into_iter()
                .filter_map(|(on, m)| on.then_some(m))
                .collect();
            // Keep what is open and still wanted, in the same order.
            let keep = open.iter().zip(&want).take_while(|(a, b)| a == b).count();
            close_to(&mut out, &mut open, keep);
            for &m in &want[keep..] {
                out.push_str(match m {
                    'u' => "<u>",
                    'b' => "**",
                    _ => "*",
                });
                open.push(m);
            }
            escape(chars, &mut out);
        }
        if let Some(url) = &link {
            close_to(&mut out, &mut open, 0);
            let needs_angle = url.contains(|c: char| c.is_whitespace() || c == '(' || c == ')')
                && !url.contains(['>', '\n', '\r']);
            if needs_angle {
                out.push_str("](<");
                out.push_str(url);
                out.push_str(">)");
            } else {
                out.push_str("](");
                out.push_str(url);
                out.push(')');
            }
        }
        k = end;
    }
    close_to(&mut out, &mut open, 0);
    out
}

// ------------------------------------------------------------ link prompt

/// `⌘K`'s prompt under an editor, while it is open: the address (with
/// the id `id`, to focus it), Remove for a link already there, and ×.
pub fn link_prompt<'a, M: Clone + 'a>(
    content: &'a Content,
    id: &'static str,
    on: impl Fn(Action) -> M + Clone + 'a,
) -> Option<Element<'a, M>> {
    let (url, existing) = content.link_prompt()?;
    let input = text_input("Paste or type a link, then Enter", url)
        .id(id)
        .on_input({
            let on = on.clone();
            move |s| on(Action::LinkInput(s))
        })
        .on_submit(on(Action::LinkSubmit))
        .size(theme::TEXT_BODY)
        .padding([6, 10])
        .style(theme::input);
    let mut bar = row![theme::icon(theme::Icon::Link).width(16).height(16), input]
        .spacing(8)
        .align_y(Center);
    if existing {
        bar = bar.push(
            button(text("Remove link").size(theme::TEXT_SMALL))
                .style(theme::text_button)
                .on_press(on(Action::LinkRemove)),
        );
    }
    bar = bar.push(
        button(theme::icon(theme::Icon::Close).width(16).height(16))
            .padding(4)
            .style(theme::icon_button)
            .on_press(on(Action::LinkCancel)),
    );
    Some(bar.into())
}

// ----------------------------------------------------------------- toolbar

fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// The formatting buttons: bold, italic, underline, link, then the three
/// lists.
pub fn toolbar<'a, M: Clone + 'a>(on: impl Fn(Command) -> M + 'a) -> Element<'a, M> {
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
    let icon = |which: theme::Icon| theme::icon(which).width(16).height(16).into();
    let italic = Font {
        style: FontStyle::Italic,
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
            icon(theme::Icon::Link),
            format!("Link  {}", shortcut("K")),
            Command::Link
        ),
        container(text("")).width(6),
        tool(
            icon(theme::Icon::ListBullet),
            format!("Bullet list  {}", shortcut("⇧8")),
            Command::Bullets
        ),
        tool(
            icon(theme::Icon::ListDash),
            "Dash list".into(),
            Command::Dashes
        ),
        tool(
            icon(theme::Icon::ListNumber),
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

    fn round_trip(md: &str) {
        assert_eq!(Content::with_text(md).text(), md, "{md:?}");
    }

    fn styled(md: &str) -> Vec<(String, &'static str)> {
        let c = Content::with_text(md);
        c.blocks[0]
            .runs()
            .into_iter()
            .map(|(t, s)| {
                let name = match (s.bold, s.italic, s.underline, s.link.is_some()) {
                    (_, _, _, true) => "link",
                    (true, true, _, _) => "bold italic",
                    (true, _, _, _) => "bold",
                    (_, true, _, _) => "italic",
                    (_, _, true, _) => "underline",
                    _ => "plain",
                };
                (t, name)
            })
            .collect()
    }

    #[test]
    fn reads_markdown_formatting() {
        assert_eq!(
            styled("a **b** *c* <u>d</u> ***e*** __f__ _g_ snake_case"),
            vec![
                ("a ".into(), "plain"),
                ("b".into(), "bold"),
                (" ".into(), "plain"),
                ("c".into(), "italic"),
                (" ".into(), "plain"),
                ("d".into(), "underline"),
                (" ".into(), "plain"),
                ("e".into(), "bold italic"),
                (" ".into(), "plain"),
                ("f".into(), "bold"),
                (" ".into(), "plain"),
                ("g".into(), "italic"),
                (" snake_case".into(), "plain"),
            ]
        );
        assert_eq!(
            styled("**ab*c***"),
            vec![("ab".into(), "bold"), ("c".into(), "bold italic")]
        );
        // Unclosed or spaced markers, and escapes, are text.
        assert_eq!(
            styled("2 * 3 * 4 **open \\*x\\*"),
            vec![("2 * 3 * 4 **open *x*".into(), "plain")]
        );
        assert_eq!(
            styled("see [the **docs**](https://x.y/a_b) or https://q.r/s_t."),
            vec![
                ("see ".into(), "plain"),
                ("the ".into(), "link"),
                ("docs".into(), "link"),
                (" or ".into(), "plain"),
                ("https://q.r/s_t".into(), "link"),
                (".".into(), "plain"),
            ]
        );
    }

    #[test]
    fn reads_lists() {
        let c = Content::with_text("• a\n  - b\n1. c\n    2) d\nplain\n  indented");
        let kinds: Vec<(List, u8, String)> = c
            .blocks
            .iter()
            .map(|b| (b.list, b.indent, b.plain()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (List::Bullet, 0, "a".into()),
                (List::Dash, 1, "b".into()),
                (List::Number, 0, "c".into()),
                (List::Number, 2, "d".into()),
                (List::None, 0, "plain".into()),
                (List::None, 1, "indented".into()),
            ]
        );
    }

    #[test]
    fn writes_what_it_reads() {
        for md in [
            "",
            "plain text",
            "**bold** and *italic* and <u>under</u>",
            "***both*** then **b*bi***",
            "H**ello** w*or*ld",
            "<u>**x**</u> y",
            "[a **link**](https://x.y) and https://bare.example/p_q",
            "• a\n  • b\n- c\n1. d\n2. e\n  1. f\n3. g",
            "\n\nblank lines\n\n",
            "2 \\* 3 and \\[not a link\\] and a_b\\_ and \\<u> tag",
            "\\- not a list\n1\\. nor this",
        ] {
            round_trip(md);
        }
        // Lists are renumbered; bullets and spaces are tidied.
        assert_eq!(Content::with_text("3. a\n7) b").text(), "1. a\n2. b");
        assert_eq!(Content::with_text("* a\n\t- b").text(), "- a\n  - b");
        // Formatting across a space closes and opens around it.
        let mut c = Content::with_text("ab cd");
        c.perform(Action::SelectAll);
        c.perform(Action::Format(Command::Bold));
        assert_eq!(c.text(), "**ab** **cd**");
    }

    fn typed(c: &mut Content, s: &str) {
        c.perform(Action::Insert(s.into()));
    }

    #[test]
    fn typing_with_styles_and_list_shortcuts() {
        let mut c = Content::default();
        typed(&mut c, "* milk");
        assert_eq!(c.text(), "• milk");
        c.perform(Action::Enter);
        typed(&mut c, "eggs ");
        c.perform(Action::Format(Command::Bold));
        typed(&mut c, "now");
        c.perform(Action::Format(Command::Bold));
        typed(&mut c, "!");
        assert_eq!(c.text(), "• milk\n• eggs **now**!");
        // Enter on an empty item ends the list (the line stays, as a
        // paragraph); Backspace at an item's start takes the marker away,
        // then joins the lines.
        c.perform(Action::Enter);
        c.perform(Action::Enter);
        typed(&mut c, "1. one");
        c.perform(Action::Enter);
        typed(&mut c, "two");
        assert_eq!(c.text(), "• milk\n• eggs **now**!\n1. one\n2. two");
        c.perform(Action::Move {
            motion: Motion::BlockStart,
            select: false,
        });
        c.perform(Action::Backspace);
        assert_eq!(c.text(), "• milk\n• eggs **now**!\n1. one\ntwo");
        c.perform(Action::Backspace);
        assert_eq!(c.text(), "• milk\n• eggs **now**!\n1. onetwo");
        // Nested items step out one level on an empty Enter.
        let mut c = Content::with_text("• a");
        c.perform(Action::Move {
            motion: Motion::DocEnd,
            select: false,
        });
        c.perform(Action::Enter);
        c.perform(Action::Format(Command::Indent));
        assert_eq!(c.text(), "• a\n  • ");
        c.perform(Action::Enter);
        assert_eq!(c.text(), "• a\n• ");
    }

    #[test]
    fn formatting_a_selection() {
        let mut c = Content::with_text("one two\nthree");
        c.perform(Action::MoveTo {
            at: Pos::new(0, 4),
            select: false,
        });
        c.perform(Action::MoveTo {
            at: Pos::new(1, 2),
            select: true,
        });
        c.perform(Action::Format(Command::Italic));
        assert_eq!(c.text(), "one *two*\n*th*ree");
        c.perform(Action::Format(Command::Italic));
        assert_eq!(c.text(), "one two\nthree");
        c.perform(Action::Format(Command::Numbers));
        assert_eq!(c.text(), "1. one two\n2. three");
        c.perform(Action::Format(Command::Numbers));
        assert_eq!(c.text(), "one two\nthree");
        // Typing replaces the selection.
        typed(&mut c, "X");
        assert_eq!(c.text(), "one Xree");
        assert_eq!(c.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn cmd_k_asks_for_an_address() {
        let mut c = Content::with_text("see the docs");
        c.perform(Action::MoveTo {
            at: Pos::new(0, 10),
            select: false,
        });
        c.perform(Action::Format(Command::Link));
        assert_eq!(c.link_prompt(), Some(("", false)));
        assert_eq!(c.take_focus(), Some(Focus::LinkPrompt));
        c.perform(Action::LinkInput("https://x.y".into()));
        c.perform(Action::LinkSubmit);
        assert_eq!(c.text(), "see the [docs](https://x.y)");
        assert_eq!(c.take_focus(), Some(Focus::Editor));
        // In a link: the prompt has its address; empty takes it away.
        c.perform(Action::MoveTo {
            at: Pos::new(0, 9),
            select: false,
        });
        c.perform(Action::Format(Command::Link));
        assert_eq!(c.link_prompt(), Some(("https://x.y", true)));
        c.perform(Action::LinkInput(String::new()));
        c.perform(Action::LinkSubmit);
        assert_eq!(c.text(), "see the docs");
        // Nothing there: the address becomes the text.
        let mut c = Content::with_text("go ");
        c.perform(Action::Move {
            motion: Motion::DocEnd,
            select: false,
        });
        c.perform(Action::Format(Command::Link));
        c.perform(Action::LinkInput("www.x.y".into()));
        c.perform(Action::LinkSubmit);
        assert_eq!(c.text(), "go www.x.y");
        // Typing after a link doesn't extend it.
        typed(&mut c, " ok");
        assert_eq!(c.text(), "go www.x.y ok");
        assert_eq!(href("www.x.y"), "https://www.x.y");
    }

    #[test]
    fn paste_reads_markdown_and_copy_writes_it() {
        let mut c = Content::with_text("ab");
        c.perform(Action::MoveTo {
            at: Pos::new(0, 1),
            select: false,
        });
        c.perform(Action::Paste("**x**\n- y".into()));
        assert_eq!(c.text(), "a**x**\n- yb");
        c.perform(Action::MoveTo {
            at: Pos::new(0, 0),
            select: false,
        });
        c.perform(Action::MoveTo {
            at: Pos::new(1, 1),
            select: true,
        });
        assert_eq!(c.selected_text().as_deref(), Some("a**x**\n- y"));
    }

    #[test]
    fn moving_by_words_and_blocks() {
        let mut c = Content::with_text("one, two\nthree");
        let mv = |c: &mut Content, motion| {
            c.perform(Action::Move {
                motion,
                select: false,
            });
            c.cursor()
        };
        assert_eq!(mv(&mut c, Motion::WordRight), Pos::new(0, 3));
        assert_eq!(mv(&mut c, Motion::WordRight), Pos::new(0, 8));
        assert_eq!(mv(&mut c, Motion::Right), Pos::new(1, 0));
        assert_eq!(mv(&mut c, Motion::WordLeft), Pos::new(0, 8));
        assert_eq!(mv(&mut c, Motion::WordLeft), Pos::new(0, 5));
        c.perform(Action::SelectWord(Pos::new(1, 2)));
        assert_eq!(c.selection(), Some((Pos::new(1, 0), Pos::new(1, 5))));
        // Left with a selection goes to its start.
        assert_eq!(mv(&mut c, Motion::Left), Pos::new(1, 0));
    }

    /// An empty list item is not blank: the placeholder would cover its
    /// marker.
    #[test]
    fn a_list_marker_alone_is_not_empty() {
        let mut c = Content::default();
        assert!(c.is_empty());
        c.perform(Action::Format(Command::Numbers));
        assert!(!c.is_empty());
        assert_eq!(c.text(), "1. ");
        c.perform(Action::Format(Command::Numbers));
        c.perform(Action::Format(Command::Indent));
        assert!(!c.is_empty());
    }

    #[test]
    fn numbering() {
        let c = Content::with_text("1. a\n  1. b\n  2. c\n1. d\nx\n1. e");
        assert_eq!(c.numbers(), vec![1, 1, 2, 2, 0, 1]);
    }

    #[test]
    fn link_prompt_and_markdown_edge_cases() {
        // Opening the link prompt is not a content edit.
        assert!(!Action::Format(Command::Link).is_edit());
        assert!(Action::Format(Command::Bold).is_edit());

        // Editing the document clears an open link prompt instead of panicking
        // on stale positions when submitted later.
        let mut c = Content::with_text("first line\nsecond line");
        c.perform(Action::MoveTo {
            at: Pos::new(0, 6),
            select: false,
        });
        c.perform(Action::MoveTo {
            at: Pos::new(1, 3),
            select: true,
        });
        c.perform(Action::Format(Command::Link));
        assert!(c.link_prompt().is_some());
        c.perform(Action::SelectAll);
        c.perform(Action::Backspace);
        assert!(c.link_prompt().is_none());
        c.perform(Action::LinkSubmit);

        // Case-insensitive mailto href and bare mailto round-trip.
        assert_eq!(href("MAILTO:user@example.com"), "MAILTO:user@example.com");
        round_trip("email mailto:user@example.com now");

        // URLs with parentheses or spaces round-trip cleanly.
        let mut c = Content::with_text("Rust");
        c.perform(Action::SelectAll);
        c.perform(Action::Format(Command::Link));
        c.perform(Action::LinkInput(
            "https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
        ));
        c.perform(Action::LinkSubmit);
        let md = c.text();
        assert_eq!(
            md,
            "[Rust](<https://en.wikipedia.org/wiki/Rust_(programming_language)>)"
        );
        assert_eq!(Content::with_text(&md).text(), md);
        // Existing balanced-paren markdown links also parse.
        let wiki = "[Rust](https://en.wikipedia.org/wiki/Rust_(programming_language))";
        let parsed = Content::with_text(wiki);
        assert_eq!(
            parsed.blocks()[0].runs()[0].1.link.as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(programming_language)")
        );

        // Plain paragraphs starting with `• ` or leading space + `- ` escape cleanly.
        round_trip("\\• not a bullet");
        let mut c = Content::default();
        c.perform(Action::Insert(" - foo".into()));
        let md = c.text();
        assert!(md.contains("\\- foo"), "{md:?}");
        assert_eq!(Content::with_text(&md).blocks()[0].list, List::None);
    }

    fn type_chars(c: &mut Content, s: &str) {
        for ch in s.chars() {
            c.perform(Action::Insert(ch.to_string()));
        }
    }

    #[test]
    fn undo_redo_typing_and_coalescing() {
        assert!(Action::Undo.is_edit());
        assert!(Action::Redo.is_edit());

        let mut c = Content::default();
        assert!(!c.can_undo());
        assert!(!c.can_redo());

        // Consecutive single non-whitespace characters coalesce into one word step,
        // while whitespace breaks groups.
        type_chars(&mut c, "hello world");
        assert_eq!(c.text(), "hello world");
        assert!(c.can_undo());
        assert!(!c.can_redo());

        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello ");
        assert_eq!(c.cursor(), Pos::new(0, 6));
        assert!(c.can_redo());

        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello");
        assert_eq!(c.cursor(), Pos::new(0, 5));

        c.perform(Action::Undo);
        assert_eq!(c.text(), "");
        assert_eq!(c.cursor(), Pos::new(0, 0));
        assert!(!c.can_undo());

        // Redo replays the exact steps forward.
        c.perform(Action::Redo);
        assert_eq!(c.text(), "hello");
        assert_eq!(c.cursor(), Pos::new(0, 5));
        c.perform(Action::Redo);
        assert_eq!(c.text(), "hello ");
        c.perform(Action::Redo);
        assert_eq!(c.text(), "hello world");
        assert_eq!(c.cursor(), Pos::new(0, 11));
        assert!(!c.can_redo());

        // Enter and Paste each break coalescing into their own undo steps.
        c.perform(Action::Enter);
        type_chars(&mut c, "line");
        c.perform(Action::Paste("**two**".into()));
        assert_eq!(c.text(), "hello world\nline**two**");

        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world\nline");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world\n");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world");
    }

    #[test]
    fn undo_redo_deletions_coalesce_and_break_properly() {
        let mut c = Content::with_text("hello world");
        c.perform(Action::Move {
            motion: Motion::DocEnd,
            select: false,
        });

        // Consecutive non-whitespace backspaces coalesce into one step.
        for _ in 0..5 {
            c.perform(Action::Backspace);
        }
        assert_eq!(c.text(), "hello ");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world");
        assert_eq!(c.cursor(), Pos::new(0, 11));

        // Switching from typing to backspace creates separate undo steps.
        type_chars(&mut c, "!");
        c.perform(Action::Backspace);
        assert_eq!(c.text(), "hello world");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world!");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world");

        // Consecutive forward-deletes coalesce; whitespace breaks the group.
        c.perform(Action::Move {
            motion: Motion::DocStart,
            select: false,
        });
        for _ in 0..5 {
            c.perform(Action::Delete);
        }
        assert_eq!(c.text(), " world");
        c.perform(Action::Delete); // deletes space -> separate step
        assert_eq!(c.text(), "world");

        c.perform(Action::Undo);
        assert_eq!(c.text(), " world");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world");

        // Deleting a selection (e.g. Cut) is its own discrete undo step and restores selection.
        c.perform(Action::SelectWord(Pos::new(0, 1)));
        assert_eq!(c.selection(), Some((Pos::new(0, 0), Pos::new(0, 5))));
        c.perform(Action::Delete);
        assert_eq!(c.text(), " world");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world");
        assert_eq!(c.selection(), Some((Pos::new(0, 0), Pos::new(0, 5))));
    }

    #[test]
    fn undo_redo_formatting_lists_indent_and_links() {
        let mut c = Content::with_text("hello world");
        c.perform(Action::SelectWord(Pos::new(0, 1)));
        c.perform(Action::Format(Command::Bold));
        assert_eq!(c.text(), "**hello** world");

        c.perform(Action::Format(Command::Italic));
        assert_eq!(c.text(), "***hello*** world");

        c.perform(Action::Format(Command::Bullets));
        assert_eq!(c.text(), "• ***hello*** world");

        c.perform(Action::Format(Command::Indent));
        assert_eq!(c.text(), "  • ***hello*** world");

        // Add a link to "world".
        c.perform(Action::MoveTo {
            at: Pos::new(0, 8),
            select: false,
        });
        c.perform(Action::Format(Command::Link));
        c.perform(Action::LinkInput("https://example.com".into()));
        c.perform(Action::LinkSubmit);
        assert_eq!(c.text(), "  • ***hello*** [world](https://example.com)");

        // Undo link, indent, bullets, italic, bold in reverse order.
        c.perform(Action::Undo);
        assert_eq!(c.text(), "  • ***hello*** world");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "• ***hello*** world");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "***hello*** world");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "**hello** world");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "hello world");

        // Redo all the way back to the linked, indented bullet.
        for _ in 0..5 {
            c.perform(Action::Redo);
        }
        assert_eq!(c.text(), "  • ***hello*** [world](https://example.com)");

        // Removing the link can also be undone.
        c.perform(Action::MoveTo {
            at: Pos::new(0, 8),
            select: false,
        });
        c.perform(Action::Format(Command::Link));
        c.perform(Action::LinkRemove);
        assert_eq!(c.text(), "  • ***hello*** world");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "  • ***hello*** [world](https://example.com)");

        // Toggling bold with no selection breaks the insert group without pushing a no-op snapshot.
        let mut c = Content::default();
        type_chars(&mut c, "ab");
        c.perform(Action::Format(Command::Bold));
        type_chars(&mut c, "cd");
        assert_eq!(c.text(), "ab**cd**");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "ab");
        c.perform(Action::Undo);
        assert_eq!(c.text(), "");
    }

    #[test]
    fn new_edit_clears_redo_stack() {
        let mut c = Content::default();
        type_chars(&mut c, "first");
        c.perform(Action::Enter);
        type_chars(&mut c, "second");
        assert_eq!(c.text(), "first\nsecond");

        c.perform(Action::Undo);
        assert_eq!(c.text(), "first\n");
        assert!(c.can_redo());

        // Making a new edit discards redo history.
        type_chars(&mut c, "other");
        assert_eq!(c.text(), "first\nother");
        assert!(!c.can_redo());

        c.perform(Action::Redo);
        assert_eq!(c.text(), "first\nother");
    }

    #[test]
    fn undo_redo_on_empty_stacks_is_noop_and_partial_eq_ignores_history() {
        let mut c = Content::with_text("initial");
        assert!(!c.can_undo());
        assert!(!c.can_redo());

        c.perform(Action::Undo);
        c.perform(Action::Redo);
        assert_eq!(c.text(), "initial");
        assert_eq!(c.cursor(), Pos::default());
        assert!(!c.can_undo());
        assert!(!c.can_redo());

        // PartialEq compares blocks only, not undo/redo stacks.
        let fresh = Content::with_text("initial");
        type_chars(&mut c, "x");
        c.perform(Action::Undo);
        assert_eq!(c, fresh);
        assert!(c.can_redo());
        assert!(!fresh.can_redo());
    }

    #[test]
    fn undo_stack_depth_is_capped() {
        let mut c = Content::default();
        for i in 0..(MAX_UNDO + 25) {
            c.perform(Action::Insert(format!("{i}")));
            c.perform(Action::Enter);
        }
        assert_eq!(c.undo.len(), MAX_UNDO);

        let mut steps = 0;
        while c.can_undo() {
            c.perform(Action::Undo);
            steps += 1;
        }
        assert_eq!(steps, MAX_UNDO);
        assert_eq!(c.redo.len(), MAX_UNDO);
    }
}
