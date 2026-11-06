use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Window, WindowBounds,
    WindowOptions,
};

// Placeholder palette until the brand concept from Claude design is picked.
const PAPER: u32 = 0xf4efe4;
const INK: u32 = 0x1c1b19;

struct Lagom;

impl Render for Lagom {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .justify_center()
            .items_center()
            .gap_2()
            .bg(rgb(PAPER))
            .text_color(rgb(INK))
            .child(div().text_3xl().child("lagom"))
            .child(div().text_sm().child("Not too much. Not too little. Just right."))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Lagom),
        )
        .expect("open main window");
        cx.activate(true);
    });
}
