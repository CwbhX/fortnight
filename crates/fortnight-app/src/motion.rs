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
//!   task opening into its form) growing to its open height.
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

/// How long a page (the open tab) takes to fade in.
const PAGE: Duration = Duration::from_millis(170);
/// How long a panel takes to rise into place.
const PANEL: Duration = Duration::from_millis(200);
/// How long the dark layer takes to reach a new level.
const DIM: Duration = Duration::from_millis(180);
/// How long the tab pill takes to reach another tab.
const PILL: Duration = Duration::from_millis(220);
/// How long something takes to open to its full height.
const UNFOLD: Duration = Duration::from_millis(220);
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
        self.start = None;
        self.skipped = false;
    }

    fn skip(&mut self) {
        self.skipped = true;
    }

    /// Takes a frame's time; `true` while the run has further to go.
    fn frame(&mut self, now: Instant, duration: Duration) -> bool {
        self.start.get_or_insert(now);
        self.now = Some(now);
        self.progress(duration) < 1.0
    }

    /// How far along it is at the latest frame, 0–1 (not eased).
    fn progress(&self, duration: Duration) -> f32 {
        if let Some(held) = held() {
            return held;
        }
        if self.skipped || !enabled() || duration.is_zero() {
            return 1.0;
        }
        match (self.start, self.now) {
            (Some(start), Some(now)) => (now.saturating_duration_since(start).as_secs_f32()
                / duration.as_secs_f32())
            .min(1.0),
            _ => 0.0,
        }
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
            run: Run::default(),
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

/// `content` growing to its full height when `open` turns true: from the
/// height it had closed, showing more of the open content from the top
/// down. Closing is immediate. Give it the closed content while closed and
/// the open content while open; see [`fold`] for a body that is simply
/// absent while closed.
pub fn unfold<'a, M, T, R>(
    open: bool,
    content: impl Into<Element<'a, M, T, R>>,
) -> Unfold<'a, M, T, R> {
    Unfold {
        content: content.into(),
        open,
        key: 0,
    }
}

/// A folded section's body: nothing while `body` is `None`, unfolding when
/// it is given.
pub fn fold<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    body: Option<Element<'a, M, T, R>>,
) -> Unfold<'a, M, T, R> {
    let open = body.is_some();
    unfold(
        open,
        body.unwrap_or_else(|| iced::widget::Space::new().into()),
    )
}

/// See [`unfold`].
pub struct Unfold<'a, M, T, R> {
    content: Element<'a, M, T, R>,
    open: bool,
    key: u64,
}

impl<M, T, R> Unfold<'_, M, T, R> {
    /// What this is (a task's id, a section): in a list whose items move,
    /// an item that lands where a closed one was doesn't open with a
    /// flourish it didn't earn.
    pub fn key(mut self, key: u64) -> Self {
        self.key = key;
        self
    }
}

struct UnfoldState {
    run: Run,
    open: bool,
    key: u64,
    /// The height it had when it last opened.
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
        } else if self.open && !state.open {
            state.from = state.height;
            state.run.restart();
            state.settled = false;
        } else if !self.open {
            state.run.skip();
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
        let state = tree.state.downcast_mut::<UnfoldState>();
        let t = ease_out(state.run.progress(UNFOLD));
        // Whole pixels, so what follows it stays sharp as it moves.
        state.height = if t < 1.0 {
            lerp(state.from, full.height, t).round()
        } else {
            full.height
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
        if let Some(content) = layout.children().next() {
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
            // The height changes on every frame of the run, and once more
            // for the full height at its end.
            if (moving || !state.settled) && state.asked != Some(now) {
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
        if let Some(content) = layout.children().next() {
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
        match layout.children().next() {
            Some(content) => self.content.as_widget().mouse_interaction(
                &tree.children[0],
                content,
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
    struct Stage<'a> {
        element: El<'a>,
        tree: Tree,
        node: layout::Node,
        /// How many times a frame asked for a new layout.
        layouts: usize,
    }

    impl<'a> Stage<'a> {
        fn new(element: El<'a>) -> Self {
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
        fn set(&mut self, element: El<'a>) {
            self.element = element;
            self.tree.diff(&self.element);
            self.lay_out();
        }

        /// The frame at `now`; whether the widget asked for another.
        fn frame(&mut self, now: Instant) -> bool {
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

        fn state<S: 'static>(&self) -> &S {
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
        fold(open.then(|| space().width(200).height(120).into())).into()
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

        // Folding is immediate.
        stage.set(section(false));
        assert_eq!(height(&stage), 0.0);
        assert!(!stage.frame(t1 + ms(6000)));
    }

    #[test]
    fn a_section_open_from_the_start_does_not_animate() {
        let mut stage = Stage::new(section(true));
        assert_eq!(stage.node.size().height, 120.0);
        assert!(!stage.frame(Instant::now()));
        assert_eq!(stage.layouts, 0);
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
}
