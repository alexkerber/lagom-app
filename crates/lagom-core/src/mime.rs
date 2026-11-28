use crate::{
    store::{Message, MessageSummary},
    Error, Result,
};
use html5ever::tokenizer::{states::RawKind, BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer};
use html5ever::{local_name, LocalName};
use std::sync::Arc;
use mail_parser::{AddressList, MessageParser, PartKind};

pub const MAX_MIME_BYTES: usize = 4 * 1024 * 1024;
/// The stored body of a message whose text is over `MAX_MIME_BYTES`: the headers are kept, the
/// text is not downloaded. Readers compare against this to show why the body is missing.
pub const BODY_UNAVAILABLE: &str = "The text of this message is larger than 4 MB and was not downloaded.";
/// The text a message may download when it is opened: sync leaves bodies over `MAX_MIME_BYTES` out.
pub const MAX_OPEN_BYTES: usize = 25 * 1024 * 1024;

/// A row for a message whose headers do not parse: the list shows it, the body says why.
pub fn placeholder() -> Message {
    Message {
        summary: MessageSummary { subject: "(headers too large to read)".into(), ..Default::default() },
        body_text: BODY_UNAVAILABLE.into(),
        body_html: None,
    }
}

/// Headers only, with the body marked unavailable, so one huge message never stops a sync.
pub fn metadata_only(header: &[u8]) -> Result<Message> {
    let mut message = parse(header)?;
    message.body_text = BODY_UNAVAILABLE.into();
    message.body_html = None;
    message.summary.snippet = String::new();
    Ok(message)
}
/// Zero-width characters that newsletters repeat after the preheader to push the body text
/// out of list previews (`&#847;`, `&zwnj;`, soft hyphens).
const INVISIBLE: &[char] = &['\u{34F}', '\u{AD}', '\u{180E}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}'];

/// The visible text of mail HTML: hidden elements dropped, block structure kept as line breaks.
/// Empty when the tree builder stops at its work limit: a partial tree can show text that the
/// whole document hides, so an input that costly gets no preview or search text.
pub fn html_to_text(html: &str) -> String {
    // The document the reader parses: renamed tags (an `iframe`'s text is markup there), no quirks
    // mode, and the mail after `head`.
    let html = crate::html::inert_tags(html);
    // Four times the most work per byte measured on real mail, so a mail of many costly HTML
    // parts stays linear in its size.
    html5ever::tree_builder::set_work_limit(32 * html.len() + (1 << 16));
    let document = dom_query::Document::from(format!("<!DOCTYPE html><html><head></head>{html}"));
    if document.errors.borrow().iter().any(|e| e == html5ever::tree_builder::WORK_LIMIT_ERROR) {
        return String::new();
    }
    // A closed `details` draws only its first `summary`, chosen before hidden children go. Of
    // open `details` that share a `name`, the first stays open.
    let mut groups = std::collections::HashSet::new();
    for details in document.select("details").nodes() {
        let name = details.attr("name").filter(|n| !n.is_empty());
        let open = details.has_attr("open") && name.is_none_or(|n| groups.insert(n.to_string()));
        if open { continue; }
        let summary = details.children_it(false).find(|c| c.has_name("summary")).map(|c| c.id);
        for child in details.children() {
            if Some(child.id) != summary { child.remove_from_parent(); }
        }
    }
    // A declarative shadow root replaces its host's children; lagom does not render it.
    for template in document.select("template[shadowrootmode]").nodes() {
        if let Some(host) = template.parent() { host.remove_from_parent(); }
    }
    // `svg` and `math` as the streaming path hides them: their desc, metadata and CDATA text is not drawn.
    document.select("head, style, script, title, template, noframes, frameset, svg, math, [hidden], dialog:not([open]), \
        [popover], datalist, video, audio, noembed, rp, meter, progress, canvas, select").remove();
    for node in document.select("[style]").iter() {
        if style_hides(&node.attr_or("style", "")) { node.remove(); }
    }
    // Document::formatted_text trims by byte and panics on trailing multibyte whitespace in `pre`
    // (dom_query 0.28); the selection form skips that trim and clean_lines trims instead.
    clean_lines(&document.select("html").formatted_text())
}

fn read(html: &str, marks: Option<Vec<Mark>>) -> TextState {
    tokenize(html, TextSink(TextState { marks, ..Default::default() }.into())).0.into_inner()
}

fn tokenize<S: TokenSink>(html: &str, sink: S) -> S {
    let queue = BufferQueue::default();
    queue.push_back(html.into());
    let tokenizer = Tokenizer::new(sink, Default::default());
    let _ = tokenizer.feed(&queue);
    tokenizer.end();
    tokenizer.sink
}

/// Text with one style: a native view draws mail HTML as paragraphs of these.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    /// The `href` of the link around it, as written. Shared: runs of one link point at one copy.
    pub link: Option<Arc<str>>,
}

/// An `<img>`: its `src` as written, its `width` attribute in CSS pixels, and the link around it.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub src: Arc<str>,
    pub width: Option<f32>,
    pub link: Option<Arc<str>>,
}

/// A paragraph, or images side by side with no text between them.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text(Vec<Run>),
    Images(Vec<Image>),
}

/// A link, emphasis or image in the text that `read` collects. It stands in the text as one
/// private-use character, so a hidden element cuts its marks together with its text.
enum Mark {
    Link(Option<Arc<str>>),
    Bold(bool),
    /// Headings do not nest, and a heading's end tag closes any heading: on or off, no count.
    Heading(bool),
    Italic(bool),
    Image(Arc<str>, Option<f32>),
}

/// Marks are U+F0000 and up, in Supplementary Private Use Area-A, which mail text never needs.
const MARK: u32 = 0xF0000;
const MARKS: usize = 0xFFFE;

fn mark_index(c: char) -> Option<usize> {
    (c as u32).checked_sub(MARK).map(|i| i as usize).filter(|&i| i < MARKS)
}

/// The streaming converter's visible text as blocks for a native view, with
/// bold, italic, links and images. Styles, scripts and hidden elements show nothing. At most
/// 100 000 characters and 256 images, as GPUI lays out all text on screen.
pub fn html_to_blocks(html: &str) -> Vec<Block> {
    let state = read(html, Some(Vec::new()));
    let marks = state.marks.unwrap_or_default();
    let mut out = Blocks::default();
    for paragraph in clean_lines(state.text.trim_end()).split("\n\n") {
        for c in paragraph.chars() {
            let Some(mark) = mark_index(c).and_then(|i| marks.get(i)) else { out.push(c); continue };
            match mark {
                Mark::Link(href) => out.link = href.clone(),
                Mark::Bold(on) => out.bold = if *on { out.bold + 1 } else { out.bold.saturating_sub(1) },
                Mark::Heading(on) => out.heading = *on,
                Mark::Italic(on) => out.italic = if *on { out.italic + 1 } else { out.italic.saturating_sub(1) },
                Mark::Image(src, width) => out.image(Image { src: src.clone(), width: *width, link: out.link.clone() }),
            }
            out.gap = true;
        }
        out.flush();
        out.row = false;
    }
    out.blocks
}

#[derive(Default)]
struct Blocks {
    blocks: Vec<Block>,
    runs: Vec<Run>,
    /// Open `b`/`strong`/heading and `i`/`em` elements.
    bold: usize,
    italic: usize,
    heading: bool,
    link: Option<Arc<str>>,
    /// The last block is an image row that the next image joins.
    row: bool,
    /// A mark came after the last character: white space on both sides of it collapses.
    gap: bool,
    chars: usize,
    images: usize,
}

impl Blocks {
    fn push(&mut self, c: char) {
        if self.chars >= 100_000 { return; }
        let last = self.runs.last().and_then(|r| r.text.chars().last());
        if self.gap && c.is_whitespace() && last.is_none_or(char::is_whitespace) { return; }
        self.gap = false;
        self.chars += 1;
        let (bold, italic) = (self.bold > 0 || self.heading, self.italic > 0);
        match self.runs.last_mut() {
            Some(run) if run.bold == bold && run.italic == italic && run.link == self.link => run.text.push(c),
            _ => self.runs.push(Run { text: c.into(), bold, italic, link: self.link.clone() }),
        }
    }

    fn image(&mut self, image: Image) {
        self.flush();
        if self.chars >= 100_000 || self.images >= 256 { return; }
        self.images += 1;
        match self.blocks.last_mut() {
            Some(Block::Images(row)) if self.row => row.push(image),
            _ => self.blocks.push(Block::Images(vec![image])),
        }
        self.row = true;
    }

    /// The paragraph so far, without white space at its ends.
    fn flush(&mut self) {
        let mut runs = std::mem::take(&mut self.runs);
        let Some(first) = runs.iter().position(|r| !r.text.trim().is_empty()) else { return };
        runs.drain(..first);
        while runs.last().is_some_and(|r| r.text.trim().is_empty()) { runs.pop(); }
        let Some(first) = runs.first_mut() else { return };
        first.text = first.text.trim_start().into();
        let last = runs.last_mut().unwrap();
        last.text = last.text.trim_end().into();
        self.blocks.push(Block::Text(runs));
        self.row = false;
    }
}

#[derive(Default)]
struct TextSink(std::cell::RefCell<TextState>);

#[derive(Default)]
struct TextState {
    text: String,
    /// The outermost hidden element. Its text is cut when it closes.
    hidden: Option<Hidden>,
    /// Open `pre` elements: their text is kept as is, as in dom_query's `formatted_text`.
    pre: usize,
    /// A `p` is open: a block start closes it.
    p: bool,
    /// The tree builder drops a line feed right after `<pre>`.
    skip_newline: bool,
    /// In the text of a raw text element (`title`, `style`, `script`): no tags until its end tag.
    raw: bool,
    /// Open lists, `dl` and tables (see `container`): their end tag closes a hidden item.
    containers: [usize; 5],
    /// Scope boundaries opened so far (`table`, `object`, `select`): never decreases.
    scopes: usize,
    /// `scopes` when the innermost open list, `dl` or table opened; `usize::MAX` when unsure.
    scopes_at: [usize; 5],
    /// A `form` is open: the tree builder ignores another `<form>`.
    form: bool,
    /// Open `select` elements, at most one when well formed.
    selects: usize,
    /// For `html_to_blocks`: what each mark character in `text` stands for.
    marks: Option<Vec<Mark>>,
}

/// Elements tracked inside a hidden element. Deeper nesting keeps it hidden to the end of the mail.
const HIDDEN_DEPTH: usize = 256;

/// A hidden element, and the text length and state when it started. When unsure whether a tag
/// closes it, it stays hidden: a lost line in a preview is better than shown hidden text.
struct Hidden {
    name: LocalName,
    /// The elements open inside it, innermost last, and whether each hides; `None` past
    /// `HIDDEN_DEPTH`.
    inside: Option<Vec<(LocalName, bool)>>,
    /// Formatting elements closed inside it by another element's end tag, and whether each
    /// hides: the tree builder opens them again in later blocks.
    lingering: Vec<(LocalName, bool)>,
    /// `TextState::scopes` when it opened.
    scopes: usize,
    at: usize,
    pre: usize,
    p: bool,
    /// Open `svg` and `math` elements in it, itself included: there `<g/>` closes itself.
    foreign: usize,
}

impl Hidden {
    /// Whether this tag closes the hidden element, as the tree builder would. Otherwise it tracks
    /// the elements this tag opens or closes inside it.
    fn closed_by(&mut self, name: &LocalName, start: bool, opens: bool, hides: bool, containers: &mut [usize; 5], scopes_at: &[usize; 5]) -> bool {
        let Some(inside) = &mut self.inside else { return false };
        // `<select>` and `<input>` in a `select` close it, as `</select>` does.
        let select = local_name!("select");
        let (name, start) = if start && matches!(*name, local_name!("select") | local_name!("input")) && inside.iter().any(|(n, _)| *n == select) { (&select, false) } else { (name, start) };
        // In `svg` and `math` every tag opens an element, with no implied ends.
        if start && self.foreign > 0 {
            if opens { self.push(name, hides); }
            return false;
        }
        if start {
            // A sibling's start closes it, unless an element open inside it stops that.
            if start_closes(&self.name, name, inside) { return true; }
            // The same implied ends inside it: the innermost element of each kind this tag closes.
            let table = matches!(*name, local_name!("td") | local_name!("th") | local_name!("tr") | local_name!("caption") | local_name!("col") | local_name!("colgroup") | local_name!("tbody") | local_name!("thead") | local_name!("tfoot"));
            let kinds = [(local_name!("li"), *name == local_name!("li")), (local_name!("dd"), matches!(*name, local_name!("dd") | local_name!("dt"))), (local_name!("td"), table), (local_name!("tr"), table), (local_name!("tbody"), table), (local_name!("option"), matches!(*name, local_name!("option") | local_name!("optgroup"))), (local_name!("p"), closes_p(name))];
            for kind in kinds.into_iter().filter_map(|(kind, closes)| closes.then_some(kind)) {
                let Some(inside) = &self.inside else { return false };
                let at = inside.iter().rposition(|(n, _)| kind_of(n) == kind).filter(|&at| start_closes(&inside[at].0, name, &inside[at + 1..]));
                if let Some(at) = at { self.pop(at, false, containers); }
            }
            // A heading start closes a heading open last.
            if let Some(inside) = &self.inside {
                if heading(name) && inside.last().is_some_and(|(n, _)| heading(n)) {
                    let at = inside.len() - 1;
                    self.pop(at, false, containers);
                }
            }
            // The tree builder ignores `head`, `html` and `body` in the body.
            if opens && !matches!(*name, local_name!("head") | local_name!("html") | local_name!("body")) { self.push(name, hides); }
            return false;
        }
        // A formatting end tag first takes a closed one of its name off the formatting list.
        if formatting(name) {
            if let Some(at) = self.lingering.iter().rposition(|(n, _)| n == name) {
                self.lingering.remove(at);
                return false;
            }
        }
        // A heading end tag closes any heading.
        if let Some(at) = inside.iter().rposition(|(n, _)| n == name || heading(n) && heading(name)) {
            // As the tree builder: an end tag does not close through a scope boundary above it.
            if !inside[at + 1..].iter().any(|(n, _)| end_scope(name)(n)) {
                self.pop(at, *name == local_name!("form"), containers);
            }
            return false;
        }
        let open = |set: fn(&LocalName) -> bool| inside.iter().any(|(n, _)| set(n));
        let hidden = &self.name;
        if *name == local_name!("form") && *hidden == local_name!("form") {
            // `</form>` takes only the form off the stack: what is open in it, reopened
            // formatting too, stays hidden. Unsure what that is, hidden to the end.
            self.inside = None;
            return false;
        }
        // A container end tag closes the item only when that container is open around it.
        // A list or `dl` end tag also needs no scope boundary opened between it and this element.
        let around = |n: &LocalName| container(n).is_some_and(|k| containers[k] > inside.iter().filter(|(m, _)| container(m) == Some(k)).count() && (k == TABLE || scopes_at[k] == self.scopes));
        // Its own end tag, or the end of its list or table, unless a scope boundary is open inside.
        let own = *name == *hidden || heading(hidden) && heading(name);
        let parent = match *hidden {
            local_name!("li") => matches!(*name, local_name!("ul") | local_name!("ol") | local_name!("menu")) && !open(default_scope),
            local_name!("dt") | local_name!("dd") => *name == local_name!("dl") && !open(default_scope),
            local_name!("td") | local_name!("th") => matches!(*name, local_name!("tr") | local_name!("tbody") | local_name!("thead") | local_name!("tfoot") | local_name!("table")) && !open(table_scope),
            local_name!("tr") => matches!(*name, local_name!("tbody") | local_name!("thead") | local_name!("tfoot") | local_name!("table")) && !open(table_scope),
            local_name!("tbody") | local_name!("thead") | local_name!("tfoot") => *name == local_name!("table") && !open(table_scope),
            _ => false,
        } && around(name);
        own && !open(end_scope(hidden)) || parent
    }

    /// Text and start tags open closed formatting elements again, as the tree builder does.
    fn reopen(&mut self) {
        if self.foreign > 0 { return; }
        for (name, hides) in std::mem::take(&mut self.lingering) { self.push(&name, hides); }
    }

    fn push(&mut self, name: &LocalName, hides: bool) {
        let Some(inside) = &mut self.inside else { return };
        if inside.len() < HIDDEN_DEPTH {
            inside.push((name.clone(), hides));
            self.foreign += is_foreign(name) as usize;
        } else {
            self.inside = None;
        }
    }

    /// Closes the element open inside it at `at`, and the elements open in that one. `</form>`
    /// closes only the form: what is open in it stays; in a hidden form, hidden to the end.
    fn pop(&mut self, at: usize, form: bool, containers: &mut [usize; 5]) {
        let Some(inside) = &mut self.inside else { return };
        let mut popped = inside.split_off(at + 1);
        let (element, hid) = inside.pop().unwrap();
        self.foreign -= is_foreign(&element) as usize;
        if form {
            inside.extend(popped);
            if hid { self.inside = None; }
            return;
        }
        for (n, _) in &popped {
            if let Some(k) = container(n) { containers[k] = containers[k].saturating_sub(1); }
            self.foreign -= is_foreign(n) as usize;
        }
        // Formatting elements come back in later blocks. A hidden element closed by another's
        // end may still be open when this model is wrong: it comes back too.
        popped.retain(|(n, h)| formatting(n) || *h);
        self.lingering.extend(popped);
        if self.lingering.len() > HIDDEN_DEPTH { self.inside = None; }
    }
}

/// Whether a start tag `name` closes an open `element` with `above` open in it, as the tree
/// builder's implied ends do.
fn start_closes(element: &LocalName, name: &LocalName, above: &[(LocalName, bool)]) -> bool {
    let open = |set: fn(&LocalName) -> bool| above.iter().any(|(n, _)| set(n));
    let item = |n: &LocalName| special(n) && !matches!(*n, local_name!("address") | local_name!("div") | local_name!("p"));
    let table = |n: &LocalName| matches!(*n, local_name!("caption") | local_name!("col") | local_name!("colgroup") | local_name!("tbody") | local_name!("thead") | local_name!("tfoot"));
    match *element {
        local_name!("head") => !head_content(name) && !open(|n| *n == local_name!("template")),
        local_name!("p") => closes_p(name) && !open(button_scope),
        local_name!("li") => *name == local_name!("li") && !open(item),
        local_name!("dt") | local_name!("dd") => matches!(*name, local_name!("dt") | local_name!("dd")) && !open(item),
        local_name!("td") | local_name!("th") => (matches!(*name, local_name!("td") | local_name!("th") | local_name!("tr")) || table(name)) && !open(table_scope),
        local_name!("tr") => (*name == local_name!("tr") || table(name)) && !open(table_scope),
        local_name!("tbody") | local_name!("thead") | local_name!("tfoot") => table(name) && !open(table_scope),
        local_name!("option") => matches!(*name, local_name!("option") | local_name!("optgroup")) && above.is_empty(),
        // A heading start closes an open heading only when nothing, not even reopened
        // formatting, is open in it: unsure, it never does here.
        _ => false,
    }
}

/// The kind of implied end an element has: `dt` as `dd`, cells as `td`, row groups as `tbody`.
fn kind_of(name: &LocalName) -> LocalName {
    match *name {
        local_name!("dt") => local_name!("dd"),
        local_name!("th") => local_name!("td"),
        local_name!("thead") | local_name!("tfoot") => local_name!("tbody"),
        _ => name.clone(),
    }
}

impl TokenSink for TextSink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut state = self.0.borrow_mut();
        match token {
            Token::TagToken(tag) => return state.tag(tag),
            // Mark characters from the mail, raw or as entities, would forge links and images.
            Token::CharacterTokens(chars) if state.marks.is_some() && chars.chars().any(|c| mark_index(c).is_some()) => {
                state.chars(&chars.chars().filter(|&c| mark_index(c).is_none()).collect::<String>())
            }
            Token::CharacterTokens(chars) => state.chars(&chars),
            // Unclosed at the end: hidden to the end, as in a DOM.
            Token::EOFToken => state.unhide(),
            _ => {}
        }
        TokenSinkResult::Continue
    }

    /// `<![CDATA[` starts text in `svg` and `math`.
    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        self.0.borrow().foreign() > 0
    }
}

impl TextState {
    /// Closes the hidden element. In well-formed HTML nothing is open inside. Otherwise a hidden
    /// element open inside may outlive it (adoption, reconstructed formatting, ignored table
    /// tags), and a hidden formatting element closed in it comes back in later blocks: hiding
    /// goes on from the first one, with what is open in it.
    fn close_hidden(&mut self, opened: Option<(LocalName, bool)>) {
        let Some(hidden) = &mut self.hidden else { return };
        let mut inside = hidden.inside.take().unwrap_or_default();
        let mut lingering = std::mem::take(&mut hidden.lingering);
        self.unhide();
        for (n, _) in &inside {
            if let Some(k) = container(n) { self.containers[k] = self.containers[k].saturating_sub(1); }
        }
        let revive = match inside.iter().position(|(_, h)| *h) {
            Some(i) => {
                let rest = inside.split_off(i + 1);
                let revived = inside.pop().unwrap().0;
                lingering.extend(inside.into_iter().filter(|(n, _)| formatting(n)));
                Some((revived, rest, lingering))
            }
            None => {
                lingering.extend(inside.into_iter().filter(|(n, _)| formatting(n)));
                lingering.iter().position(|(_, h)| *h).map(|i| (lingering.remove(i).0, Vec::new(), lingering))
            }
        };
        if let Some((revived, mut inside, lingering)) = revive {
            inside.extend(opened);
            let foreign = is_foreign(&revived) as usize + inside.iter().filter(|(n, _)| is_foreign(n)).count();
            self.hidden = Some(Hidden { name: revived, inside: Some(inside), lingering, scopes: self.scopes, at: self.text.len(), pre: self.pre, p: self.p, foreign });
        }
    }

    fn unhide(&mut self) {
        if let Some(hidden) = self.hidden.take() {
            self.text.truncate(hidden.at);
            (self.pre, self.p) = (hidden.pre, hidden.p);
        }
    }

    /// A `template` is open: it hides, so only a hidden element holds one.
    fn in_template(&self) -> bool {
        self.hidden.as_ref().is_some_and(|h| h.name == local_name!("template") || h.inside.as_ref().is_none_or(|i| i.iter().any(|(n, _)| *n == local_name!("template"))))
    }

    fn foreign(&self) -> usize {
        self.hidden.as_ref().map_or(0, |h| h.foreign)
    }

    fn count(&mut self, name: &LocalName, start: bool, opens: bool) {
        self.scopes += (opens && default_scope(name)) as usize;
        match *name {
            // In a `template`, a form sets no form pointer.
            local_name!("form") if !start || opens && !self.in_template() => self.form = start,
            // In a `select`, `<select>` closes it and opens none; unsure, the count stays.
            local_name!("select") if opens && self.selects == 0 => self.selects = 1,
            local_name!("select") if !start => self.selects = self.selects.saturating_sub(1),
            local_name!("input") if self.selects > 0 => self.selects -= 1,
            _ => {}
        }
        if let Some(kind) = container(name) {
            if opens {
                self.containers[kind] += 1;
                self.scopes_at[kind] = self.scopes;
            } else if !start {
                self.containers[kind] = self.containers[kind].saturating_sub(1);
                self.scopes_at[kind] = usize::MAX;
            }
        }
    }

    fn tag(&mut self, tag: Tag) -> TokenSinkResult<()> {
        let (name, start) = (&tag.name, tag.kind == TagKind::StartTag);
        self.raw = false;
        self.skip_newline = false;
        let void = matches!(*name, local_name!("area") | local_name!("base") | local_name!("br") | local_name!("col") | local_name!("embed") | local_name!("hr") | local_name!("img") | local_name!("image") | local_name!("input") | local_name!("link") | local_name!("meta") | local_name!("source") | local_name!("track") | local_name!("wbr") | local_name!("param") | local_name!("keygen") | local_name!("basefont") | local_name!("bgsound") | local_name!("frame"));
        // Table parts outside a table, and a second `<form>` before `</form>`: the tree builder
        // ignores them.
        if start && self.foreign() == 0 && (self.containers[TABLE] == 0 && matches!(*name, local_name!("caption") | local_name!("col") | local_name!("colgroup") | local_name!("tbody") | local_name!("td") | local_name!("tfoot") | local_name!("th") | local_name!("thead") | local_name!("tr")) || *name == local_name!("form") && self.form && !self.in_template()) {
            return TokenSinkResult::Continue;
        }
        let foreign = is_foreign(name);
        // The tag opens an element: `<svg/>`, and `<g/>` in `svg` or `math`, do not.
        let opens = start && !void && !(tag.self_closing && (foreign || self.foreign() > 0));
        // `svg` and `math` are hidden: the tree builder has other rules in them. An HTML tag in
        // them (`<p>`, or any tag in `foreignObject`) hides the rest of the mail.
        let hides = start && (foreign || hides(&tag));
        // Unsure, hidden to the end: `<select>` in a `select` around a hidden element may close
        // both, and `<form>` in a table closes no `p`.
        if let Some(hidden) = &mut self.hidden {
            let selects = matches!(*name, local_name!("select") | local_name!("input")) && self.selects > 0 && !hidden.inside.iter().flatten().any(|(n, _)| *n == local_name!("select"));
            let form = *name == local_name!("form") && opens && self.containers[TABLE] > 0;
            if start && (selects || form) { hidden.inside = None; }
        }
        // Only a hidden element holds `svg` or `math`.
        if let Some(hidden) = self.hidden.as_mut().filter(|h| h.foreign > 0) {
            // An end tag of no element open there may close an HTML element around it.
            let open = |i: &Vec<(LocalName, bool)>| hidden.name == *name || i.iter().any(|(n, _)| n == name);
            let html = match hidden.inside.as_ref() {
                Some(i) if i.last().is_some_and(|(n, _)| integration(n)) => start || i.last().unwrap().0 != *name,
                Some(i) => breakout(&tag) || !start && !open(i),
                None => false,
            };
            if html { hidden.inside = None; }
        }
        if let Some(hidden) = &mut self.hidden {
            if start { hidden.reopen(); }
            if hidden.closed_by(name, start, opens, hides, &mut self.containers, &self.scopes_at) {
                let own = !start && (hidden.name == *name || heading(&hidden.name) && heading(name));
                // The sibling start tag that closed it opens in a revived element.
                self.close_hidden(opens.then(|| (name.clone(), hides)));
                // Its own end tag is done; a sibling's start or a parent's end goes on below.
                if own {
                    self.count(name, start, opens);
                    return TokenSinkResult::Continue;
                }
            }
        }
        self.count(name, start, opens);
        // In a `template` the tree builder may ignore `<textarea>` and the like: unsure, hidden
        // to the end.
        if opens && raw_kind(name).is_some() && self.in_template() {
            if let Some(hidden) = &mut self.hidden { hidden.inside = None; }
        }
        let result = match raw_kind(name) {
            // In `svg` and `math`, `<style>` and `<title>` hold tags.
            // Hidden to the end, every tag is read: a later `<body hidden>` hides all text.
            Some(_) if !opens || self.foreign() > 0 || self.hidden.as_ref().is_some_and(|h| h.inside.is_none()) => TokenSinkResult::Continue,
            Some(Some(kind)) => { self.raw = true; TokenSinkResult::RawData(kind) }
            Some(None) => TokenSinkResult::Plaintext,
            None => TokenSinkResult::Continue,
        };
        let closes_p = start && closes_p(name);
        // A later `<html>` or `<body>` adds its attributes to the one element: all text is hidden.
        if hides && matches!(*name, local_name!("html") | local_name!("body")) {
            self.unhide();
            self.hidden = Some(Hidden { name: name.clone(), inside: None, lingering: Vec::new(), scopes: self.scopes, at: 0, pre: 0, p: false, foreign: 0 });
            return result;
        }
        // A `frameset` the tree builder takes drops all text.
        if *name == local_name!("frameset") && start {
            self.unhide();
            self.hidden = Some(Hidden { name: name.clone(), inside: None, lingering: Vec::new(), scopes: self.scopes, at: 0, pre: 0, p: false, foreign: 0 });
            return result;
        }
        if self.hidden.is_none() && hides {
            // A hidden void element (`<br hidden>`) breaks no line.
            if !opens { return result; }
            self.hidden = Some(Hidden { name: name.clone(), inside: Some(Vec::new()), lingering: Vec::new(), scopes: self.scopes, at: self.text.len(), pre: self.pre, p: self.p, foreign: foreign as usize });
        }
        if self.pre > 0 {
            if *name == local_name!("pre") {
                if start { self.pre += 1; } else { self.pre -= 1; }
                if self.pre == 0 { self.close(name); }
            }
            self.mark(&tag);
            return result;
        }
        if !start {
            self.p &= *name != local_name!("p");
            self.mark(&tag);
            self.close(name);
            return result;
        }
        // An unclosed `p`, `li`, `tr` or cell before this one: the tree builder closes it here.
        if closes_p && std::mem::take(&mut self.p) {
            self.close(&local_name!("p"));
        }
        self.p |= *name == local_name!("p");
        match *name {
            local_name!("li") | local_name!("tr") if !self.text.ends_with('\n') => self.close(name),
            local_name!("td") | local_name!("th") => self.close(name),
            _ => {}
        }
        if block(name) && !(self.text.is_empty() || self.text.ends_with("\n\n")) {
            self.text.push('\n');
        }
        if *name == local_name!("pre") {
            self.pre = 1;
            self.skip_newline = true;
        }
        if void { self.close(name); }
        self.mark(&tag);
        result
    }

    /// For `html_to_blocks`: a mark character for a link, emphasis or image tag.
    fn mark(&mut self, tag: &Tag) {
        let Some(marks) = self.marks.as_mut().filter(|m| m.len() < MARKS) else { return };
        let start = tag.kind == TagKind::StartTag;
        let attr = |name: &str| tag.attrs.iter().find(|a| &*a.name.local == name).map(|a| a.value.trim().to_string());
        let mark = match &*tag.name {
            // A new `<a>` closes the one before, with or without `href`.
            // Longer links are no links: real ones fit, and each run would scan it.
            "a" => Mark::Link(attr("href").filter(|href| start && href.len() <= 16 * 1024).map(Into::into)),
            "b" | "strong" => Mark::Bold(start),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Mark::Heading(start),
            "i" | "em" => Mark::Italic(start),
            "img" | "image" if start => match attr("src").filter(|s| !s.is_empty()) {
                Some(src) => Mark::Image(src.into(), attr("width").and_then(|w| w.trim_end_matches("px").parse().ok()).filter(|w: &f32| w.is_finite() && *w >= 0.)),
                None => return,
            },
            _ => return,
        };
        self.text.push(char::from_u32(MARK + marks.len() as u32).unwrap());
        marks.push(mark);
    }

    fn chars(&mut self, chars: &str) {
        // Text ends `head`, unless it is in a `title`, `style` or `template` there.
        if self.hidden.as_ref().is_some_and(|h| h.name == local_name!("head") && h.inside.as_ref().is_some_and(|i| !i.iter().any(|(n, _)| *n == local_name!("template"))))
            && !self.raw && !chars.trim().is_empty()
        {
            self.close_hidden(None);
        }
        if let Some(hidden) = self.hidden.as_mut().filter(|_| !self.raw) { hidden.reopen(); }
        let chars = if std::mem::take(&mut self.skip_newline) { chars.strip_prefix('\n').unwrap_or(chars) } else { chars };
        if self.pre > 0 {
            self.text.push_str(chars);
            return;
        }
        // As dom_query's `formatted_text`: white space collapses to one space between words.
        let follows_space = self.text.is_empty() || self.text.ends_with(['\n', ' ']);
        let mut words = chars.split_whitespace();
        if let Some(first) = words.next() {
            if !follows_space && chars.starts_with(char::is_whitespace) { self.text.push(' '); }
            self.text.push_str(first);
            for word in words {
                self.text.push(' ');
                self.text.push_str(word);
            }
        } else if follows_space {
            return;
        }
        if chars.ends_with(char::is_whitespace) && !self.text.ends_with(char::is_whitespace) {
            self.text.push(' ');
        }
    }

    /// The line breaks after an element, as in dom_query's `formatted_text`.
    fn close(&mut self, name: &LocalName) {
        if self.text.is_empty() || self.text.ends_with("\n\n") { return; }
        // Spaces before a hidden element stay: its text is cut back to where it started. Scanning
        // only its own text keeps repeated hidden elements after a long run of spaces linear.
        let floor = self.hidden.as_ref().map_or(0, |h| h.at);
        let trimmed = floor + self.text[floor..].trim_end_matches(' ').len();
        if block(name) {
            self.text.truncate(trimmed);
            self.text.push_str("\n\n");
        } else if matches!(*name, local_name!("br") | local_name!("hr") | local_name!("li") | local_name!("tr")) {
            self.text.truncate(trimmed);
            self.text.push('\n');
        } else if matches!(*name, local_name!("td") | local_name!("th")) && !self.text.ends_with(['\n', ' ']) {
            self.text.push(' ');
        }
    }
}

fn block(name: &LocalName) -> bool {
    matches!(*name, local_name!("article") | local_name!("blockquote") | local_name!("section") | local_name!("div") | local_name!("p") | local_name!("pre") | local_name!("h1") | local_name!("h2") | local_name!("h3") | local_name!("h4") | local_name!("h5") | local_name!("h6") | local_name!("ul") | local_name!("ol") | local_name!("dl") | local_name!("table"))
}

fn is_foreign(name: &LocalName) -> bool {
    matches!(*name, local_name!("svg") | local_name!("math"))
}

/// `svg` and `math` elements whose content follows HTML rules.
fn integration(name: &LocalName) -> bool {
    matches!(&**name, "foreignobject" | "desc" | "title" | "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml")
}

/// A tag that ends `svg` and `math` content in the tree builder.
fn breakout(tag: &Tag) -> bool {
    match tag.kind {
        TagKind::EndTag => matches!(&*tag.name, "p" | "br"),
        TagKind::StartTag if tag.name == local_name!("font") => tag.attrs.iter().any(|a| matches!(&*a.name.local, "color" | "face" | "size")),
        TagKind::StartTag => matches!(&*tag.name, "b" | "big" | "blockquote" | "body" | "br" | "center" | "code" | "dd" | "div" | "dl" | "dt" | "em" | "embed" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "head" | "hr" | "i" | "img" | "li" | "listing" | "menu" | "meta" | "nobr" | "ol" | "p" | "pre" | "ruby" | "s" | "small" | "span" | "strong" | "strike" | "sub" | "sup" | "table" | "tt" | "u" | "ul" | "var"),
    }
}

/// The elements an end tag of `name` does not close through.
fn end_scope(name: &LocalName) -> fn(&LocalName) -> bool {
    match *name {
        local_name!("p") => button_scope,
        local_name!("li") => |n| default_scope(n) || matches!(*n, local_name!("ul") | local_name!("ol")),
        local_name!("td") | local_name!("th") | local_name!("tr") | local_name!("tbody") | local_name!("thead") | local_name!("tfoot") | local_name!("table") | local_name!("caption") => table_scope,
        // Block end tags close through other elements; any other end tag (`</noscript>` too)
        // stops at the first special element.
        _ if heading(name) || matches!(&**name, "address" | "article" | "aside" | "blockquote" | "button" | "center" | "details" | "dialog" | "dir" | "div" | "dl" | "fieldset" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "listing" | "main" | "menu" | "nav" | "ol" | "pre" | "search" | "section" | "select" | "summary" | "ul" | "dd" | "dt" | "applet" | "marquee" | "object" | "form" | "template") => default_scope,
        // A formatting end tag with a special element open in it moves that element out
        // (adoption): unsure, it closes nothing here.
        _ => special,
    }
}

/// Lists, `dl` and tables, by kind.
const TABLE: usize = 4;

fn container(name: &LocalName) -> Option<usize> {
    match *name {
        local_name!("ul") => Some(0),
        local_name!("ol") => Some(1),
        local_name!("menu") => Some(2),
        local_name!("dl") => Some(3),
        local_name!("table") => Some(TABLE),
        _ => None,
    }
}

/// The start tags that may stay in `head`.
fn head_content(name: &LocalName) -> bool {
    matches!(*name, local_name!("head") | local_name!("base") | local_name!("basefont") | local_name!("bgsound") | local_name!("link") | local_name!("meta") | local_name!("title") | local_name!("style") | local_name!("script") | local_name!("noscript") | local_name!("template") | local_name!("noframes"))
}

fn heading(name: &LocalName) -> bool {
    matches!(*name, local_name!("h1") | local_name!("h2") | local_name!("h3") | local_name!("h4") | local_name!("h5") | local_name!("h6"))
}

fn formatting(name: &LocalName) -> bool {
    matches!(*name, local_name!("a") | local_name!("b") | local_name!("big") | local_name!("code") | local_name!("em") | local_name!("font") | local_name!("i") | local_name!("nobr") | local_name!("s") | local_name!("small") | local_name!("strike") | local_name!("strong") | local_name!("tt") | local_name!("u"))
}

/// The boundaries of the tree builder's "has an element in scope". `svg` and `math` stand in for
/// their integration points.
fn default_scope(name: &LocalName) -> bool {
    matches!(*name, local_name!("applet") | local_name!("caption") | local_name!("html") | local_name!("table") | local_name!("td") | local_name!("th") | local_name!("marquee") | local_name!("object") | local_name!("select") | local_name!("template") | local_name!("svg") | local_name!("math"))
}

fn button_scope(name: &LocalName) -> bool {
    default_scope(name) || *name == local_name!("button")
}

fn table_scope(name: &LocalName) -> bool {
    matches!(*name, local_name!("html") | local_name!("table") | local_name!("template"))
}

/// The tree builder's special category: an end tag of another element does not close through it.
fn special(name: &LocalName) -> bool {
    default_scope(name) || head_content(name) || closes_p(name) || heading(name)
        || matches!(*name, local_name!("area") | local_name!("body") | local_name!("br") | local_name!("button") | local_name!("col") | local_name!("colgroup") | local_name!("embed") | local_name!("frameset") | local_name!("iframe") | local_name!("img") | local_name!("input") | local_name!("keygen") | local_name!("noembed") | local_name!("param") | local_name!("select") | local_name!("source") | local_name!("tbody") | local_name!("textarea") | local_name!("tfoot") | local_name!("thead") | local_name!("tr") | local_name!("track") | local_name!("wbr"))
}

/// The start tags that close an open `p`, except `table`: mail without a doctype is in quirks mode.
fn closes_p(name: &LocalName) -> bool {
    block(name) && *name != local_name!("table")
        || matches!(*name, local_name!("li") | local_name!("dd") | local_name!("dt") | local_name!("hr") | local_name!("address") | local_name!("aside") | local_name!("center") | local_name!("details") | local_name!("dialog") | local_name!("dir") | local_name!("fieldset") | local_name!("figcaption") | local_name!("figure") | local_name!("footer") | local_name!("header") | local_name!("hgroup") | local_name!("main") | local_name!("menu") | local_name!("nav") | local_name!("summary") | local_name!("listing") | local_name!("form") | local_name!("xmp") | local_name!("plaintext"))
}

/// The tokenizer state the tree builder sets after this start tag; `Some(None)` is plaintext.
pub(crate) fn raw_kind(name: &LocalName) -> Option<Option<RawKind>> {
    match *name {
        local_name!("title") | local_name!("textarea") => Some(Some(RawKind::Rcdata)),
        local_name!("style") | local_name!("xmp") | local_name!("iframe") | local_name!("noembed") | local_name!("noframes") => Some(Some(RawKind::Rawtext)),
        local_name!("script") => Some(Some(RawKind::ScriptData)),
        local_name!("plaintext") => Some(None),
        _ => None,
    }
}

fn hides(tag: &Tag) -> bool {
    matches!(tag.name, local_name!("head") | local_name!("style") | local_name!("script") | local_name!("title") | local_name!("template") | local_name!("noframes") | local_name!("frameset"))
        || tag.attrs.iter().any(|a| a.name.local == local_name!("hidden") || a.name.local == local_name!("style") && style_hides(&a.value))
}

/// `display: none` or `visibility: hidden` in an inline style. A later declaration wins, unless
/// an earlier one is `!important`.
// shortcut: only simple inline declarations are supported; add a CSS parser/cascade for complex styles or class-based preheaders.
fn style_hides(style: &str) -> bool {
    // Escapes, at-rules, blocks and comments change how CSS reads the declarations; lagom does
    // not model them and treats the element as hidden.
    if style.contains(['\\', '@', '{', '}', '[', ']']) || style.contains("/*") { return true; }
    let (mut display, mut visibility, mut content, mut opaque) = ((false, false), (false, false), (false, false), false);
    declarations(style, |declaration| {
        let Some((name, value)) = declaration.split_once(':') else { return };
        let name = name.trim_matches(css_space);
        let (property, keywords, hidden): (_, &[&str], &[&str]) = if name.eq_ignore_ascii_case("display") {
            (&mut display, DISPLAY, &["none", "table-column", "table-column-group"])
        } else if name.eq_ignore_ascii_case("visibility") {
            (&mut visibility, &["visible", "hidden", "collapse"], &["hidden", "collapse"])
        } else if name.eq_ignore_ascii_case("content-visibility") {
            (&mut content, &["visible", "auto", "hidden"], &["hidden"])
        } else { return };
        // Functions (`var()` can resolve to `none`, and may hold a `!`) count as hidden: lagom
        // does not resolve them.
        if value.contains('(') { opaque = true; return; }
        let (value, important) = match value.rsplit_once('!') {
            Some((value, priority)) if priority.trim_matches(css_space).eq_ignore_ascii_case("important") => (value, true),
            Some(_) => return,
            None => (value, false),
        };
        let value = value.trim_matches(css_space).to_ascii_lowercase();
        // A browser drops an invalid value, so it cannot undo an earlier `none`. Only single,
        // widely supported keywords undo a hidden value.
        let valid = keywords.contains(&value.as_str()) || GLOBAL.contains(&value.as_str());
        if valid && (!property.1 || important) {
            *property = (hidden.contains(&value.as_str()), important);
        }
    });
    opaque || display.0 || visibility.0 || content.0
}

fn css_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

const GLOBAL: &[&str] = &["inherit", "initial", "unset", "revert", "revert-layer"];
const DISPLAY: &[&str] = &[
    "none", "contents", "block", "inline", "flow-root", "table", "flex", "grid", "list-item", "inline-block",
    "inline-table", "inline-flex", "inline-grid", "table-row-group", "table-header-group", "table-footer-group",
    "table-row", "table-cell", "table-column-group", "table-column", "table-caption", "-webkit-box", "-webkit-inline-box",
];

/// Calls `each` with an inline style's declarations, as CSS reads them: split at `;` outside
/// quotes and brackets (`url(a;b)`), with comments removed.
fn declarations(style: &str, mut each: impl FnMut(&str)) {
    let (mut current, mut quote, mut depth) = (String::new(), None, 0usize);
    let mut chars = style.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), _) if c == q => { quote = None; current.push(c); }
            // A newline ends an unclosed string in CSS.
            (Some(_), '\n' | '\r' | '\x0c') => { quote = None; current.push(c); }
            (Some(_), '\\') => { current.push(c); if let Some(n) = chars.next() { current.push(n); } }
            (Some(_), _) => current.push(c),
            (None, '"' | '\'') => { quote = Some(c); current.push(c); }
            (None, '/') if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                for n in chars.by_ref() { if last == '*' && n == '/' { break; } last = n; }
                current.push(' ');
            }
            (None, '(' | '[' | '{') => { depth += 1; current.push(c); }
            (None, ')' | ']' | '}') => { depth = depth.saturating_sub(1); current.push(c); }
            (None, ';') if depth == 0 => { each(&current); current.clear(); }
            (None, _) => current.push(c),
        }
    }
    each(&current);
}

/// Repair an older HTML fallback without replacing a sender's plain-text alternative.
pub fn readable_text<'a>(text: &'a str, html: Option<&str>) -> std::borrow::Cow<'a, str> {
    // shortcut: legacy rows lack provenance; repair exact converter matches until bodies can be re-fetched.
    match html {
        Some(html) if !text.starts_with(html) && text == mail_parser::decoders::html::html_to_text(html) => html_to_text(html).into(),
        _ => text.into(),
    }
}

pub(crate) fn clean_lines(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        if line.trim().is_empty() { blank = !out.is_empty(); continue; }
        if !out.is_empty() {
            out.push('\n');
            if blank { out.push('\n'); }
        }
        out.push_str(line.trim_end());
        blank = false;
    }
    out
}

/// The list preview, at most 200 characters on one line. From the HTML when there is one: text
/// parts are often generated, with CSS and "[Image]" lines in them.
pub fn snippet(text: &str, html: Option<&str>) -> String {
    match html {
        Some(html) => clean_snippet(&html_to_text(html)),
        None => clean_snippet(text),
    }
}

/// Invisible padding dropped, white space collapsed, cut at 200 characters. Also for snippets
/// stored before this cleanup.
pub fn clean_snippet(text: &str) -> String {
    let visible: String = text.chars().filter(|c| !INVISIBLE.contains(c)).collect();
    let mut out = String::new();
    for word in visible.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.len() >= 800 {
            break;
        }
    }
    out.chars().take(200).collect()
}

fn addresses(list: Option<AddressList<'_>>) -> String {
    list.map(|l| {
        l.mailboxes()
            .filter_map(|m| m.address().map(|address| crate::contacts::format_recipient(m.name().unwrap_or_default(), address)))
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_default()
}
pub fn parse(raw: &[u8]) -> Result<Message> {
    parse_within(raw, MAX_MIME_BYTES)
}
pub(crate) fn parse_within(raw: &[u8], limit: usize) -> Result<Message> {
    if raw.len() > limit {
        return Err(Error::Invalid("MIME input exceeds the size limit"));
    }
    let parsed = MessageParser::default()
        .parse(raw)
        .ok_or(Error::Invalid("invalid MIME message"))?;
    let from = parsed.from().and_then(|a| a.first());
    let subject = parsed.subject().unwrap_or_default().to_owned();
    let message_id = parsed.message_id().unwrap_or_default().to_owned();
    let thread_key = parsed
        .references()
        .and_then(|v| v.first())
        .or_else(|| parsed.in_reply_to().and_then(|v| v.first()))
        .unwrap_or(&message_id)
        .to_owned();
    let body_text = parsed
        .text_body()
        .filter_map(|part| match part.kind() {
            PartKind::Text => part.text().map(|text| text.into_owned()),
            PartKind::Html => part.text().map(|html| html_to_text(&html)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let body_html = parsed
        .html_body()
        .find(|part| matches!(part.kind(), PartKind::Html))
        .and_then(|part| part.text())
        .map(|v| v.into_owned());
    let snippet = snippet(&body_text, body_html.as_deref());
    Ok(Message {
        summary: MessageSummary {
            message_id,
            thread_key,
            thread_refs: parsed.references().into_iter().flat_map(|ids| ids.iter())
                .chain(parsed.in_reply_to().into_iter().flat_map(|ids| ids.iter())).collect::<Vec<_>>().join(" "),
            from_name: from.and_then(|m| m.name()).unwrap_or_default().into(),
            from_address: from.and_then(|m| m.address()).unwrap_or_default().into(),
            to: addresses(parsed.to()),
            cc: addresses(parsed.cc()),
            subject,
            date: parsed.date().map(|d| d.to_timestamp()).unwrap_or_default(),
            snippet,
            has_attachments: parsed.has_attachments(),
            size: raw.len() as u32,
            ..Default::default()
        },
        body_text,
        body_html,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, bold: bool, italic: bool, link: Option<&str>) -> Run {
        Run { text: text.into(), bold, italic, link: link.map(Into::into) }
    }

    fn link(href: &str) -> Option<Arc<str>> {
        Some(href.into())
    }

    #[test]
    fn blocks_keep_links_emphasis_and_images() {
        let html = r#"<style>p{color:red}</style><script>alert(1)</script><p>Hi <b>bold</b> and <em>soft</em>, <a href="https://x.example/a">read <i>this</i></a>.</p>
            <table><tr><td><a href="https://shop.example"><img src="cid:logo@x" width="120px"></a></td><td><img src=https://t.example/p.gif width=1></td></tr></table>
            <p>After<br>line</p>"#;
        assert_eq!(html_to_blocks(html), vec![
            Block::Text(vec![run("Hi ", false, false, None), run("bold", true, false, None), run(" and ", false, false, None), run("soft", false, true, None),
                run(", ", false, false, None), run("read ", false, false, Some("https://x.example/a")), run("this", false, true, Some("https://x.example/a")), run(".", false, false, None)]),
            Block::Images(vec![
                Image { src: "cid:logo@x".into(), width: Some(120.), link: link("https://shop.example") },
                Image { src: "https://t.example/p.gif".into(), width: Some(1.), link: None },
            ]),
            Block::Text(vec![run("After\nline", false, false, None)]),
        ]);
    }

    #[test]
    fn blocks_hide_what_text_hides() {
        let html = r#"<div style="display:none"><a href="https://t.example">pre</a><img src="https://t.example/x.gif"></div><img hidden src="https://t.example/y.gif"><p>Kept <a href="https://ok.example">link</a> text</p>"#;
        assert_eq!(html_to_blocks(html), vec![Block::Text(vec![run("Kept ", false, false, None), run("link", false, false, Some("https://ok.example")), run(" text", false, false, None)])]);
        // Mark characters in the mail, raw or as entities, are dropped: no forged link or image,
        // and no replay of a mark that a hidden element cut.
        let forged = format!("<p>a{}b&#xF0000;c&#983041;</p>", char::from_u32(MARK).unwrap());
        assert_eq!(html_to_blocks(&forged), vec![Block::Text(vec![run("abc", false, false, None)])]);
        let replay = r#"<div hidden><img src="https://t.example/x.gif"></div><p>&#xF0000;Shown</p>"#;
        assert_eq!(html_to_blocks(replay), vec![Block::Text(vec![run("Shown", false, false, None)])]);
        // A heading start closes no other heading, and an omitted `</th>` leaves nothing bold.
        assert_eq!(html_to_blocks("<h1>A<h2>B</h2><p>C<table><tr><th>D<td>E</table>"), vec![
            Block::Text(vec![run("A\nB", true, false, None)]), Block::Text(vec![run("C\nD E", false, false, None)]),
        ]);
        // Links inside `pre` stay links and keep the spacing around them.
        assert_eq!(html_to_blocks("<pre>x <a href=https://p.example>y</a>  z</pre>"),
            vec![Block::Text(vec![run("x ", false, false, None), run("y", false, false, Some("https://p.example")), run("  z", false, false, None)])]);
        // An unclosed `<b>` stays bold to the end, as in a browser; a stray `</b>` changes nothing.
        assert_eq!(html_to_blocks("</b><p><b>x</p><p>y"), vec![Block::Text(vec![run("x", true, false, None)]), Block::Text(vec![run("y", true, false, None)])]);
    }

    #[test]
    fn blocks_agree_with_text() {
        let newsletter = include_str!("../tests/fixtures/newsletter.html");
        let text: String = html_to_blocks(newsletter).iter().filter_map(|b| match b {
            Block::Text(runs) => Some(runs.iter().map(|r| r.text.as_str()).collect::<String>()),
            Block::Images(_) => None,
        }).collect::<Vec<_>>().join("\n\n");
        let words = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(words(&text), words(&html_to_text(newsletter)));
    }

    #[test]
    fn multipart_reply_keeps_plain_text_and_quote_levels() {
        let plain = "My reply\n\n> Previous answer\n\nPlain-only detail";
        let raw = format!("Content-Type: multipart/alternative; boundary=reply\r\n\r\n--reply\r\nContent-Type: text/plain\r\n\r\n{plain}\r\n--reply\r\nContent-Type: text/html\r\n\r\n<p>My reply</p><blockquote>Previous answer</blockquote>\r\n--reply--\r\n");
        let m = parse(raw.as_bytes()).unwrap();
        let text = readable_text(&m.body_text, m.body_html.as_deref());
        assert_eq!(text, plain);
        let quote = crate::compose::compose_body("", Some(crate::compose::Original {
            from: "Sender", address: "sender@example.test", date: "", subject: "", to: "", text: &text,
        }), crate::compose::ComposeKind::Reply);
        assert!(quote.contains("> > Previous answer"), "{quote}");
        assert!(quote.ends_with("> Plain-only detail"));
    }

    #[test]
    fn outlook_button_keeps_non_outlook_text() {
        let html = include_str!("../tests/fixtures/outlook-button.html");
        let m = parse(format!("Content-Type: text/html\r\n\r\n{html}").as_bytes()).unwrap();
        assert_eq!(m.body_text, "Read the update");
        assert_eq!(m.summary.snippet, "Read the update");
    }

    #[test]
    fn legacy_generated_text_is_cleaned_but_current_text_is_borrowed() {
        let html = include_str!("../tests/fixtures/newsletter.html");
        let legacy = mail_parser::decoders::html::html_to_text(html);
        let clean = html_to_text(html);
        assert_eq!(readable_text(&legacy, Some(html)), clean);
        assert!(matches!(readable_text(&clean, Some(html)), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn newsletter_body_has_only_visible_text() {
        let html = include_str!("../tests/fixtures/newsletter.html");
        let m = parse(format!("Content-Type: text/html; charset=utf-8\r\n\r\n{html}").as_bytes()).unwrap();
        assert_eq!(m.body_text, "Hello & welcome\n\nRead the news");
        assert_eq!(m.body_html.as_deref(), Some(html));
    }

    #[test]
    fn inline_visibility_respects_declaration_order_and_importance() {
        for (style, visible) in [
            ("display:none;display:block", true),
            ("display:block;display:none", false),
            ("visibility:hidden;visibility:visible", true),
            ("visibility:visible;visibility:hidden", false),
            ("display:none!important;display:block", false),
            ("display:block!important;display:none", true),
            ("display:none;display:block!important", true),
            ("display:block;display:none!important", false),
            ("display:none!important;display:block!important", true),
            ("display:block!important;display:none!important", false),
            ("visibility:hidden!important;visibility:visible", false),
            ("visibility:visible!important;visibility:hidden", true),
            ("visibility:hidden;visibility:visible!important", true),
            ("visibility:visible;visibility:hidden!important", false),
            ("visibility:hidden!important;visibility:visible!important", true),
            ("visibility:visible!important;visibility:hidden!important", false),
            (" DISPLAY : BLOCK ! IMPORTANT ; display : none ", true),
            (" VISIBILITY : VISIBLE ! IMPORTANT ; visibility : hidden ", true),
            ("display:none;visibility:visible!important", false),
            ("visibility:hidden;display:block!important", false),
            ("display:none;display:", false),
            ("display:block;display:none!invalid", true),
            ("display:block;display:none!important junk", true),
            ("display:none;color:red;broken;display:block", true),
        ] {
            let html = format!("<p style='{style}'>Order confirmed</p>");
            let m = parse(format!("Content-Type: text/html\r\n\r\n{html}").as_bytes()).unwrap();
            let expected = if visible { "Order confirmed" } else { "" };
            assert_eq!(m.body_text, expected, "{style}");
            assert_eq!(m.summary.snippet, expected, "{style}");
            let legacy = mail_parser::decoders::html::html_to_text(&html);
            assert_eq!(readable_text(&legacy, Some(&html)), expected, "{style}");
        }
    }

    #[test]
    fn html_text_keeps_visible_links_and_plain_mail() {
        let html = "<title>Title</title><div hidden=false><p>Hidden</p></div><div style='mso-hide:ALL;display:none'>Hidden</div><!-- Hidden --><p>Read <a href='https://tracking.example.test' title='Hidden'>this &amp; that</a>.</p><p>https://visible.example.test</p>";
        assert_eq!(html_to_text(html), "Read this & that.\n\nhttps://visible.example.test");
        assert_eq!(html_to_text("<p>Keep</p><style>unclosed { color: red }"), "Keep");
        assert_eq!(html_to_text("<div hidden><span style='display:none'>Nested</span></div><p>Keep</p>"), "Keep");
        let plain = "A <literal> & URL https://visible.example.test\n\n\n  indented";
        let m = parse(format!("Content-Type: text/plain\r\n\r\n{plain}").as_bytes()).unwrap();
        assert_eq!(m.body_text, plain);
        assert!(m.body_html.is_none());
        assert_eq!(readable_text(&m.body_text, m.body_html.as_deref()), plain);
        // Older parsers also stored the plain body in the HTML slot.
        assert_eq!(readable_text(plain, Some(plain)), plain);
        let multipart = format!("{plain}\nSecond plain part");
        assert_eq!(readable_text(&multipart, Some(plain)), multipart);
    }

    #[test]
    fn recipient_names_survive_sync_headers() {
        let message = parse(b"From: Sender <sender@example.com>\r\nTo: \"Team, West\" <west@example.com>, east@example.com\r\nCc: Team <team@example.com>\r\n\r\nbody").unwrap();
        assert_eq!(crate::contacts::split_recipients(&message.summary.to), ["\"Team, West\" <west@example.com>", "east@example.com"]);
        assert_eq!(message.summary.cc, "Team <team@example.com>");
    }

    #[test]
    fn multipart_and_attachment() {
        let raw=b"From: Alice <alice@example.com>\r\nTo: bob@example.com\r\nSubject: =?utf-8?Q?H=C3=A4lsning?=\r\nMessage-ID: <child@test>\r\nReferences: <root@test> <parent@test>\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: multipart/alternative; boundary=y\r\n\r\n--y\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello plain\r\n--y\r\nContent-Type: text/html\r\n\r\n<b>Hello HTML</b>\r\n--y--\r\n--x\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=a.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--x--\r\n";
        let m = parse(raw).unwrap();
        assert_eq!(m.summary.subject, "Hälsning");
        assert_eq!(m.summary.from_address, "alice@example.com");
        assert_eq!(m.summary.thread_key, "root@test");
        assert!(m.summary.has_attachments);
        assert_eq!(m.body_text, "Hello plain");
        assert!(m.body_html.unwrap().contains("Hello HTML"));
        assert!(!m.body_text.contains("aGVsbG8="));
    }
    #[test]
    fn html_only_and_charset() {
        let m=parse(b"Content-Type: text/html; charset=iso-8859-1\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n<p>H=E4lsning &amp; hej</p>").unwrap();
        assert!(m.body_text.contains("Hälsning & hej"));
        assert!(!m.body_text.contains("<p>"));
        assert!(m.body_html.is_some());
        assert!(!m.summary.has_attachments);
        let m = parse(b"Content-Type: text/plain; charset=windows-1252\r\n\r\nPrice \x80").unwrap();
        assert!(m.body_text.contains('€'));
        assert!(m.body_html.is_none());
    }
    #[test]
    fn newsletter_snippet_has_no_css_or_padding() {
        let raw = "Content-Type: text/html; charset=utf-8\r\n\r\n<!DOCTYPE html><html><head><title>News</title><style>@media only screen and (max-width: 600px) { .container { width: 100% !important; } }</style></head><body><div style=\"display:none\">Autumn sale &#847; &zwnj; &nbsp; &#847; &zwnj;&nbsp;&shy;</div><table><tr><td><style>.x { color: red }</style><p>Fifty&nbsp;kinds of tomato &amp; two&nbsp;&nbsp; kinds of soil</p></td></tr></table></body></html>";
        let m = parse(raw.as_bytes()).unwrap();
        assert_eq!(m.summary.snippet, "Fifty kinds of tomato & two kinds of soil");
        // A generated text part with junk loses to the HTML part.
        assert_eq!(snippet("[Image] Mobile px) { .conta", Some("<p>Real text</p>")), "Real text");
        assert_eq!(snippet("a\u{200C}\u{200C}  b\n\nc", None), "a b c");
        assert_eq!(snippet(&"word ".repeat(100), None).chars().count(), 200);
    }
    #[test]
    fn rejects_empty_and_oversized() {
        assert!(parse(b"").is_err());
        assert!(parse(&vec![0; MAX_MIME_BYTES + 1]).is_err());
        let m = metadata_only(b"From: Big <big@example.com>\r\nSubject: Huge\r\nMessage-ID: <big@x>\r\n\r\n").unwrap();
        assert_eq!((m.summary.subject.as_str(), m.summary.from_address.as_str(), m.body_text.as_str()), ("Huge", "big@example.com", BODY_UNAVAILABLE));
    }

    #[test]
    fn html_bombs_parse_in_linear_time() {
        // Unclosed formatting elements, re-created by a tree builder in every later block: 39 KB
        // took 7.8 s and 1.4 GB with a DOM. Deep nesting and one tag with many attributes were
        // quadratic in time.
        let mut formatting = "<p>".to_string();
        for i in 0..1500 { formatting.push_str(&format!("<b a={i}>")); }
        formatting.push_str("</p>");
        formatting.push_str(&"<p>x\r\n</p>".repeat(2500));
        let mut attributes = "<a".to_string();
        for i in 0..80_000 { attributes.push_str(&format!(" x{i}")); }
        attributes.push_str(">Tail");
        // Each hidden element closed a `pre` and trimmed the same spaces again.
        let spaces = "<pre>".to_string() + &" ".repeat(200_000) + &"<span hidden></pre></span>".repeat(50_000) + "Spaces";
        // Unmatched end tags inside a hidden element search its open elements, at most 256 deep.
        let stray = "<p>Shown</p><div hidden>".to_string() + &"<span>".repeat(255) + &"</b>".repeat(200_000);
        // Block starts check the implied ends of each open element; closed formatting elements
        // collect, at most 256.
        let blocks = "<p>Shown</p><div hidden>".to_string() + &"<span>".repeat(255) + &"<div>".repeat(200_000);
        let closed = "<p>Shown</p><div hidden>".to_string() + &"<span><b><i><u></span>".repeat(50_000);
        // An empty repeated `body` merged nothing but copied every attribute name merged before.
        let names: String = (0..256).map(|i| format!(" a{i}")).collect();
        let merges = (0..20).map(|k| format!("<body{}>", names.replace(" a", &format!(" a{k}x")))).collect::<String>() + &"<body>".repeat(200_000) + "Tail";
        // Same-name formatting elements compare their sorted attributes.
        let long = format!("<b{}>", (0..256).map(|i| format!(" a{i}={}", "v".repeat(40))).collect::<String>());
        let noah = format!("{long}x").repeat(300);
        // Comments and parse errors were nearly free, and the DOM kept every one.
        let comments = "a<!>".repeat(900_000);
        let nul = "\0".repeat(3_500_000) + "Tail";
        let slashes = "<a".to_string() + &"/".repeat(3_500_000) + ">Tail";
        // Past the tree builder's work limit the text is empty: a partial tree can show text the
        // whole document hides.
        for (html, text) in [(formatting, ""), ("<div>".repeat(30_000) + "Deep", ""), (attributes, "Tail"), (spaces, "Spaces"), (stray, ""), (blocks, ""), (closed, ""), (merges, "Tail"), (noah, ""), (comments, ""), (nul, ""), (slashes, "")] {
            let start = std::time::Instant::now();
            let m = parse(format!("Content-Type: text/html\r\n\r\n{html}").as_bytes()).unwrap();
            // About 1 s each in a debug build on a MacBook; Windows runners are several times slower.
            assert!(start.elapsed() < std::time::Duration::from_secs(30), "{:?} for {} bytes", start.elapsed(), html.len());
            assert_eq!(m.body_text.trim_start().chars().take(12).collect::<String>(), text, "{} bytes", html.len());
            let start = std::time::Instant::now();
            html_to_blocks(&html);
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "blocks: {:?} for {} bytes", start.elapsed(), html.len());
        }
    }

    #[test]
    fn trailing_multibyte_whitespace_does_not_panic() {
        assert_eq!(html_to_text("<pre>x\u{3000}</pre>"), "x");
        assert_eq!(html_to_text("<pre>x\u{2003}\u{2003}</pre>"), "x");
    }

    #[test]
    fn svg_and_math_text_stays_hidden() {
        for html in ["<svg>Secret</svg>", "<svg><desc>Secret</desc></svg>", "<svg><metadata>Secret</metadata></svg>",
            "<svg><![CDATA[<div hidden>]]>Secret</svg>", "<math><annotation>Secret</annotation></math>", "<svg><text>Logo</text></svg>"] {
            assert_eq!(html_to_text(html), "", "{html}");
        }
    }

    #[test]
    fn documented_html_leaks() {
        for html in [
            "<div><ul></div><li hidden>x</ul>Secret",
            "<button><ul></button><li hidden>x</ul>Secret",
            "<table><tr><td><ul></td></tr></table><li hidden>x</ul>Secret",
            "<dd><dl></dd><dd hidden></dl>Secret",
            "<template><select></template><select hidden></template>Secret",
            "<svg><select></svg><select hidden></svg>Secret",
            "<svg><summary></svg></summary><summary hidden/></svg>Secret",
            "<math><plaintext></math></plaintext><plaintext hidden/></math>Secret",
            "<div hidden><table><table></table><div></table></div>Secret",
            "<div hidden><button><button></button><div></button></div>Secret",
            "<span hidden><h1><div hidden></h1><h1></div></span>Secret",
            "<table><font hidden/></table><h1 hidden/>w49 <h1></h1></font>Secret",
            "<table><tr><td><template></table></template><td hidden>Secret",
            "<svg><td><form></td></svg><form hidden>Secret",
        ] {
            let message = parse(format!("Content-Type: text/html\r\n\r\n{html}").as_bytes()).unwrap();
            assert_eq!(message.body_text, "", "{html}");
            assert_eq!(message.summary.snippet, "", "{html}");
            // Padding once moved a mail past the DOM budget onto the streaming converter.
            let padded = format!("{}{html}", "<p hidden>x</p>".repeat(20_000));
            assert_eq!(html_to_text(&padded), "", "padded {html}");
        }
    }

    #[test]
    fn many_costly_html_parts_stay_linear() {
        // Each part reached the tree builder's limit on its own: 830 of them took 13 to 18 s.
        let part = "<p>".to_string() + &(0..300).map(|i| format!("<i a={i}>")).collect::<String>() + "</p>" + &"<p>x".repeat(600);
        let mut mail = String::from("Content-Type: multipart/mixed; boundary=b\r\n\r\n");
        while mail.len() < MAX_MIME_BYTES - 2 * part.len() {
            mail.push_str(&format!("--b\r\nContent-Type: text/html\r\n\r\n{part}\r\n"));
        }
        mail.push_str("--b--\r\n");
        let start = std::time::Instant::now();
        parse(mail.as_bytes()).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "{:?}", start.elapsed());
        // The limit follows the input: this part needs about 2 M units, far below the fixed
        // limit, but above 32 per byte.
        let small = "<p>".to_string() + &(0..100).map(|i| format!("<i a={i}>")).collect::<String>() + "</p>" + &"<p>x".repeat(100);
        assert_eq!(html_to_text(&small), "");
        assert_eq!(html_to_text(&"<p>x".repeat(100)), "x\n\n".repeat(100).trim_end());
    }

    #[test]
    fn closed_details_dialog_frames_and_invalid_styles_stay_hidden() {
        for (html, text) in [
            ("<details><summary>Shown</summary>Secret<p>Secret</p></details>", "Shown"),
            ("<details open><summary>Shown</summary> Also</details>", "Shown Also"),
            ("<dialog>Secret</dialog><dialog open>Shown</dialog>", "Shown"),
            // The reader renames `iframe`, so its content is markup there, not raw text.
            ("<iframe><span hidden>Secret</span>Shown</iframe>", "Shown"),
            ("<p style='display:none;display:bogus'>Secret</p>Shown", "Shown"),
            ("<p style='display:none;font-family:\"a;display:block\"'>Secret</p>Shown", "Shown"),
            ("<p style='display:/**/none'>Secret</p>Shown", "Shown"),
            ("<p style='--x:none;display:var(--x)'>Secret</p>Shown", "Shown"),
            ("<p style='visibility:collapse'>Secret</p>Shown", "Shown"),
            ("<p style='display:none;display:block'>Shown</p>", "Shown"),
            ("<p style='display:none!important;display:block'>Secret</p>Shown", "Shown"),
            ("<p style='display:none;display:block flow'>Secret</p>Shown", "Shown"),
            ("<p style='visibility:hidden;visibility:visible visible'>Secret</p>Shown", "Shown"),
            ("<p style='display:none;background:url(a;display:block)'>Secret</p>Shown", "Shown"),
            ("<details><summary hidden>Secret</summary><summary>Secret</summary></details>Shown", "Shown"),
            ("<details name=g open>Shown</details><details name=g open>Secret</details>", "Shown"),
            ("<body>Shown<body>", "Shown"),
            // Measured hidden in WebKit and Chrome with the reader's document around the mail.
            ("<p><table></table><span hidden></p>Secret", ""),
            ("<noscript hidden>Secret</noscript>Shown", "Shown"),
            ("<div><template shadowrootmode=open></template>Secret</div>Shown", "Shown"),
            ("<div popover>Secret</div>Shown", "Shown"),
            ("<datalist>Secret</datalist><video>Secret</video>Shown", "Shown"),
            // A `select` draws one option, maybe by its `label`: none of its text counts.
            ("<select>Secret<option label=Shown>Secret</select>", ""),
            ("<select><option hidden>Hidden<option>Visible</select>Shown", "Shown"),
            ("<details><summary><template shadowrootmode=open></template>A</summary><summary>Secret</summary></details>Shown", "Shown"),
            ("<p style=\"content-visibility:var(--x, '!')\">Secret</p>Shown", "Shown"),
            ("<p style='d\\isplay:none'>Secret</p>Shown", "Shown"),
            ("<p style='display:none!imp\\ortant'>Secret</p>Shown", "Shown"),
            ("<p style=\"font-family:'a\n;display:none\">Secret</p>Shown", "Shown"),
            ("<p style='background:url(/*);display:none;x:url(*/)'>Secret</p>Shown", "Shown"),
            ("<p style='@x{;}display:none'>Secret</p>Shown", "Shown"),
            ("<p style='display:none;display:block\u{a0}'>Secret</p>Shown", "Shown"),
            ("<p style='display:none;display:run-in'>Secret</p>Shown", "Shown"),
            ("<p style='display:table-column'>Secret</p>Shown", "Shown"),
            ("<p style='content-visibility:hidden'>Secret</p>Shown", "Shown"),
        ] {
            assert_eq!(html_to_text(html), text, "{html}");
        }
    }

    #[test]
    fn html_text_matches_the_dom_converter() {
        // Expected text from the earlier dom_query converter on the same input.
        for (html, text) in [
            ("<div style=\"display:none\">Preheader &zwnj;&#847;</div><table role=\"presentation\"><tr><td>Name</td><td>Qty</td></tr><tr><td>Tomato</td><td>2</td></tr></table><p>Read <a href=\"https://example.test/x\">the news</a>.<br>Thanks,<br>Anna</p><ul><li>One</li><li>Two</li></ul><pre>\n  keep\n   spacing</pre><p hidden>gone</p><span style=\"visibility:hidden\">gone</span><p>End</p>",
                "Name Qty\nTomato 2\n\nRead the news.\nThanks,\nAnna\n\nOne\nTwo\n\n  keep\n   spacing\n\nEnd"),
            ("<p>First<p>Second<br>line<div>Block</div>after", "First\n\nSecond\nline\n\nBlock\n\nafter"),
            ("<!DOCTYPE html><html><head><meta charset=utf-8><title>T</title><style>p { color: red }</style></head><body><h1>Title</h1><blockquote>Quoted<br>text</blockquote><table><tr><th>A</th><th>B</th></tr><tr><td>1<td>2</table></body></html>",
                "Title\n\nQuoted\ntext\n\nA B\n1 2"),
            ("<html><head><title>T</title><style>x</style><p>Body", "Body"),
            ("<ul><li>a<li>b</ul>c", "a\nb\n\nc"),
            ("x <em style='display:none'><br> </em>Hello", "x Hello"),
            ("<ul><li hidden>Hidden<li>Visible</ul>", "Visible"),
            ("<ul><li hidden>Hidden<ul><li>Inner</ul>Still<li>Visible</ul>", "Visible"),
            ("<head><template><div>Hidden</div></template></head><body>Visible</body>", "Visible"),
            ("<table><tr><td style='display:none'>Hidden<td>Visible</table>", "Visible"),
            // A tag closes a hidden element only where the tree builder would.
            ("<p hidden><template><div>Hidden</div></template>Still hidden</p><p>Visible</p>", "Visible"),
            ("<p hidden><button><div>Hidden</div></button>Still hidden</p><p>Visible</p>", "Visible"),
            ("<ul><li hidden><blockquote><li>Hidden</blockquote>Still hidden<li>Visible</ul>", "Visible"),
            ("<table><tbody hidden><tr><td>Hidden</td></tr></table><p>Visible</p>", "Visible"),
            ("<svg><g hidden><g/><text>Hidden</text></g></svg><p>Visible</p>", "Visible"),
            ("<p>Visible</p><template>Hidden", "Visible"),
            ("<span style='display:none'>Pre<div>Body", ""),
            // A hidden element left open inside another outlives it.
            ("<i hidden>x<p hidden>y</i>Still hidden", ""),
            ("Visible<body style='display:none'>Hidden", ""),
            // An end tag does not close through a special element (`table`), and closes no
            // hidden formatting element for good: the tree builder opens it again.
            ("<div hidden><span><table></span></div>Secret</table>", ""),
            ("<div hidden><p><i hidden>x</p></div>Secret", ""),
            ("<span hidden><b hidden><font hidden></span></b>Secret", ""),
            ("<b hidden/><dd style='display:none'><b/></dd></b>Secret", ""),
            ("<div hidden><p><b hidden></p><b></b></div>Secret", ""),
            // `</form>` takes only the form off the stack.
            ("<form hidden><div></form>Secret</div>", ""),
            ("<li hidden/><form><ul></form></ul>Secret", ""),
            // A start tag closes elements inside a hidden one as the tree builder does.
            ("<a hidden><p style='display:none'><dd hidden/></p></a>Secret", ""),
            ("<p hidden/><select color=red><p color=red>Secret", ""),
            ("<h1 hidden><option><h2/></h1>Secret", ""),
            // `svg` and `math`: `<div/>` opens in `foreignObject`, and HTML tags break out.
            ("<svg><foreignObject><div hidden/>Secret</div></foreignObject></svg>", ""),
            ("<svg><p><div hidden/>Secret</div>", ""),
            ("<svg><style><div hidden></style>Secret", ""),
            ("<div><svg></div><div hidden/>Secret</div>", ""),
            ("<svg/>Visible", "Visible"),
            ("<p>Hi<svg><path d='M0'/></svg> there", "Hi there"),
            // Tags the tree builder ignores.
            ("<head><head><ul style='display:none'>Secret", ""),
            ("<mi hidden/><tbody/><h1></tbody></mi>Secret", ""),
            ("<form><p hidden/><form>Secret", ""),
            ("<ul><select><li hidden/></ul>Secret", ""),
            ("<ol><li hidden></menu>Secret", ""),
            ("<noframes>Secret", ""),
        ] {
            assert_eq!(clean_lines(read(html, None).text.trim_end()), text, "{html} (streaming)");
            assert_eq!(html_to_text(html), text, "{html}");
        }
        // The streaming fallback can lose visible text on these inputs.
        for (html, dom) in [
            // A heading start may close a hidden heading, unless reopened formatting is in it.
            ("<h1 hidden>Hidden<h2>Shown</h2>", "Shown"),
            // HTML tags in `svg` end it; `svg` text is hidden.
            ("<svg><g/><span hidden>a</span>Shown</svg>", "Shown"),
        ] {
            assert_eq!(html_to_text(html), dom, "{html}");
            assert_eq!(clean_lines(read(html, None).text.trim_end()), "", "{html} (streaming)");
        }
        // `hidden` and `style` count after the tokenizer's 256-attribute cap.
        let many: String = (0..256).map(|i| format!(" x{i}")).collect();
        assert_eq!(html_to_text(&format!("<div{many} hidden>Hidden</div><p{many} style='display:none'>Hidden</p>Visible")), "Visible");
    }
}
