use super::*;
use gpui::{Bounds, Element, GlobalElementId, Hitbox, InspectorElementId, LayoutId, Svg, TestAppContext};
use std::cell::Cell;

struct PaintCheck { glyph: Svg, ink: Rc<Cell<Option<gpui::Hsla>>> }

impl IntoElement for PaintCheck {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for PaintCheck {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;
    fn id(&self) -> Option<ElementId> { Element::id(&self.glyph) }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> { None }
    fn request_layout(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        self.glyph.request_layout(id, inspector, window, cx)
    }
    fn prepaint(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, bounds: Bounds<Pixels>, layout: &mut (), window: &mut Window, cx: &mut App) -> Option<Hitbox> {
        self.glyph.prepaint(id, inspector, bounds, layout, window, cx)
    }
    fn paint(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, bounds: Bounds<Pixels>, layout: &mut (), hitbox: &mut Option<Hitbox>, window: &mut Window, cx: &mut App) {
        let style = self.glyph.interactivity().compute_style(id, hitbox.as_ref(), window, cx);
        let color = style.text.color.expect("SVG paint requires its own color");
        assert!(color.a > 0., "icon-only button must not paint transparent ink");
        self.ink.set(Some(color));
        self.glyph.paint(id, inspector, bounds, layout, hitbox, window, cx);
    }
}

struct GlyphButton { action: bool, theme: Theme, fill: Option<Rgba>, path: &'static str, ink: Rc<Cell<Option<gpui::Hsla>>> }

struct ActionButton(&'static str);

impl Render for ActionButton {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(button(&Theme::light(DEFAULT_ACCENT), self.0, None))
    }
}

#[gpui::test]
fn action_buttons_render_their_own_icons(cx: &mut TestAppContext) {
    for label in ["Connect", "Connecting", "TLS", "STARTTLS", "None", "Add account", "Remove", "Keep", "Send", "Save event", "Saving…"] {
        let (_, cx) = cx.add_window_view(|_, _| ActionButton(label));
        cx.run_until_parked();
        let control = cx.debug_bounds(label).unwrap();
        let path = action_icon(label).expect("action has an icon");
        let glyph = cx.debug_bounds(format!("icon-{path}").leak()).expect("button renders its icon");
        assert_eq!(glyph.size, gpui::size(px(16.), px(16.)));
        assert!(control.contains(&glyph.center()), "{label}");
    }
}

impl Render for GlyphButton {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let control = if self.action { button(&self.theme, "Rename", self.fill) }
            else { icon_button(&self.theme, "fixture", self.path, "Fixture") };
        div().size_full().child(control.absolute().left(px(100.)).top(px(100.))
            .child(PaintCheck { glyph: action_glyph(&self.theme, self.path, self.fill), ink: self.ink.clone() }))
    }
}

#[gpui::test]
fn icon_only_button_svg_paints_with_visible_ink(cx: &mut TestAppContext) {
    for theme in [Theme::light(DEFAULT_ACCENT), Theme::dark(DEFAULT_ACCENT)] {
        for (action, fill) in [(false, None), (true, None), (true, Some(theme.accent))] {
            for path in ["icons/chevron-left.svg", "icons/calendar-panel.svg", "icons/reply-all.svg", "icons/cc.svg", "icons/trash.svg"] {
                let ink = Rc::new(Cell::new(None));
                let (_, cx) = cx.add_window_view(|_, _| GlyphButton { action, theme, fill, path, ink: ink.clone() });
                cx.run_until_parked();
                assert_eq!(ink.get(), Some(if fill.is_some() { theme.accent_ink } else { theme.ink2 }.into()));
                cx.simulate_event(MouseMoveEvent { position: gpui::point(px(108.), px(108.)), pressed_button: None, modifiers: Default::default() });
                cx.run_until_parked();
                assert_eq!(ink.get(), Some(if fill.is_some() { theme.accent_ink } else { theme.ink }.into()), "hover: {path}");
            }
        }
    }
}
