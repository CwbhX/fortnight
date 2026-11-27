//! Motion: the app's few short animations.
//!
//! Each is a widget that animates itself. It keeps its timing in its own
//! widget state, moves along on every frame iced draws
//! (`RedrawRequested`, which iced sends before each draw), and asks for
//! the next frame only while it is still moving, so an idle window draws
//! nothing. `main.rs` holds no animation state and no timer.
//!
//! - [`appear`]: content that eases into place when it first shows (a
//!   dialog's panel rising), or each time its [`Appear::page`] changes (the
//!   open tab fading in, sliding from the side the period moved towards).
//! - [`dim`]: the dark layer behind a dialog, fading to whatever level it
//!   is given, in and out.
//! - [`pill`]: the selected-tab pill, sliding to the selected tab.
//! - [`unfold`] and [`fold`]: something that opens (a folded section, a
//!   task opening into its form) growing to its open height, and shrinking
//!   back when it closes.
//! - [`list`]: a column of keyed items (tasks) that grow, fade and zoom in
//!   when they arrive and fade, zoom out and fold away when they leave.
//!
//! Whatever plays on the way in plays back on the way out where the widget
//! can still draw what is leaving: the dark layer fades out, folds shrink
//! (a [`fold`] keeps its body for that, hidden while closed), and a list
//! draws an item the view still gives it, marked [`Item::leaving`] (the
//! Today tab keeps a task that just left a section for a moment for that:
//! data to draw, not timing). A dialog's panel can't sink back out: once
//! the view drops it there is nothing left to draw, so it goes at once
//! while its dark layer fades.
//!
//! iced 0.14 can't draw a widget tree at part opacity, so a fade here is a
//! "veil": a rectangle in the colour behind the content, drawn over it and
//! thinning out. That is exact on a plain background (the page) and not
//! possible over a mixed one (a dialog over the dimmed page), which is why
//! panels rise without fading.
//!
//! Drawing order matters in iced: within a layer all rectangles are drawn
//! before any text, so anything that must cover text (a veil, the pill) is
//! drawn in a layer of its own.
//!
//! [`set_enabled`] turns all of it off (every animation jumps to its end):
//! `main.rs` does so for the self screenshot and when the system asks for
//! reduced motion ([`system_reduces_motion`]). [`hold_at`] is a dev aid for
//! screenshots of an animation mid-way.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::time::{Duration, Instant};
use iced::{Color, Element, Event, Length, Rectangle, Shadow, Size, Vector, mouse, window};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

mod list;
pub use list::{FADED, Item, List, list};

/// How long a page (the open tab) takes to fade in.
const PAGE: Duration = Duration::from_millis(170);
/// How long a panel takes to rise into place.
const PANEL: Duration = Duration::from_millis(200);
/// How long the dark layer takes to reach a new level.
const DIM: Duration = Duration::from_millis(180);
/// How long the tab pill takes to reach another tab.
const PILL: Duration = Duration::from_millis(220);
/// How long something takes to open to its full height, or close.
const UNFOLD: Duration = Duration::from_millis(220);
/// How long an inline task card takes to expand into its form, and to close
/// back down (closing is slightly brisker so it feels snappy).
const EXPAND_OPEN: Duration = Duration::from_millis(240);
const EXPAND_CLOSE: Duration = Duration::from_millis(200);
/// How long a full-width horizontal page slide (Week view) takes.
const PAGE_SLIDE: Duration = Duration::from_millis(280);
/// A page changed again this soon after the last change (a held arrow key,
/// quick clicks) is shown at once.
const QUIET: Duration = Duration::from_millis(300);
/// How far a page slides in from the side, in logical pixels.
const SLIDE: f32 = 14.0;
/// How far a panel or a newly opened tab rises, in logical pixels.
pub const RISE: f32 = 10.0;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Turns the animations on or off. Off, everything is drawn in its final
/// place at once.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Not held: no `f32` in 0–1 has these bits.
const FREE: u32 = u32::MAX;
static HELD: AtomicU32 = AtomicU32::new(FREE);

/// Dev aid: holds every animation `progress` (0–1) of the way along, for a
/// screenshot of one mid-way.
pub fn hold_at(progress: f32) {
    HELD.store(progress.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

fn held() -> Option<f32> {
    let bits = HELD.load(Ordering::Relaxed);
    (bits != FREE).then(|| f32::from_bits(bits))
}

/// Whether the system asks apps to reduce motion (macOS: Accessibility,
/// Display, Reduce motion). `false` where there is no such setting to read.
pub fn system_reduces_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        // SAFETY: `+[NSWorkspace sharedWorkspace]` returns the shared
        // instance, which lives as long as the app, and
        // `accessibilityDisplayShouldReduceMotion` takes no arguments and
        // returns a BOOL. A nil workspace is not messaged.
        unsafe {
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            if workspace.is_null() {
                return false;
            }
            let reduce: bool = msg_send![workspace, accessibilityDisplayShouldReduceMotion];
            reduce
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Fast at first, settling gently: `t` (0–1) eased.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Immediate start with a longer, silky settle: `t` (0–1) eased.
fn ease_out_quart(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(4)
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// One run of an animation, from 0 to 1.
#[derive(Debug, Clone, Copy, Default)]
struct Run {
    /// When it began: the first frame after [`Run::restart`].
    start: Option<Instant>,
    /// The latest frame.
    now: Option<Instant>,
    /// Jump to the end.
    skipped: bool,
    /// How long it holds still at the start before it moves.
    delay: Duration,
}

impl Run {
    /// A run already at its end.
    fn finished() -> Self {
        Run {
            skipped: true,
            ..Run::default()
        }
    }

    /// Begin again at the next frame.
    fn restart(&mut self) {
        self.restart_after(Duration::ZERO);
    }

    /// Begin again at the next frame, holding still for `delay` first.
    fn restart_after(&mut self, delay: Duration) {
        self.start = None;
        self.skipped = false;
        self.delay = delay;
    }

    /// Still holding before it moves (begun or about to begin).
    fn waiting(&self) -> bool {
        if self.skipped || !enabled() || held().is_some() {
            return false;
        }
        match (self.start, self.now) {
            (Some(start), Some(now)) => now.saturating_duration_since(start) < self.delay,
            _ => !self.delay.is_zero(),
        }
    }

    fn skip(&mut self) {
        self.skipped = true;
    }

    /// Takes a frame's time; `true` while the run has further to go.
    fn frame(&mut self, now: Instant, duration: Duration) -> bool {
        self.start.get_or_insert(now);
        self.now = Some(now);
        self.waiting() || self.progress(duration) < 1.0
    }

    /// How far along it is at the latest frame, 0–1 (not eased).
    fn progress(&self, duration: Duration) -> f32 {
        if self.skipped || !enabled() || duration.is_zero() {
            return 1.0;
        }
        if let Some(held) = held() {
            return held;
        }
        match (self.start, self.now) {
            (Some(start), Some(now)) => {
                let moving = now
                    .saturating_duration_since(start)
                    .saturating_sub(self.delay);
                (moving.as_secs_f32() / duration.as_secs_f32()).min(1.0)
            }
            _ => 0.0,
        }
    }
}

/// `cursor` as seen by content showing only within `visible`: elsewhere
/// it is not over the content at all. Content partly shown (part open,
/// coming in) is laid out at full size over whatever follows; this keeps a
/// click or hover there for what is drawn there.
fn shown_cursor(cursor: mouse::Cursor, visible: Rectangle) -> mouse::Cursor {
    if cursor.is_over(visible) {
        cursor
    } else {
        mouse::Cursor::Unavailable
    }
}

/// The time of the frame about to be drawn, if `event` is that.
fn frame_time(event: &Event) -> Option<Instant> {
    match event {
        Event::Window(window::Event::RedrawRequested(now)) => Some(*now),
        _ => None,
    }
}

// ----------------------------------------------------------------- appear

/// `content` easing into place: see [`Appear`].
pub fn appear<'a, M, T, R>(content: impl Into<Element<'a, M, T, R>>) -> Appear<'a, M, T, R> {
    Appear {
        content: content.into(),
        rise: 0.0,
        veil: None,
        page: None,
    }
}

/// Content that eases into place. Otherwise invisible: it lays out, draws
/// and behaves as its content does. (While it moves, only the drawing is
/// offset; clicks land where the content will end up.)
///
/// Without [`page`](Self::page) it plays once, when it first shows. With
/// it, it stays still at first and plays each time the page changes.
pub struct Appear<'a, M, T, R> {
    content: Element<'a, M, T, R>,
    rise: f32,
    veil: Option<Behind<'a, T>>,
    page: Option<(u32, i64)>,
}

impl<'a, M, T, R> Appear<'a, M, T, R> {
    /// Start `distance` pixels low and rise into place.
    pub fn rise(mut self, distance: f32) -> Self {
        self.rise = distance;
        self
    }

    /// Fade in, over a plain background of the colour `behind` returns.
    pub fn fade_from(mut self, behind: impl Fn(&T) -> Color + 'a) -> Self {
        self.veil = Some(Box::new(behind));
        self
    }

    /// Which page the content shows: play again whenever this changes. A
    /// new `group` (another tab) rises; a new `position` in the same group
    /// (another day, week or month) slides in from the side it lies on,
    /// later ones from the right.
    pub fn page(mut self, group: u32, position: i64) -> Self {
        self.page = Some((group, position));
        self
    }

    fn duration(&self) -> Duration {
        if self.page.is_some() { PAGE } else { PANEL }
    }
}

/// The colour behind some content, from the theme.
type Behind<'a, T> = Box<dyn Fn(&T) -> Color + 'a>;

struct AppearState {
    run: Run,
    page: Option<(u32, i64)>,
    /// Where this run starts, relative to the final place.
    from: Vector,
    /// The page changed since the last frame.
    turned: bool,
    /// The frame at which the page last changed.
    last_turn: Option<Instant>,
}

impl AppearState {
    /// The offset to draw at and the veil's opacity; `None` once in place.
    fn pose(&self, duration: Duration) -> Option<(Vector, f32)> {
        let left = 1.0 - ease_out(self.run.progress(duration));
        // Half pixels: whole ones on a 2x display, so text stays sharp.
        let snap = |v: f32| (v * left * 2.0).round() / 2.0;
        (left > 0.0).then(|| (Vector::new(snap(self.from.x), snap(self.from.y)), left))
    }
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Appear<'_, M, T, R> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<AppearState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(AppearState {
            // A page is already there when the app opens.
            run: if self.page.is_some() {
                Run::finished()
            } else {
                Run::default()
            },
            page: self.page,
            from: Vector::new(0.0, self.rise),
            turned: false,
            last_turn: None,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<AppearState>();
        if let (Some(new), Some(old)) = (self.page, state.page)
            && new != old
        {
            state.from = if new.0 != old.0 {
                Vector::new(0.0, self.rise)
            } else if new.1 > old.1 {
                Vector::new(SLIDE, 0.0)
            } else {
                Vector::new(-SLIDE, 0.0)
            };
            state.run.restart();
            state.turned = true;
        }
        state.page = self.page;
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        if let Some(now) = frame_time(event) {
            let duration = self.duration();
            let state = tree.state.downcast_mut::<AppearState>();
            if std::mem::take(&mut state.turned) {
                // Flicking through pages: no fade on each one.
                if state
                    .last_turn
                    .is_some_and(|last| now.saturating_duration_since(last) < QUIET)
                {
                    state.run.skip();
                }
                state.last_turn = Some(now);
            }
            if state.run.frame(now, duration) {
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let content = self.content.as_widget();
        let child = &tree.children[0];
        let state = tree.state.downcast_ref::<AppearState>();
        let Some((offset, veil)) = state.pose(self.duration()) else {
            content.draw(child, renderer, theme, style, layout, cursor, viewport);
            return;
        };
        renderer.with_translation(offset, |renderer| {
            content.draw(child, renderer, theme, style, layout, cursor, viewport);
        });
        if let Some(behind) = &self.veil
            && let Some(bounds) = layout.bounds().intersection(viewport)
        {
            // Its own layer, to cover the content's text too.
            renderer.with_layer(bounds, |renderer| {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds,
                        ..renderer::Quad::default()
                    },
                    behind(theme).scale_alpha(veil),
                );
            });
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &R,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a> From<Appear<'a, M, T, R>>
    for Element<'a, M, T, R>
{
    fn from(appear: Appear<'a, M, T, R>) -> Self {
        Element::new(appear)
    }
}

// -------------------------------------------------------------------- dim

/// A layer that fills its space with black at `level` opacity (0–1),
/// fading from the level it had. It takes no clicks. Put it under a
/// dialog; left in place at level 0 once the dialog has gone, it fades out.
pub fn dim<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(level: f32) -> Element<'a, M, T, R> {
    Element::new(Dim { level })
}

struct Dim {
    level: f32,
}

struct DimState {
    run: Run,
    from: f32,
    to: f32,
    /// The level at the latest frame.
    shown: f32,
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Dim {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<DimState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(DimState {
            run: if self.level == 0.0 {
                Run::finished()
            } else {
                Run::default()
            },
            from: 0.0,
            to: self.level,
            shown: 0.0,
        })
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<DimState>();
        if state.to != self.level {
            state.from = state.shown;
            state.to = self.level;
            state.run.restart();
        }
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, _: &mut Tree, _: &R, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _: Layout<'_>,
        _: mouse::Cursor,
        _: &R,
        _: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _: &Rectangle,
    ) {
        if let Some(now) = frame_time(event) {
            let state = tree.state.downcast_mut::<DimState>();
            let moving = state.run.frame(now, DIM);
            state.shown = lerp(state.from, state.to, ease_out(state.run.progress(DIM)));
            if moving {
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        _: &T,
        _: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
        let shown = tree.state.downcast_ref::<DimState>().shown;
        if shown > 0.0 {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds(),
                    ..renderer::Quad::default()
                },
                Color::BLACK.scale_alpha(shown),
            );
        }
    }
}

// ------------------------------------------------------------------- pill

/// How a [`pill`] looks.
#[derive(Debug, Clone, Copy)]
pub struct PillStyle {
    pub color: Color,
    pub radius: f32,
    pub shadow: Shadow,
}

/// A row of tabs with a pill behind the selected one that slides to
/// another when the selection changes.
///
/// `tabs` is the row itself (it must be a `row`: the pill takes the place
/// of its child number `selected`). `lit` is the same row as it reads on
/// the pill (the same labels, sizes and spacing, in the pill's text
/// colour); only the part under the pill is drawn, so a label changes
/// colour exactly as the pill passes over it. `lit` is never clicked.
pub fn pill<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    tabs: impl Into<Element<'a, M, T, R>>,
    lit: impl Into<Element<'a, M, T, R>>,
    selected: usize,
    style: PillStyle,
) -> Element<'a, M, T, R> {
    Element::new(Pill {
        layers: [tabs.into(), lit.into()],
        selected,
        style,
    })
}

struct Pill<'a, M, T, R> {
    /// The tabs, then the same row as it reads on the pill.
    layers: [Element<'a, M, T, R>; 2],
    selected: usize,
    style: PillStyle,
}

/// Rectangles are relative to the widget's corner, so the pill doesn't
/// trail behind when the whole bar moves.
struct PillState {
    run: Run,
    selected: usize,
    /// Where the pill was when the selection last changed.
    from: Option<Rectangle>,
    /// Where it is at the latest frame.
    shown: Option<Rectangle>,
}

impl<M, T, R> Pill<'_, M, T, R> {
    /// The selected tab's place, relative to the widget's corner.
    fn target(&self, layout: Layout<'_>) -> Option<Rectangle> {
        let tab = layout.children().next()?.children().nth(self.selected)?;
        let origin = layout.position();
        Some(tab.bounds() + Vector::new(-origin.x, -origin.y))
    }
}

fn lerp_rect(from: Rectangle, to: Rectangle, t: f32) -> Rectangle {
    Rectangle {
        x: lerp(from.x, to.x, t),
        y: lerp(from.y, to.y, t),
        width: lerp(from.width, to.width, t),
        height: lerp(from.height, to.height, t),
    }
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Pill<'_, M, T, R> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<PillState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(PillState {
            run: Run::finished(),
            selected: self.selected,
            from: None,
            shown: None,
        })
    }

    fn children(&self) -> Vec<Tree> {
        self.layers.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<PillState>();
        if state.selected != self.selected {
            state.selected = self.selected;
            state.from = state.shown;
            state.run.restart();
        }
        tree.diff_children(&self.layers);
    }

    fn size(&self) -> Size<Length> {
        self.layers[0].as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.layers[0].as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        let nodes: Vec<layout::Node> = self
            .layers
            .iter_mut()
            .zip(&mut tree.children)
            .map(|(layer, tree)| layer.as_widget_mut().layout(tree, renderer, limits))
            .collect();
        layout::Node::with_children(nodes[0].size(), nodes)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Some(tabs) = layout.children().next() {
            self.layers[0].as_widget_mut().update(
                &mut tree.children[0],
                event,
                tabs,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
        if let Some(now) = frame_time(event) {
            let target = self.target(layout);
            let state = tree.state.downcast_mut::<PillState>();
            let moving = state.run.frame(now, PILL);
            let t = ease_out(state.run.progress(PILL));
            state.shown = target.map(|to| match state.from {
                Some(from) => lerp_rect(from, to, t),
                None => to,
            });
            if moving {
                shell.request_redraw();
            } else {
                state.from = None;
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let mut layouts = layout.children();
        let (Some(tabs), Some(lit)) = (layouts.next(), layouts.next()) else {
            return;
        };
        self.layers[0].as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            tabs,
            cursor,
            viewport,
        );
        let state = tree.state.downcast_ref::<PillState>();
        let Some(place) = state.shown.or_else(|| self.target(layout)) else {
            return;
        };
        let origin = layout.position();
        let bounds = place + Vector::new(origin.x, origin.y);
        // Room below for the shadow, none at the sides: the labels beside
        // the pill stay in the tabs' own colour.
        let clip = Rectangle {
            height: bounds.height + 4.0,
            ..bounds
        };
        let Some(clip) = clip.intersection(viewport) else {
            return;
        };
        // A layer of its own: the pill covers the tabs' labels, and the lit
        // labels are cut to it.
        renderer.with_layer(clip, |renderer| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: iced::border::rounded(self.style.radius),
                    shadow: self.style.shadow,
                    ..renderer::Quad::default()
                },
                self.style.color,
            );
            self.layers[1].as_widget().draw(
                &tree.children[1],
                renderer,
                theme,
                style,
                lit,
                mouse::Cursor::Unavailable,
                viewport,
            );
        });
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &R,
        operation: &mut dyn Operation,
    ) {
        if let Some(tabs) = layout.children().next() {
            self.layers[0].as_widget_mut().operate(
                &mut tree.children[0],
                tabs,
                renderer,
                operation,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        match layout.children().next() {
            Some(tabs) => self.layers[0].as_widget().mouse_interaction(
                &tree.children[0],
                tabs,
                cursor,
                viewport,
                renderer,
            ),
            None => mouse::Interaction::None,
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        let tabs = layout.children().next()?;
        self.layers[0].as_widget_mut().overlay(
            &mut tree.children[0],
            tabs,
            renderer,
            viewport,
            translation,
        )
    }
}

// ----------------------------------------------------------------- unfold

/// `content` changing height smoothly when `open` changes: opening, it
/// grows from the height it had closed, showing more of the open content
/// from the top down; closing, it shrinks back to the closed content's
/// height, the closed content drawn at the top as the space under it
/// closes up. Give it the closed content while closed and the open content
/// while open; see [`fold`] for a body that is hidden while closed.
pub fn unfold<'a, M, T, R>(
    open: bool,
    content: impl Into<Element<'a, M, T, R>>,
) -> Unfold<'a, M, T, R> {
    Unfold {
        content: content.into(),
        open,
        key: 0,
        hide: false,
        close_after: Duration::ZERO,
    }
}

/// A folded section's body: nothing while closed, unfolding when `open`
/// turns true and folding away again when it turns false. Give it the body
/// either way: it is laid out but not drawn while closed, and sent only
/// the frames (so its own animations run on, unseen), and drawn shrinking
/// away as it closes.
pub fn fold<'a, M, T, R>(open: bool, body: impl Into<Element<'a, M, T, R>>) -> Unfold<'a, M, T, R> {
    Unfold {
        hide: true,
        ..unfold(open, body)
    }
}

/// See [`unfold`].
pub struct Unfold<'a, M, T, R> {
    content: Element<'a, M, T, R>,
    open: bool,
    key: u64,
    /// Closed, it takes no room and its content is hidden ([`fold`]).
    hide: bool,
    /// How long it stays as it is before it starts to close.
    close_after: Duration,
}

impl<M, T, R> Unfold<'_, M, T, R> {
    /// What this is (a task's id, a section): in a list whose items move,
    /// an item that lands where another one was doesn't open or close with
    /// a flourish it didn't earn.
    pub fn key(mut self, key: u64) -> Self {
        self.key = key;
        self
    }

    /// When it closes, it first stays as it is (at its content's height)
    /// for `delay`: a section whose last item is leaving folds away once
    /// the item has gone, rather than cutting it off.
    pub fn close_after(mut self, delay: Duration) -> Self {
        self.close_after = delay;
        self
    }

    /// Closed and folded away: its content takes no clicks or keys.
    fn hidden(&self) -> bool {
        self.hide && !self.open
    }
}

struct UnfoldState {
    run: Run,
    open: bool,
    key: u64,
    /// The height it had when it last opened or closed.
    from: f32,
    /// The height it was last laid out at.
    height: f32,
    /// The frame for which a new layout was last asked: once per frame
    /// (iced sends a frame again after laying out).
    asked: Option<Instant>,
    /// Laid out at its full height since the run ended.
    settled: bool,
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Unfold<'_, M, T, R> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<UnfoldState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(UnfoldState {
            // Something already open when it first shows just is open.
            run: Run::finished(),
            open: self.open,
            key: self.key,
            from: 0.0,
            height: 0.0,
            asked: None,
            settled: true,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<UnfoldState>();
        if state.key != self.key {
            state.key = self.key;
            state.run.skip();
        } else if self.open != state.open {
            state.from = state.height;
            if self.open {
                state.run.restart();
            } else {
                state.run.restart_after(self.close_after);
            }
            state.settled = false;
        }
        state.open = self.open;
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        let content = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        let full = content.size();
        let target = if self.hidden() { 0.0 } else { full.height };
        let state = tree.state.downcast_mut::<UnfoldState>();
        if state.run.waiting() {
            // Holding before it closes: as tall as what it shows now.
            state.from = full.height;
        }
        let t = ease_out(state.run.progress(UNFOLD));
        // Half-pixel steps (whole physical pixels on 2x displays) so the
        // decelerating tail settles without 2px jumps.
        state.height = if t < 1.0 {
            (lerp(state.from, target, t) * 2.0).round() * 0.5
        } else {
            target
        };
        layout::Node::with_children(Size::new(full.width, state.height), vec![content])
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        // Folded away, its content still gets the frames (and nothing
        // else), so what moves in it (a task arriving in a closed section)
        // gets there unseen instead of waiting to play when it opens.
        if (!self.hidden() || frame_time(event).is_some())
            && let Some(content) = layout.children().next()
        {
            let cursor = shown_cursor(cursor, layout.bounds());
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                content,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
        if let Some(now) = frame_time(event) {
            let state = tree.state.downcast_mut::<UnfoldState>();
            let moving = state.run.frame(now, UNFOLD);
            let progress = state.run.progress(UNFOLD);
            // The height changes once the run moves past 0, and once more
            // for the full height at its end. Skipping frame 0 avoids a
            // redundant full-window relayout on the click frame itself.
            let needs_layout = (moving && progress > 0.0) || (!moving && !state.settled);
            if needs_layout && state.asked != Some(now) {
                state.asked = Some(now);
                state.settled = !moving;
                shell.invalidate_layout();
            }
            if moving {
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let Some(content) = layout.children().next() else {
            return;
        };
        let widget = self.content.as_widget();
        let child = &tree.children[0];
        let bounds = layout.bounds();
        // Folding away, it is only drawn: nothing in it lights up.
        let cursor = if self.hidden() {
            mouse::Cursor::Unavailable
        } else {
            cursor
        };
        if content.bounds().height <= bounds.height {
            widget.draw(child, renderer, theme, style, content, cursor, viewport);
        } else if let Some(clip) = bounds.intersection(viewport) {
            // Part open: only what fits is drawn.
            renderer.with_layer(clip, |renderer| {
                widget.draw(child, renderer, theme, style, content, cursor, &clip);
            });
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &R,
        operation: &mut dyn Operation,
    ) {
        // Nothing folded away takes the focus.
        if !self.hidden()
            && let Some(content) = layout.children().next()
        {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                content,
                renderer,
                operation,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        match layout.children().next().filter(|_| !self.hidden()) {
            Some(content) => self.content.as_widget().mouse_interaction(
                &tree.children[0],
                content,
                shown_cursor(cursor, layout.bounds()),
                viewport,
                renderer,
            ),
            None => mouse::Interaction::None,
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        if self.hidden() {
            return None;
        }
        let content = layout.children().next()?;
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            content,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a> From<Unfold<'a, M, T, R>>
    for Element<'a, M, T, R>
{
    fn from(unfold: Unfold<'a, M, T, R>) -> Self {
        Element::new(unfold)
    }
}

// ----------------------------------------------------------------- expand

/// A card that morphs smoothly between its `closed` row and its `opened`
/// form: the rounded card surface resizes cleanly (keeping rounded bottom
/// corners on every frame), while the closed row and the open form
/// crossfade through the card fill. Pass `opened` while `open` is true and
/// briefly while closing so the form collapses smoothly instead of
/// disappearing into an empty gap.
pub fn expand<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    open: bool,
    closed: impl Into<Element<'a, M, T, R>>,
    opened: Option<Element<'a, M, T, R>>,
) -> Expand<'a, M, T, R> {
    let has_opened = opened.is_some();
    let opened = opened.unwrap_or_else(|| iced::widget::Space::new().into());
    Expand {
        children: [closed.into(), opened],
        open,
        has_opened,
        key: 0,
        radius: 0.0,
        surface: None,
    }
}

/// See [`expand`].
pub struct Expand<'a, M, T, R> {
    children: [Element<'a, M, T, R>; 2],
    open: bool,
    has_opened: bool,
    key: u64,
    radius: f32,
    surface: Option<Behind<'a, T>>,
}

impl<'a, M, T, R> Expand<'a, M, T, R> {
    pub fn key(mut self, key: u64) -> Self {
        self.key = key;
        self
    }

    /// Corner radius of the expanding card surface.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Card background fill drawn behind the open form and used as the
    /// crossfade veil during opening and closing.
    pub fn surface(mut self, fill: impl Fn(&T) -> Color + 'a) -> Self {
        self.surface = Some(Box::new(fill));
        self
    }

    fn duration(&self) -> Duration {
        if self.open { EXPAND_OPEN } else { EXPAND_CLOSE }
    }
}

struct ExpandState {
    run: Run,
    open: bool,
    key: u64,
    from_height: f32,
    from_open: f32,
    height: f32,
    shown_open: f32,
    measured: bool,
    asked: Option<Instant>,
    settled: bool,
}

impl ExpandState {
    fn resting_open(&self) -> bool {
        self.open && self.shown_open >= 1.0
    }

    fn resting_closed(&self) -> bool {
        !self.open && self.shown_open <= 0.0
    }
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Expand<'_, M, T, R> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ExpandState>()
    }

    fn state(&self) -> tree::State {
        let shown_open = if self.open { 1.0 } else { 0.0 };
        tree::State::new(ExpandState {
            run: Run::finished(),
            open: self.open,
            key: self.key,
            from_height: 0.0,
            from_open: shown_open,
            height: 0.0,
            shown_open,
            measured: false,
            asked: None,
            settled: true,
        })
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<ExpandState>();
        if state.key != self.key {
            state.key = self.key;
            state.open = self.open;
            state.run.skip();
            state.shown_open = if self.open { 1.0 } else { 0.0 };
            state.from_open = state.shown_open;
            state.measured = false;
            state.settled = true;
        } else if self.open != state.open {
            state.from_height = state.height;
            state.from_open = state.shown_open;
            state.open = self.open;
            state.run.restart();
            state.settled = false;
        }
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn size_hint(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        let closed =
            self.children[0]
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
        let opened =
            self.children[1]
                .as_widget_mut()
                .layout(&mut tree.children[1], renderer, limits);
        let target_size = if self.open {
            opened.size()
        } else {
            closed.size()
        };
        let target_open = if self.open { 1.0 } else { 0.0 };
        let dur = self.duration();
        let state = tree.state.downcast_mut::<ExpandState>();
        let t = ease_out(state.run.progress(dur));
        if !state.measured || t >= 1.0 {
            state.height = target_size.height;
            state.shown_open = target_open;
            state.measured = true;
        } else {
            // Half-pixel snapping keeps lines crisp on Retina while avoiding
            // coarse 2-physical-pixel steps during the settle.
            state.height = (lerp(state.from_height, target_size.height, t) * 2.0).round() * 0.5;
            state.shown_open = lerp(state.from_open, target_open, t);
        }
        let width = closed.size().width.max(opened.size().width);
        layout::Node::with_children(Size::new(width, state.height), vec![closed, opened])
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let active = usize::from(self.open);
        if let Some(child_layout) = layout.children().nth(active) {
            let cursor = shown_cursor(cursor, layout.bounds());
            self.children[active].as_widget_mut().update(
                &mut tree.children[active],
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
        if let Some(now) = frame_time(event) {
            let dur = self.duration();
            let state = tree.state.downcast_mut::<ExpandState>();
            let moving = state.run.frame(now, dur);
            let progress = state.run.progress(dur);
            let needs_layout = (moving && progress > 0.0) || (!moving && !state.settled);
            if needs_layout && state.asked != Some(now) {
                state.asked = Some(now);
                state.settled = !moving;
                shell.invalidate_layout();
            }
            if moving {
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let mut children = layout.children();
        let (Some(closed_layout), Some(opened_layout)) = (children.next(), children.next()) else {
            return;
        };
        let state = tree.state.downcast_ref::<ExpandState>();
        let bounds = layout.bounds();

        // At rest, closed: draw the closed card directly with no extra layer.
        if state.resting_closed() {
            self.children[0].as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                closed_layout,
                cursor,
                viewport,
            );
            return;
        }

        let fill = self.surface.as_ref().map(|f| f(theme));
        if let Some(bg) = fill {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: iced::border::rounded(self.radius),
                    ..renderer::Quad::default()
                },
                bg,
            );
        }

        // At rest, open: draw the open form directly inside its card surface.
        if state.resting_open() {
            self.children[1].as_widget().draw(
                &tree.children[1],
                renderer,
                theme,
                style,
                opened_layout,
                cursor,
                viewport,
            );
            return;
        }

        // Animating: smoothly crossfade between the closed row and the open
        // form through the rounded card fill.
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        let p = state.shown_open.clamp(0.0, 1.0);
        const CROSS: f32 = 0.25;
        if p < CROSS || !self.has_opened {
            let fade = ease_out((p / CROSS).clamp(0.0, 1.0));
            renderer.with_layer(clip, |renderer| {
                self.children[0].as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    closed_layout,
                    mouse::Cursor::Unavailable,
                    &clip,
                );
                if let Some(bg) = fill
                    && fade > 0.001
                {
                    renderer.with_layer(clip, |renderer| {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds,
                                border: iced::border::rounded(self.radius),
                                ..renderer::Quad::default()
                            },
                            bg.scale_alpha(fade),
                        );
                    });
                }
            });
        } else {
            // Keep a sliver of rounded bottom edge clear while sliding.
            let inner = Rectangle {
                height: (bounds.height - self.radius * 0.5).max(0.0),
                ..bounds
            };
            let Some(inner_clip) = inner.intersection(viewport) else {
                return;
            };
            let shown = ease_out(((p - CROSS) / 0.45).clamp(0.0, 1.0));
            let veil = 1.0 - shown;
            renderer.with_layer(inner_clip, |renderer| {
                self.children[1].as_widget().draw(
                    &tree.children[1],
                    renderer,
                    theme,
                    style,
                    opened_layout,
                    mouse::Cursor::Unavailable,
                    &inner_clip,
                );
                if let Some(bg) = fill
                    && veil > 0.001
                {
                    renderer.with_layer(inner_clip, |renderer| {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: inner_clip,
                                ..renderer::Quad::default()
                            },
                            bg.scale_alpha(veil),
                        );
                    });
                }
            });
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &R,
        operation: &mut dyn Operation,
    ) {
        let active = usize::from(self.open);
        if let Some(child_layout) = layout.children().nth(active) {
            self.children[active].as_widget_mut().operate(
                &mut tree.children[active],
                child_layout,
                renderer,
                operation,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        let active = usize::from(self.open);
        match layout.children().nth(active) {
            Some(child_layout) => self.children[active].as_widget().mouse_interaction(
                &tree.children[active],
                child_layout,
                shown_cursor(cursor, layout.bounds()),
                viewport,
                renderer,
            ),
            None => mouse::Interaction::None,
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        if !self.open {
            return None;
        }
        let opened = layout.children().nth(1)?;
        self.children[1].as_widget_mut().overlay(
            &mut tree.children[1],
            opened,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a> From<Expand<'a, M, T, R>>
    for Element<'a, M, T, R>
{
    fn from(expand: Expand<'a, M, T, R>) -> Self {
        Element::new(expand)
    }
}

// ------------------------------------------------------------------ slide

/// A side-by-side page carousel for switching periods (horizontal for days
/// and weeks, or `.vertical()` for months). When `page` changes, `previous`
/// slides smoothly out while `current` glides in right beside it using pure
/// GPU translation (zero per-frame relayout, zero blank flash). Rapid clicks
/// and mid-slide reversals pick up seamlessly from the exact current offset.
pub fn slide<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    page: i64,
    current: impl Into<Element<'a, M, T, R>>,
    previous: Option<Element<'a, M, T, R>>,
) -> Slide<'a, M, T, R> {
    let has_previous = previous.is_some();
    let previous = previous.unwrap_or_else(|| iced::widget::Space::new().into());
    Slide {
        children: [current.into(), previous],
        page,
        has_previous,
        vertical: false,
        gap: 0.0,
        veil: None,
    }
}

/// See [`slide`].
pub struct Slide<'a, M, T, R> {
    children: [Element<'a, M, T, R>; 2],
    page: i64,
    has_previous: bool,
    vertical: bool,
    gap: f32,
    veil: Option<Behind<'a, T>>,
}

impl<'a, M, T, R> Slide<'a, M, T, R> {
    /// Slides vertically (next page enters from below, previous from above)
    /// instead of horizontally.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// Adds a visual gutter in logical pixels between the outgoing and
    /// incoming pages as they slide past each other.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap.max(0.0);
        self
    }

    /// Gently recedes the outgoing page into `behind` as it slides away.
    pub fn fade_from(mut self, behind: impl Fn(&T) -> Color + 'a) -> Self {
        self.veil = Some(Box::new(behind));
        self
    }
}

struct SlideState {
    run: Run,
    page: i64,
    /// +1.0 when moving forward (entering from right / bottom), -1.0 backward.
    dir: f32,
    /// Offset of `current` along the slide axis at the start of the active slide.
    from: f32,
    /// Last laid-out extent of the carousel along the slide axis.
    extent: f32,
}

impl SlideState {
    /// Current offset of `current` along the slide axis (0.0 at rest), and
    /// progress 0–1.
    fn offset(&self) -> Option<(f32, f32)> {
        let p = self.run.progress(PAGE_SLIDE);
        if p >= 1.0 || self.extent <= 0.0 {
            return None;
        }
        let eased = ease_out_quart(p);
        let raw = self.from * (1.0 - eased);
        // Half-pixel steps (whole physical pixels on 2x displays).
        let off = (raw * 2.0).round() * 0.5;
        Some((off, eased))
    }
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Slide<'_, M, T, R> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SlideState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SlideState {
            run: Run::finished(),
            page: self.page,
            dir: 1.0,
            from: 0.0,
            extent: 0.0,
        })
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<SlideState>();
        if self.page != state.page {
            let dir = if self.page > state.page { 1.0 } else { -1.0 };
            let cur = state.offset().map_or(0.0, |(off, _)| off);
            state.page = self.page;
            state.dir = dir;
            if state.extent > 0.0 {
                // Keep the outgoing page glued to the exact pixel it already
                // occupied (even on rapid clicks or mid-slide reversals).
                let stride = state.extent + self.gap;
                let max = stride * 1.5;
                state.from = (cur + dir * stride).clamp(-max, max);
                state.run.restart();
            } else {
                state.from = 0.0;
                state.run.skip();
            }
            // Move the outgoing page's widget tree into slot 1 so it keeps
            // its state (e.g. scroll position, open card, or summary panel)
            // while sliding away, and start the incoming page with fresh state.
            if tree.children.len() == 2 {
                let outgoing =
                    std::mem::replace(&mut tree.children[0], Tree::new(&self.children[0]));
                tree.children[1] = outgoing;
            }
        }
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        self.children[0].as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.children[0].as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        let current =
            self.children[0]
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
        let previous =
            self.children[1]
                .as_widget_mut()
                .layout(&mut tree.children[1], renderer, limits);
        let size = current.size();
        tree.state.downcast_mut::<SlideState>().extent = if self.vertical {
            size.height
        } else {
            size.width
        };
        layout::Node::with_children(size, vec![current, previous])
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Some(current_layout) = layout.children().next() {
            self.children[0].as_widget_mut().update(
                &mut tree.children[0],
                event,
                current_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
        if let Some(now) = frame_time(event) {
            let state = tree.state.downcast_mut::<SlideState>();
            if state.run.frame(now, PAGE_SLIDE) {
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let mut children = layout.children();
        let (Some(current_layout), Some(previous_layout)) = (children.next(), children.next())
        else {
            return;
        };
        let state = tree.state.downcast_ref::<SlideState>();
        let Some((curr_off, eased)) = state.offset() else {
            self.children[0].as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                current_layout,
                cursor,
                viewport,
            );
            return;
        };

        let bounds = layout.bounds();
        // Horizontal slides clip vertically to the viewport so a taller
        // outgoing week never clips abruptly; vertical slides clip strictly
        // to `bounds` so rows glide cleanly underneath a pinned header.
        let clip_region = if self.vertical {
            bounds
        } else {
            Rectangle {
                x: bounds.x,
                width: bounds.width,
                y: viewport.y,
                height: viewport.height,
            }
        };
        let Some(clip) = clip_region.intersection(viewport) else {
            return;
        };

        let extent = if self.vertical {
            bounds.height
        } else {
            bounds.width
        };
        let stride = extent + self.gap;
        let prev_off = curr_off - state.dir * stride;
        let translation = |off: f32| {
            if self.vertical {
                Vector::new(0.0, off)
            } else {
                Vector::new(off, 0.0)
            }
        };
        let local_viewport = |off: f32| {
            if self.vertical {
                Rectangle {
                    x: clip.x,
                    y: clip.y - off,
                    width: clip.width,
                    height: clip.height,
                }
            } else {
                Rectangle {
                    x: clip.x - off,
                    y: clip.y,
                    width: clip.width,
                    height: clip.height,
                }
            }
        };

        renderer.with_layer(clip, |renderer| {
            // Outgoing page slides smoothly away beside the incoming page.
            if self.has_previous {
                let prev_rect = if self.vertical {
                    Rectangle {
                        x: clip.x,
                        width: clip.width,
                        y: bounds.y + prev_off,
                        height: bounds.height,
                    }
                } else {
                    Rectangle {
                        x: bounds.x + prev_off,
                        width: bounds.width,
                        y: clip.y,
                        height: clip.height,
                    }
                };
                if let Some(prev_clip) = prev_rect.intersection(&clip) {
                    let prev_vp = local_viewport(prev_off);
                    renderer.with_translation(translation(prev_off), |renderer| {
                        self.children[1].as_widget().draw(
                            &tree.children[1],
                            renderer,
                            theme,
                            style,
                            previous_layout,
                            mouse::Cursor::Unavailable,
                            &prev_vp,
                        );
                    });
                    if let Some(behind) = &self.veil {
                        let veil = 0.25 * eased;
                        if veil > 0.001 {
                            renderer.with_layer(prev_clip, |renderer| {
                                renderer.fill_quad(
                                    renderer::Quad {
                                        bounds: prev_clip,
                                        ..renderer::Quad::default()
                                    },
                                    behind(theme).scale_alpha(veil),
                                );
                            });
                        }
                    }
                }
            }

            // Incoming page glides crisply into place.
            let curr_vp = local_viewport(curr_off);
            renderer.with_translation(translation(curr_off), |renderer| {
                self.children[0].as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    current_layout,
                    mouse::Cursor::Unavailable,
                    &curr_vp,
                );
            });
        });
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &R,
        operation: &mut dyn Operation,
    ) {
        if let Some(current_layout) = layout.children().next() {
            self.children[0].as_widget_mut().operate(
                &mut tree.children[0],
                current_layout,
                renderer,
                operation,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        match layout.children().next() {
            Some(current_layout) => self.children[0].as_widget().mouse_interaction(
                &tree.children[0],
                current_layout,
                cursor,
                viewport,
                renderer,
            ),
            None => mouse::Interaction::None,
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        let current_layout = layout.children().next()?;
        self.children[0].as_widget_mut().overlay(
            &mut tree.children[0],
            current_layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a> From<Slide<'a, M, T, R>>
    for Element<'a, M, T, R>
{
    fn from(slide: Slide<'a, M, T, R>) -> Self {
        Element::new(slide)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::clipboard;
    use iced::widget::{row, space};
    use iced::{Point, Theme};

    type El<'a> = Element<'a, (), Theme, ()>;

    const WINDOW: Size = Size::new(400.0, 300.0);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A widget laid out in a 400 x 300 window, as iced keeps it between
    /// views: its state lives on while `set` swaps in a new view of it.
    pub(super) struct Stage<'a> {
        pub element: El<'a>,
        pub tree: Tree,
        pub node: layout::Node,
        /// How many times a frame asked for a new layout.
        pub layouts: usize,
    }

    impl<'a> Stage<'a> {
        pub fn new(element: El<'a>) -> Self {
            let mut stage = Stage {
                tree: Tree::new(&element),
                element,
                node: layout::Node::default(),
                layouts: 0,
            };
            stage.lay_out();
            stage
        }

        fn lay_out(&mut self) {
            self.node = self.element.as_widget_mut().layout(
                &mut self.tree,
                &(),
                &layout::Limits::new(Size::ZERO, WINDOW),
            );
        }

        /// A new view of the same widget.
        pub fn set(&mut self, element: El<'a>) {
            self.element = element;
            self.tree.diff(&self.element);
            self.lay_out();
        }

        /// The frame at `now`; whether the widget asked for another.
        pub fn frame(&mut self, now: Instant) -> bool {
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            self.element.as_widget_mut().update(
                &mut self.tree,
                &Event::Window(window::Event::RedrawRequested(now)),
                Layout::new(&self.node),
                mouse::Cursor::Unavailable,
                &(),
                &mut clipboard::Null,
                &mut shell,
                &Rectangle::with_size(WINDOW),
            );
            let again = shell.redraw_request() == window::RedrawRequest::NextFrame;
            // As iced does: lay out again when asked, then send the frame
            // once more.
            if shell.is_layout_invalid() {
                self.layouts += 1;
                self.lay_out();
                let mut shell = Shell::new(&mut messages);
                self.element.as_widget_mut().update(
                    &mut self.tree,
                    &Event::Window(window::Event::RedrawRequested(now)),
                    Layout::new(&self.node),
                    mouse::Cursor::Unavailable,
                    &(),
                    &mut clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(WINDOW),
                );
                assert!(
                    !shell.is_layout_invalid(),
                    "a new layout asked for twice in one frame"
                );
            }
            again
        }

        pub fn state<S: 'static>(&self) -> &S {
            self.tree.state.downcast_ref::<S>()
        }
    }

    #[test]
    fn the_system_setting_can_be_read() {
        // Whatever it says: asking must not fail.
        let _ = system_reduces_motion();
    }

    #[test]
    fn easing_starts_fast_and_ends_exactly() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.8);
        assert_eq!(ease_out(7.0), 1.0);
    }

    fn panel<'a>() -> El<'a> {
        appear(space().width(100).height(50)).rise(RISE).into()
    }

    #[test]
    fn a_panel_rises_into_place_once_then_asks_for_no_more_frames() {
        let mut stage = Stage::new(panel());
        let pose = |stage: &Stage<'_>| stage.state::<AppearState>().pose(PANEL);
        // Not drawn yet: it starts low.
        assert_eq!(pose(&stage).unwrap().0, Vector::new(0.0, RISE));
        let t0 = Instant::now();
        assert!(stage.frame(t0));
        assert!(stage.frame(t0 + PANEL / 2));
        let (offset, _) = pose(&stage).unwrap();
        assert!(offset.y > 0.0 && offset.y < RISE / 2.0, "{offset:?}");
        assert_eq!(offset.x, 0.0);
        // At the end it is in place and the window can rest.
        assert!(!stage.frame(t0 + PANEL));
        assert_eq!(pose(&stage), None);
        // A new view of the same panel doesn't play it again.
        stage.set(panel());
        assert!(!stage.frame(t0 + ms(5000)));
        assert_eq!(pose(&stage), None);
    }

    fn page<'a>(group: u32, position: i64) -> El<'a> {
        appear(space().width(100).height(50))
            .rise(RISE)
            .fade_from(|_: &Theme| Color::WHITE)
            .page(group, position)
            .into()
    }

    #[test]
    fn a_page_is_still_at_first_and_plays_when_it_changes() {
        let mut stage = Stage::new(page(0, 10));
        let pose = |stage: &Stage<'_>| stage.state::<AppearState>().pose(PAGE);
        let t0 = Instant::now();
        assert!(!stage.frame(t0));
        assert_eq!(pose(&stage), None);

        // A later day: in from the right, fully veiled at first.
        stage.set(page(0, 11));
        assert_eq!(pose(&stage), Some((Vector::new(SLIDE, 0.0), 1.0)));
        let t1 = t0 + ms(1000);
        assert!(stage.frame(t1));
        assert!(stage.frame(t1 + PAGE / 2));
        let (offset, veil) = pose(&stage).unwrap();
        assert!(offset.x > 0.0 && offset.x < SLIDE / 2.0, "{offset:?}");
        assert!(veil > 0.0 && veil < 0.5, "{veil}");
        assert!(!stage.frame(t1 + PAGE));
        assert_eq!(pose(&stage), None);

        // An earlier one: from the left.
        stage.set(page(0, 3));
        assert_eq!(pose(&stage).unwrap().0, Vector::new(-SLIDE, 0.0));
        let t2 = t1 + ms(1000);
        stage.frame(t2);
        assert!(!stage.frame(t2 + PAGE));

        // Another tab: it rises.
        stage.set(page(1, 3));
        assert_eq!(pose(&stage).unwrap().0, Vector::new(0.0, RISE));
    }

    #[test]
    fn flicking_through_pages_shows_each_at_once() {
        let mut stage = Stage::new(page(0, 10));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(page(0, 11));
        assert!(stage.frame(t0 + ms(1000)));
        // Again 80 ms later, and again: no fade.
        stage.set(page(0, 12));
        assert!(!stage.frame(t0 + ms(1080)));
        assert_eq!(stage.state::<AppearState>().pose(PAGE), None);
        stage.set(page(0, 13));
        assert!(!stage.frame(t0 + ms(1160)));
        // After a pause it plays again.
        stage.set(page(0, 14));
        assert!(stage.frame(t0 + ms(2000)));
    }

    #[test]
    fn the_dark_layer_fades_in_and_back_out() {
        let shown = |stage: &Stage<'_>| stage.state::<DimState>().shown;
        let mut stage = Stage::new(dim(0.5));
        assert_eq!(stage.node.size(), WINDOW);
        let t0 = Instant::now();
        assert!(stage.frame(t0));
        assert_eq!(shown(&stage), 0.0);
        assert!(stage.frame(t0 + DIM / 2));
        assert!(shown(&stage) > 0.25 && shown(&stage) < 0.5);
        assert!(!stage.frame(t0 + DIM));
        assert_eq!(shown(&stage), 0.5);

        // Closed: it fades out from where it was, then rests at nothing.
        stage.set(dim(0.0));
        let t1 = t0 + ms(1000);
        assert!(stage.frame(t1));
        assert_eq!(shown(&stage), 0.5);
        assert!(stage.frame(t1 + DIM / 2));
        assert!(shown(&stage) > 0.0 && shown(&stage) < 0.25);
        assert!(!stage.frame(t1 + DIM));
        assert_eq!(shown(&stage), 0.0);

        // Mounted already clear: still from the start.
        let mut clear = Stage::new(dim(0.0));
        assert!(!clear.frame(t0));
    }

    /// Three tabs, 60, 80 and 100 wide and 4 apart, with tab `selected`
    /// selected.
    fn tabs<'a>(selected: usize) -> El<'a> {
        let tabs = || -> El<'a> {
            row![
                space().width(60).height(30),
                space().width(80).height(30),
                space().width(100).height(30),
            ]
            .spacing(4)
            .into()
        };
        pill(
            tabs(),
            tabs(),
            selected,
            PillStyle {
                color: Color::BLACK,
                radius: 6.0,
                shadow: Shadow::default(),
            },
        )
    }

    #[test]
    fn the_pill_starts_on_the_selected_tab_and_slides_to_the_next() {
        let shown = |stage: &Stage<'_>| stage.state::<PillState>().shown.unwrap();
        let rect = |x: f32, width: f32| Rectangle::new(Point::new(x, 0.0), Size::new(width, 30.0));
        let mut stage = Stage::new(tabs(0));
        let t0 = Instant::now();
        // On the selected tab straight away, with nothing to animate.
        assert!(!stage.frame(t0));
        assert_eq!(shown(&stage), rect(0.0, 60.0));

        stage.set(tabs(2));
        let t1 = t0 + ms(1000);
        assert!(stage.frame(t1));
        assert_eq!(shown(&stage), rect(0.0, 60.0));
        assert!(stage.frame(t1 + PILL / 2));
        let mid = shown(&stage);
        assert!(mid.x > 74.0 && mid.x < 148.0, "{mid:?}");
        assert!(mid.width > 80.0 && mid.width < 100.0, "{mid:?}");
        assert!(!stage.frame(t1 + PILL));
        assert_eq!(shown(&stage), rect(148.0, 100.0));

        // Turned back half way there, it leaves from where it is.
        stage.set(tabs(1));
        let t2 = t1 + ms(1000);
        stage.frame(t2);
        stage.frame(t2 + PILL / 2);
        let half = shown(&stage);
        stage.set(tabs(2));
        assert!(stage.frame(t2 + PILL / 2 + ms(16)));
        assert_eq!(shown(&stage), half);
    }

    /// A section's body, 120 tall, folded away or open.
    fn section<'a>(open: bool) -> El<'a> {
        fold(open, space().width(200).height(120)).into()
    }

    #[test]
    fn a_folded_section_opens_from_nothing_to_its_full_height() {
        let height = |stage: &Stage<'_>| stage.node.size().height;
        let mut stage = Stage::new(section(false));
        let t0 = Instant::now();
        assert!(!stage.frame(t0));
        assert_eq!(height(&stage), 0.0);

        stage.set(section(true));
        assert_eq!(height(&stage), 0.0);
        let t1 = t0 + ms(1000);
        assert!(stage.frame(t1));
        assert_eq!(height(&stage), 0.0);
        assert!(stage.frame(t1 + UNFOLD / 2));
        assert!(height(&stage) > 60.0 && height(&stage) < 120.0);
        // The whole content is laid out inside, cut to the part showing.
        assert_eq!(stage.node.children()[0].size().height, 120.0);
        assert!(!stage.frame(t1 + UNFOLD));
        assert_eq!(height(&stage), 120.0);
        // Then it rests: no more frames, no more layouts.
        let layouts = stage.layouts;
        assert!(!stage.frame(t1 + ms(5000)));
        assert_eq!(stage.layouts, layouts);

        // Folding plays it back: from the full height to nothing, the
        // body still laid out inside.
        stage.set(section(false));
        assert_eq!(height(&stage), 120.0);
        let t2 = t1 + ms(6000);
        assert!(stage.frame(t2));
        assert!(stage.frame(t2 + UNFOLD / 2));
        assert!(height(&stage) > 0.0 && height(&stage) < 60.0);
        assert_eq!(stage.node.children()[0].size().height, 120.0);
        assert!(!stage.frame(t2 + UNFOLD));
        assert_eq!(height(&stage), 0.0);
        let layouts = stage.layouts;
        assert!(!stage.frame(t2 + ms(5000)));
        assert_eq!(stage.layouts, layouts);
    }

    #[test]
    fn a_section_turned_back_half_open_folds_from_where_it_is() {
        let height = |stage: &Stage<'_>| stage.node.size().height;
        let mut stage = Stage::new(section(false));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(section(true));
        stage.frame(t0 + ms(1000));
        stage.frame(t0 + ms(1000) + UNFOLD / 2);
        let half = height(&stage);
        stage.set(section(false));
        assert_eq!(height(&stage), half);
        assert!(stage.frame(t0 + ms(1000) + UNFOLD / 2 + ms(16)));
        assert!(height(&stage) <= half);
    }

    #[test]
    fn a_folded_body_takes_no_clicks() {
        use iced::widget::button;
        let body = |open: bool| -> El<'static> {
            fold(open, button(space().width(200).height(120)).on_press(())).into()
        };
        let click = |stage: &mut Stage<'_>| {
            let mut messages = Vec::new();
            for event in [
                mouse::Event::ButtonPressed(mouse::Button::Left),
                mouse::Event::ButtonReleased(mouse::Button::Left),
            ] {
                // A shell per event, as iced does.
                let mut shell = Shell::new(&mut messages);
                stage.element.as_widget_mut().update(
                    &mut stage.tree,
                    &Event::Mouse(event),
                    Layout::new(&stage.node),
                    mouse::Cursor::Available(Point::new(10.0, 10.0)),
                    &(),
                    &mut clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(WINDOW),
                );
            }
            messages.len()
        };
        let mut stage = Stage::new(body(true));
        assert_eq!(click(&mut stage), 1);
        // Folding away (still drawn) and folded: nothing.
        stage.set(body(false));
        assert_eq!(click(&mut stage), 0);
        stage.frame(Instant::now());
        assert_eq!(click(&mut stage), 0);
    }

    #[test]
    fn a_section_open_from_the_start_does_not_animate() {
        let mut stage = Stage::new(section(true));
        assert_eq!(stage.node.size().height, 120.0);
        assert!(!stage.frame(Instant::now()));
        assert_eq!(stage.layouts, 0);
    }

    #[test]
    fn a_section_closing_after_a_delay_holds_then_folds() {
        let delayed = |open: bool| -> El<'_> {
            fold(open, space().width(200).height(120))
                .close_after(ms(100))
                .into()
        };
        let mut stage = Stage::new(delayed(true));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(delayed(false));
        assert!(stage.frame(t0 + ms(1000)));
        assert!(stage.frame(t0 + ms(1050)));
        assert_eq!(stage.node.size().height, 120.0, "held");
        assert!(stage.frame(t0 + ms(1100) + UNFOLD / 2));
        let h = stage.node.size().height;
        assert!(h > 0.0 && h < 120.0, "{h}");
        assert!(!stage.frame(t0 + ms(1100) + UNFOLD));
        assert_eq!(stage.node.size().height, 0.0);
        // Opening is not delayed.
        stage.set(delayed(true));
        assert!(stage.frame(t0 + ms(2000)));
        assert!(stage.frame(t0 + ms(2000) + UNFOLD / 2));
        assert!(stage.node.size().height > 0.0);
    }

    /// A list item: 40 tall closed, 200 open.
    fn item<'a>(open: bool, key: u64) -> El<'a> {
        unfold(open, space().width(200).height(if open { 200 } else { 40 }))
            .key(key)
            .into()
    }

    #[test]
    fn an_item_opens_from_its_closed_height() {
        let height = |stage: &Stage<'_>| stage.node.size().height;
        let mut stage = Stage::new(item(false, 7));
        let t0 = Instant::now();
        stage.frame(t0);
        assert_eq!(height(&stage), 40.0);
        stage.set(item(true, 7));
        assert_eq!(height(&stage), 40.0);
        assert!(stage.frame(t0 + ms(1000)));
        assert!(stage.frame(t0 + ms(1000) + UNFOLD / 2));
        assert!(height(&stage) > 120.0 && height(&stage) < 200.0);
        assert!(!stage.frame(t0 + ms(1000) + UNFOLD));
        assert_eq!(height(&stage), 200.0);

        // Closing, it shrinks back to its closed height.
        stage.set(item(false, 7));
        assert_eq!(height(&stage), 200.0);
        let t1 = t0 + ms(2000);
        assert!(stage.frame(t1));
        assert!(stage.frame(t1 + UNFOLD / 2));
        assert!(height(&stage) > 40.0 && height(&stage) < 120.0);
        assert!(!stage.frame(t1 + UNFOLD));
        assert_eq!(height(&stage), 40.0);
    }

    #[test]
    fn another_item_taking_a_closed_ones_place_is_shown_open_at_once() {
        let mut stage = Stage::new(item(false, 7));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(item(true, 8));
        assert_eq!(stage.node.size().height, 200.0);
        assert!(!stage.frame(t0 + ms(1000)));
    }

    #[test]
    fn another_item_taking_an_open_ones_place_is_shown_closed_at_once() {
        let mut stage = Stage::new(item(true, 7));
        stage.set(item(false, 8));
        assert_eq!(stage.node.size().height, 40.0);
        assert!(!stage.frame(Instant::now()));
    }

    fn card<'a>(open: bool, key: u64, has_opened: bool) -> El<'a> {
        let closed = space().width(200).height(44);
        let opened = has_opened.then(|| space().width(200).height(300).into());
        expand(open, closed, opened)
            .key(key)
            .radius(6.0)
            .surface(|_: &Theme| Color::BLACK)
            .into()
    }

    #[test]
    fn expanding_card_opens_closes_and_reverses_smoothly() {
        let height = |stage: &Stage<'_>| stage.node.size().height;
        let mut stage = Stage::new(card(false, 7, false));
        let t0 = Instant::now();
        assert!(!stage.frame(t0));
        assert_eq!(height(&stage), 44.0);
        assert_eq!(stage.state::<ExpandState>().shown_open, 0.0);

        // Open: starts from closed height without an extra click-frame layout.
        stage.set(card(true, 7, true));
        assert_eq!(height(&stage), 44.0);
        let layouts_before = stage.layouts;
        let t1 = t0 + ms(1000);
        assert!(stage.frame(t1));
        assert_eq!(stage.layouts, layouts_before);
        assert!(stage.frame(t1 + EXPAND_OPEN / 2));
        assert!(height(&stage) > 170.0 && height(&stage) < 300.0);
        assert!(!stage.frame(t1 + EXPAND_OPEN));
        assert_eq!(height(&stage), 300.0);
        assert_eq!(stage.state::<ExpandState>().shown_open, 1.0);

        // Close: shrinks smoothly while the closing form stays mounted.
        stage.set(card(false, 7, true));
        assert_eq!(height(&stage), 300.0);
        let t2 = t1 + ms(1000);
        assert!(stage.frame(t2));
        assert!(stage.frame(t2 + EXPAND_CLOSE / 2));
        let mid_h = height(&stage);
        let mid_p = stage.state::<ExpandState>().shown_open;
        assert!(mid_h > 44.0 && mid_h < 180.0);
        assert!(mid_p > 0.0 && mid_p < 0.5);

        // Re-open mid-close: picks up from the current height & fade phase.
        stage.set(card(true, 7, true));
        assert_eq!(height(&stage), mid_h);
        assert_eq!(stage.state::<ExpandState>().shown_open, mid_p);
        let t3 = t2 + EXPAND_CLOSE / 2 + ms(16);
        assert!(stage.frame(t3));
        assert!(!stage.frame(t3 + EXPAND_OPEN));
        assert_eq!(height(&stage), 300.0);
        assert_eq!(stage.state::<ExpandState>().shown_open, 1.0);
    }

    fn week_slide<'a>(page: i64, has_prev: bool) -> El<'a> {
        let cur = space().width(400).height(200);
        let prev = has_prev.then(|| space().width(400).height(200).into());
        slide(page, cur, prev)
            .fade_from(|_: &Theme| Color::BLACK)
            .into()
    }

    #[test]
    fn sliding_pages_move_without_relayout_and_reverse_continuously() {
        let mut stage = Stage::new(week_slide(10, false));
        let t0 = Instant::now();
        assert!(!stage.frame(t0));
        assert_eq!(stage.state::<SlideState>().offset(), None);

        // Next week: starts one full width to the right, slides to 0 with zero relayouts.
        let layouts_before = stage.layouts;
        stage.set(week_slide(11, true));
        assert_eq!(stage.state::<SlideState>().offset().unwrap().0, 400.0);
        let t1 = t0 + ms(1000);
        assert!(stage.frame(t1));
        assert!(stage.frame(t1 + PAGE_SLIDE / 4));
        let (mid_x, _) = stage.state::<SlideState>().offset().unwrap();
        assert!(mid_x > 0.0 && mid_x < 400.0, "{mid_x}");
        assert_eq!(stage.layouts, layouts_before);

        // Reversing mid-slide keeps both pages glued with zero jump:
        // page 10 was at `mid_x - 400.0`, so returning to page 10 starts there.
        stage.set(week_slide(10, true));
        let (rev_x, _) = stage.state::<SlideState>().offset().unwrap();
        assert!(
            (rev_x - (mid_x - 400.0)).abs() <= 0.5,
            "{rev_x} vs {}",
            mid_x - 400.0
        );
        let t2 = t1 + PAGE_SLIDE / 4 + ms(16);
        assert!(stage.frame(t2));
        assert!(!stage.frame(t2 + PAGE_SLIDE));
        assert_eq!(stage.state::<SlideState>().offset(), None);
        assert_eq!(stage.layouts, layouts_before);
    }

    #[test]
    fn vertical_slide_uses_height_plus_gap() {
        let make = |page: i64, has_prev: bool| -> El<'static> {
            let cur = space().width(400).height(200);
            let prev = has_prev.then(|| space().width(400).height(200).into());
            slide(page, cur, prev).vertical().gap(6.0).into()
        };
        let mut stage = Stage::new(make(1, false));
        let t0 = Instant::now();
        assert!(!stage.frame(t0));
        // Next month enters from below (`height + gap = 206`).
        stage.set(make(2, true));
        assert_eq!(stage.state::<SlideState>().offset().unwrap().0, 206.0);
        let t1 = t0 + ms(1000);
        assert!(stage.frame(t1));
        assert!(!stage.frame(t1 + PAGE_SLIDE));
        assert_eq!(stage.state::<SlideState>().offset(), None);
        // Previous month enters from above (`-(height + gap) = -206`).
        stage.set(make(1, true));
        assert_eq!(stage.state::<SlideState>().offset().unwrap().0, -206.0);
    }

    #[test]
    fn slide_preserves_outgoing_tree_and_starts_incoming_fresh() {
        let page = |p: i64, open: bool, prev_open: Option<bool>| -> El<'static> {
            let cur = fold(open, space().width(400).height(100));
            let prev = prev_open.map(|o| fold(o, space().width(400).height(100)).into());
            slide(p, cur, prev).into()
        };
        let mut stage = Stage::new(page(1, true, None));
        let t0 = Instant::now();
        assert!(!stage.frame(t0));
        // Page 1 starts open (laid out at height 100). Switch to page 2 (closed):
        // slot 1 keeps page 1's settled open tree, slot 0 starts cleanly closed
        // at rest rather than animating closed from page 1's state.
        stage.set(page(2, false, Some(true)));
        let outgoing = stage.tree.children[1].state.downcast_ref::<UnfoldState>();
        let incoming = stage.tree.children[0].state.downcast_ref::<UnfoldState>();
        assert!(outgoing.open);
        assert_eq!(outgoing.height, 100.0);
        assert!(!incoming.open);
        assert!(incoming.settled);
        assert_eq!(incoming.run.progress(ms(100)), 1.0);
    }
}
