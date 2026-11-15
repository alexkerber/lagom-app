//! HTML mail drawn by GPUI, where no web view runs: on Linux wry's child web view needs GTK's
//! main loop and X11, and GPUI runs neither. This works the same on X11 and Wayland.
//!
//! The core turns the mail into paragraphs of bold, italic and linked text and rows of images
//! (`html_to_blocks`), from the same pass that hides hidden text in previews. No script, style
//! sheet, font or frame is ever read. Images follow the web view's CSP: `data:` and the
//! message's own `cid:` parts always, the web only after "Load remote images". Links open in
//! the system browser, only for the schemes `link_allowed` passes.

use std::{collections::HashMap, sync::{Arc, Mutex}};

use gpui::{div, img, prelude::*, px, AnyElement, App, FontStyle, FontWeight, HighlightStyle, InteractiveText, StyledText, UnderlineStyle, Window};
use lagom_core::{html::{data_image, image_type, link_allowed, remote_image}, mime::{Block, Image, Run}};

use crate::{mail::Mail, theme::Theme};

pub struct Rich {
    blocks: Vec<Block>,
    /// The mail has images from the web, which "Load remote images" fetches.
    pub has_remote: bool,
    /// Decoded `data:` and `cid:` images by `src`, `None` when they do not decode. A part never
    /// changes once it has arrived; a part not yet there is not kept.
    local: Mutex<HashMap<String, Option<Arc<gpui::Image>>>>,
}

impl Rich {
    /// Off the UI thread: the parse is linear but reads the whole mail.
    pub fn new(html: &str) -> Self {
        let blocks = lagom_core::mime::html_to_blocks(html);
        let has_remote = blocks.iter().any(|b| matches!(b, Block::Images(row) if row.iter().any(|i| remote_image(&i.src).is_some())));
        Self { blocks, has_remote, local: Default::default() }
    }

    /// The body: one element per paragraph or image row.
    pub fn view(&self, m: &Mail, t: &Theme) -> Vec<AnyElement> {
        let mut images = 0;
        self.blocks.iter().enumerate().map(|(ix, block)| match block {
            Block::Text(runs) => paragraph(ix, runs, t),
            Block::Images(row) => div().flex().flex_wrap().items_end().gap(px(4.))
                .children(row.iter().filter_map(|image| { images += 1; self.image(images, image, m) }))
                .into_any_element(),
        }).collect()
    }

    fn image(&self, ix: usize, image: &Image, m: &Mail) -> Option<AnyElement> {
        let source: gpui::ImageSource = match self.local(&image.src, m) {
            Some(local) => local.into(),
            None => {
                let url = remote_image(&image.src).filter(|_| m.remote)?;
                (move |window: &mut Window, cx: &mut App| window.use_asset::<Remote>(&url, cx).flatten()?.use_render_image(window, cx).map(Ok)).into()
            }
        };
        let picture = img(source).max_w_full().when_some(image.width, |i, w| i.w(px(w)));
        Some(match image.link.clone().filter(|url| link_allowed(url)) {
            Some(url) => div().id(("rich-image", ix)).max_w_full().cursor_pointer().child(picture)
                .on_click(move |_, _, cx| cx.open_url(&url)).into_any_element(),
            None => picture.into_any_element(),
        })
    }

    /// A `data:` image, or a part of this message.
    fn local(&self, src: &str, m: &Mail) -> Option<Arc<gpui::Image>> {
        if let Some(image) = self.local.lock().unwrap().get(src) { return image.clone(); }
        let bytes = if src.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("data:")) { data_image(src) }
            else { Some(lagom_core::inline::find(&m.inline, src)?.data.clone()) };
        let image = bytes.and_then(|bytes| Some(Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::from_mime_type(image_type(&bytes)?)?, bytes))));
        self.local.lock().unwrap().insert(src.into(), image.clone());
        image
    }
}

/// A paragraph as one text element; links are accent coloured, underlined and clickable.
fn paragraph(ix: usize, runs: &[Run], t: &Theme) -> AnyElement {
    let (mut text, mut highlights, mut links) = (String::new(), Vec::new(), Vec::new());
    for run in runs {
        let range = text.len()..text.len() + run.text.len();
        text.push_str(&run.text);
        let link = run.link.clone().filter(|url| link_allowed(url));
        let style = HighlightStyle {
            font_weight: run.bold.then_some(FontWeight::BOLD),
            font_style: run.italic.then_some(FontStyle::Italic),
            color: link.is_some().then(|| t.accent.into()),
            underline: link.is_some().then(|| UnderlineStyle { thickness: px(1.), color: Some(t.accent.into()), wavy: false }),
            ..Default::default()
        };
        if style != HighlightStyle::default() { highlights.push((range.clone(), style)); }
        if let Some(url) = link { links.push((range, url)); }
    }
    let text = StyledText::new(text).with_highlights(highlights);
    if links.is_empty() { return div().child(text).into_any_element(); }
    let (ranges, urls): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    div().child(InteractiveText::new(("rich-text", ix), text).on_click(ranges, move |i, _, cx| cx.open_url(&urls[i]))).into_any_element()
}

/// A remote image, fetched once per run and URL. A failed fetch stays failed until restart.
struct Remote;

impl gpui::Asset for Remote {
    type Source = String;
    type Output = Option<Arc<gpui::Image>>;

    fn load(url: String, _: &mut App) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let fetch = runtime().spawn(async move { lagom_core::html::fetch_image(&url).await });
        async move {
            let (kind, bytes) = fetch.await.ok().flatten()?;
            Some(Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::from_mime_type(kind)?, bytes)))
        }
    }
}

/// One thread with a Tokio runtime for image fetches: GPUI's executors have no Tokio reactor.
fn runtime() -> &'static tokio::runtime::Handle {
    static HANDLE: std::sync::OnceLock<tokio::runtime::Handle> = std::sync::OnceLock::new();
    HANDLE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("lagom-images".into()).spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("image runtime");
            tx.send(rt.handle().clone()).ok();
            rt.block_on(std::future::pending::<()>());
        }).expect("image thread");
        rx.recv().expect("image runtime")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DEFAULT_ACCENT;

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

    #[test]
    fn remote_images_wait_for_the_button() {
        let html = format!(r#"<p>Hi <a href="https://x.example">there</a></p><img src="data:image/png;base64,{PNG}"><img src="cid:logo"><img src="https://t.example/p.png"><img src="file:///etc/passwd">"#);
        let rich = Rich::new(&html);
        assert!(rich.has_remote);
        let mut m = Mail { inline: Arc::new(vec![lagom_core::inline::InlinePart { cid: "logo".into(), mime_type: "image/png".into(), data: data_image(&format!("data:image/png;base64,{PNG}")).unwrap() }]), ..Default::default() };
        let Block::Images(row) = &rich.blocks[1] else { panic!("{:?}", rich.blocks) };
        let shown = |m: &Mail| row.iter().enumerate().filter(|(ix, image)| rich.image(*ix, image, m).is_some()).count();
        assert_eq!(shown(&m), 2, "data and cid only");
        m.remote = true;
        assert_eq!(shown(&m), 3, "the web after Load remote images, never a file");
        assert_eq!(rich.local.lock().unwrap().len(), 2);
        // Data that is no image is decoded once, not on every frame.
        let broken = Rich::new("<img src='data:image/png;base64,AAAA'>");
        let Block::Images(row) = &broken.blocks[0] else { panic!() };
        assert!(broken.image(0, &row[0], &m).is_none());
        assert!(broken.local.lock().unwrap().values().all(Option::is_none) && broken.local.lock().unwrap().len() == 1);
        assert!(!Rich::new("<p>text</p><img src=cid:x>").has_remote);
    }

    #[gpui::test]
    fn rich_mail_draws(cx: &mut gpui::TestAppContext) {
        let theme = Theme::light(DEFAULT_ACCENT);
        let rich = Arc::new(Rich::new(&format!(r#"<h1>Sale</h1><p>Read <a href="https://x.example">more</a> or <a href="javascript:alert(1)">not</a>.</p><a href="https://x.example"><img src="data:image/png;base64,{PNG}" width=40></a>"#)));
        let m = Mail { rich: Some(rich), ..Default::default() };
        let window = cx.add_window(|_, _| Probe(m, theme));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear()).unwrap();
    }

    struct Probe(Mail, Theme);

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div().size_full().children(self.0.rich.clone().unwrap().view(&self.0, &self.1))
        }
    }
}
