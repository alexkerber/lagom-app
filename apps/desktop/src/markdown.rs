//! Live Markdown for the compose body. The text stays Markdown; this only decides how it looks.
//! It parses with pulldown-cmark's offsets and the same options as the send path, so the field
//! styles what the HTML part will contain.

use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag};

/// How one byte of the source looks.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Look {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
    /// Syntax: `**`, `_`, backticks, `#` and the link brackets and URL.
    pub marker: bool,
    /// A list bullet or number.
    pub bullet: bool,
    /// 1–6 on a heading line.
    pub heading: u8,
}

/// Syntax that hides while the caret is outside `span`, so the caret never moves through hidden text.
#[derive(Debug, PartialEq)]
pub struct Hide {
    pub span: Range<usize>,
    pub markers: Vec<Range<usize>>,
}

/// A replacement in the shown text: `range` of the source shows as `with` ("" hides it).
pub type Swap = (Range<usize>, &'static str);

#[derive(Default, Debug)]
pub struct Markdown {
    /// One per source byte.
    pub looks: Vec<Look>,
    pub hide: Vec<Hide>,
    /// `-`, `*` and `+` list markers, shown as "•".
    pub bullets: Vec<Range<usize>>,
}

// shortcut: one `Look` per byte and a full parse per edit; keep ranges and reparse only the
// edited block if bodies reach hundreds of KB.
pub fn parse(text: &str) -> Markdown {
    let mut md = Markdown { looks: vec![Look::default(); text.len()], ..Default::default() };
    // Open links: start, and the end of the label so far.
    let mut links: Vec<(usize, usize)> = Vec::new();
    for (event, r) in Parser::new(text).into_offset_iter() {
        if let Some((_, label)) = links.last_mut().filter(|(start, _)| r.start > *start) {
            *label = (*label).max(r.end);
        }
        let pair = |n: usize| vec![r.start..r.start + n, r.end - n..r.end];
        match event {
            Event::Start(Tag::Strong) if r.len() >= 4 => {
                set(&mut md.looks, r.clone(), |l| l.bold = true);
                md.hide.push(Hide { markers: pair(2), span: r });
            }
            Event::Start(Tag::Emphasis) if r.len() >= 2 => {
                set(&mut md.looks, r.clone(), |l| l.italic = true);
                md.hide.push(Hide { markers: pair(1), span: r });
            }
            Event::Code(_) => {
                let n = text[r.clone()].bytes().take_while(|b| *b == b'`').count();
                set(&mut md.looks, r.clone(), |l| l.code = true);
                if r.len() >= 2 * n {
                    md.hide.push(Hide { markers: pair(n), span: r });
                }
            }
            Event::Start(Tag::Link { .. }) => {
                set(&mut md.looks, r.clone(), |l| l.link = true);
                links.push((r.start, r.start + 1));
            }
            Event::End(pulldown_cmark::TagEnd::Link) => {
                let Some((start, label)) = links.pop() else { continue };
                // Inline and reference links: hide `[` and everything from `]` on. Autolinks keep their `<>`.
                if text.as_bytes()[start] == b'[' && label < r.end {
                    md.hide.push(Hide { markers: vec![start..start + 1, label..r.end], span: start..r.end });
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let line = r.start..r.start + text[r.clone()].trim_end_matches(['\n', '\r']).len();
                let level = level as u8;
                md.looks[line.clone()].iter_mut().for_each(|l| l.heading = level);
                let hashes = text[line.clone()].bytes().take_while(|b| *b == b'#').count();
                if hashes > 0 {
                    let end = line.start + hashes + text[line.start + hashes..line.end].bytes().take_while(|b| *b == b' ').count();
                    md.hide.push(Hide { markers: std::iter::once(line.start..end).collect(), span: line });
                }
            }
            Event::Start(Tag::Item) => {
                let digits = text[r.clone()].bytes().take_while(u8::is_ascii_digit).count();
                let marker = match text.as_bytes().get(r.start + digits) {
                    Some(b'-' | b'*' | b'+') if digits == 0 => {
                        md.bullets.push(r.start..r.start + 1);
                        1
                    }
                    Some(b'.' | b')') if digits > 0 => digits + 1,
                    _ => 0,
                };
                set(&mut md.looks, r.start..r.start + marker, |l| l.bullet = true);
            }
            _ => {}
        }
    }
    for h in &md.hide {
        for m in &h.markers {
            set(&mut md.looks, m.clone(), |l| l.marker = true);
        }
    }
    md
}

/// The swaps for a caret at `caret`: markers of spans the caret is outside of, and bullets.
pub fn swaps(md: &Markdown, caret: usize) -> Vec<Swap> {
    let mut swaps: Vec<Swap> = md
        .hide
        .iter()
        .filter(|h| caret < h.span.start || caret > h.span.end)
        .flat_map(|h| h.markers.iter().filter(|m| !m.is_empty()).map(|m| (m.clone(), "")))
        .chain(md.bullets.iter().map(|b| (b.clone(), "•")))
        .collect();
    swaps.sort_by_key(|(r, _)| r.start);
    swaps
}

/// The shown text and one look per shown byte.
pub fn shown(text: &str, looks: &[Look], swaps: &[Swap]) -> (String, Vec<Look>) {
    let (mut out, mut out_looks, mut at) = (String::with_capacity(text.len()), Vec::with_capacity(looks.len()), 0);
    for (r, with) in swaps {
        out.push_str(&text[at..r.start]);
        out_looks.extend_from_slice(&looks[at..r.start]);
        out.push_str(with);
        out_looks.extend(std::iter::repeat_n(looks[r.start], with.len()));
        at = r.end;
    }
    out.push_str(&text[at..]);
    out_looks.extend_from_slice(&looks[at..]);
    (out, out_looks)
}

/// A source offset in the shown text. Inside a swapped range it snaps to the range's start.
pub fn to_shown(swaps: &[Swap], offset: usize) -> usize {
    let mut delta = 0isize;
    for (r, with) in swaps {
        if r.end <= offset {
            delta += with.len() as isize - r.len() as isize;
        } else {
            if r.start < offset {
                return r.start.saturating_add_signed(delta);
            }
            break;
        }
    }
    offset.saturating_add_signed(delta)
}

/// A shown offset in the source. At a hidden range it is the offset before it, so a click
/// next to hidden syntax lands at the edge of its span and reveals it.
pub fn from_shown(swaps: &[Swap], shown: usize) -> usize {
    let mut delta = 0isize;
    for (r, with) in swaps {
        let start = r.start.saturating_add_signed(-delta);
        if shown <= start {
            break;
        }
        if shown < start + with.len() {
            return r.end;
        }
        delta += r.len() as isize - with.len() as isize;
    }
    shown.saturating_add_signed(delta)
}

fn set(looks: &mut [Look], r: Range<usize>, f: fn(&mut Look)) {
    looks[r].iter_mut().for_each(f);
}

/// A drag between two source offsets, as the pointer maps them: at hidden syntax a pointer
/// maps before it, which suits the selection's end. Its start moves past the syntax, so a drag
/// over a hidden-bold word selects the word, not one of its markers.
pub fn drag(swaps: &[Swap], from: usize, to: usize) -> (usize, usize) {
    let past = |mut offset: usize| {
        for (r, with) in swaps {
            if with.is_empty() && r.start == offset {
                offset = r.end;
            }
        }
        offset
    };
    match to.cmp(&from) {
        std::cmp::Ordering::Equal => (from, to),
        std::cmp::Ordering::Greater => (past(from).min(to), to),
        std::cmp::Ordering::Less => (from, past(to).min(from)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source ranges where `f` holds, merged.
    fn ranges(text: &str, f: fn(&Look) -> bool) -> Vec<&str> {
        let md = parse(text);
        let mut out = Vec::new();
        let mut start = None;
        for i in 0..=text.len() {
            match (start, md.looks.get(i).is_some_and(f)) {
                (None, true) => start = Some(i),
                (Some(s), false) => {
                    out.push(&text[s..i]);
                    start = None;
                }
                _ => {}
            }
        }
        out
    }

    const SAMPLE: &str = "# Rubrik\n\nasd **fet** _kursiv_ *också* `kod` [Lagom](https://example.com)\n\n- ett\n1. två\n";

    #[test]
    fn styles_match_the_sent_markdown() {
        assert!(ranges("**test **", |l| *l != Look::default()).is_empty());
        assert_eq!(ranges(SAMPLE, |l| l.heading == 1), ["# Rubrik"]);
        assert_eq!(ranges(SAMPLE, |l| l.bold), ["**fet**"]);
        assert_eq!(ranges(SAMPLE, |l| l.italic), ["_kursiv_", "*också*"]);
        assert_eq!(ranges(SAMPLE, |l| l.code), ["`kod`"]);
        assert_eq!(ranges(SAMPLE, |l| l.link), ["[Lagom](https://example.com)"]);
        assert_eq!(ranges(SAMPLE, |l| l.bullet), ["-", "1."]);
        assert_eq!(
            ranges(SAMPLE, |l| l.marker),
            ["# ", "**", "**", "_", "_", "*", "*", "`", "`", "[", "](https://example.com)"]
        );
        // Not Markdown: nothing styled.
        assert!(ranges("a * b _ c ** d", |l| *l != Look::default()).is_empty());
    }

    #[test]
    fn markers_hide_unless_the_caret_is_in_the_span() {
        let text = "asd **asd** _test_\n- x";
        let md = parse(text);
        let shown_at = |caret| shown(text, &md.looks, &swaps(&md, caret)).0;
        assert_eq!(shown_at(0), "asd asd test\n• x");
        // At either edge of the bold span its markers show; the italic ones stay hidden.
        assert_eq!(shown_at(4), "asd **asd** test\n• x");
        assert_eq!(shown_at(11), "asd **asd** test\n• x");
        assert_eq!(shown_at(14), "asd asd _test_\n• x");
    }

    #[test]
    fn offsets_round_trip_through_hidden_and_swapped_text() {
        let text = "a **b** c\n- d";
        let md = parse(text);
        let s = swaps(&md, 0);
        assert_eq!(shown(text, &md.looks, &s).0, "a b c\n• d");
        // "b" is at 4 in the source, 2 shown; after the closing marker the source is 2 ahead.
        assert_eq!(to_shown(&s, 4), 2);
        assert_eq!(to_shown(&s, 7), 3);
        assert_eq!(to_shown(&s, 5), 3);
        // Inside a hidden marker: its start.
        assert_eq!(to_shown(&s, 6), 3);
        assert_eq!(from_shown(&s, 2), 2);
        assert_eq!(from_shown(&s, 3), 5);
        // "•" (shown 6..9) is three bytes for the one-byte "-" at 10; inside it snaps after.
        assert_eq!(to_shown(&s, 11), 9);
        assert_eq!(from_shown(&s, 9), 11);
        assert_eq!(from_shown(&s, 7), 11);
        // A drag over the shown "b" selects "b" both ways, without a hidden marker.
        let (from, to) = (from_shown(&s, 2), from_shown(&s, 3));
        assert_eq!(drag(&s, from, to), (4, 5));
        assert_eq!(drag(&s, to, from), (5, 4));
        assert_eq!(drag(&s, from, from), (2, 2));
        for offset in [0, 2, 5, 8, text.len()] {
            assert_eq!(from_shown(&s, to_shown(&s, offset)), offset, "{offset}");
        }
    }
}
