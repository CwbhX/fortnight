//! [`list`]: a column of keyed items that come and go smoothly.

use super::{Behind, Run, ease_out, frame_time, lerp, shown_cursor};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::time::{Duration, Instant};
use iced::{Color, Element, Event, Length, Point, Rectangle, Size, Transformation, Vector, mouse};
use std::collections::HashMap;

/// How long an item takes to come in.
const ENTER: Duration = Duration::from_millis(300);
/// How long an item takes to go: a beat to show what changed (a ticked
/// box), then it fades and shrinks away, and the gap closes.
const LEAVE: Duration = Duration::from_millis(380);
/// How long a leaving item takes to fade away (its gap is still closing
/// then): a section folding away after its last item waits this long
/// ([`Unfold::close_after`](super::Unfold::close_after)). See `Motion::pose`.
pub const FADED: Duration = Duration::from_millis(LEAVE.as_millis() as u64 * 7 / 10);
/// How far an item drifts as it comes or goes, in logical pixels.
const DRIFT: f32 = 10.0;
/// How small an item is at the start of coming in or the end of going.
const SMALL: f32 = 0.95;

/// A column of items, each known by its key, that grow, fade and zoom in
/// when they first appear in it and fade, zoom out and fold away when the
/// view marks them [`Item::leaving`]. Items keep their own widget state as
/// they move about (it follows the key, not the place).
///
/// A list doesn't play anything when it first shows, nor when its own
/// [`key`](List::key) changes (another day shown): only what changes
/// while it stays.
pub fn list<'a, M, T, R>() -> List<'a, M, T, R> {
    List {
        items: Vec::new(),
        spacing: 0.0,
        key: 0,
        veil: None,
    }
}

/// One item of a [`list`].
pub struct Item<'a, M, T = iced::Theme, R = iced::Renderer> {
    key: u64,
    content: Element<'a, M, T, R>,
    /// Up (-1), down (1) or neither (0): where it goes when leaving, where
    /// it comes from when it arrives.
    towards: f32,
    leaving: bool,
}

impl<'a, M, T, R> Item<'a, M, T, R> {
    pub fn new(key: u64, content: impl Into<Element<'a, M, T, R>>) -> Self {
        Item {
            key,
            content: content.into(),
            towards: 0.0,
            leaving: false,
        }
    }

    /// If it arrives, it comes from the side `towards` points to (negative:
    /// from above, positive: from below).
    pub fn from(mut self, towards: f32) -> Self {
        self.towards = towards.signum() * f32::from(towards != 0.0);
        self
    }

    /// It has left: it is drawn going (towards above if `towards` is
    /// negative, below if positive) and then takes no room. It takes no
    /// clicks or keys. Keep giving it until it is long gone; drop it then.
    pub fn leaving(mut self, towards: f32) -> Self {
        self.leaving = true;
        self.towards = towards.signum() * f32::from(towards != 0.0);
        self
    }
}

/// See [`list`].
pub struct List<'a, M, T = iced::Theme, R = iced::Renderer> {
    items: Vec<Item<'a, M, T, R>>,
    spacing: f32,
    key: u64,
    veil: Option<Behind<'a, T>>,
}

impl<'a, M, T, R> List<'a, M, T, R> {
    pub fn push(mut self, item: Item<'a, M, T, R>) -> Self {
        self.items.push(item);
        self
    }

    /// Room between items; an item coming or going takes its share.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// What the list shows (a day): when this changes, everything is
    /// simply there or gone.
    pub fn key(mut self, key: u64) -> Self {
        self.key = key;
        self
    }

    /// Fade items over a plain background of the colour `behind` returns.
    pub fn fade_from(mut self, behind: impl Fn(&T) -> Color + 'a) -> Self {
        self.veil = Some(Box::new(behind));
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Way {
    In,
    Out,
}

#[derive(Debug, Clone, Copy)]
struct Motion {
    way: Way,
    run: Run,
    towards: f32,
}

/// How an item is drawn at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    /// The share of its height (and of the gap before it) it takes.
    room: f32,
    opacity: f32,
    scale: f32,
    /// Its drawing's offset, down.
    drift: f32,
}

impl Pose {
    const STILL: Pose = Pose {
        room: 1.0,
        opacity: 1.0,
        scale: 1.0,
        drift: 0.0,
    };

    /// The part of an item laid out at `bounds` that takes room, from its
    /// top: what it is drawn in.
    fn room_of(&self, bounds: Rectangle) -> Rectangle {
        Rectangle {
            height: (bounds.height * self.room).round(),
            ..bounds
        }
    }
}

/// `t` (0–1) of the way from `start` to `end`, clamped.
fn phase(t: f32, start: f32, end: f32) -> f32 {
    ((t - start) / (end - start)).clamp(0.0, 1.0)
}

/// Slow, fast, slow: for the gap closing.
fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Motion {
    fn duration(&self) -> Duration {
        match self.way {
            Way::In => ENTER,
            Way::Out => LEAVE,
        }
    }

    fn pose(&self) -> Pose {
        let t = self.run.progress(self.duration());
        match self.way {
            // Room first, then the item fades and zooms in, settling from
            // the side it came from.
            Way::In => {
                let shown = ease_out(phase(t, 0.15, 1.0));
                Pose {
                    room: ease_out(phase(t, 0.0, 0.6)),
                    opacity: shown,
                    scale: lerp(SMALL, 1.0, shown),
                    drift: self.towards * DRIFT * (1.0 - shown),
                }
            }
            // A beat as it is, then it fades and zooms out, drifting the
            // way it went, and the gap closes after it.
            Way::Out => {
                let gone = ease_out(phase(t, 0.2, 0.7));
                Pose {
                    room: 1.0 - ease_in_out(phase(t, 0.4, 1.0)),
                    opacity: 1.0 - gone,
                    scale: lerp(1.0, SMALL, gone),
                    drift: self.towards * DRIFT * gone,
                }
            }
        }
    }

    fn finished(&self) -> bool {
        self.run.progress(self.duration()) >= 1.0
    }
}

struct State {
    key: u64,
    /// The items' keys at the last view, in order: the children's trees
    /// are in this order.
    keys: Vec<u64>,
    /// Items coming or going (or gone).
    motions: HashMap<u64, Motion>,
    /// The frame for which a new layout was last asked: once per frame.
    asked: Option<Instant>,
    /// Laid out with every item at rest since the last run ended.
    settled: bool,
}

impl State {
    fn pose(&self, key: u64) -> Pose {
        self.motions.get(&key).map_or(Pose::STILL, Motion::pose)
    }
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for List<'_, M, T, R> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        // What is there when it first shows just is there; what is
        // leaving is already gone.
        let motions = self
            .items
            .iter()
            .filter(|item| item.leaving)
            .map(|item| (item.key, gone(item.towards)))
            .collect();
        tree::State::new(State {
            key: self.key,
            keys: self.items.iter().map(|item| item.key).collect(),
            motions,
            asked: None,
            settled: true,
        })
    }

    fn children(&self) -> Vec<Tree> {
        self.items
            .iter()
            .map(|item| Tree::new(&item.content))
            .collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        let shown_before = state.key == self.key;
        if !shown_before {
            state.key = self.key;
            state.motions.clear();
        }
        let before: Vec<u64> = std::mem::take(&mut state.keys);
        for item in &self.items {
            let was_there = shown_before && before.contains(&item.key);
            let motion = state.motions.get(&item.key).copied();
            let next = if item.leaving {
                match motion {
                    Some(m) if m.way == Way::Out => None,
                    _ if was_there => Some(start(Way::Out, item.towards)),
                    _ => Some(gone(item.towards)),
                }
            } else {
                match motion {
                    // Back before it had gone, or long after.
                    Some(m) if m.way == Way::Out => Some(start(Way::In, item.towards)),
                    Some(_) => None,
                    None if !was_there && shown_before => Some(start(Way::In, item.towards)),
                    None => None,
                }
            };
            if let Some(next) = next {
                state.motions.insert(item.key, next);
                state.settled = false;
            }
        }
        state
            .motions
            .retain(|key, _| self.items.iter().any(|item| item.key == *key));
        state.keys = self.items.iter().map(|item| item.key).collect();

        // Each item keeps its own tree, wherever it now is.
        let mut old: HashMap<u64, Tree> = before.into_iter().zip(tree.children.drain(..)).collect();
        tree.children = self
            .items
            .iter()
            .map(|item| match old.remove(&item.key) {
                Some(mut child) => {
                    child.diff(&item.content);
                    child
                }
                None => Tree::new(&item.content),
            })
            .collect();
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        let max = limits.max();
        let child_limits = layout::Limits::new(Size::ZERO, Size::new(max.width, f32::INFINITY));
        let state = tree.state.downcast_ref::<State>();
        let poses: Vec<Pose> = self.items.iter().map(|item| state.pose(item.key)).collect();
        // Whole pixels while anything moves, so what moves stays sharp; at
        // rest, exactly as a column would place them.
        let moving = state.motions.values().any(|m| !m.finished());
        let snap = |y: f32| if moving { y.round() } else { y };
        let mut y = 0.0;
        // The most room any item before this one takes: the gap before an
        // item is only there with something above it.
        let mut above: f32 = 0.0;
        let mut nodes = Vec::with_capacity(self.items.len());
        for ((item, child), pose) in self.items.iter_mut().zip(&mut tree.children).zip(&poses) {
            let node = item
                .content
                .as_widget_mut()
                .layout(child, renderer, &child_limits);
            let gap = self.spacing * pose.room.min(above);
            y += gap;
            let height = node.size().height;
            nodes.push(node.move_to(Point::new(0.0, snap(y))));
            y += height * pose.room;
            above = above.max(pose.room);
        }
        let size = limits.resolve(Length::Fill, Length::Shrink, Size::new(max.width, snap(y)));
        layout::Node::with_children(size, nodes)
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
        let state = tree.state.downcast_ref::<State>();
        let poses: Vec<Pose> = self.items.iter().map(|item| state.pose(item.key)).collect();
        for (((item, child), layout), pose) in self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .zip(&poses)
        {
            if !item.leaving {
                let cursor = shown_cursor(cursor, pose.room_of(layout.bounds()));
                item.content.as_widget_mut().update(
                    child, event, layout, cursor, renderer, clipboard, shell, viewport,
                );
            }
        }
        if let Some(now) = frame_time(event) {
            let state = tree.state.downcast_mut::<State>();
            let mut moving = false;
            for motion in state.motions.values_mut() {
                moving |= motion.run.frame(now, motion.duration());
            }
            if (moving || !state.settled) && state.asked != Some(now) {
                state.asked = Some(now);
                state.settled = !moving;
                shell.invalidate_layout();
            }
            if moving {
                shell.request_redraw();
            } else {
                // Arrived ones are simply there now; gone ones stay gone.
                state
                    .motions
                    .retain(|_, motion| motion.way == Way::Out || !motion.finished());
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
        let state = tree.state.downcast_ref::<State>();
        for ((item, child), layout) in self.items.iter().zip(&tree.children).zip(layout.children())
        {
            let widget = item.content.as_widget();
            let pose = state.pose(item.key);
            if pose == Pose::STILL {
                widget.draw(child, renderer, theme, style, layout, cursor, viewport);
                continue;
            }
            let bounds = layout.bounds();
            let room = pose.room_of(bounds);
            let Some(clip) = room.intersection(viewport) else {
                continue;
            };
            let cursor = if item.leaving {
                mouse::Cursor::Unavailable
            } else {
                cursor
            };
            let centre = bounds.center();
            let zoom = Transformation::translate(centre.x, centre.y + pose.drift)
                * Transformation::scale(pose.scale)
                * Transformation::translate(-centre.x, -centre.y);
            renderer.with_layer(clip, |renderer| {
                renderer.with_transformation(zoom, |renderer| {
                    widget.draw(child, renderer, theme, style, layout, cursor, &clip);
                });
                if let Some(behind) = &self.veil
                    && pose.opacity < 1.0
                {
                    // Its own layer, to cover the item's text too.
                    renderer.with_layer(clip, |renderer| {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: clip,
                                ..renderer::Quad::default()
                            },
                            behind(theme).scale_alpha(1.0 - pose.opacity),
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
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((item, child), layout) in self
                .items
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
            {
                if !item.leaving {
                    item.content
                        .as_widget_mut()
                        .operate(child, layout, renderer, operation);
                }
            }
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        self.items
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|((item, _), _)| !item.leaving)
            .map(|((item, child), layout)| {
                let room = state.pose(item.key).room_of(layout.bounds());
                item.content.as_widget().mouse_interaction(
                    child,
                    layout,
                    shown_cursor(cursor, room),
                    viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_default()
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        let children: Vec<_> = self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .filter(|((item, _), _)| !item.leaving)
            .filter_map(|((item, child), layout)| {
                item.content
                    .as_widget_mut()
                    .overlay(child, layout, renderer, viewport, translation)
            })
            .collect();
        (!children.is_empty()).then(|| overlay::Group::with_children(children).overlay())
    }
}

fn start(way: Way, towards: f32) -> Motion {
    Motion {
        way,
        run: Run::default(),
        towards,
    }
}

fn gone(towards: f32) -> Motion {
    Motion {
        way: Way::Out,
        run: Run::finished(),
        towards,
    }
}

impl<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a> From<List<'a, M, T, R>>
    for Element<'a, M, T, R>
{
    fn from(list: List<'a, M, T, R>) -> Self {
        Element::new(list)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Stage;
    use super::*;
    use iced::Theme;
    use iced::widget::space;

    type El<'a> = Element<'a, (), Theme, ()>;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Items 40 tall, 10 apart: `(key, leaving)`.
    fn items<'a>(day: u64, items: &[(u64, bool)]) -> El<'a> {
        let mut list = list().spacing(10.0).key(day);
        for &(key, leaving) in items {
            let item = Item::new(key, space().width(200).height(40));
            list = list.push(if leaving {
                item.leaving(1.0)
            } else {
                item.from(1.0)
            });
        }
        list.into()
    }

    fn height(stage: &Stage<'_>) -> f32 {
        stage.node.size().height
    }

    fn poses(stage: &Stage<'_>, keys: &[u64]) -> Vec<Pose> {
        let state = stage.state::<State>();
        keys.iter().map(|&key| state.pose(key)).collect()
    }

    #[test]
    fn items_there_from_the_start_are_still() {
        let mut stage = Stage::new(items(1, &[(1, false), (2, false), (3, true)]));
        assert_eq!(height(&stage), 90.0);
        assert!(!stage.frame(Instant::now()));
        assert_eq!(poses(&stage, &[1, 2, 3])[2].room, 0.0);
    }

    #[test]
    fn a_new_item_grows_and_fades_in() {
        let mut stage = Stage::new(items(1, &[(1, false)]));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(items(1, &[(1, false), (2, false)]));
        // No room yet: the gap and the item open up as it plays.
        assert_eq!(height(&stage), 40.0);
        assert!(stage.frame(t0 + ms(1000)));
        assert!(stage.frame(t0 + ms(1000) + ENTER / 3));
        let pose = poses(&stage, &[2])[0];
        assert!(pose.room > 0.3 && pose.room < 1.0, "{pose:?}");
        assert!(pose.opacity < pose.room, "{pose:?}");
        assert!(pose.drift > 0.0, "it rises from below: {pose:?}");
        assert!(height(&stage) > 40.0 && height(&stage) < 90.0);
        assert!(!stage.frame(t0 + ms(1000) + ENTER));
        assert_eq!(poses(&stage, &[2])[0], Pose::STILL);
        assert_eq!(height(&stage), 90.0);
        // At rest: no more layouts.
        let layouts = stage.layouts;
        assert!(!stage.frame(t0 + ms(5000)));
        assert_eq!(stage.layouts, layouts);
    }

    #[test]
    fn a_leaving_item_fades_then_its_gap_closes() {
        let mut stage = Stage::new(items(1, &[(1, false), (2, false), (3, false)]));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(items(1, &[(1, false), (2, true), (3, false)]));
        assert_eq!(height(&stage), 140.0);
        let t1 = t0 + ms(1000);
        assert!(stage.frame(t1));
        // A beat as it was.
        assert!(stage.frame(t1 + LEAVE / 10));
        assert_eq!(poses(&stage, &[2])[0].opacity, 1.0);
        assert!(stage.frame(t1 + LEAVE / 2));
        let pose = poses(&stage, &[2])[0];
        assert!(pose.opacity < 0.5 && pose.room > 0.5, "{pose:?}");
        assert!(pose.scale < 1.0 && pose.drift > 0.0, "{pose:?}");
        assert!(!stage.frame(t1 + LEAVE));
        assert_eq!(poses(&stage, &[2])[0].room, 0.0);
        // Gone, gap and all; the one below moved up.
        assert_eq!(height(&stage), 90.0);
        assert_eq!(stage.node.children()[2].bounds().y, 50.0);
        // Dropped later: nothing moves.
        stage.set(items(1, &[(1, false), (3, false)]));
        assert_eq!(height(&stage), 90.0);
        assert!(!stage.frame(t1 + ms(2000)));
    }

    #[test]
    fn the_first_item_leaving_takes_the_gap_under_it_too() {
        let mut stage = Stage::new(items(1, &[(1, false), (2, false)]));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(items(1, &[(1, true), (2, false)]));
        stage.frame(t0 + ms(1000));
        stage.frame(t0 + ms(1000) + LEAVE);
        assert_eq!(height(&stage), 40.0);
        assert_eq!(stage.node.children()[1].bounds().y, 0.0);
    }

    #[test]
    fn an_item_keeps_its_own_state_when_it_moves() {
        use super::super::{UnfoldState, unfold};
        // Two items that remember the height they were laid out at.
        let two = |order: [u64; 2]| -> El<'static> {
            let item = |key: u64| {
                Item::new(
                    key,
                    unfold(false, space().width(10).height(key as f32 * 10.0)),
                )
            };
            list().push(item(order[0])).push(item(order[1])).into()
        };
        let mut stage = Stage::new(two([1, 2]));
        // The new view, before it is laid out again.
        stage.element = two([2, 1]);
        stage.tree.diff(&stage.element);
        let heights: Vec<f32> = stage
            .tree
            .children
            .iter()
            .map(|child| child.state.downcast_ref::<UnfoldState>().height)
            .collect();
        assert_eq!(heights, vec![20.0, 10.0]);
        assert_eq!(stage.state::<State>().keys, vec![2, 1]);
    }

    #[test]
    fn another_day_shows_its_items_at_once() {
        let mut stage = Stage::new(items(1, &[(1, false)]));
        stage.set(items(2, &[(5, false), (6, false), (1, true)]));
        assert_eq!(height(&stage), 90.0);
        assert!(!stage.frame(Instant::now()));
    }

    #[test]
    fn an_item_back_before_it_had_gone_comes_back_in() {
        let mut stage = Stage::new(items(1, &[(1, false)]));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(items(1, &[(1, true)]));
        stage.frame(t0 + ms(1000));
        stage.frame(t0 + ms(1000) + LEAVE / 2);
        stage.set(items(1, &[(1, false)]));
        assert!(stage.frame(t0 + ms(1000) + LEAVE / 2 + ms(16)));
        assert!(!stage.frame(t0 + ms(3000)));
        assert_eq!(poses(&stage, &[1])[0], Pose::STILL);
    }

    #[test]
    fn an_item_arriving_in_a_folded_list_is_there_when_it_opens() {
        let folded = |open: bool, keys: &[(u64, bool)]| -> El<'_> {
            super::super::fold(open, items(1, keys)).into()
        };
        let mut stage = Stage::new(folded(false, &[(1, false)]));
        let t0 = Instant::now();
        stage.frame(t0);
        stage.set(folded(false, &[(1, false), (2, false)]));
        stage.frame(t0 + ms(1000));
        stage.frame(t0 + ms(1000) + ENTER);
        // Opened later: nothing left to play.
        stage.set(folded(true, &[(1, false), (2, false)]));
        let list = stage.tree.children[0].state.downcast_ref::<State>();
        assert_eq!(list.pose(2), Pose::STILL);
    }

    #[test]
    fn a_click_goes_only_to_the_item_drawn_under_it() {
        use iced::advanced::{Shell, clipboard};
        use iced::widget::checkbox;
        use iced::{Event, Point, Rectangle};
        // Checkboxes: each sends a message when pressed, even when another
        // widget has taken the click already.
        let pressable = |keys: &[u64]| -> El<'_> {
            let mut list = list().spacing(10.0).key(1);
            for &key in keys {
                let tick = checkbox(false).size(40).on_toggle(|_| ());
                list = list.push(Item::new(key, tick).from(1.0));
            }
            list.into()
        };
        let mut stage = Stage::new(pressable(&[2]));
        let t0 = Instant::now();
        stage.frame(t0);
        // 1 arrives above 2: it takes no room yet, but is laid out at its
        // full height over 2.
        stage.set(pressable(&[1, 2]));
        stage.frame(t0 + ms(1000));
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        stage.element.as_widget_mut().update(
            &mut stage.tree,
            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Layout::new(&stage.node),
            mouse::Cursor::Available(Point::new(20.0, 20.0)),
            &(),
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::with_size(Size::new(400.0, 300.0)),
        );
        assert_eq!(messages.len(), 1, "one click, one item");
    }
}
