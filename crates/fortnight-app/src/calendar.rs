//! A month calendar date picker and the pop-up that shows it over the
//! page. Used by the task form's due date and the top bar's date.

use fortnight_core::report::short_weekday;
use fortnight_core::week::{month_label, week_containing};
use iced::widget::{Column, Row, Space, button, column, container, row, text};
use iced::{Center, Color, Element, Fill, Theme, border};
use jiff::ToSpan;
use jiff::civil::{Date, Weekday};

use crate::theme;

pub use popup::Popup;

/// A month calendar, weeks starting on `week_start`, as a raised panel. `selected` is filled
/// with the accent; the days of `range` (first, last) get a soft accent
/// band; `today` is shaded and in the accent colour so it is easy to find.
/// The arrows send `on_month(-1 | 1)`, a day sends `on_pick`, and
/// `on_clear` (if any, and a date is selected) adds a Clear button.
#[allow(clippy::too_many_arguments)]
pub fn month_picker<'a, Message: Clone + 'a>(
    month: Date,
    selected: Option<Date>,
    range: Option<(Date, Date)>,
    today: Date,
    week_start: Weekday,
    on_month: impl Fn(i32) -> Message,
    on_pick: impl Fn(Date) -> Message,
    on_clear: Option<Message>,
) -> Element<'a, Message> {
    const CELL: f32 = 36.0;
    let first = month.first_of_month();
    let start = week_containing(first, week_start).start;

    let arrow = |icon, step| {
        button(theme::icon(icon))
            .padding(theme::ICON_PADDING)
            .style(theme::icon_button)
            .on_press(on_month(step))
    };
    let header = row![
        arrow(theme::Icon::ChevronLeft, -1),
        text(month_label(first))
            .size(theme::TEXT_BODY)
            .width(Fill)
            .align_x(Center),
        arrow(theme::Icon::ChevronRight, 1),
    ]
    .align_y(Center)
    .width(CELL * 7.0);

    // "Mo", "Tu", …: the first two letters of the short name.
    let names = Row::with_children((0..7).map(|i| {
        text(&short_weekday(week_start.wrapping_add(i))[..2])
            .size(theme::TEXT_SMALL)
            .style(theme::muted)
            .width(CELL)
            .align_x(Center)
            .into()
    }));

    let mut weeks = Column::new().spacing(2);
    for w in 0..6i64 {
        let days = (0..7i64).map(|d| {
            let date = start.saturating_add((w * 7 + d).days());
            let in_month = date.month() == first.month();
            let is_selected = selected == Some(date);
            let is_today = date == today;
            let in_range = range.is_some_and(|(a, b)| a <= date && date <= b);
            button(
                text(date.day().to_string())
                    .size(theme::TEXT_SMALL)
                    .width(Fill)
                    .align_x(Center),
            )
            .width(CELL)
            .padding([7, 0])
            .style(move |theme: &Theme, status| {
                calendar_day(theme, status, in_month, is_selected, in_range, is_today)
            })
            .on_press(on_pick(date))
            .into()
        });
        weeks = weeks.push(Row::with_children(days).spacing(0));
    }

    let mut col = column![header, names, weeks].spacing(6);
    if let Some(clear) = on_clear.filter(|_| selected.is_some()) {
        col = col.push(
            row![
                Space::new().width(Fill),
                button(text("Clear").size(theme::TEXT_SMALL))
                    .padding([4, 10])
                    .style(theme::text_button)
                    .on_press(clear),
            ]
            .width(CELL * 7.0),
        );
    }
    panel(col.into())
}

/// The twelve months of `year` in a 4 × 3 grid, as a raised panel like
/// [`month_picker`]. `selected`'s month is filled with the accent; today's
/// month is shaded and in the accent colour. The arrows send
/// `on_year(-1 | 1)`; a month sends `on_pick` with its first day.
pub fn year_picker<'a, Message: Clone + 'a>(
    year: i16,
    selected: Date,
    today: Date,
    on_year: impl Fn(i32) -> Message,
    on_pick: impl Fn(Date) -> Message,
) -> Element<'a, Message> {
    const CELL: f32 = 72.0;
    let arrow = |icon, step| {
        button(theme::icon(icon))
            .padding(theme::ICON_PADDING)
            .style(theme::icon_button)
            .on_press(on_year(step))
    };
    let header = row![
        arrow(theme::Icon::ChevronLeft, -1),
        text(year.to_string())
            .size(theme::TEXT_BODY)
            .width(Fill)
            .align_x(Center),
        arrow(theme::Icon::ChevronRight, 1),
    ]
    .align_y(Center)
    .width(CELL * 3.0);

    let mut grid = Column::new().spacing(4);
    for r in 0..4i8 {
        let months = (1..=3i8).map(|c| {
            let month = r * 3 + c;
            let first = Date::new(year, month, 1).unwrap_or(selected);
            let is_selected = selected.year() == year && selected.month() == month;
            let is_today = today.year() == year && today.month() == month;
            button(
                text(first.strftime("%b").to_string())
                    .size(theme::TEXT_SMALL)
                    .width(Fill)
                    .align_x(Center),
            )
            .width(CELL)
            .padding([10, 0])
            .style(move |theme: &Theme, status| {
                calendar_day(theme, status, true, is_selected, false, is_today)
            })
            .on_press(on_pick(first))
            .into()
        });
        grid = grid.push(Row::with_children(months).spacing(0));
    }
    panel(column![header, grid].spacing(8).into())
}

/// The raised panel the pickers sit in.
fn panel<'a, Message: 'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    container(content)
        .padding(theme::SPACING)
        .style(|theme: &Theme| container::Style {
            background: Some(theme.palette().background.into()),
            border: border::rounded(theme::RADIUS)
                .width(1)
                .color(theme::muted_color(theme).scale_alpha(0.25)),
            shadow: iced::Shadow {
                color: Color::BLACK.scale_alpha(0.3),
                offset: iced::Vector::new(0.0, 6.0),
                blur_radius: 18.0,
            },
            ..container::Style::default()
        })
        .into()
}

fn calendar_day(
    theme: &Theme,
    status: button::Status,
    in_month: bool,
    selected: bool,
    in_range: bool,
    today: bool,
) -> button::Style {
    let p = theme.palette();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let (background, text_color) = if selected {
        (Some(theme::accent_color().into()), Color::WHITE)
    } else if today {
        let shade = if hovered || in_range { 0.32 } else { 0.18 };
        (
            Some(theme::accent_color().scale_alpha(shade).into()),
            theme::accent_color(),
        )
    } else {
        let color = if in_month || in_range {
            p.text
        } else {
            theme::muted_color(theme).scale_alpha(0.6)
        };
        let fill = if in_range {
            Some(
                theme::accent_color()
                    .scale_alpha(if hovered { 0.24 } else { 0.14 })
                    .into(),
            )
        } else {
            hovered.then(|| p.text.scale_alpha(0.08).into())
        };
        (fill, color)
    };
    button::Style {
        background,
        text_color,
        border: border::rounded(6),
        ..button::Style::default()
    }
}

/// A widget that shows a pop-up under its anchor, on the overlay layer, so
/// it covers what is below instead of pushing it down. A click outside both
/// the pop-up and the anchor sends `on_dismiss`.
pub mod popup {
    use iced::advanced::layout::{self, Layout};
    use iced::advanced::overlay;
    use iced::advanced::renderer;
    use iced::advanced::widget::{self, Tree, Widget};
    use iced::advanced::{Clipboard, Shell};
    use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

    /// The space between the anchor and the pop-up.
    const GAP: f32 = 6.0;

    pub struct Popup<'a, Message> {
        anchor: Element<'a, Message>,
        /// The pop-up, or a placeholder while closed (so the tree keeps two
        /// children).
        content: Element<'a, Message>,
        open: bool,
        on_dismiss: Message,
        /// Centre the pop-up under the anchor instead of aligning their
        /// left edges.
        centered: bool,
    }

    impl<'a, Message: 'a> Popup<'a, Message> {
        /// Centres the pop-up under the anchor.
        pub fn centered(mut self) -> Self {
            self.centered = true;
            self
        }

        pub fn new(
            anchor: impl Into<Element<'a, Message>>,
            content: Option<Element<'a, Message>>,
            on_dismiss: Message,
        ) -> Self {
            let open = content.is_some();
            Popup {
                anchor: anchor.into(),
                content: content.unwrap_or_else(|| iced::widget::space().into()),
                open,
                on_dismiss,
                centered: false,
            }
        }
    }

    impl<Message: Clone> Widget<Message, Theme, Renderer> for Popup<'_, Message> {
        fn children(&self) -> Vec<Tree> {
            vec![Tree::new(&self.anchor), Tree::new(&self.content)]
        }

        fn diff(&self, tree: &mut Tree) {
            tree.diff_children(&[&self.anchor, &self.content]);
        }

        fn size(&self) -> Size<Length> {
            self.anchor.as_widget().size()
        }

        fn layout(
            &mut self,
            tree: &mut Tree,
            renderer: &Renderer,
            limits: &layout::Limits,
        ) -> layout::Node {
            self.anchor
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits)
        }

        fn update(
            &mut self,
            tree: &mut Tree,
            event: &Event,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            renderer: &Renderer,
            clipboard: &mut dyn Clipboard,
            shell: &mut Shell<'_, Message>,
            viewport: &Rectangle,
        ) {
            self.anchor.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }

        fn draw(
            &self,
            tree: &Tree,
            renderer: &mut Renderer,
            theme: &Theme,
            style: &renderer::Style,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
        ) {
            self.anchor.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        }

        fn mouse_interaction(
            &self,
            tree: &Tree,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
            renderer: &Renderer,
        ) -> mouse::Interaction {
            self.anchor.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        }

        fn operate(
            &mut self,
            tree: &mut Tree,
            layout: Layout<'_>,
            renderer: &Renderer,
            operation: &mut dyn widget::Operation,
        ) {
            self.anchor
                .as_widget_mut()
                .operate(&mut tree.children[0], layout, renderer, operation);
        }

        fn overlay<'b>(
            &'b mut self,
            tree: &'b mut Tree,
            layout: Layout<'b>,
            renderer: &Renderer,
            viewport: &Rectangle,
            translation: Vector,
        ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
            let mut children = tree.children.iter_mut();
            let anchor_tree = children.next()?;
            let content_tree = children.next()?;
            let base = self.anchor.as_widget_mut().overlay(
                anchor_tree,
                layout,
                renderer,
                viewport,
                translation,
            );
            if !self.open {
                return base;
            }
            let anchor = layout.bounds() + translation;
            let popup = overlay::Element::new(Box::new(Overlay {
                content: &mut self.content,
                tree: content_tree,
                anchor,
                on_dismiss: self.on_dismiss.clone(),
                centered: self.centered,
            }));
            Some(overlay::Group::with_children(base.into_iter().chain([popup]).collect()).overlay())
        }
    }

    struct Overlay<'a, 'b, Message> {
        content: &'b mut Element<'a, Message>,
        tree: &'b mut Tree,
        /// The anchor's bounds, in window coordinates.
        anchor: Rectangle,
        on_dismiss: Message,
        centered: bool,
    }

    impl<Message: Clone> overlay::Overlay<Message, Theme, Renderer> for Overlay<'_, '_, Message> {
        fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
            let limits = layout::Limits::new(Size::ZERO, bounds);
            let node = self
                .content
                .as_widget_mut()
                .layout(self.tree, renderer, &limits);
            let size = node.size();
            // Below the anchor, kept inside the window; above it when there
            // is no room below.
            let left = if self.centered {
                self.anchor.center_x() - size.width / 2.0
            } else {
                self.anchor.x
            };
            let x = left.min(bounds.width - size.width).max(0.0);
            let below = self.anchor.y + self.anchor.height + GAP;
            let y = if below + size.height <= bounds.height {
                below
            } else {
                (self.anchor.y - GAP - size.height).max(0.0)
            };
            node.move_to(Point::new(x, y))
        }

        fn draw(
            &self,
            renderer: &mut Renderer,
            theme: &Theme,
            style: &renderer::Style,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
        ) {
            self.content.as_widget().draw(
                self.tree,
                renderer,
                theme,
                style,
                layout,
                cursor,
                &layout.bounds(),
            );
        }

        fn update(
            &mut self,
            event: &Event,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            renderer: &Renderer,
            clipboard: &mut dyn Clipboard,
            shell: &mut Shell<'_, Message>,
        ) {
            self.content.as_widget_mut().update(
                self.tree,
                event,
                layout,
                cursor,
                renderer,
                clipboard,
                shell,
                &layout.bounds(),
            );
            if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event {
                if cursor.is_over(layout.bounds()) {
                    // Nothing under the pop-up gets the click.
                    shell.capture_event();
                } else if !cursor.is_over(self.anchor) {
                    shell.publish(self.on_dismiss.clone());
                }
            }
        }

        fn mouse_interaction(
            &self,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            renderer: &Renderer,
        ) -> mouse::Interaction {
            self.content.as_widget().mouse_interaction(
                self.tree,
                layout,
                cursor,
                &layout.bounds(),
                renderer,
            )
        }
    }

    impl<'a, Message: Clone + 'a> From<Popup<'a, Message>> for Element<'a, Message> {
        fn from(popup: Popup<'a, Message>) -> Self {
            Element::new(popup)
        }
    }
}
