//! The notes editor: edits a [`Content`] showing its formatting (bold,
//! italic, underline, links, lists) rather than its Markdown.
//!
//! Each block is laid out as one iced paragraph of styled spans
//! (`Paragraph::with_spans`), indented by its level, with its list marker
//! (`•`, `-` or its number) drawn in a gutter beside it. The caret,
//! selection and underlines are drawn from the paragraphs' cosmic-text
//! layout runs (iced draws no underlines for a paragraph itself), and
//! clicks are placed with the same layout (`Buffer::hit`).
//!
//! The widget only reports what was done as [`Action`]s, through
//! `on_action`; the owner applies them to the content
//! ([`Content::perform`]), as with iced's `text_editor`. Copy, cut and
//! paste go through the clipboard as Markdown. A click on a link with
//! `Ctrl` or `⌘` held opens it (`on_link`); a plain click puts the caret
//! there and shows a popup above it with the address, Open and Edit
//! (which opens `⌘K`'s prompt).
//!
//! The widget works with iced's own renderer only, whose paragraphs are
//! cosmic-text buffers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::graphics::text::cosmic_text;
use iced::advanced::input_method::{self, InputMethod};
use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{
    Alignment, LineHeight, Paragraph as _, Renderer as _, Shaping, Span, Text, Wrapping,
};
use iced::advanced::widget::{self, Id, Widget, operation, tree};
use iced::advanced::{Clipboard, Shell, clipboard, mouse, overlay, renderer};
use iced::alignment::Vertical;
use iced::keyboard::{self, key::Named};
use iced::time::{Duration, Instant};
use iced::widget::text_editor::Status;
use iced::{
    Border, Element, Event, Font, Length, Padding, Pixels, Point, Rectangle, Size, Theme, Vector,
    window,
};

use super::rich_notes::{Action, Block, Command, Content, List, Motion, Pos, href};
use crate::theme;

/// The indentation of one level.
const INDENT: f32 = 18.0;
/// The width of a list item's marker column, at least: room for the
/// marker and a small gap (a long number gets more).
fn gutter(list: List) -> f32 {
    match list {
        List::None => 0.0,
        List::Bullet | List::Dash => 14.0,
        List::Number => 16.0,
    }
}

/// The space between a list marker and its text, at least.
const MARKER_GAP: f32 = 6.0;

static NEXT_CONTENT: AtomicU64 = AtomicU64::new(1);

/// A number for each new [`Content`], so the editor can tell a different
/// one from an edited one.
pub(super) fn content_id() -> u64 {
    NEXT_CONTENT.fetch_add(1, Ordering::Relaxed)
}

/// An editor for `content`, reporting what is done with `on_action`.
pub fn rich_editor<'a, Message>(
    content: &'a Content,
    on_action: impl Fn(Action) -> Message + 'a,
) -> RichEditor<'a, Message> {
    RichEditor {
        content,
        id: None,
        placeholder: "",
        on_action: Box::new(on_action),
        on_link: None,
        min_height: 0.0,
        padding: Padding::new(10.0),
        size: theme::TEXT_BODY,
    }
}

pub struct RichEditor<'a, Message> {
    content: &'a Content,
    id: Option<Id>,
    placeholder: &'a str,
    on_action: Box<dyn Fn(Action) -> Message + 'a>,
    on_link: Option<Box<dyn Fn(String) -> Message + 'a>>,
    min_height: f32,
    padding: Padding,
    size: f32,
}

impl<'a, Message> RichEditor<'a, Message> {
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = placeholder;
        self
    }

    /// What a click on a link sends, with its address ([`href`]).
    pub fn on_link(mut self, on_link: impl Fn(String) -> Message + 'a) -> Self {
        self.on_link = Some(Box::new(on_link));
        self
    }

    pub fn min_height(mut self, min_height: f32) -> Self {
        self.min_height = min_height;
        self
    }

    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

// ------------------------------------------------------------------ state

#[derive(Debug, Clone)]
struct Focus {
    updated_at: Instant,
    now: Instant,
    window_focused: bool,
}

impl Focus {
    const BLINK: u128 = 500;

    fn now() -> Self {
        let now = Instant::now();
        Focus {
            updated_at: now,
            now,
            window_focused: true,
        }
    }

    fn caret_visible(&self) -> bool {
        self.window_focused
            && ((self.now - self.updated_at).as_millis() / Self::BLINK).is_multiple_of(2)
    }
}

/// One block, laid out.
#[derive(Debug)]
struct Laid {
    /// What it was laid out from: the block, its number, the text area's
    /// width and the text size.
    source: (Block, u32, f32, f32),
    paragraph: Paragraph,
    marker: Option<Paragraph>,
    /// The width of the marker's column.
    gutter: f32,
    /// The text's top left, from the top left of the text area.
    origin: Point,
    height: f32,
    /// The block's characters (byte and character places differ).
    plain: String,
    /// For each span: underlined, and its link.
    spans: Vec<(bool, Option<Arc<str>>)>,
}

impl Laid {
    fn new(block: &Block, number: u32, width: f32, size: f32) -> Self {
        let indent = f32::from(block.indent) * INDENT;
        let marker = match block.list {
            List::None => None,
            List::Bullet => Some("•".to_string()),
            List::Dash => Some("-".to_string()),
            List::Number => Some(format!("{number}.")),
        }
        .map(|m| Paragraph::with_text(text(m.as_str(), f32::INFINITY, size)));
        // A number's column fits the number ("10." is wider).
        let gutter = marker
            .as_ref()
            .map_or(0.0, |m| gutter(block.list).max(m.min_width() + MARKER_GAP));
        let runs = block.runs();
        let spans: Vec<Span<'_, (), Font>> = runs
            .iter()
            .map(|(t, style)| {
                let span = Span::new(t.as_str()).font(style.font());
                if style.link.is_some() {
                    span.color(theme::accent_color())
                } else {
                    span
                }
            })
            .collect();
        let text_width = (width - indent - gutter).max(1.0);
        let paragraph = Paragraph::with_spans(text(spans.as_slice(), text_width, size));
        let height = paragraph.min_bounds().height.max(line_height(size));
        Laid {
            source: (block.clone(), number, width, size),
            paragraph,
            marker,
            gutter,
            origin: Point::new(indent + gutter, 0.0),
            height,
            plain: block.plain(),
            spans: runs
                .into_iter()
                .map(|(_, s)| (s.underline || s.link.is_some(), s.link))
                .collect(),
        }
    }

    fn made_for(&self, block: &Block, number: u32, width: f32, size: f32) -> bool {
        let (b, n, w, s) = &self.source;
        b == block && *n == number && *w == width && *s == size
    }
}

#[derive(Debug, Default)]
struct State {
    focus: Option<Focus>,
    last_click: Option<mouse::Click>,
    dragging: bool,
    modifiers: keyboard::Modifiers,
    /// What the layout was made for: the content's id and version, the
    /// width and the text size.
    made_for: Option<(u64, u64, f32, f32)>,
    blocks: Vec<Laid>,
    placeholder: Paragraph,
    /// What the placeholder was laid out from: its text, width and size.
    placeholder_for: Option<(String, f32, f32)>,
    /// The look last drawn, to redraw when hovering or focus changes it
    /// (iced only redraws when asked).
    last_status: Option<Status>,
    /// The popup over a clicked link.
    menu: Option<Menu>,
}

/// The popup over a link clicked without `Ctrl`/`⌘`: Open or Edit.
#[derive(Debug)]
struct Menu {
    url: Arc<str>,
    /// Where the link was clicked.
    at: Pos,
    /// The top of the clicked line at the click, in the text area.
    anchor: Point,
    line_height: f32,
    /// Its texts, kept here: iced draws a paragraph after the frame is
    /// built, through a weak reference.
    shown_url: Paragraph,
    open: Paragraph,
    edit: Paragraph,
    /// The button under the pointer: Open (`true`) or Edit.
    hovered: Option<bool>,
}

impl Menu {
    fn new(url: Arc<str>, at: Pos, anchor: Point, line_height: f32) -> Self {
        let shown: String = if url.chars().count() > 40 {
            url.chars().take(39).chain(['…']).collect()
        } else {
            url.to_string()
        };
        let label = |s: &str, font| {
            Paragraph::with_text(Text {
                font,
                ..text(s, f32::INFINITY, theme::TEXT_SMALL)
            })
        };
        Menu {
            shown_url: label(&shown, theme::FONT),
            open: label("Open", theme::FONT_BOLD),
            edit: label("Edit", theme::FONT_BOLD),
            url,
            at,
            anchor,
            line_height,
            hovered: None,
        }
    }

    /// The Open and Edit buttons' areas within the popup's bounds.
    fn buttons(&self, bounds: Rectangle) -> (Rectangle, Rectangle) {
        let h = bounds.height;
        let open_x = bounds.x + MENU_PAD + self.shown_url.min_width() + MENU_GAP / 2.0;
        let open_w = self.open.min_width() + MENU_GAP;
        let edit_w = self.edit.min_width() + MENU_GAP;
        (
            Rectangle::new(Point::new(open_x, bounds.y), Size::new(open_w, h)),
            Rectangle::new(Point::new(open_x + open_w, bounds.y), Size::new(edit_w, h)),
        )
    }

    /// Where the popup goes in a window of size `window`, the text area
    /// at `text`: above the clicked line, starting at the click, kept
    /// inside the window (under the line when there is no room above).
    fn place(&self, text: Point, window: Size) -> Rectangle {
        let height = self.shown_url.min_height() + 2.0 * MENU_PAD;
        let width = MENU_PAD * 2.0
            + self.shown_url.min_width()
            + self.open.min_width()
            + self.edit.min_width()
            + MENU_GAP * 2.0;
        let x = (text.x + self.anchor.x - MENU_PAD)
            .min(window.width - width - 4.0)
            .max(4.0);
        let top = text.y + self.anchor.y;
        let y = if top - height - 4.0 >= 0.0 {
            top - height - 4.0
        } else {
            top + self.line_height + 4.0
        };
        Rectangle::new(Point::new(x, y), Size::new(width, height))
    }
}

const MENU_PAD: f32 = 8.0;
const MENU_GAP: f32 = 14.0;

impl operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focus.is_some()
    }

    fn focus(&mut self) {
        self.focus = Some(Focus::now());
    }

    fn unfocus(&mut self) {
        self.focus = None;
        self.dragging = false;
    }
}

fn text<C>(content: C, width: f32, size: f32) -> Text<C, Font> {
    Text {
        content,
        bounds: Size::new(width, f32::INFINITY),
        size: Pixels(size),
        line_height: LineHeight::default(),
        font: theme::FONT,
        align_x: Alignment::Default,
        align_y: Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::WordOrGlyph,
    }
}

fn line_height(size: f32) -> f32 {
    LineHeight::default().to_absolute(Pixels(size)).0
}

fn byte_of(plain: &str, col: usize) -> usize {
    plain
        .char_indices()
        .nth(col)
        .map_or(plain.len(), |(i, _)| i)
}

fn col_of(plain: &str, byte: usize) -> usize {
    let mut byte = byte.min(plain.len());
    while !plain.is_char_boundary(byte) {
        byte -= 1;
    }
    plain[..byte].chars().count()
}

impl State {
    /// Lays the content out at `width`. Lines that haven't changed (the
    /// same at the start and at the end) keep their layout, so typing
    /// lays out only the line typed in.
    fn lay_out(&mut self, content: &Content, width: f32, size: f32, placeholder: &str) {
        let blocks = content.blocks();
        let numbers = content.numbers();
        let mut old: Vec<Option<Laid>> = std::mem::take(&mut self.blocks)
            .into_iter()
            .map(Some)
            .collect();
        let same = |laid: &Option<Laid>, i: usize| {
            laid.as_ref()
                .is_some_and(|l| l.made_for(&blocks[i], numbers[i], width, size))
        };
        let (n, m) = (blocks.len(), old.len());
        let start = (0..n.min(m)).take_while(|&i| same(&old[i], i)).count();
        let end = (0..(n - start).min(m - start))
            .take_while(|&k| same(&old[m - 1 - k], n - 1 - k))
            .count();
        let mut y = 0.0;
        for i in 0..n {
            let kept = if i < start {
                old[i].take()
            } else if i >= n - end {
                old[m - (n - i)].take()
            } else {
                None
            };
            let mut laid = kept.unwrap_or_else(|| Laid::new(&blocks[i], numbers[i], width, size));
            laid.origin.y = y;
            y += laid.height;
            self.blocks.push(laid);
        }
        let wanted = (placeholder.to_string(), width, size);
        if self.placeholder_for.as_ref() != Some(&wanted) {
            self.placeholder = Paragraph::with_text(text(placeholder, width, size));
            self.placeholder_for = Some(wanted);
        }
    }

    fn total_height(&self) -> f32 {
        self.blocks.last().map_or(0.0, |b| b.origin.y + b.height)
    }

    /// The block under a point of the text area (the nearest one).
    fn block_at(&self, p: Point) -> usize {
        self.blocks
            .iter()
            .rposition(|b| b.origin.y <= p.y)
            .unwrap_or(0)
    }

    /// The place at a point of the text area.
    fn hit(&self, p: Point) -> Pos {
        let b = self.block_at(p);
        let Some(laid) = self.blocks.get(b) else {
            return Pos::default();
        };
        let x = p.x - laid.origin.x;
        let y = (p.y - laid.origin.y).clamp(0.0, (laid.height - 1.0).max(0.0));
        let col = laid
            .paragraph
            .buffer()
            .hit(x.max(0.0), y)
            .map_or(laid.plain.chars().count(), |c| col_of(&laid.plain, c.index));
        Pos::new(b, col)
    }

    /// The link under a point of the text area.
    fn link_at(&self, p: Point) -> Option<Arc<str>> {
        let laid = self.blocks.get(self.block_at(p))?;
        let local = Point::new(p.x - laid.origin.x, p.y - laid.origin.y);
        if local.x < 0.0 || local.y < 0.0 || local.y > laid.height {
            return None;
        }
        let span = laid.paragraph.hit_span(local)?;
        laid.spans.get(span)?.1.clone()
    }

    /// The caret at a place: its top left in the text area and height.
    fn caret(&self, at: Pos, size: f32) -> (Point, f32) {
        let Some(laid) = self.blocks.get(at.block) else {
            return (Point::ORIGIN, line_height(size));
        };
        let index = byte_of(&laid.plain, at.col);
        let mut found = None;
        let mut last = (0.0, 0.0, line_height(size));
        for run in laid.paragraph.buffer().layout_runs() {
            let (top, h) = (run.line_top, run.line_height);
            let (Some(first), Some(end)) = (run.glyphs.first(), run.glyphs.last()) else {
                last = (0.0, top, h);
                continue;
            };
            if index < first.start {
                // A space a line was wrapped at has no glyph: the caret
                // stays at the end of the line before.
                found = Some(if run.line_i == 0 && top > 0.0 {
                    last
                } else {
                    (first.x, top, h)
                });
                break;
            }
            if let Some(g) = run
                .glyphs
                .iter()
                .find(|g| g.start <= index && index < g.end)
            {
                let part = if index > g.start {
                    g.w * (index - g.start) as f32 / (g.end - g.start) as f32
                } else {
                    0.0
                };
                found = Some((g.x + part, top, h));
                break;
            }
            last = (end.x + end.w, top, h);
        }
        let (x, top, h) = found.unwrap_or(last);
        (Point::new(laid.origin.x + x, laid.origin.y + top), h)
    }

    /// The place a line up (or down) from `at`, at the same x.
    fn vertical(&self, at: Pos, down: bool, size: f32) -> Pos {
        let (p, h) = self.caret(at, size);
        let y = if down { p.y + h + 1.0 } else { p.y - 1.0 };
        if y < 0.0 {
            return Pos::new(0, 0);
        }
        if y >= self.total_height() {
            let last = self.blocks.len().saturating_sub(1);
            return Pos::new(
                last,
                self.blocks.get(last).map_or(0, |b| b.plain.chars().count()),
            );
        }
        self.hit(Point::new(p.x, y))
    }
}

/// The editor's look for its focus and the pointer.
fn status(state: &State, cursor: mouse::Cursor, bounds: Rectangle) -> Status {
    let is_hovered = cursor.is_over(bounds);
    match &state.focus {
        Some(_) => Status::Focused { is_hovered },
        None if is_hovered => Status::Hovered,
        None => Status::Active,
    }
}

// ----------------------------------------------------------------- widget

impl<Message> Widget<Message, Theme, iced::Renderer> for RichEditor<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let limits = limits.width(Length::Fill).height(Length::Shrink);
        let width = limits.max().width;
        let inner = (width - self.padding.x()).max(1.0);
        let made_for = (self.content.id(), self.content.version(), inner, self.size);
        if state.made_for != Some(made_for) {
            state.lay_out(self.content, inner, self.size, self.placeholder);
            state.made_for = Some(made_for);
        }
        let height = (state.total_height() + self.padding.y()).max(self.min_height);
        layout::Node::new(limits.resolve(Length::Fill, Length::Shrink, Size::new(width, height)))
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let text_bounds = bounds.shrink(self.padding);
        let local = |p: Point| Point::new(p.x - text_bounds.x, p.y - text_bounds.y);
        let publish =
            |shell: &mut Shell<'_, Message>, action| shell.publish((self.on_action)(action));

        // iced redraws only when asked: ask when the look changes.
        let status = status(state, cursor, bounds);
        if matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
            state.last_status = Some(status);
        } else if state.last_status != Some(status) {
            shell.request_redraw();
        }

        match event {
            Event::Window(window::Event::Unfocused) => {
                if let Some(f) = &mut state.focus {
                    f.window_focused = false;
                }
            }
            Event::Window(window::Event::Focused) => {
                if let Some(f) = &mut state.focus {
                    f.window_focused = true;
                    f.updated_at = Instant::now();
                    shell.request_redraw();
                }
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                let ime = match &mut state.focus {
                    Some(f) if f.window_focused => {
                        f.now = *now;
                        let wait = Focus::BLINK - (f.now - f.updated_at).as_millis() % Focus::BLINK;
                        shell.request_redraw_at(*now + Duration::from_millis(wait as u64));
                        let (p, h) = state.caret(self.content.cursor(), self.size);
                        InputMethod::Enabled {
                            cursor: Rectangle::new(
                                Point::new(text_bounds.x + p.x, text_bounds.y + p.y),
                                Size::new(1.0, h),
                            ),
                            purpose: input_method::Purpose::Normal,
                            preedit: None,
                        }
                    }
                    _ => InputMethod::Disabled,
                };
                shell.request_input_method::<&str>(&ime);
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => state.modifiers = *m,
            // `Ctrl`-click on a Mac can arrive as a right click.
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
                if state.modifiers.control() =>
            {
                if let (Some(on_link), Some(p)) = (&self.on_link, cursor.position_over(bounds))
                    && let Some(url) = state.link_at(local(p))
                {
                    state.menu = None;
                    shell.publish(on_link(href(&url)));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Any click closes the link popup (its own buttons act
                // before this).
                if state.menu.take().is_some() {
                    shell.request_redraw();
                }
                let Some(p) = cursor.position_over(bounds) else {
                    if state.focus.is_some() {
                        state.focus = None;
                        state.dragging = false;
                        shell.request_redraw();
                    }
                    return;
                };
                let p = local(p);
                let link = self
                    .on_link
                    .as_ref()
                    .and_then(|on| Some((on, state.link_at(p)?)));
                if let Some((on_link, url)) = &link
                    && (state.modifiers.command() || state.modifiers.control())
                {
                    // `Ctrl`/`⌘`-click: open it.
                    shell.publish(on_link(href(url)));
                    shell.capture_event();
                    return;
                }
                let at = state.hit(p);
                if let Some((_, url)) = link {
                    // A plain click: the caret goes there, and the popup
                    // offers Open and Edit.
                    let (caret, line_height) = state.caret(at, self.size);
                    state.menu = Some(Menu::new(url, at, Point::new(p.x, caret.y), line_height));
                }
                let click = mouse::Click::new(p, mouse::Button::Left, state.last_click);
                let action = match click.kind() {
                    mouse::click::Kind::Single => {
                        state.dragging = true;
                        Action::MoveTo {
                            at,
                            select: state.modifiers.shift(),
                        }
                    }
                    mouse::click::Kind::Double => Action::SelectWord(at),
                    mouse::click::Kind::Triple => Action::SelectBlock(at),
                };
                state.last_click = Some(click);
                state.focus = Some(Focus::now());
                publish(shell, action);
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging = false;
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.dragging => {
                let at = state.hit(local(*position));
                publish(shell, Action::MoveTo { at, select: true });
            }
            Event::InputMethod(input_method::Event::Commit(s)) if state.focus.is_some() => {
                publish(shell, Action::Insert(s.clone()));
                if let Some(f) = &mut state.focus {
                    f.updated_at = Instant::now();
                }
                shell.capture_event();
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modified_key,
                physical_key,
                modifiers,
                text,
                ..
            }) if state.focus.is_some() => {
                let m = *modifiers;
                let mut actions = Vec::new();
                let shortcut = m.command() || m.control();
                let letter = key.to_latin(*physical_key);
                if shortcut && !m.alt() {
                    let cmd = m.command();
                    match (letter, m.shift()) {
                        (Some('c'), _) if cmd => {
                            if let Some(s) = self.content.selected_text() {
                                clipboard.write(clipboard::Kind::Standard, s);
                            }
                        }
                        (Some('x'), _) if cmd => {
                            if let Some(s) = self.content.selected_text() {
                                clipboard.write(clipboard::Kind::Standard, s);
                                actions.push(Action::Delete);
                            }
                        }
                        (Some('v'), false) if cmd => {
                            if let Some(s) = clipboard.read(clipboard::Kind::Standard) {
                                actions.push(Action::Paste(s));
                            }
                        }
                        (Some('a'), _) if cmd => actions.push(Action::SelectAll),
                        (Some('b'), false) => actions.push(Action::Format(Command::Bold)),
                        (Some('i'), false) => actions.push(Action::Format(Command::Italic)),
                        (Some('u'), false) => actions.push(Action::Format(Command::Underline)),
                        (Some('k'), false) => actions.push(Action::Format(Command::Link)),
                        (Some('8'), true) => actions.push(Action::Format(Command::Bullets)),
                        (Some('7'), true) => actions.push(Action::Format(Command::Numbers)),
                        _ => {}
                    }
                }
                let select = m.shift();
                let moving = |motion| Action::Move { motion, select };
                let jump = m.jump();
                let line = m.macos_command();
                match modified_key.as_ref() {
                    keyboard::Key::Named(Named::Enter) => actions.push(Action::Enter),
                    keyboard::Key::Named(named @ (Named::Backspace | Named::Delete)) => {
                        let back = named == Named::Backspace;
                        if self.content.selection().is_none() && (jump || line) {
                            let motion = match (back, line) {
                                (true, true) => Motion::BlockStart,
                                (true, false) => Motion::WordLeft,
                                (false, true) => Motion::BlockEnd,
                                (false, false) => Motion::WordRight,
                            };
                            actions.push(Action::Move {
                                motion,
                                select: true,
                            });
                        }
                        actions.push(if back {
                            Action::Backspace
                        } else {
                            Action::Delete
                        });
                    }
                    keyboard::Key::Named(Named::Tab) if !shortcut => {
                        actions.push(Action::Format(if m.shift() {
                            Command::Outdent
                        } else {
                            Command::Indent
                        }));
                    }
                    keyboard::Key::Named(Named::Escape) => {
                        state.focus = None;
                        state.dragging = false;
                        state.menu = None;
                        shell.request_redraw();
                        // Not captured: Escape also does what it does
                        // around the editor.
                        return;
                    }
                    keyboard::Key::Named(Named::ArrowLeft) => actions.push(moving(if line {
                        Motion::BlockStart
                    } else if jump {
                        Motion::WordLeft
                    } else {
                        Motion::Left
                    })),
                    keyboard::Key::Named(Named::ArrowRight) => actions.push(moving(if line {
                        Motion::BlockEnd
                    } else if jump {
                        Motion::WordRight
                    } else {
                        Motion::Right
                    })),
                    keyboard::Key::Named(named @ (Named::ArrowUp | Named::ArrowDown)) => {
                        let down = named == Named::ArrowDown;
                        actions.push(if line {
                            moving(if down {
                                Motion::DocEnd
                            } else {
                                Motion::DocStart
                            })
                        } else {
                            let at = state.vertical(self.content.cursor(), down, self.size);
                            Action::MoveTo { at, select }
                        });
                    }
                    keyboard::Key::Named(Named::Home) => actions.push(moving(Motion::BlockStart)),
                    keyboard::Key::Named(Named::End) => actions.push(moving(Motion::BlockEnd)),
                    _ if !shortcut => {
                        let typed: String = text
                            .as_deref()
                            .unwrap_or_default()
                            .chars()
                            .filter(|c| !c.is_control())
                            .collect();
                        if !typed.is_empty() {
                            actions.push(Action::Insert(typed));
                        }
                    }
                    _ => {}
                }
                if let Some(f) = &mut state.focus {
                    f.updated_at = Instant::now();
                }
                state.menu = None;
                for action in actions {
                    publish(shell, action);
                }
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;

        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let style = theme::editor(theme, status(state, cursor, bounds));
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: style.border,
                ..renderer::Quad::default()
            },
            style.background,
        );
        let text_bounds = bounds.shrink(self.padding);
        let at = |p: Point| Point::new(text_bounds.x + p.x, text_bounds.y + p.y);

        if self.content.is_empty() {
            renderer.fill_paragraph(
                &state.placeholder,
                at(Point::ORIGIN),
                style.placeholder,
                text_bounds,
            );
        }

        // The selection, behind the text.
        if let (Some(_), Some((from, to))) = (&state.focus, self.content.selection()) {
            for b in from.block..=to.block {
                let Some(laid) = state.blocks.get(b) else {
                    continue;
                };
                let start = if b == from.block {
                    byte_of(&laid.plain, from.col)
                } else {
                    0
                };
                let end = if b == to.block {
                    byte_of(&laid.plain, to.col)
                } else {
                    laid.plain.len()
                };
                for run in laid.paragraph.buffer().layout_runs() {
                    let Some((x, w)) = run.highlight(
                        cosmic_text::Cursor::new(0, start),
                        cosmic_text::Cursor::new(0, end),
                    ) else {
                        continue;
                    };
                    // A selected line break shows as a sliver.
                    let w = if b < to.block
                        && run.glyphs.last().is_none_or(|g| g.end >= laid.plain.len())
                    {
                        w + 4.0
                    } else {
                        w
                    };
                    let rect = Rectangle::new(
                        at(Point::new(laid.origin.x + x, laid.origin.y + run.line_top)),
                        Size::new(w, run.line_height),
                    );
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: rect,
                            ..renderer::Quad::default()
                        },
                        style.selection,
                    );
                }
            }
        }

        let muted = theme::muted_color(theme);
        for laid in &state.blocks {
            if let Some(marker) = &laid.marker {
                let p = at(Point::new(laid.origin.x - laid.gutter, laid.origin.y));
                renderer.fill_paragraph(marker, p, muted, text_bounds);
            }
            let origin = at(laid.origin);
            renderer.fill_paragraph(&laid.paragraph, origin, style.value, text_bounds);
            // Underlines (and link underlines) under their glyphs.
            for run in laid.paragraph.buffer().layout_runs() {
                let mut line: Option<(f32, f32, bool)> = None;
                let flush = |line: &mut Option<(f32, f32, bool)>, renderer: &mut iced::Renderer| {
                    if let Some((x0, x1, link)) = line.take() {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: Rectangle::new(
                                    Point::new(origin.x + x0, origin.y + run.line_y + 2.0),
                                    Size::new(x1 - x0, 1.0),
                                ),
                                ..renderer::Quad::default()
                            },
                            if link {
                                theme::accent_color()
                            } else {
                                style.value
                            },
                        );
                    }
                };
                for g in run.glyphs {
                    let (under, link) = laid
                        .spans
                        .get(g.metadata)
                        .map_or((false, false), |(u, l)| (*u, l.is_some()));
                    match &mut line {
                        Some((_, x1, l)) if under && *l == link => *x1 = g.x + g.w,
                        _ => {
                            flush(&mut line, renderer);
                            if under {
                                line = Some((g.x, g.x + g.w, link));
                            }
                        }
                    }
                }
                flush(&mut line, renderer);
            }
        }

        // The caret.
        if let Some(focus) = &state.focus
            && focus.caret_visible()
            && self.content.selection().is_none()
        {
            let (p, h) = state.caret(self.content.cursor(), self.size);
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(at(p), Size::new(1.0, h)),
                    ..renderer::Quad::default()
                },
                style.value,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let Some(p) = cursor.position_over(bounds) else {
            return mouse::Interaction::default();
        };
        let text_bounds = bounds.shrink(self.padding);
        let p = Point::new(p.x - text_bounds.x, p.y - text_bounds.y);
        if self.on_link.is_some() && state.link_at(p).is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::Text
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        _renderer: &iced::Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        let state = tree.state.downcast_mut::<State>();
        state.menu.as_ref()?;
        let on_link = self.on_link.as_deref()?;
        let text = layout.bounds().shrink(self.padding).position() + translation;
        Some(overlay::Element::new(Box::new(LinkMenu {
            menu: &mut state.menu,
            text,
            on_link,
            on_action: &*self.on_action,
        })))
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(self.id.as_ref(), layout.bounds(), state);
    }
}

impl<'a, Message: 'a> From<RichEditor<'a, Message>> for Element<'a, Message> {
    fn from(editor: RichEditor<'a, Message>) -> Self {
        Element::new(editor)
    }
}

// --------------------------------------------------------------- link menu

/// The popup over a clicked link (the editor's [`Menu`]), as an overlay.
struct LinkMenu<'b, Message> {
    menu: &'b mut Option<Menu>,
    /// The text area's top left, in the window.
    text: Point,
    on_link: &'b dyn Fn(String) -> Message,
    on_action: &'b dyn Fn(Action) -> Message,
}

impl<Message> overlay::Overlay<Message, Theme, iced::Renderer> for LinkMenu<'_, Message> {
    fn layout(&mut self, _renderer: &iced::Renderer, window: Size) -> layout::Node {
        let Some(menu) = self.menu.as_ref() else {
            return layout::Node::new(Size::ZERO);
        };
        let place = menu.place(self.text, window);
        layout::Node::new(place.size()).move_to(place.position())
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;

        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let bounds = layout.bounds();
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    radius: 6.0.into(),
                    width: 1.0,
                    color: theme::tint(theme, 0.16),
                },
                shadow: iced::Shadow {
                    color: iced::Color::BLACK.scale_alpha(0.25),
                    offset: Vector::new(0.0, 2.0),
                    blur_radius: 8.0,
                },
                ..renderer::Quad::default()
            },
            theme.palette().background,
        );
        let (open, edit) = menu.buttons(bounds);
        for (b, which) in [(open, true), (edit, false)] {
            if menu.hovered == Some(which) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: b.shrink(3.0),
                        border: Border {
                            radius: 4.0.into(),
                            ..Border::default()
                        },
                        ..renderer::Quad::default()
                    },
                    theme::tint(theme, 0.08),
                );
            }
        }
        let y = bounds.y + MENU_PAD;
        renderer.fill_paragraph(
            &menu.shown_url,
            Point::new(bounds.x + MENU_PAD, y),
            theme::muted_color(theme),
            bounds,
        );
        renderer.fill_paragraph(
            &menu.open,
            Point::new(open.x + MENU_GAP / 2.0, y),
            theme::accent_color(),
            bounds,
        );
        renderer.fill_paragraph(
            &menu.edit,
            Point::new(edit.x + MENU_GAP / 2.0, y),
            theme.palette().text,
            bounds,
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();
        if let Event::Mouse(mouse::Event::CursorMoved { .. }) = event
            && let Some(menu) = self.menu.as_mut()
        {
            let (open, edit) = menu.buttons(bounds);
            let hovered = if cursor.is_over(open) {
                Some(true)
            } else if cursor.is_over(edit) {
                Some(false)
            } else {
                None
            };
            if hovered != menu.hovered {
                menu.hovered = hovered;
                shell.request_redraw();
            }
            return;
        }
        let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else {
            return;
        };
        if !cursor.is_over(bounds) {
            // The editor closes the popup on this click.
            return;
        }
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let (open, edit) = menu.buttons(bounds);
        if cursor.is_over(open) {
            shell.publish((self.on_link)(href(&menu.url)));
            *self.menu = None;
        } else if cursor.is_over(edit) {
            shell.publish((self.on_action)(Action::MoveTo {
                at: menu.at,
                select: false,
            }));
            shell.publish((self.on_action)(Action::Format(Command::Link)));
            *self.menu = None;
        }
        shell.capture_event();
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(menu) = self.menu.as_ref() else {
            return mouse::Interaction::None;
        };
        let (open, edit) = menu.buttons(layout.bounds());
        if cursor.is_over(open) || cursor.is_over(edit) {
            mouse::Interaction::Pointer
        } else if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Idle
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: f32 = 14.0;

    fn laid_out(md: &str, width: f32) -> (Content, State) {
        let content = Content::with_text(md);
        let mut state = State::default();
        state.lay_out(&content, width, SIZE, "");
        (content, state)
    }

    /// Clicking just right of where the caret is drawn puts the caret
    /// back there, at every place: wrapped lines, list items, styles.
    #[test]
    fn caret_and_clicks_agree() {
        let (_, state) = laid_out(
            "• a first line long enough to wrap around more than once here\n\
             second **bold** *line* and [a link](https://x.y)\n\n  1. nested",
            160.0,
        );
        for (b, laid) in state.blocks.iter().enumerate() {
            for col in 0..=laid.plain.chars().count() {
                let at = Pos::new(b, col);
                let (p, h) = state.caret(at, SIZE);
                assert_eq!(state.hit(Point::new(p.x + 0.5, p.y + h / 2.0)), at);
            }
        }
    }

    #[test]
    fn blocks_stack_with_gutters_and_indents() {
        let (_, state) = laid_out("• a\nplain\n  - b", 300.0);
        let [bullet, plain, nested] = &state.blocks[..] else {
            panic!("three blocks");
        };
        assert_eq!(bullet.origin, Point::new(gutter(List::Bullet), 0.0));
        assert!(bullet.marker.is_some() && plain.marker.is_none());
        assert_eq!(plain.origin, Point::new(0.0, bullet.height));
        assert_eq!(nested.origin.x, INDENT + gutter(List::Dash));
        assert_eq!(
            state.total_height(),
            bullet.height + plain.height + nested.height
        );
    }

    /// Laying out again after an edit keeps the unchanged lines and puts
    /// every line in its place.
    #[test]
    fn layout_follows_edits() {
        let mut content = Content::with_text("a\nb\nc");
        let mut state = State::default();
        state.lay_out(&content, 300.0, SIZE, "");
        let check = |state: &State, content: &Content| {
            let plain: Vec<String> = state.blocks.iter().map(|b| b.plain.clone()).collect();
            let want: Vec<String> = content.blocks().iter().map(|b| b.plain()).collect();
            assert_eq!(plain, want);
            let mut y = 0.0;
            for b in &state.blocks {
                assert_eq!(b.origin.y, y);
                y += b.height;
            }
        };
        content.perform(Action::MoveTo {
            at: Pos::new(0, 1),
            select: false,
        });
        for action in [
            Action::Enter,
            Action::Insert("new".into()),
            Action::Insert("\nmore".into()),
            Action::Backspace,
            Action::SelectAll,
            Action::Delete,
            Action::Paste("x\n• y\nz".into()),
        ] {
            content.perform(action);
            state.lay_out(&content, 300.0, SIZE, "");
            check(&state, &content);
        }
        // Another width lays every line out again.
        state.lay_out(&content, 120.0, SIZE, "");
        assert!(state.blocks.iter().all(|b| b.source.2 == 120.0));
    }

    #[test]
    fn up_and_down_move_by_lines() {
        let (_, state) = laid_out("one two three four five six seven eight\nnext", 90.0);
        let lines = state.blocks[0].paragraph.buffer().layout_runs().count();
        assert!(lines > 1, "the first block wraps");
        let mut at = Pos::new(0, 0);
        for _ in 1..lines {
            let down = state.vertical(at, true, SIZE);
            assert_eq!(down.block, 0);
            assert!(down.col > at.col);
            at = down;
        }
        assert_eq!(state.vertical(at, true, SIZE).block, 1);
        assert_eq!(state.vertical(Pos::new(1, 2), false, SIZE).block, 0);
        // Past the ends: the start and the end.
        assert_eq!(state.vertical(Pos::new(0, 1), false, SIZE), Pos::new(0, 0));
        assert_eq!(state.vertical(Pos::new(1, 0), true, SIZE), Pos::new(1, 4));
    }

    /// The link popup sits above the clicked line, inside the window, or
    /// under the line at the top of the window; Open and Edit sit side by
    /// side inside it.
    #[test]
    fn the_link_popup_stays_in_the_window() {
        let menu = |x: f32, y: f32| {
            Menu::new(
                Arc::from("https://example.com/a/long/address"),
                Pos::default(),
                Point::new(x, y),
                18.0,
            )
        };
        let window = Size::new(800.0, 600.0);
        let m = menu(100.0, 200.0);
        let at = m.place(Point::new(20.0, 30.0), window);
        assert!(at.y + at.height <= 230.0 && at.x >= 100.0);
        let (o, e) = m.buttons(at);
        assert!(o.x + o.width <= e.x + 0.01 && e.x + e.width <= at.x + at.width + 0.01);
        let at = menu(790.0, 0.0).place(Point::new(0.0, 4.0), window);
        assert!(at.x + at.width <= 800.0 && at.y >= 4.0 + 18.0);
    }

    #[test]
    fn links_are_found_under_the_pointer() {
        let (_, state) = laid_out("see [docs](https://x.y) now", 400.0);
        let (start, h) = state.caret(Pos::new(0, 4), SIZE);
        let (end, _) = state.caret(Pos::new(0, 8), SIZE);
        let mid = Point::new((start.x + end.x) / 2.0, start.y + h / 2.0);
        assert_eq!(state.link_at(mid).as_deref(), Some("https://x.y"));
        assert_eq!(state.link_at(Point::new(1.0, h / 2.0)), None);
    }
}
