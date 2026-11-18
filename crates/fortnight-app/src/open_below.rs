//! Drop-down menus that open below their field when they fit there.
//!
//! iced's pick list and combo box open their menu on whichever side of the
//! field has more room, so any field in the lower half of the window opens
//! upwards even when the menu would fit below. [`open_below`] wraps such a
//! field (it is otherwise invisible) and moves the menu under the field
//! whenever it fits, leaving iced's choice only for menus that don't.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

/// `content` (a pick list or combo box, possibly inside a stack) with its
/// menu opening below it whenever the menu fits there.
pub fn open_below<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Element::new(OpenBelow {
        content: content.into(),
    })
}

struct OpenBelow<'a, M> {
    content: Element<'a, M>,
}

impl<M> Widget<M, Theme, Renderer> for OpenBelow<'_, M> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
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
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
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
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        // Where the field is on screen, as iced's menu works it out.
        let field_bottom = layout.position().y + translation.y + layout.bounds().height;
        let menu =
            self.content
                .as_widget_mut()
                .overlay(tree, layout, renderer, viewport, translation)?;
        Some(overlay::Element::new(Box::new(Below {
            menu,
            field_bottom,
        })))
    }
}

/// The field's menu, moved under the field when it fits there.
struct Below<'a, M, T, R> {
    menu: overlay::Element<'a, M, T, R>,
    /// The field's bottom edge, in window coordinates.
    field_bottom: f32,
}

impl<M, T, R: renderer::Renderer> overlay::Overlay<M, T, R> for Below<'_, M, T, R> {
    fn layout(&mut self, renderer: &R, bounds: Size) -> layout::Node {
        let node = self.menu.as_overlay_mut().layout(renderer, bounds);
        let placed = node.bounds();
        let room_below = bounds.height - self.field_bottom;
        // iced put it above (it had more room there); when its whole height
        // fits below too, it goes below instead.
        if placed.y < self.field_bottom && placed.height <= room_below {
            node.move_to(Point::new(placed.x, self.field_bottom))
        } else {
            node
        }
    }

    fn draw(
        &self,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.menu
            .as_overlay()
            .draw(renderer, theme, style, layout, cursor);
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &R, operation: &mut dyn Operation) {
        self.menu
            .as_overlay_mut()
            .operate(layout, renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
    ) {
        self.menu
            .as_overlay_mut()
            .update(event, layout, cursor, renderer, clipboard, shell);
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
    ) -> mouse::Interaction {
        self.menu
            .as_overlay()
            .mouse_interaction(layout, cursor, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        layout: Layout<'b>,
        renderer: &R,
    ) -> Option<overlay::Element<'b, M, T, R>> {
        self.menu.as_overlay_mut().overlay(layout, renderer)
    }

    fn index(&self) -> f32 {
        self.menu.as_overlay().index()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for iced's menu: a fixed rectangle, as iced placed it.
    struct Placed(Rectangle);

    impl overlay::Overlay<(), (), ()> for Placed {
        fn layout(&mut self, _: &(), _: Size) -> layout::Node {
            layout::Node::new(self.0.size()).move_to(self.0.position())
        }

        fn draw(&self, _: &mut (), _: &(), _: &renderer::Style, _: Layout<'_>, _: mouse::Cursor) {}
    }

    /// Where the menu ends up for a field whose bottom edge is at
    /// `field_bottom`, in an 800 px tall window, when iced put it at `menu`.
    fn placed(field_bottom: f32, menu: Rectangle) -> Point {
        let mut below = Below {
            menu: overlay::Element::new(Box::new(Placed(menu))),
            field_bottom,
        };
        overlay::Overlay::layout(&mut below, &(), Size::new(1000.0, 800.0))
            .bounds()
            .position()
    }

    fn rect(y: f32, height: f32) -> Rectangle {
        Rectangle::new(Point::new(40.0, y), Size::new(200.0, height))
    }

    #[test]
    fn a_menu_that_fits_below_goes_below_even_when_iced_chose_above() {
        // Field 500–540: 260 px below, 500 above; a 120 px menu fits below.
        assert_eq!(placed(540.0, rect(380.0, 120.0)), Point::new(40.0, 540.0));
    }

    #[test]
    fn a_menu_already_below_stays_put() {
        assert_eq!(placed(200.0, rect(200.0, 120.0)), Point::new(40.0, 200.0));
    }

    #[test]
    fn a_menu_too_tall_for_below_stays_above() {
        // 260 px below, a 300 px menu: iced's choice (above) stands.
        assert_eq!(placed(540.0, rect(200.0, 300.0)), Point::new(40.0, 200.0));
    }
}
