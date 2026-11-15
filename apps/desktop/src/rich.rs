//! HTML mail drawn by GPUI, where no web view runs: on Linux wry's child web view needs GTK's
//! main loop and X11, and GPUI runs neither. This works the same on X11 and Wayland.
//!
//! The core turns the mail into paragraphs of bold, italic and linked text and rows of images
//! (`html_to_blocks`), from the same pass that hides hidden text in previews. No script, style
//! sheet, font or frame is ever read. Images follow the web view's CSP: `data:` and the
//! message's own `cid:` parts always, the web only after "Load remote images". Links open in
//! the system browser, only for the schemes `link_allowed` passes.
//!
//! lagom decodes images itself, off the UI thread and within limits, never through GPUI's
//! decoder: that one has no size limits, so a small GIF or PNG can ask for gigabytes.

use std::{collections::HashMap, sync::{Arc, Mutex}};

use gpui::{div, img, prelude::*, px, AnyElement, App, FontStyle, FontWeight, HighlightStyle, InteractiveText, RenderImage, StyledText, UnderlineStyle, Window};
use lagom_core::{html::{data_image, image_type, link_allowed, remote_image}, inline::InlinePart, mime::{Block, Image, Run}};

use crate::{mail::Mail, main_window::Tip, theme::Theme};

/// Pixels a decoded image may take as RGBA: 32 MiB, about 2900 × 2900.
const DECODE_BYTES: u64 = 32 << 20;
/// What a mail's images may take after downscaling, and fetch from the web.
const DECODED_BUDGET: usize = 128 << 20;
const REMOTE_BUDGET: usize = 25 << 20;

pub struct Rich {
    blocks: Vec<Block>,
    /// The mail has images from the web, which "Load remote images" fetches.
    pub has_remote: bool,
    images: Images,
}

/// An image of this mail, by what it is: a part by its content ID (however `src` spells it),
/// a `data:` image by its place in the mail, a remote one by URL.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Key { Part(String), Data(usize), Web(String) }

#[derive(Clone)]
enum Source { Part(Arc<Vec<InlinePart>>, String), Data(Arc<str>), Web(String) }

/// A decoded image, with the windows that drew it: each frees its own texture.
enum Slot { Loading, Done(Option<Arc<RenderImage>>, Vec<gpui::WindowId>) }

/// The images of a mail while it is on screen. `forget` starts a new generation: a load that
/// finishes for an older one is dropped, never drawn.
struct Store { slots: HashMap<Key, Slot>, budget: Arc<Budget>, generation: u64 }

type Images = Arc<Mutex<Store>>;

/// What is left of this mail's budgets. Remote fetches hold `remote` while they run: one at a
/// time per mail, so the budget is exact and no image fails for bytes another one reserved.
struct Budget { decoded: Mutex<usize>, remote: tokio::sync::Mutex<usize> }

/// Decodes at a time, across all mail: each may hold up to `DECODE_BYTES`.
static DECODES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

impl Budget {
    fn new() -> Arc<Self> {
        Arc::new(Budget { decoded: DECODED_BUDGET.into(), remote: tokio::sync::Mutex::const_new(REMOTE_BUDGET) })
    }
}

thread_local! {
    /// Textures of mail no window shows any more, with the windows that still hold them.
    /// `Drop` and `forget` have no window, so each window frees its own in its next frame.
    static RELEASED: std::cell::RefCell<Vec<(Arc<RenderImage>, Vec<gpui::WindowId>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Empties the store into `RELEASED`, with fresh budgets.
fn retire(store: &mut Store) {
    let slots = std::mem::take(&mut store.slots);
    (store.budget, store.generation) = (Budget::new(), store.generation + 1);
    RELEASED.with_borrow_mut(|released| released.extend(slots.into_values().filter_map(|slot| match slot {
        Slot::Done(Some(image), windows) if !windows.is_empty() => Some((image, windows)),
        _ => None,
    })));
}

impl Drop for Rich {
    fn drop(&mut self) { retire(&mut self.images.lock().unwrap()); }
}

/// Frees this window's textures of mail it no longer shows. Each window calls it while it
/// builds a new frame, so no window loses a texture its last frame still draws; another
/// window's atlas is never touched. Windows that closed are dropped from the list.
pub fn release(window: &mut Window, cx: &App) {
    let id = window.window_handle().window_id();
    let open: Vec<_> = cx.windows().iter().map(|w| w.window_id()).collect();
    RELEASED.with_borrow_mut(|released| released.retain_mut(|(image, windows)| {
        windows.retain(|w| open.contains(w));
        if let Some(at) = windows.iter().position(|w| *w == id) {
            windows.swap_remove(at);
            window.drop_image(image.clone()).ok();
        }
        !windows.is_empty()
    }));
}

impl Rich {
    /// Off the UI thread: the parse is linear but reads the whole mail.
    pub fn new(html: &str) -> Self {
        let blocks = lagom_core::mime::html_to_blocks(html);
        let has_remote = blocks.iter().any(|b| matches!(b, Block::Images(row) if row.iter().any(|i| remote_image(&i.src).is_some())));
        Self { blocks, has_remote, images: Arc::new(Mutex::new(Store { slots: HashMap::new(), budget: Budget::new(), generation: 0 })) }
    }

    /// Drops the decoded images of a mail that is no longer on screen; they load again when it
    /// is. Their textures go when each window that drew them draws its next frame.
    pub fn forget(&self) { retire(&mut self.images.lock().unwrap()); }

    #[cfg(test)]
    pub fn decoded(&self) -> usize { self.images.lock().unwrap().slots.len() }

    /// The body: one element per paragraph or image row.
    pub fn view(&self, m: &Mail, t: &Theme) -> Vec<AnyElement> {
        let mut images = 0;
        self.blocks.iter().enumerate().map(|(ix, block)| match block {
            Block::Text(runs) => paragraph(ix, runs, t),
            Block::Images(row) => div().flex().flex_wrap().items_end().gap(px(4.))
                .children(row.iter().filter_map(|image| { images += 1; self.image(images, image, m, t) }))
                .into_any_element(),
        }).collect()
    }

    /// What `src` loads under the CSP, if anything yet: a part that has not arrived is nothing.
    fn source(ix: usize, src: &Arc<str>, m: &Mail) -> Option<(Key, Source)> {
        if src.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("data:")) {
            return Some((Key::Data(ix), Source::Data(src.clone())));
        }
        if let Some(part) = lagom_core::inline::find(&m.inline, src) {
            return Some((Key::Part(part.cid.clone()), Source::Part(m.inline.clone(), part.cid.clone())));
        }
        let url = remote_image(src).filter(|_| m.remote)?;
        Some((Key::Web(url.clone()), Source::Web(url)))
    }

    fn image(&self, ix: usize, image: &Image, m: &Mail, t: &Theme) -> Option<AnyElement> {
        let (key, source) = Self::source(ix, &image.src, m)?;
        let picture = match self.images.lock().unwrap().slots.get(&key) {
            Some(Slot::Done(None, _)) => div().px(px(8.)).py(px(4.)).border_1().border_color(t.line).rounded(px(4.))
                .text_color(t.ink2).child("Image not shown").into_any_element(),
            _ => {
                let images = self.images.clone();
                img(move |window: &mut Window, cx: &mut App| load(&images, &key, &source, window, cx).map(Ok))
                    .max_w_full().when_some(image.width, |i, w| i.w(px(w))).into_any_element()
            }
        };
        Some(match image.link.clone().filter(|url| link_allowed(url)) {
            Some(url) => {
                let (tip, theme) = (gpui::SharedString::from(url.to_string()), *t);
                div().id(("rich-image", ix)).max_w_full().cursor_pointer().child(picture)
                    .tooltip(move |_, cx| cx.new(|_| Tip(theme, tip.clone())).into())
                    .on_click(move |_, _, cx| cx.open_url(&url)).into_any_element()
            }
            None => picture,
        })
    }
}

/// The decoded image, or `None` while it loads. The first call starts the load off the UI
/// thread and redraws the view when it is done.
fn load(images: &Images, key: &Key, source: &Source, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>> {
    let (budget, generation) = {
        let mut store = images.lock().unwrap();
        let id = window.window_handle().window_id();
        match store.slots.get_mut(key) {
            Some(Slot::Done(image, windows)) => {
                if image.is_some() && !windows.contains(&id) { windows.push(id); }
                return image.clone();
            }
            Some(Slot::Loading) => return None,
            None => { store.slots.insert(key.clone(), Slot::Loading); }
        }
        (store.budget.clone(), store.generation)
    };
    let task = cx.background_executor().spawn(fetch_and_decode(source.clone(), budget));
    let (images, key, view) = (images.clone(), key.clone(), window.current_view());
    window.spawn(cx, async move |cx| {
        let image = task.await;
        let mut store = images.lock().unwrap();
        if store.generation != generation { return; }
        store.slots.insert(key, Slot::Done(image, Vec::new()));
        drop(store);
        cx.update(|_, cx| cx.notify(view)).ok();
    }).detach();
    None
}

async fn fetch_and_decode(source: Source, budget: Arc<Budget>) -> Option<Arc<RenderImage>> {
    let bytes = match source {
        Source::Data(src) => data_image(&src)?,
        Source::Part(parts, cid) => {
            let _permit = DECODES.acquire().await.ok()?;
            return decode(&parts.iter().find(|p| p.cid == cid)?.data, &budget.decoded);
        }
        Source::Web(url) => {
            let mut left = budget.remote.lock().await;
            if *left == 0 { return None; }
            let limit = *left;
            let (used, fetched) = runtime().spawn(async move {
                let mut used = 0;
                let fetched = lagom_core::html::fetch_image(&url, limit, &mut used).await;
                (used, fetched)
            }).await.ok()?;
            *left -= used.min(*left);
            fetched?.1
        }
    };
    let _permit = DECODES.acquire().await.ok()?;
    decode(&bytes, &budget.decoded)
}

/// Raster image bytes as a GPUI image: at most 8192 pixels a side and `DECODE_BYTES` as RGBA,
/// checked by a decoder with the same limits before any pixels are read; then at most
/// 1600 × 4096, under every GPU's texture limit. The size after that comes off `budget` before
/// decoding. A GIF shows its first frame only. WebP is not drawn: its decoder allocates by the
/// frame's own size, which may differ from the size it reports.
fn decode(bytes: &[u8], budget: &Mutex<usize>) -> Option<Arc<RenderImage>> {
    if !matches!(image_type(bytes)?, "image/png" | "image/jpeg" | "image/gif" | "image/bmp") { return None; }
    let reader = || {
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
        let mut limits = image::Limits::default();
        (limits.max_image_width, limits.max_image_height, limits.max_alloc) = (Some(8192), Some(8192), Some(DECODE_BYTES));
        reader.limits(limits);
        Some(reader)
    };
    let (width, height) = reader()?.into_dimensions().ok()?;
    if width == 0 || height == 0 || width > 8192 || height > 8192 || u64::from(width) * u64::from(height) * 4 > DECODE_BYTES { return None; }
    // At most this much stays; the rest goes back once the real size is known.
    let most = (width.min(1600) as usize) * (height.min(4096) as usize) * 4;
    {
        let mut left = budget.lock().unwrap();
        if *left < most { return None; }
        *left -= most;
    }
    let image = (|| {
        let image = reader()?.decode().ok()?;
        let image = if image.width() > 1600 || image.height() > 4096 { image.thumbnail(1600, 4096) } else { image };
        if image.width() == 0 || image.height() == 0 { return None; }
        let mut rgba = image.into_rgba8();
        // GPUI draws BGRA.
        for pixel in rgba.chunks_exact_mut(4) { pixel.swap(0, 2); }
        Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
    })();
    let kept = image.as_ref().and_then(|i| i.as_bytes(0)).map_or(0, <[u8]>::len);
    *budget.lock().unwrap() += most - kept.min(most);
    image
}

/// A paragraph as one text element; links are accent coloured, underlined and clickable, and
/// show where they go on hover, as the text may say otherwise.
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
    // The tooltip builder gets a text position, the click handler a range index.
    let (tips, theme): (Vec<_>, _) = (ranges.iter().cloned().zip(urls.iter().cloned()).collect(), *t);
    div().child(InteractiveText::new(("rich-text", ix), text)
        .on_click(ranges, move |i, _, cx| cx.open_url(&urls[i]))
        .tooltip(move |at, _, cx| {
            let url = tips.iter().find(|(range, _)| range.contains(&at))?.1.to_string();
            Some(cx.new(|_| Tip(theme, url.into())).into())
        }))
        .into_any_element()
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
    /// A 1 × 1 GIF; bytes 6..10 are the screen size every frame is drawn onto.
    const GIF: [u8; 43] = [0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 4, 1, 0, 0, 0, 0, 0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3B];

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in bytes { c ^= u32::from(b); for _ in 0..8 { c = if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 }; } }
            !c
        }
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        for (kind, data) in [(&b"IHDR"[..], [&width.to_be_bytes()[..], &height.to_be_bytes(), &[8, 0, 0, 0, 0]].concat()), (b"IDAT", vec![0x78, 0x9C, 3, 0, 0, 0, 0, 1]), (b"IEND", vec![])] {
            out.extend((data.len() as u32).to_be_bytes());
            let body = [kind, &data].concat();
            out.extend(&body);
            out.extend(crc(&body).to_be_bytes());
        }
        out
    }

    #[test]
    fn image_bombs_are_refused_before_decoding() {
        let budget = Mutex::new(DECODED_BUDGET);
        let decode = |bytes: &[u8]| super::decode(bytes, &budget);
        assert!(decode(&GIF).is_some(), "the 1 × 1 GIF draws");
        let mut bomb = GIF;
        bomb[6..10].copy_from_slice(&[0xA0, 0x0F, 0xA0, 0x0F]);
        assert!(decode(&bomb).is_none(), "a 4000 × 4000 screen is 61 MiB a frame");
        let mut empty = GIF;
        empty[6..8].copy_from_slice(&[0, 0]);
        assert!(decode(&empty).is_none(), "an empty texture would panic GPUI's atlas");
        assert!(decode(&png_header(16_000, 30_000)).is_none());
        assert!(decode(&png_header(8_000, 8_000)).is_none(), "grayscale within the side limit is still 244 MiB as RGBA");
        assert!(decode(b"<svg xmlns='http://www.w3.org/2000/svg' width='99999' height='99999'/>").is_none());
        assert!(decode(b"RIFF\0\0\0\0WEBPVP8X").is_none(), "WebP is not drawn");
        // A wide image shrinks to 1600 pixels; one that fits keeps its size.
        let mut wide = Vec::new();
        image::RgbImage::new(3200, 100).write_to(&mut std::io::Cursor::new(&mut wide), image::ImageFormat::Png).unwrap();
        let image = decode(&wide).unwrap();
        assert_eq!((image.size(0).width.0, image.size(0).height.0), (1600, 50));
        let image = decode(&data_image(&format!("data:image/png;base64,{PNG}")).unwrap()).unwrap();
        assert_eq!((image.size(0).width.0, image.size(0).height.0), (1, 1));
    }

    #[test]
    fn images_follow_the_csp_and_share_one_key_per_part() {
        let parts = Arc::new(vec![part()]);
        let mut m = Mail { inline: parts, ..Default::default() };
        let key = |src: &str, m: &Mail| Rich::source(7, &Arc::from(src), m).map(|(key, _)| key);
        for spelling in ["cid:x", "CID:x", "cId:%78"] { assert!(key(spelling, &m) == Some(Key::Part("x".into())), "{spelling}"); }
        assert!(key("cid:missing", &m).is_none(), "a part that has not arrived");
        assert!(key("data:image/png;base64,AA==", &m) == Some(Key::Data(7)));
        assert!(key("https://t.example/p.png", &m).is_none(), "remote images wait for the button");
        assert!(key("file:///etc/passwd", &m).is_none() && key("/x.png", &m).is_none());
        m.remote = true;
        assert!(key("//t.example/p.png", &m) == Some(Key::Web("https://t.example/p.png".into())));
        assert!(key("file:///etc/passwd", &m).is_none());
        assert!(Rich::new("<img src=https://t.example/p.png>").has_remote && !Rich::new("<p>text</p><img src=cid:x>").has_remote);
    }

    fn part() -> InlinePart {
        InlinePart { cid: "x".into(), mime_type: "image/png".into(), data: data_image(&format!("data:image/png;base64,{PNG}")).unwrap() }
    }

    #[test]
    fn budgets_run_out() {
        // A refused or shrunk image gives back what it does not keep.
        let mut wide = Vec::new();
        image::RgbImage::new(3200, 100).write_to(&mut std::io::Cursor::new(&mut wide), image::ImageFormat::Png).unwrap();
        let budget = Mutex::new(1600 * 100 * 4);
        assert!(decode(&wide, &budget).is_some());
        assert_eq!(*budget.lock().unwrap(), 1600 * 50 * 4, "1600 × 50 kept");
        assert!(decode(&wide, &budget).is_none(), "not enough for the largest it could be");
        assert_eq!(*budget.lock().unwrap(), 1600 * 50 * 4, "a refusal costs nothing");
        assert!(decode(&GIF, &budget).is_some());
        // Images past the decoded budget show the placeholder, and are never decoded.
        let budget = Budget::new();
        *budget.decoded.lock().unwrap() = 3;
        let source = Source::Data(format!("data:image/png;base64,{PNG}").into());
        assert!(block_on(fetch_and_decode(source.clone(), budget.clone())).is_none(), "4 bytes, 3 left: refused");
        *budget.decoded.lock().unwrap() = 4;
        assert!(block_on(fetch_and_decode(source, budget)).is_some());
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    #[gpui::test]
    fn rich_mail_draws_and_decodes_once(cx: &mut gpui::TestAppContext) {
        let theme = Theme::light(DEFAULT_ACCENT);
        let html = format!(r#"<h1>Sale</h1><p>Read <a href="https://x.example">more</a> or <a href="javascript:alert(1)">not</a>.</p><a href="https://x.example"><img src="cid:x" width=40><img src="CID:%78"></a><img src="data:image/png;base64,AAAA">"#);
        let rich = Arc::new(Rich::new(&html));
        let m = Mail { rich: Some(rich.clone()), inline: Arc::new(vec![part()]), ..Default::default() };
        let window = cx.add_window(|_, _| Probe(m, theme));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear()).unwrap();
        cx.run_until_parked();
        let id = window.window_id();
        {
            let images = &rich.images.lock().unwrap().slots;
            assert_eq!(images.len(), 2, "two spellings of one part decode once");
            assert!(matches!(images.get(&Key::Part("x".into())), Some(Slot::Done(Some(_), windows)) if *windows == [id]), "the window that drew it is known");
            assert!(matches!(images.get(&Key::Data(3)), Some(Slot::Done(None, _))), "data that is no image is refused once");
        }
        // Off screen: the images go, and the window that drew them frees its texture.
        *rich.images.lock().unwrap().budget.decoded.lock().unwrap() = 0;
        rich.forget();
        assert!(rich.images.lock().unwrap().slots.is_empty());
        assert_eq!(*rich.images.lock().unwrap().budget.decoded.lock().unwrap(), DECODED_BUDGET, "a fresh budget");
        assert_eq!(RELEASED.with_borrow(|r| r.iter().filter(|(_, w)| w.contains(&id)).count()), 1);
        let other = cx.add_window(|_, _| Probe(Mail::default(), theme));
        cx.update_window(other.into(), |_, window, cx| release(window, cx)).unwrap();
        assert_eq!(RELEASED.with_borrow(|r| r.iter().filter(|(_, w)| w.contains(&id)).count()), 1, "another window frees nothing of this one");
        cx.update_window(window.into(), |_, window, cx| release(window, cx)).unwrap();
        assert_eq!(RELEASED.with_borrow(|r| r.iter().filter(|(_, w)| w.contains(&id)).count()), 0);
    }

    struct Probe(Mail, Theme);

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div().size_full().children(self.0.rich.clone().map(|rich| rich.view(&self.0, &self.1)).unwrap_or_default())
        }
    }
}
