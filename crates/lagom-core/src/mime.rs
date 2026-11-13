use crate::{
    store::{Message, MessageSummary},
    Error, Result,
};
use html5ever::tokenizer::{states::RawKind, BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer};
use html5ever::{local_name, LocalName};
use mail_parser::{AddressList, MessageParser, PartKind};

pub const MAX_MIME_BYTES: usize = 4 * 1024 * 1024;
/// The stored body of a message whose text is over `MAX_MIME_BYTES`: the headers are kept, the
/// text is not downloaded. Readers compare against this to show why the body is missing.
pub const BODY_UNAVAILABLE: &str = "The text of this message is larger than 4 MiB and was not downloaded.";

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
/// One pass over html5ever's tokens, without a tree builder: a tree builder re-creates every
/// unclosed formatting element in each later block, so a 20 KB mail can build millions of nodes
/// at sync. Time and memory here are linear in the input.
pub fn html_to_text(html: &str) -> String {
    let queue = BufferQueue::default();
    queue.push_back(html.into());
    let tokenizer = Tokenizer::new(TextSink::default(), Default::default());
    let _ = tokenizer.feed(&queue);
    tokenizer.end();
    clean_lines(tokenizer.sink.0.into_inner().text.trim_end())
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
}

/// A hidden element: how many elements of its name are open, how many of its containers are open
/// inside it (see `container`), and the text length and state when it started.
struct Hidden {
    name: LocalName,
    open: usize,
    nested: usize,
    at: usize,
    pre: usize,
    p: bool,
}

impl TokenSink for TextSink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut state = self.0.borrow_mut();
        match token {
            Token::TagToken(tag) => return state.tag(tag),
            Token::CharacterTokens(chars) => state.chars(&chars),
            // Unclosed at the end: `head` and raw text elements stay hidden, as in a DOM. Any other
            // hidden element shows, so one unmatched end tag cannot hide the rest of a mail.
            Token::EOFToken => {
                if let Some(hidden) = &state.hidden {
                    if raw_kind(&hidden.name).is_some() || hidden.name == local_name!("head") {
                        let at = hidden.at;
                        state.text.truncate(at);
                    }
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

impl TextState {
    fn unhide(&mut self) {
        if let Some(hidden) = self.hidden.take() {
            self.text.truncate(hidden.at);
            (self.pre, self.p) = (hidden.pre, hidden.p);
        }
    }

    fn tag(&mut self, tag: Tag) -> TokenSinkResult<()> {
        let (name, start) = (&tag.name, tag.kind == TagKind::StartTag);
        self.raw = false;
        self.skip_newline = false;
        // The tree builder closes some elements without an end tag: at a sibling's start or at
        // their container's end, unless that tag is inside a container opened within them.
        if let Some(hidden) = &mut self.hidden {
            if hidden.nested == 0 && implied_end(&hidden.name, name, start) {
                self.unhide();
            } else if container(&hidden.name, name) {
                if start { hidden.nested += 1; } else { hidden.nested = hidden.nested.saturating_sub(1); }
            }
        }
        let void = matches!(*name, local_name!("area") | local_name!("base") | local_name!("br") | local_name!("col") | local_name!("embed") | local_name!("hr") | local_name!("img") | local_name!("image") | local_name!("input") | local_name!("link") | local_name!("meta") | local_name!("source") | local_name!("track") | local_name!("wbr") | local_name!("param") | local_name!("keygen") | local_name!("basefont") | local_name!("bgsound") | local_name!("frame"));
        let result = match raw_kind(name) {
            Some(_) if !start => TokenSinkResult::Continue,
            Some(Some(kind)) => { self.raw = true; TokenSinkResult::RawData(kind) }
            Some(None) => TokenSinkResult::Plaintext,
            None => TokenSinkResult::Continue,
        };
        let closes_p = start && closes_p(name);
        match &mut self.hidden {
            Some(hidden) if hidden.name == *name && hidden.name != local_name!("head") => {
                if start { hidden.open += 1; } else { hidden.open -= 1; }
                if hidden.open == 0 {
                    self.unhide();
                    return result;
                }
            }
            Some(_) => {}
            None if start && hides(&tag) => {
                // A hidden void element (`<br hidden>`) breaks no line.
                if void { return result; }
                self.hidden = Some(Hidden { name: name.clone(), open: 1, nested: 0, at: self.text.len(), pre: self.pre, p: self.p });
            }
            None => {}
        }
        if self.pre > 0 {
            if *name == local_name!("pre") {
                if start { self.pre += 1; } else { self.pre -= 1; }
                if self.pre == 0 { self.close(name); }
            }
            return result;
        }
        if !start {
            self.p &= *name != local_name!("p");
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
        result
    }

    fn chars(&mut self, chars: &str) {
        // Text ends `head`, unless it is in a `title`, `style` or `template` there.
        if self.hidden.as_ref().is_some_and(|h| h.name == local_name!("head") && h.nested == 0) && !self.raw && !chars.trim().is_empty() {
            self.unhide();
        }
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

/// Whether this tag closes an open `hidden` element without its end tag.
fn implied_end(hidden: &LocalName, name: &LocalName, start: bool) -> bool {
    match *hidden {
        local_name!("head") => if start { !matches!(*name, local_name!("head") | local_name!("base") | local_name!("basefont") | local_name!("bgsound") | local_name!("link") | local_name!("meta") | local_name!("title") | local_name!("style") | local_name!("script") | local_name!("noscript") | local_name!("template") | local_name!("noframes")) } else { *name == local_name!("head") },
        local_name!("p") => start && closes_p(name),
        local_name!("li") => if start { *name == local_name!("li") } else { matches!(*name, local_name!("ul") | local_name!("ol") | local_name!("menu")) },
        local_name!("dt") | local_name!("dd") => if start { matches!(*name, local_name!("dt") | local_name!("dd")) } else { *name == local_name!("dl") },
        local_name!("td") | local_name!("th") => matches!(*name, local_name!("tr")) || if start { matches!(*name, local_name!("td") | local_name!("th")) } else { matches!(*name, local_name!("tbody") | local_name!("thead") | local_name!("tfoot") | local_name!("table")) },
        local_name!("tr") => if start { *name == local_name!("tr") } else { matches!(*name, local_name!("tbody") | local_name!("thead") | local_name!("tfoot") | local_name!("table")) },
        _ => false,
    }
}

/// An element inside `hidden` that holds its own `implied_end` tags: a nested list or table.
fn container(hidden: &LocalName, name: &LocalName) -> bool {
    match *hidden {
        local_name!("head") => *name == local_name!("template"),
        local_name!("li") => matches!(*name, local_name!("ul") | local_name!("ol") | local_name!("menu")),
        local_name!("dt") | local_name!("dd") => *name == local_name!("dl"),
        local_name!("td") | local_name!("th") | local_name!("tr") => *name == local_name!("table"),
        _ => false,
    }
}

/// The start tags that close an open `p`, except `table`: mail without a doctype is in quirks mode.
fn closes_p(name: &LocalName) -> bool {
    block(name) && *name != local_name!("table")
        || matches!(*name, local_name!("li") | local_name!("dd") | local_name!("dt") | local_name!("hr") | local_name!("address") | local_name!("aside") | local_name!("center") | local_name!("details") | local_name!("dialog") | local_name!("dir") | local_name!("fieldset") | local_name!("figcaption") | local_name!("figure") | local_name!("footer") | local_name!("header") | local_name!("hgroup") | local_name!("main") | local_name!("menu") | local_name!("nav") | local_name!("summary") | local_name!("listing") | local_name!("form") | local_name!("xmp") | local_name!("plaintext"))
}

/// The tokenizer state the tree builder sets after this start tag; `Some(None)` is plaintext.
fn raw_kind(name: &LocalName) -> Option<Option<RawKind>> {
    match *name {
        local_name!("title") | local_name!("textarea") => Some(Some(RawKind::Rcdata)),
        local_name!("style") | local_name!("xmp") | local_name!("iframe") | local_name!("noembed") | local_name!("noframes") => Some(Some(RawKind::Rawtext)),
        local_name!("script") => Some(Some(RawKind::ScriptData)),
        local_name!("plaintext") => Some(None),
        _ => None,
    }
}

fn hides(tag: &Tag) -> bool {
    matches!(tag.name, local_name!("head") | local_name!("style") | local_name!("script") | local_name!("title") | local_name!("template"))
        || tag.attrs.iter().any(|a| a.name.local == local_name!("hidden") || a.name.local == local_name!("style") && style_hides(&a.value))
}

/// `display: none` or `visibility: hidden` in an inline style. A later declaration wins, unless
/// an earlier one is `!important`.
// shortcut: only simple inline declarations are supported; add a CSS parser/cascade for complex styles or class-based preheaders.
fn style_hides(style: &str) -> bool {
    let (mut display, mut visibility) = ((false, false), (false, false));
    for declaration in style.split(';') {
        let Some((name, value)) = declaration.split_once(':') else { continue; };
        let (property, hidden) = if name.trim().eq_ignore_ascii_case("display") {
            (&mut display, "none")
        } else if name.trim().eq_ignore_ascii_case("visibility") {
            (&mut visibility, "hidden")
        } else { continue; };
        let (value, important) = match value.split_once('!') {
            Some((value, priority)) if priority.trim().eq_ignore_ascii_case("important") => (value, true),
            Some(_) => continue,
            None => (value, false),
        };
        if !value.trim().is_empty() && (!property.1 || important) {
            *property = (value.trim().eq_ignore_ascii_case(hidden), important);
        }
    }
    display.0 || visibility.0
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
    if raw.len() > MAX_MIME_BYTES {
        return Err(Error::Invalid("MIME input exceeds 4 MiB"));
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
        for (html, text) in [(formatting, "x\n\nx"), ("<div>".repeat(30_000) + "Deep", "Deep"), (attributes, "Tail"), (spaces, "Spaces")] {
            let start = std::time::Instant::now();
            let m = parse(format!("Content-Type: text/html\r\n\r\n{html}").as_bytes()).unwrap();
            assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?} for {} bytes", start.elapsed(), html.len());
            assert!(m.body_text.trim_start().starts_with(text), "{:?}", &m.body_text.trim_start()[..20.min(m.body_text.trim_start().len())]);
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
        ] {
            assert_eq!(html_to_text(html), text, "{html}");
        }
        // An unclosed hidden element shows its text rather than hide the rest of the mail.
        assert_eq!(html_to_text("<span style='display:none'>Pre<div>Body"), "Pre\nBody");
    }
}
