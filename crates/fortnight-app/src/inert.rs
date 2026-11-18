//! A widget that is only drawn: it gets no events, and the pointer goes
//! through it to what lies under it.
//!
//! For the card that follows the pointer while dragging. It sits on top of
//! everything in a `stack`, and iced 0.14's `Stack` hides the pointer from
//! the layers below any layer whose `mouse_interaction` isn't `None` (the
//! card's checkbox says `Pointer`), so the drop targets under it would lose
//! the pointer whenever it was over that checkbox.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::Tree;
use iced::advanced::{Widget, renderer};
use iced::{Element, Length, Rectangle, Size, mouse};

/// `content`, drawn as it is and otherwise left out.
pub fn inert<'a, M: 'a, T: 'a, R: renderer::Renderer + 'a>(
    content: impl Into<Element<'a, M, T, R>>,
) -> Element<'a, M, T, R> {
    Element::new(Inert {
        content: content.into(),
    })
}

struct Inert<'a, M, T, R> {
    content: Element<'a, M, T, R>,
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Inert<'_, M, T, R> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
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

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Nothing in it lights up under the pointer either.
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            mouse::Cursor::Unavailable,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        _: &Tree,
        _: Layout<'_>,
        _: mouse::Cursor,
        _: &Rectangle,
        _: &R,
    ) -> mouse::Interaction {
        mouse::Interaction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::Shell;
    use iced::advanced::clipboard;
    use iced::widget::{checkbox, container, mouse_area, space, stack};
    use iced::{Event, Point, Theme};

    /// A layer of checkboxes over a mouse area: the area still sees the
    /// pointer, and the click lands on it.
    #[test]
    fn the_pointer_goes_through() {
        let under = mouse_area(container(space()).width(100).height(100)).on_release(1);
        let over = inert(container(checkbox(false).on_toggle(|_| 2)).width(100));
        let mut el: Element<'_, i32, Theme, ()> = stack![under, over].into();
        let mut tree = Tree::new(&el);
        let limits = layout::Limits::new(Size::ZERO, Size::new(100.0, 100.0));
        let node = el.as_widget_mut().layout(&mut tree, &(), &limits);
        let at = mouse::Cursor::Available(Point::new(5.0, 5.0));
        assert_eq!(
            el.as_widget()
                .mouse_interaction(&tree, Layout::new(&node), at, &node.bounds(), &()),
            mouse::Interaction::None
        );
        let mut messages = Vec::new();
        let mut clip = clipboard::Null;
        for event in [
            Event::Mouse(mouse::Event::CursorMoved {
                position: Point::new(5.0, 5.0),
            }),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ] {
            let mut shell = Shell::new(&mut messages);
            el.as_widget_mut().update(
                &mut tree,
                &event,
                Layout::new(&node),
                at,
                &(),
                &mut clip,
                &mut shell,
                &node.bounds(),
            );
        }
        assert_eq!(messages, vec![1]);
    }
}
