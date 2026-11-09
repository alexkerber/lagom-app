//! A text field: single-line, or multi-line with wrapping. Selection by mouse and keys, word and
//! line movement, clipboard, undo and IME (marked text). Built on GPUI's `examples/input.rs`.
//! The editing model, `Buffer`, is plain Rust so tests can drive it without a window.

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    anchored, deferred, div, fill, point, prelude::*, px, relative, size, Action, App, Bounds, BoxShadow,
    ClipboardItem, Context, CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight, GlobalElementId, Hsla,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    Rgba, ScrollWheelEvent, SharedString, Style, Subscription, Task, TextAlign, TextRun, TextStyle,
    UTF16Selection, UnderlineStyle, Window, WrappedLine,
};
use unicode_segmentation::GraphemeCursor;

use crate::main_window::{icon, Undo};
use crate::markdown::{self, Look, Markdown, Swap};
use crate::theme::{radius, Theme, DEFAULT_ACCENT};

gpui::actions!(
    text_input,
    [
        Backspace, Delete, DeleteWordLeft, DeleteWordRight, Left, Right, Up, Down, SelectLeft,
        SelectRight, SelectUp, SelectDown, WordLeft, WordRight, SelectWordLeft, SelectWordRight,
        LineStart, LineEnd, SelectLineStart, SelectLineEnd, DocStart, DocEnd, SelectDocStart,
        SelectDocEnd, SelectAll, Copy, Cut, Paste, Redo, Enter, Escape, FocusNext, FocusPrev,
        ShowCharacterPalette, Bold, Italic, Bullets, Numbers, Link
    ]
);

/// macOS keys. Undo (⌘Z) comes from the main window's binding: a focused field handles it first.
/// shortcut: ⌘⌫ (delete to line start) is not bound, as Compose and the inbox use ⌘⌫; bind it in
/// a context that excludes Compose once a form needs it. Linux and Windows word keys (ctrl-arrows)
/// are not bound yet; add them with the first non-macOS build.
pub fn bind_keys(cx: &mut App) {
    let ctx = Some("TextInput");
    // Single-line fields leave ↑ and ↓ to their parent, so the palette can move its selection.
    let multi = Some("TextInput && multiline");
    let markdown = Some("TextInput && markdown");
    cx.bind_keys([
        KeyBinding::new("secondary-b", Bold, markdown),
        KeyBinding::new("secondary-i", Italic, markdown),
        KeyBinding::new("secondary-shift-8", Bullets, markdown),
        KeyBinding::new("secondary-shift-7", Numbers, markdown),
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("shift-backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("alt-backspace", DeleteWordLeft, ctx),
        KeyBinding::new("alt-delete", DeleteWordRight, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("alt-left", WordLeft, ctx),
        KeyBinding::new("alt-right", WordRight, ctx),
        KeyBinding::new("alt-shift-left", SelectWordLeft, ctx),
        KeyBinding::new("alt-shift-right", SelectWordRight, ctx),
        KeyBinding::new("cmd-left", LineStart, ctx),
        KeyBinding::new("cmd-right", LineEnd, ctx),
        KeyBinding::new("home", LineStart, ctx),
        KeyBinding::new("end", LineEnd, ctx),
        KeyBinding::new("cmd-shift-left", SelectLineStart, ctx),
        KeyBinding::new("cmd-shift-right", SelectLineEnd, ctx),
        KeyBinding::new("shift-home", SelectLineStart, ctx),
        KeyBinding::new("shift-end", SelectLineEnd, ctx),
        KeyBinding::new("cmd-up", DocStart, ctx),
        KeyBinding::new("cmd-down", DocEnd, ctx),
        KeyBinding::new("cmd-shift-up", SelectDocStart, ctx),
        KeyBinding::new("cmd-shift-down", SelectDocEnd, ctx),
        KeyBinding::new("up", Up, multi),
        KeyBinding::new("down", Down, multi),
        KeyBinding::new("shift-up", SelectUp, multi),
        KeyBinding::new("shift-down", SelectDown, multi),
        KeyBinding::new("secondary-a", SelectAll, ctx),
        KeyBinding::new("secondary-c", Copy, ctx),
        KeyBinding::new("secondary-x", Cut, ctx),
        KeyBinding::new("secondary-v", Paste, ctx),
        KeyBinding::new("secondary-shift-z", Redo, ctx),
        KeyBinding::new("enter", Enter, ctx),
        KeyBinding::new("shift-enter", Enter, ctx),
        KeyBinding::new("escape", Escape, ctx),
        KeyBinding::new("tab", FocusNext, ctx),
        KeyBinding::new("shift-tab", FocusPrev, ctx),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, ctx),
    ]);
}

// ---- Editing model ------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    /// The logical line (between newlines). The field uses the visual line when it has a layout.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

/// Prefix bytes (including indentation), and the number for an ordered item.
fn list_prefix(line: &str) -> Option<(usize, Option<u64>)> {
    let item = line.trim_start_matches([' ', '\t']);
    let indent = line.len() - item.len();
    if item.starts_with("- ") || item.starts_with("* ") {
        return Some((indent + 2, None));
    }
    let digits = item.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) && item[digits..].starts_with(". ") {
        Some((indent + digits + 2, Some(item[..digits].parse().ok()?)))
    } else {
        None
    }
}

/// One undo step: `new` replaced `old` at `at`. `sel` is the selection before it.
#[derive(Debug)]
struct Edit {
    at: usize,
    old: String,
    new: String,
    sel: Range<usize>,
    reversed: bool,
    /// The selection a formatting edit leaves, which redo restores; else the caret after `new`.
    after: Option<Range<usize>>,
}

impl Edit {
    /// Joins a follow-up edit into this step: typing on, IME re-composing, backspace or delete runs.
    fn absorb(&mut self, e: &Edit) -> bool {
        let end = self.at + self.new.len();
        if e.at >= self.at && e.at + e.old.len() == end && self.new.ends_with(&e.old) {
            self.new.truncate(e.at - self.at);
            self.new.push_str(&e.new);
        } else if self.new.is_empty() && e.new.is_empty() && e.at + e.old.len() == self.at {
            self.at = e.at;
            self.old.insert_str(0, &e.old);
        } else if self.new.is_empty() && e.new.is_empty() && e.at == self.at {
            self.old.push_str(&e.old);
        } else {
            return false;
        }
        true
    }
}

/// Text, selection, marked (IME) range and undo history. Offsets are UTF-8 bytes on char boundaries.
/// shortcut: a `String` and a memmove per edit; switch to a rope if bodies reach megabytes.
#[derive(Default, Debug)]
pub struct Buffer {
    text: String,
    sel: Range<usize>,
    /// The caret is at `sel.start` instead of `sel.end`.
    reversed: bool,
    marked: Option<Range<usize>>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// The next edit may join the last undo step: nothing moved the caret in between.
    join: bool,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self { text: text.into(), sel: text.len()..text.len(), ..Default::default() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn selection(&self) -> Range<usize> {
        self.sel.clone()
    }

    pub fn caret(&self) -> usize {
        if self.reversed { self.sel.start } else { self.sel.end }
    }

    fn anchor(&self) -> usize {
        if self.reversed { self.sel.end } else { self.sel.start }
    }

    pub fn select(&mut self, anchor: usize, caret: usize) {
        self.reversed = caret < anchor;
        self.sel = anchor.min(caret)..anchor.max(caret);
        self.join = false;
    }

    pub fn move_to(&mut self, offset: usize) {
        self.select(offset, offset);
    }

    pub fn select_to(&mut self, offset: usize) {
        self.select(self.anchor(), offset);
    }

    /// The range, markers included, where `marker` already wraps the selection: inside it or
    /// around it. `wrap` removes those markers, and the format bar shows the button as on.
    fn wrapping(&self, marker: &str) -> Option<Range<usize>> {
        let (sel, n) = (self.selection(), marker.len());
        let selected = &self.text[sel.clone()];
        if selected.len() >= n * 2 && selected.starts_with(marker) && selected.ends_with(marker) {
            Some(sel)
        } else if sel.start >= n && self.text[..sel.start].ends_with(marker) && self.text[sel.end..].starts_with(marker) {
            Some(sel.start - n..sel.end + n)
        } else {
            None
        }
    }

    /// Selects after a formatting edit, and keeps that selection for redo.
    fn select_edited(&mut self, start: usize, end: usize) {
        self.select(start, end);
        if let Some(e) = self.undo.last_mut() {
            e.after = Some(start..end);
        }
    }

    fn wrap(&mut self, marker: &str) {
        let n = marker.len();
        if let Some(outer) = self.wrapping(marker) {
            let inner = self.text[outer.start + n..outer.end - n].to_string();
            self.replace(Some(outer.clone()), &inner, false);
            self.select_edited(outer.start, outer.start + inner.len());
        } else {
            let sel = self.selection();
            let wrapped = format!("{marker}{}{marker}", &self.text[sel.clone()]);
            self.replace(Some(sel.clone()), &wrapped, false);
            self.select_edited(sel.start + n, sel.end + n);
        }
    }

    /// `[selection](https://)` with the URL selected, so a paste replaces it.
    fn link(&mut self) {
        const URL: &str = "https://";
        let sel = self.selection();
        let label = self.text[sel.clone()].to_string();
        self.replace(Some(sel.clone()), &format!("[{label}]({URL})"), false);
        let url = sel.start + label.len() + "](".len() + 1;
        self.select_edited(url, url + URL.len());
    }

    fn list_enter(&mut self) {
        let sel = self.selection();
        let start = self.text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.text[sel.end..].find('\n').map_or(self.text.len(), |i| sel.end + i);
        let line = &self.text[start..line_end];
        if let Some((prefix, number)) = list_prefix(line).filter(|(prefix, _)| sel.start >= start + prefix) {
            if line[prefix..].trim().is_empty() {
                self.replace(Some(start..line_end), "", false);
            } else {
                let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
                let marker = match number {
                    Some(n) => format!("{}. ", n + 1),
                    None => line[indent.len()..prefix].to_string(),
                };
                let next = format!("\n{indent}{marker}");
                self.replace(None, &next, false);
            }
        } else {
            self.replace(None, "\n", true);
        }
    }

    fn toggle_list(&mut self, ordered: bool) {
        let sel = self.selection();
        let start = self.text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
        let end = if sel.end > sel.start && self.text[..sel.end].ends_with('\n') {
            sel.end - 1
        } else {
            self.text[sel.end..].find('\n').map_or(self.text.len(), |i| sel.end + i)
        };
        let lines = &self.text[start..end];
        let remove = lines.split('\n').all(|line| list_prefix(line).is_some_and(|(_, n)| n.is_some() == ordered));
        let mut first_delta = 0;
        let replacement = lines.split('\n').enumerate().map(|(i, line)| {
            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
            let old = list_prefix(line).map_or(indent, |(len, _)| len);
            let marker = if remove { String::new() } else if ordered { format!("{}. ", i + 1) } else { "- ".into() };
            if i == 0 { first_delta = (indent + marker.len()) as isize - old as isize; }
            format!("{}{marker}{}", &line[..indent], &line[old..])
        }).collect::<Vec<_>>().join("\n");
        self.replace(Some(start..end), &replacement, false);
        if sel.is_empty() {
            let caret = sel.start.saturating_add_signed(first_delta).clamp(start, start + replacement.len());
            self.select_edited(caret, caret);
        } else {
            self.select_edited(start, start + replacement.len());
        }
    }

    pub fn target(&self, m: Motion) -> usize {
        let (t, c) = (&self.text, self.caret());
        match m {
            Motion::Left => GraphemeCursor::new(c, t.len(), true).prev_boundary(t, 0).ok().flatten().unwrap_or(0),
            Motion::Right => GraphemeCursor::new(c, t.len(), true).next_boundary(t, 0).ok().flatten().unwrap_or(t.len()),
            // macOS: ⌥← goes to the start of this or the previous word, ⌥→ to the end of this or the next.
            // shortcut: a word is a run of alphanumerics, so CJK text moves by whole runs; use UAX #29
            // word bounds (unicode-segmentation) per paragraph if that matters.
            Motion::WordLeft => {
                let s = t[..c].trim_end_matches(|ch: char| !is_word(ch));
                s.trim_end_matches(is_word).len()
            }
            Motion::WordRight => {
                let s = t[c..].trim_start_matches(|ch: char| !is_word(ch));
                t.len() - s.trim_start_matches(is_word).len()
            }
            Motion::LineStart => t[..c].rfind('\n').map_or(0, |i| i + 1),
            Motion::LineEnd => t[c..].find('\n').map_or(t.len(), |i| c + i),
            Motion::DocStart => 0,
            Motion::DocEnd => t.len(),
        }
    }

    /// Moves the caret, or extends the selection. ← and → without shift collapse a selection first.
    pub fn motion(&mut self, m: Motion, select: bool) {
        match m {
            _ if select => self.select_to(self.target(m)),
            Motion::Left if !self.sel.is_empty() => self.move_to(self.sel.start),
            Motion::Right if !self.sel.is_empty() => self.move_to(self.sel.end),
            _ => self.move_to(self.target(m)),
        }
    }

    /// Deletes the selection, or from the caret to the target of `m`.
    pub fn delete(&mut self, m: Motion) -> bool {
        let range = if self.sel.is_empty() {
            let (c, t) = (self.caret(), self.target(m));
            c.min(t)..c.max(t)
        } else {
            self.sel.clone()
        };
        !range.is_empty() && self.replace(Some(range), "", true)
    }

    /// The word (or run of non-word characters) around `offset`, for a double-click.
    pub fn word_at(&self, offset: usize) -> Range<usize> {
        let t = &self.text;
        let kind = t[offset..].chars().next().or(t[..offset].chars().next_back()).is_none_or(is_word);
        let start = t[..offset].trim_end_matches(|c: char| is_word(c) == kind).len();
        let end = t.len() - t[offset..].trim_start_matches(|c: char| is_word(c) == kind).len();
        start..end
    }

    /// Replaces `range` (by default the marked text, else the selection) and puts the caret after it.
    /// `join`: typing and deleting, which undo as one step until the caret moves.
    pub fn replace(&mut self, range: Option<Range<usize>>, new: &str, join: bool) -> bool {
        let range = range.or(self.marked.clone()).unwrap_or(self.sel.clone());
        self.marked = None;
        if range.is_empty() && new.is_empty() {
            return false;
        }
        let edit = Edit {
            at: range.start,
            old: self.text[range.clone()].to_string(),
            new: new.to_string(),
            sel: self.sel.clone(),
            reversed: self.reversed,
            after: None,
        };
        self.text.replace_range(range.clone(), new);
        let caret = range.start + new.len();
        self.sel = caret..caret;
        self.reversed = false;
        self.redo.clear();
        let joined = join && self.join && self.undo.last_mut().is_some_and(|last| last.absorb(&edit));
        if !joined {
            self.undo.push(edit);
        }
        self.join = join;
        true
    }

    /// IME: replaces like `replace`, then marks the new text. `sel` is relative to `new`.
    pub fn replace_and_mark(&mut self, range: Option<Range<usize>>, new: &str, sel: Option<Range<usize>>) {
        let start = range.clone().or(self.marked.clone()).unwrap_or(self.sel.clone()).start;
        self.replace(range, new, true);
        self.marked = (!new.is_empty()).then(|| start..start + new.len());
        if let Some(s) = sel {
            self.sel = start + s.start..start + s.end;
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(e) = self.undo.pop() else { return false };
        self.text.replace_range(e.at..e.at + e.new.len(), &e.old);
        self.sel = e.sel.clone();
        self.reversed = e.reversed;
        self.marked = None;
        self.join = false;
        self.redo.push(e);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(e) = self.redo.pop() else { return false };
        self.text.replace_range(e.at..e.at + e.old.len(), &e.new);
        let caret = e.at + e.new.len();
        self.sel = e.after.clone().unwrap_or(caret..caret);
        self.reversed = false;
        self.marked = None;
        self.join = false;
        self.undo.push(e);
        true
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

const BULLET: char = '•';

/// Password mode shows one bullet per character.
fn mask(text: &str) -> String {
    std::iter::repeat_n(BULLET, text.chars().count()).collect()
}

fn to_mask(text: &str, offset: usize) -> usize {
    text[..offset].chars().count() * BULLET.len_utf8()
}

fn from_mask(text: &str, offset: usize) -> usize {
    text.char_indices().nth(offset / BULLET.len_utf8()).map_or(text.len(), |(i, _)| i)
}

// shortcut: UTF-16 offsets for the IME scan from the start of the text, O(n) per IME call; keep a
// chunk index if bodies grow past ~100 KB.
fn to_utf16(text: &str, offset: usize) -> usize {
    text[..offset].encode_utf16().count()
}

fn from_utf16(text: &str, offset: usize) -> usize {
    let mut n = 0;
    for (i, c) in text.char_indices() {
        if n >= offset {
            return i;
        }
        n += c.len_utf16();
    }
    text.len()
}

// ---- The field ----------------------------------------------------------------------------

pub enum TextInputEvent {
    Changed,
    /// Enter in a single-line field. A multi-line field inserts a newline instead.
    Enter,
    Escape,
}

/// The text as painted: bullets in password mode, Markdown with hidden syntax. Rebuilt after an
/// edit, or when the caret enters or leaves a Markdown span; not per frame.
#[derive(Clone)]
struct Shown {
    text: SharedString,
    /// One per shown byte; empty for plain text.
    looks: Rc<[Look]>,
    swaps: Rc<[Swap]>,
}

/// Shaped text from the last frame, for hit tests, ↑ ↓ and the IME. Offsets are display offsets.
struct Layout {
    lines: Vec<WrappedLine>,
    /// Start offset, top and line height of each line (a paragraph, wrapped). Headings are taller.
    starts: Vec<usize>,
    tops: Vec<Pixels>,
    heights: Vec<Pixels>,
    /// The body line height.
    line_height: Pixels,
    /// Window position of the text's top left, scroll applied.
    origin: Point<Pixels>,
    height: Pixels,
    /// How the shown text maps to the source.
    swaps: Rc<[Swap]>,
}

impl Layout {
    fn line(&self, offset: usize) -> usize {
        self.starts.partition_point(|s| *s <= offset).saturating_sub(1)
    }

    /// The line height at a display offset.
    fn height_at(&self, offset: usize) -> Pixels {
        self.heights.get(self.line(offset)).copied().unwrap_or(self.line_height)
    }

    fn position(&self, offset: usize) -> Point<Pixels> {
        let i = self.line(offset);
        let Some(line) = self.lines.get(i) else { return Point::default() };
        let p = line.position_for_index(offset - self.starts[i], self.heights[i]).unwrap_or_default();
        point(p.x, p.y + self.tops[i])
    }

    /// The offset closest to `p`, in text coordinates.
    fn index(&self, p: Point<Pixels>) -> usize {
        let i = self.tops.partition_point(|y| *y <= p.y).saturating_sub(1);
        let Some(line) = self.lines.get(i) else { return 0 };
        let h = self.heights[i];
        let bottom = line.size(h).height - h * 0.5;
        let y = (p.y - self.tops[i]).clamp(px(0.), bottom.max(px(0.)));
        let ix = line.closest_index_for_position(point(p.x.max(px(0.)), y), h);
        self.starts[i] + ix.unwrap_or_else(|ix| ix)
    }
}

/// Heading sizes relative to the body.
fn heading_scale(level: u8) -> f32 {
    match level {
        0 => 1.,
        1 => 1.4,
        2 => 1.2,
        _ => 1.1,
    }
}

/// The format bar: five buttons and two separators, in a hairline border.
const BAR_BUTTON: f32 = 28.;
const BAR_BUTTON_H: f32 = 24.;
const BAR_PAD: f32 = 3.;
const BAR_SEPARATOR: f32 = 9.;
const BAR_W: f32 = 2. * BAR_PAD + 5. * BAR_BUTTON + 2. * BAR_SEPARATOR + 2.;
const BAR_H: f32 = 2. * BAR_PAD + BAR_BUTTON_H + 2.;
/// Space between the bar and the selection.
const BAR_GAP: f32 = 6.;

const CARET_WIDTH: f32 = 2.;
/// The caret blinks for 5 s after the last key or click, then stays solid: idle means no timers.
const BLINK: Duration = Duration::from_millis(500);
const BLINKS: usize = 10;

pub struct TextInput {
    buffer: Buffer,
    focus: FocusHandle,
    placeholder: SharedString,
    multiline: bool,
    markdown: bool,
    masked: bool,
    bordered: bool,
    layout: Option<Layout>,
    /// Horizontal scroll for one line, vertical for many.
    scroll: Pixels,
    autoscroll: bool,
    /// The x that ↑ and ↓ keep, so the caret returns to its column past short lines.
    goal_x: Option<Pixels>,
    /// Caret affinity at a wrap boundary: (offset, row start) after ⌘← on a wrapped row.
    row_start: Option<(usize, Point<Pixels>)>,
    selecting: bool,
    /// Where a drag or shift-click selection starts, as the pointer mapped it.
    drag_from: usize,
    focused: bool,
    caret_on: bool,
    shown: Option<Shown>,
    /// Markdown fields: styles and hideable syntax, parsed after each edit.
    md: Option<Markdown>,
    /// Window position of the format bar, from the last frame's layout.
    bar_at: Option<Point<Pixels>>,
    /// Esc closed the format bar; the next caret move brings it back.
    bar_closed: bool,
    /// Screenshots: the bar shows without focus, as a capture's window is not key.
    bar_pinned: bool,
    _blink: Option<Task<()>>,
    _focus: [Subscription; 2],
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        let on = cx.on_focus(&focus, window, |this, _, cx| {
            this.focused = true;
            this.wake(cx);
        });
        let off = cx.on_blur(&focus, window, |this, _, cx| {
            this.focused = false;
            this.selecting = false;
            this._blink = None;
            cx.notify();
        });
        Self {
            buffer: Buffer::default(),
            focused: focus.is_focused(window),
            focus,
            placeholder: SharedString::default(),
            multiline: false,
            markdown: false,
            masked: false,
            bordered: false,
            layout: None,
            scroll: px(0.),
            autoscroll: true,
            goal_x: None,
            row_start: None,
            selecting: false,
            drag_from: 0,
            caret_on: true,
            shown: None,
            md: None,
            bar_at: None,
            bar_closed: false,
            bar_pinned: false,
            _blink: None,
            _focus: [on, off],
        }
    }

    /// Wraps, and Enter inserts a newline.
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Live Markdown: styles, hidden syntax and the format bar. Multi-line.
    pub fn markdown(mut self) -> Self {
        self.markdown = true;
        self.parse();
        self.multiline()
    }

    /// Password mode: bullets, and no copy or cut.
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// A box with a `line` border and an accent focus ring.
    pub fn bordered(mut self) -> Self {
        self.bordered = true;
        self
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Initial text, caret at the end, nothing to undo.
    pub fn with_text(mut self, text: &str) -> Self {
        self.buffer = Buffer::new(&self.clean(text));
        self.parse();
        self
    }

    fn parse(&mut self) {
        self.md = self.markdown.then(|| markdown::parse(self.buffer.text()));
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Replaces everything and clears the undo history.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.buffer = Buffer::new(&self.clean(text));
        self.edited(cx);
    }

    /// Screenshots only: show the format bar while the window is not key.
    pub fn pin_bar(&mut self) {
        self.bar_pinned = true;
    }

    pub fn select(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.buffer.select(range.start, range.end);
        self.moved(cx);
    }

    /// One line keeps one line: pasted or IME newlines become spaces.
    fn clean(&self, text: &str) -> String {
        if self.multiline { text.replace("\r\n", "\n") } else { text.replace(['\r', '\n'], " ") }
    }

    /// A text offset in the shown text that `swaps` describes.
    fn display(&self, swaps: &[Swap], offset: usize) -> usize {
        if self.masked { to_mask(self.text(), offset) } else { markdown::to_shown(swaps, offset) }
    }

    fn offset(&self, swaps: &[Swap], display: usize) -> usize {
        if self.masked { from_mask(self.text(), display) } else { markdown::from_shown(swaps, display).min(self.text().len()) }
    }

    fn offset_at(&self, position: Point<Pixels>) -> usize {
        self.layout.as_ref().map_or(0, |l| self.offset(&l.swaps, l.index(position - l.origin)))
    }

    /// The shown text for this frame. Markdown syntax hides outside the caret's spans; a drag
    /// keeps the text it started with, so nothing shifts under the pointer.
    fn shown(&mut self) -> Shown {
        let swaps = self.md.as_ref().map_or_else(Vec::new, |md| markdown::swaps(md, self.buffer.caret()));
        if !self.selecting && self.shown.as_ref().is_some_and(|s| *s.swaps != *swaps) {
            self.shown = None;
        }
        let (buffer, md, masked) = (&self.buffer, &self.md, self.masked);
        self.shown
            .get_or_insert_with(|| {
                let t = buffer.text();
                let (text, looks) = match md {
                    _ if masked => (mask(t), Vec::new()),
                    Some(md) => markdown::shown(t, &md.looks, &swaps),
                    None => (t.to_string(), Vec::new()),
                };
                Shown { text: text.into(), looks: looks.into(), swaps: swaps.into() }
            })
            .clone()
    }

    /// The format bar shows for a selection in a focused Markdown field, after a drag ends.
    fn bar_shows(&self) -> bool {
        self.markdown && (self.focused || self.bar_pinned) && !self.selecting && !self.bar_closed && !self.buffer.selection().is_empty()
    }

    /// A Markdown edit from a shortcut or the format bar.
    fn format(&mut self, edit: fn(&mut Buffer), cx: &mut Context<Self>) {
        edit(&mut self.buffer);
        self.edited(cx);
    }

    fn wake(&mut self, cx: &mut Context<Self>) {
        self.caret_on = true;
        cx.notify();
        if !self.focused {
            return;
        }
        // Replacing the task cancels the older blink.
        self._blink = Some(cx.spawn(async move |this, cx| {
            for _ in 0..BLINKS {
                cx.background_executor().timer(BLINK).await;
                let alive = this.update(cx, |this, cx| {
                    this.caret_on = !this.caret_on;
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        }));
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.row_start = None;
        self.bar_closed = false;
        self.autoscroll = true;
        self.wake(cx);
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        self.shown = None;
        self.parse();
        self.goal_x = None;
        self.moved(cx);
        cx.emit(TextInputEvent::Changed);
    }

    fn go(&mut self, m: Motion, select: bool, cx: &mut Context<Self>) {
        self.buffer.motion(m, select);
        self.goal_x = None;
        self.moved(cx);
    }

    fn go_to(&mut self, offset: usize, select: bool) {
        if select { self.buffer.select_to(offset) } else { self.buffer.move_to(offset) }
    }

    /// ↑ ↓ by visual line, keeping the column. Past the first or last line, to the start or end.
    fn vertical(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let Some(l) = &self.layout else { return };
        let p = self.caret_at(l);
        let x = *self.goal_x.get_or_insert(p.x);
        let h = l.height_at(self.display(&l.swaps, self.buffer.caret()));
        let y = p.y + h * if down { 1.5 } else { -0.5 };
        let target = if y < px(0.) {
            0
        } else if y >= l.height {
            self.text().len()
        } else {
            self.offset(&l.swaps, l.index(point(x, y)))
        };
        self.go_to(target, select);
        self.moved(cx);
    }

    /// ⌘← ⌘→: the visual line when wrapped, as in native text views.
    fn line_edge(&mut self, end: bool, select: bool, cx: &mut Context<Self>) {
        let Some(l) = self.layout.as_ref().filter(|_| self.multiline) else {
            return self.go(if end { Motion::LineEnd } else { Motion::LineStart }, select, cx);
        };
        let p = self.caret_at(l);
        let x = if end { px(f32::MAX / 2.) } else { px(0.) };
        let h = l.height_at(self.display(&l.swaps, self.buffer.caret()));
        let target = self.offset(&l.swaps, l.index(point(x, p.y + h * 0.5)));
        let row = point(px(0.), p.y);
        self.go_to(target, select);
        self.goal_x = None;
        self.moved(cx);
        // The start of a wrapped row is the same offset as the end of the row above, where
        // `position` puts it. Keep the caret on the row it moved along.
        if !end {
            self.row_start = Some((target, row));
        }
    }

    /// Where the caret paints: the row start after ⌘←, else the layout position of its offset.
    fn caret_at(&self, l: &Layout) -> Point<Pixels> {
        let caret = self.buffer.caret();
        match self.row_start {
            Some((at, p)) if at == caret => p,
            _ => l.position(self.display(&l.swaps, caret)),
        }
    }

    fn delete(&mut self, m: Motion, cx: &mut Context<Self>) {
        if self.buffer.delete(m) {
            self.edited(cx);
        }
    }

    pub(crate) fn insert(&mut self, text: &str, join: bool, cx: &mut Context<Self>) {
        let text = self.clean(text);
        if self.buffer.replace(None, &text, join) {
            self.edited(cx);
        }
    }

    fn selected_text(&self) -> Option<String> {
        let sel = self.buffer.selection();
        (!self.masked && !sel.is_empty()).then(|| self.text()[sel].to_string())
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.insert("", false, cx);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.insert(&text, false, cx);
        }
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.markdown {
            self.buffer.list_enter();
            self.edited(cx);
        } else if self.multiline { self.insert("\n", true, cx) } else { cx.emit(TextInputEvent::Enter) }
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let offset = self.offset_at(e.position);
        match e.click_count {
            2 => {
                let w = self.buffer.word_at(offset);
                self.buffer.select(w.start, w.end);
            }
            3.. => self.buffer.select(0, self.text().len()),
            _ if e.modifiers.shift => {
                self.drag_from = self.buffer.anchor();
                self.drag_to_offset(offset);
            }
            _ => {
                self.drag_from = offset;
                self.buffer.move_to(offset);
            }
        }
        self.selecting = e.click_count < 2;
        self.goal_x = None;
        self.moved(cx);
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.drag_to_offset(self.offset_at(position));
        self.moved(cx);
    }

    /// Selects from where the drag started, keeping hidden syntax out of the selection's ends.
    fn drag_to_offset(&mut self, offset: usize) {
        let swaps = self.layout.as_ref().map_or_else(|| Rc::from([]), |l| l.swaps.clone());
        let (anchor, caret) = markdown::drag(&swaps, self.drag_from, offset);
        self.buffer.select(anchor, caret);
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = &self.layout else { return };
        self.scroll -= e.delta.pixel_delta(l.line_height).y;
        // Prepaint clamps it to the content.
        self.autoscroll = false;
        cx.notify();
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        // A password stays out of the IME and accessibility.
        if self.masked {
            return None;
        }
        let t = self.text();
        let range = from_utf16(t, range.start)..from_utf16(t, range.end);
        actual.replace(to_utf16(t, range.start)..to_utf16(t, range.end));
        Some(t[range].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let (t, sel) = (self.text(), self.buffer.selection());
        Some(UTF16Selection { range: to_utf16(t, sel.start)..to_utf16(t, sel.end), reversed: self.buffer.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let t = self.text();
        self.buffer.marked.as_ref().map(|m| to_utf16(t, m.start)..to_utf16(t, m.end))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let t = self.text();
        let range = range.map(|r| from_utf16(t, r.start)..from_utf16(t, r.end));
        let text = self.clean(text);
        if self.buffer.replace(range, &text, true) {
            self.edited(cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        sel: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.text();
        let range = range.map(|r| from_utf16(t, r.start)..from_utf16(t, r.end));
        let text = self.clean(text);
        let sel = sel.map(|s| from_utf16(&text, s.start)..from_utf16(&text, s.end));
        self.buffer.replace_and_mark(range, &text, sel);
        self.edited(cx);
    }

    fn bounds_for_range(&mut self, range: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let l = self.layout.as_ref()?;
        let t = self.text();
        let first = self.display(&l.swaps, from_utf16(t, range.start));
        let start = l.position(first);
        let end = l.position(self.display(&l.swaps, from_utf16(t, range.end)));
        let end = if end.y == start.y { end.x } else { start.x };
        Some(Bounds::from_corners(
            l.origin + start,
            l.origin + point(end, start.y + l.height_at(first)),
        ))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        self.layout.as_ref()?;
        Some(to_utf16(self.text(), self.offset_at(p)))
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
        let focused = self.focused;
        div()
            .key_context(if self.markdown { "TextInput multiline markdown" } else if self.multiline { "TextInput multiline" } else { "TextInput" })
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|this, _: &Backspace, _, cx| this.delete(Motion::Left, cx)))
            .on_action(cx.listener(|this, _: &Delete, _, cx| this.delete(Motion::Right, cx)))
            .on_action(cx.listener(|this, _: &DeleteWordLeft, _, cx| this.delete(Motion::WordLeft, cx)))
            .on_action(cx.listener(|this, _: &DeleteWordRight, _, cx| this.delete(Motion::WordRight, cx)))
            .on_action(cx.listener(|this, _: &Left, _, cx| this.go(Motion::Left, false, cx)))
            .on_action(cx.listener(|this, _: &Right, _, cx| this.go(Motion::Right, false, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.go(Motion::Left, true, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.go(Motion::Right, true, cx)))
            .on_action(cx.listener(|this, _: &WordLeft, _, cx| this.go(Motion::WordLeft, false, cx)))
            .on_action(cx.listener(|this, _: &WordRight, _, cx| this.go(Motion::WordRight, false, cx)))
            .on_action(cx.listener(|this, _: &SelectWordLeft, _, cx| this.go(Motion::WordLeft, true, cx)))
            .on_action(cx.listener(|this, _: &SelectWordRight, _, cx| this.go(Motion::WordRight, true, cx)))
            .on_action(cx.listener(|this, _: &LineStart, _, cx| this.line_edge(false, false, cx)))
            .on_action(cx.listener(|this, _: &LineEnd, _, cx| this.line_edge(true, false, cx)))
            .on_action(cx.listener(|this, _: &SelectLineStart, _, cx| this.line_edge(false, true, cx)))
            .on_action(cx.listener(|this, _: &SelectLineEnd, _, cx| this.line_edge(true, true, cx)))
            .on_action(cx.listener(|this, _: &DocStart, _, cx| this.go(Motion::DocStart, false, cx)))
            .on_action(cx.listener(|this, _: &DocEnd, _, cx| this.go(Motion::DocEnd, false, cx)))
            .on_action(cx.listener(|this, _: &SelectDocStart, _, cx| this.go(Motion::DocStart, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDocEnd, _, cx| this.go(Motion::DocEnd, true, cx)))
            .on_action(cx.listener(|this, _: &Up, _, cx| this.vertical(false, false, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.vertical(true, false, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.vertical(false, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.vertical(true, true, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.buffer.select(0, this.text().len());
                this.moved(cx);
            }))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            // The field's history wins over the inbox's while it has focus, from ⌘Z and from the menu.
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if this.buffer.undo() {
                    this.edited(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if this.buffer.redo() {
                    this.edited(cx);
                }
            }))
            .when(self.markdown, |d| d
                .on_action(cx.listener(|this, _: &Bold, _, cx| this.format(|b| b.wrap("**"), cx)))
                .on_action(cx.listener(|this, _: &Italic, _, cx| this.format(|b| b.wrap("_"), cx)))
                .on_action(cx.listener(|this, _: &Link, _, cx| this.format(Buffer::link, cx)))
                .on_action(cx.listener(|this, _: &Bullets, _, cx| this.format(|b| b.toggle_list(false), cx)))
                .on_action(cx.listener(|this, _: &Numbers, _, cx| this.format(|b| b.toggle_list(true), cx))))
            .on_action(cx.listener(Self::enter))
            // Esc closes the format bar first, then whatever holds the field.
            .on_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.bar_shows() && this.bar_at.is_some() {
                    this.bar_closed = true;
                    cx.notify();
                } else {
                    cx.emit(TextInputEvent::Escape);
                }
            }))
            .on_action(cx.listener(|_, _: &FocusNext, window, _| window.focus_next()))
            .on_action(cx.listener(|_, _: &FocusPrev, window, _| window.focus_prev()))
            .on_action(cx.listener(|_, _: &ShowCharacterPalette, window, _| window.show_character_palette()))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .when(self.multiline, |d| d.size_full().on_scroll_wheel(cx.listener(Self::on_scroll)))
            .when(!self.multiline, |d| d.w_full())
            .when(self.bordered, |d| {
                // A bordered multi-line field keeps its parent's height; a single line is one control high.
                d.flex()
                    .when(!self.multiline, |d| d.items_center().h(px(30.)))
                    .when(self.multiline, |d| d.py(px(6.)))
                    .px(px(10.))
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(radius::CONTROL))
                    // Opaque, so the focus ring shows only outside the field, as in AppKit.
                    .bg(t.raised)
                    .when(focused, |d| {
                        d.shadow(vec![BoxShadow {
                            color: Rgba { a: 0.5, ..t.sel_focus }.into(),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: px(3.),
                        }])
                    })
            })
            .child(TextElement {
                input: cx.entity(),
                placeholder: t.ink2.into(),
                caret: t.sel_focus.into(),
                // The accent as a light highlight, as AppKit derives it; unfocused fields grey it.
                selection: if focused { Rgba { a: 0.3, ..t.sel_focus } } else { t.sel }.into(),
                secondary: t.ink2.into(),
                link: t.accent.into(),
                code: Rgba { a: if t.dark { 0.1 } else { 0.06 }, ..t.ink }.into(),
            })
            .when_some(self.bar_at.filter(|_| self.bar_shows()), |d, at| {
                d.child(deferred(anchored().position(at).child(self.format_bar(&t))).with_priority(1))
            })
    }
}

impl TextInput {
    /// Bold, Italic, Link, Bulleted and Numbered list over the selection. A button sends the same
    /// action as its shortcut to the field, which keeps focus.
    fn format_bar(&self, t: &Theme) -> impl IntoElement {
        let hover = Rgba { a: if t.dark { 0.1 } else { 0.06 }, ..t.ink };
        let button = |id: &'static str, on: bool, action: Box<dyn Action>| {
            div()
                .id(id)
                .debug_selector(move || format!("format-{id}"))
                .flex()
                .items_center()
                .justify_center()
                .w(px(BAR_BUTTON))
                .h(px(BAR_BUTTON_H))
                .rounded(px(5.))
                .text_color(if on { t.accent } else { t.ink })
                .when(on, |d| d.bg(Rgba { a: 0.14, ..t.accent }))
                .when(!on, |d| d.hover(move |s| s.bg(hover)))
                .active(move |s| s.bg(t.press))
                // Keep focus and the selection in the field: no focus change, no caret move.
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_mouse_up(MouseButton::Left, move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        };
        let separator = || div().w(px(1.)).h(px(16.)).mx(px((BAR_SEPARATOR - 1.) / 2.)).bg(t.line);
        let ink = t.ink;
        div()
            .id("format-bar")
            .occlude()
            .cursor(CursorStyle::Arrow)
            .flex()
            .items_center()
            .w(px(BAR_W))
            .h(px(BAR_H))
            .p(px(BAR_PAD))
            .bg(t.raised)
            .border_1()
            .border_color(t.line)
            .rounded(px(7.))
            .shadow(vec![BoxShadow {
                color: Rgba { r: 0., g: 0., b: 0., a: if t.dark { 0.4 } else { 0.12 } }.into(),
                offset: point(px(0.), px(2.)),
                blur_radius: px(8.),
                spread_radius: px(0.),
            }])
            .text_size(px(crate::theme::size::BASE))
            .child(button("bold", self.buffer.wrapping("**").is_some(), Box::new(Bold)).font_weight(FontWeight::BOLD).child("B"))
            .child(button("italic", self.buffer.wrapping("_").is_some(), Box::new(Italic)).italic().child("I"))
            .child(separator())
            .child(button("link", false, Box::new(Link)).child(icon("icons/link.svg", ink, 15.)))
            .child(separator())
            .child(button("bullets", false, Box::new(Bullets)).child(icon("icons/list-bullets.svg", ink, 15.)))
            .child(button("numbers", false, Box::new(Numbers)).child(icon("icons/list-numbers.svg", ink, 15.)))
    }
}

struct TextElement {
    input: Entity<TextInput>,
    placeholder: Hsla,
    caret: Hsla,
    selection: Hsla,
    /// Markdown: list markers, and syntax at a lower alpha.
    secondary: Hsla,
    link: Hsla,
    /// The fill behind inline code.
    code: Hsla,
}

struct Prepaint {
    layout: Layout,
    scroll: Pixels,
    caret: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    code: Vec<PaintQuad>,
    bar: Option<Point<Pixels>>,
}

impl TextElement {
    fn run(&self, style: &TextStyle, look: Look, marked: bool, len: usize, color: Hsla) -> TextRun {
        let mut font = style.font();
        if look.bold {
            font.weight = FontWeight::BOLD;
        } else if look.heading > 0 {
            font.weight = FontWeight::SEMIBOLD;
        }
        if look.italic {
            font.style = FontStyle::Italic;
        }
        if look.code && !look.marker {
            font.family = "Menlo".into();
        }
        let color = match look {
            Look { marker: true, .. } => Hsla { a: 0.45, ..self.secondary },
            Look { bullet: true, .. } => self.secondary,
            Look { link: true, .. } => self.link,
            _ => color,
        };
        let underline = marked.then_some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false });
        TextRun { len, font, color, background_color: None, underline, strikethrough: None }
    }
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = if self.input.read(cx).multiline { relative(1.).into() } else { window.line_height().into() };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Prepaint {
        let shown = self.input.update(cx, |input, _| input.shown());
        let input = self.input.read(cx);
        let style = window.text_style();
        let line_height = window.line_height();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let text = input.text();
        let (shown_text, color) = if text.is_empty() {
            (input.placeholder.clone(), self.placeholder)
        } else {
            (shown.text.clone(), style.color)
        };
        let looks = |i: usize| shown.looks.get(i).copied().unwrap_or_default();
        let marked = input.buffer.marked.clone().filter(|m| !m.is_empty() && !text.is_empty());
        let marked = marked.map(|m| input.display(&shown.swaps, m.start)..input.display(&shown.swaps, m.end));

        // One shape per paragraph: a heading has its own size and line height.
        let wrap = input.multiline.then_some(bounds.size.width);
        let (mut lines, mut starts, mut tops, mut heights) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut start, mut height, mut width) = (0, px(0.), px(0.));
        let mut code = Vec::new();
        for paragraph in shown_text.split('\n') {
            let end = start + paragraph.len();
            let scale = if paragraph.is_empty() { 1. } else { heading_scale(looks(start).heading) };
            let mut runs = Vec::new();
            let mut i = start;
            while i < end {
                let key = |j: usize| (looks(j), marked.as_ref().is_some_and(|m| m.contains(&j)));
                let (look, under) = key(i);
                let j = (i + 1..end).find(|j| key(*j) != (look, under)).unwrap_or(end);
                if look.code && !look.marker {
                    code.push(i..j);
                }
                runs.push(self.run(&style, look, under, j - i, color));
                i = j;
            }
            let h = (line_height * scale).round();
            let shaped = window.text_system().shape_text(paragraph.to_string().into(), font_size * scale, &runs, wrap, None);
            for line in shaped.map(|l| l.into_vec()).unwrap_or_default() {
                starts.push(start);
                tops.push(height);
                heights.push(h);
                height += line.size(h).height;
                width = width.max(line.width());
                lines.push(line);
            }
            start = end + 1;
        }
        let mut layout = Layout { lines, starts, tops, heights, line_height, origin: bounds.origin, height, swaps: shown.swaps.clone() };

        // Keep the caret in view: sideways for one line, up and down for many.
        let sel = input.buffer.selection();
        let caret = input.caret_at(&layout);
        let caret_h = layout.height_at(input.display(&layout.swaps, input.buffer.caret()));
        let mut scroll = input.scroll;
        if input.multiline {
            if input.autoscroll {
                scroll = scroll.min(caret.y).max(caret.y + caret_h - bounds.size.height);
            }
            scroll = scroll.clamp(px(0.), (height - bounds.size.height).max(px(0.)));
            layout.origin.y -= scroll;
        } else {
            let room = bounds.size.width - px(CARET_WIDTH);
            if input.autoscroll {
                scroll = scroll.min(caret.x).max(caret.x - room);
            }
            scroll = scroll.clamp(px(0.), (width - room).max(px(0.)));
            layout.origin.x -= scroll;
        }

        let o = layout.origin;
        let rect = |x0: Pixels, x1: Pixels, y0: Pixels, y1: Pixels, c: Hsla| fill(Bounds::from_corners(o + point(x0, y0), o + point(x1, y1)), c);
        let (sel_a, sel_b) = (input.display(&layout.swaps, sel.start), input.display(&layout.swaps, sel.end));
        let (a, b) = (layout.position(sel_a), layout.position(sel_b));
        let (ha, hb) = (layout.height_at(sel_a), layout.height_at(sel_b));
        let selection = if sel.is_empty() || text.is_empty() {
            Vec::new()
        } else if a.y == b.y {
            vec![rect(a.x, b.x, a.y, a.y + ha, self.selection)]
        } else {
            // First row to the right edge, full rows between, last row from the left edge.
            let right = bounds.size.width.max(width);
            let mut quads = vec![rect(a.x, right, a.y, a.y + ha, self.selection)];
            if b.y > a.y + ha {
                quads.push(rect(px(0.), right, a.y + ha, b.y, self.selection));
            }
            quads.push(rect(px(0.), b.x, b.y, b.y + hb, self.selection));
            quads
        };
        // The glyph box of a line: the font size and a little, centred in the line height.
        let glyphs = |h: Pixels| (h / line_height * font_size * 1.25).min(h);
        // shortcut: inline code that wraps gets no fill; split it per row if that shows up.
        let code = code
            .into_iter()
            .filter_map(|r| {
                let (p, q, h) = (layout.position(r.start), layout.position(r.end), layout.height_at(r.start));
                let top = p.y + (h - glyphs(h)) * 0.5;
                (p.y == q.y).then(|| rect(p.x - px(2.), q.x + px(2.), top, top + glyphs(h), self.code).corner_radii(px(3.)))
            })
            .collect();
        // GPUI blurs a field when its window deactivates, so `focused` also hides the caret then.
        let shows = input.focused && input.caret_on && sel.is_empty();
        let caret = shows.then(|| {
            let ch = glyphs(caret_h);
            let top = caret.y + (caret_h - ch) * 0.5;
            fill(Bounds::new(o + point(caret.x, top), size(px(CARET_WIDTH), ch)), self.caret)
        });
        // Above the selection, centred on it; below it when the field has no room above.
        let bar = input.bar_shows().then(|| {
            let (top, bottom) = (o.y + a.y, o.y + b.y + hb);
            let x = if a.y == b.y { o.x + (a.x + b.x) / 2. } else { bounds.center().x };
            let x = (x - px(BAR_W / 2.)).clamp(bounds.left(), (bounds.right() - px(BAR_W)).max(bounds.left()));
            let above = top - px(BAR_H + BAR_GAP);
            let y = if above >= bounds.top() { above } else { bottom + px(BAR_GAP) };
            (bottom > bounds.top() && top < bounds.bottom()).then_some(point(x, y))
        });
        Prepaint { layout, scroll, caret, selection, code, bar: bar.flatten() }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), state: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let empty = Layout {
            lines: Vec::new(),
            starts: Vec::new(),
            tops: Vec::new(),
            heights: Vec::new(),
            line_height: px(0.),
            origin: bounds.origin,
            height: px(0.),
            swaps: Rc::from([]),
        };
        let layout = std::mem::replace(&mut state.layout, empty);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in state.code.drain(..).chain(state.selection.drain(..)) {
                window.paint_quad(quad);
            }
            for ((line, top), h) in layout.lines.iter().zip(&layout.tops).zip(&layout.heights) {
                let at = point(layout.origin.x, layout.origin.y + *top);
                line.paint(at, *h, TextAlign::Left, None, window, cx).ok();
            }
            if let Some(caret) = state.caret.take() {
                window.paint_quad(caret);
            }
        });

        // A drag that leaves the field keeps selecting until the button comes up.
        if self.input.read(cx).selecting {
            let input = self.input.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && e.pressed_button == Some(MouseButton::Left) {
                    input.update(cx, |input, cx| input.drag_to(e.position, cx));
                }
            });
            let input = self.input.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble {
                    input.update(cx, |input, cx| {
                        input.selecting = false;
                        cx.notify();
                    });
                }
            });
        }

        let (scroll, bar) = (state.scroll, state.bar);
        self.input.update(cx, |input, cx| {
            input.layout = Some(layout);
            input.scroll = scroll;
            input.autoscroll = false;
            // The bar renders from this frame's layout: one more frame when it moved.
            if input.bar_at != bar {
                input.bar_at = bar;
                cx.notify();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Buffer {
        let mut b = Buffer::default();
        for c in text.chars() {
            b.replace(None, &c.to_string(), true);
        }
        b
    }

    #[test]
    fn markdown_wrapping_preserves_unicode_selection_and_undo() {
        let mut b = Buffer::new("**Åsa**");
        b.select(0, b.text().len());
        b.wrap("**");
        assert_eq!((b.text(), b.selection()), ("Åsa", 0..4));
        b.undo();
        assert_eq!(b.text(), "**Åsa**");
        b.select(2, 6);
        b.wrap("**");
        assert_eq!((b.text(), b.selection()), ("Åsa", 0..4));
        b.move_to(4);
        b.wrap("_");
        assert_eq!((b.text(), b.selection()), ("Åsa__", 5..5));
        b.wrap("_");
        assert_eq!((b.text(), b.selection()), ("Åsa", 4..4));
    }

    #[test]
    fn link_selects_the_url_and_redo_restores_formatting_selections() {
        let mut b = Buffer::new("se här");
        b.select(3, b.text().len());
        b.link();
        assert_eq!((b.text(), &b.text()[b.selection()]), ("se [här](https://)", "https://"));
        b.undo();
        assert!(b.redo());
        assert_eq!(&b.text()[b.selection()], "https://");
        b.undo();
        b.wrap("**");
        b.undo();
        b.redo();
        assert_eq!((b.text(), &b.text()[b.selection()]), ("se **här**", "här"));
        // Typing still redoes to the caret after the text.
        let mut b = typed("ab");
        b.undo();
        b.redo();
        assert_eq!(b.selection(), 2..2);
    }

    #[test]
    fn list_toggle_keeps_indentation_and_excludes_the_next_line() {
        let mut b = Buffer::new("  Åsa\n  Örjan\nslut");
        let end = b.text().find("slut").unwrap();
        b.select(0, end);
        b.toggle_list(false);
        assert_eq!(b.text(), "  - Åsa\n  - Örjan\nslut");
        b.toggle_list(true);
        assert_eq!(b.text(), "  1. Åsa\n  2. Örjan\nslut");
        b.toggle_list(true);
        assert_eq!(b.text(), "  Åsa\n  Örjan\nslut");
        b.undo();
        assert_eq!(b.text(), "  1. Åsa\n  2. Örjan\nslut");
    }

    #[test]
    fn insert_replaces_the_selection() {
        let mut b = Buffer::new("hello world");
        b.select(6, 11);
        b.replace(None, "there", true);
        assert_eq!(b.text(), "hello there");
        assert_eq!(b.selection(), 11..11);
    }

    #[test]
    fn backspace_and_delete_respect_graphemes() {
        // "e" + combining acute, then a family emoji joined by ZWJ.
        let mut b = Buffer::new("ae\u{301}👨‍👩‍👧");
        b.delete(Motion::Left);
        assert_eq!(b.text(), "ae\u{301}");
        b.delete(Motion::Left);
        assert_eq!(b.text(), "a");
        let mut b = Buffer::new("ab");
        b.move_to(0);
        b.delete(Motion::Right);
        assert_eq!(b.text(), "b");
    }

    #[test]
    fn shift_selection_and_collapse() {
        let mut b = Buffer::new("abcd");
        b.motion(Motion::Left, true);
        b.motion(Motion::Left, true);
        assert_eq!((b.selection(), b.caret()), (2..4, 2));
        // Past the anchor the selection flips to the other side.
        b.move_to(2);
        b.motion(Motion::Right, true);
        b.motion(Motion::Left, true);
        b.motion(Motion::Left, true);
        assert_eq!((b.selection(), b.caret()), (1..2, 1));
        // ← without shift goes to the start of the selection, → to its end.
        b.motion(Motion::Right, false);
        assert_eq!(b.selection(), 2..2);
    }

    #[test]
    fn word_and_line_movement() {
        let mut b = Buffer::new("one two, three\nfour");
        b.move_to(9); // before "three"
        assert_eq!(b.target(Motion::WordLeft), 4);
        assert_eq!(b.target(Motion::WordRight), 14);
        b.move_to(5); // inside "two"
        assert_eq!(b.target(Motion::WordLeft), 4);
        assert_eq!(b.target(Motion::WordRight), 7);
        b.move_to(17);
        assert_eq!(b.target(Motion::LineStart), 15);
        assert_eq!(b.target(Motion::LineEnd), 19);
        b.move_to(3);
        assert_eq!(b.target(Motion::LineEnd), 14);
        b.delete(Motion::WordLeft);
        assert_eq!(b.text(), " two, three\nfour");
        assert_eq!(b.word_at(2), 1..4);
    }

    #[test]
    fn typing_undoes_as_one_step_until_the_caret_moves() {
        let mut b = typed("hello");
        b.delete(Motion::Left);
        assert_eq!(b.text(), "hell");
        assert!(b.undo());
        assert_eq!(b.text(), "");
        assert!(b.redo());
        assert_eq!((b.text(), b.selection()), ("hell", 4..4));

        let mut b = typed("ab");
        b.motion(Motion::Left, false);
        b.replace(None, "X", true);
        assert_eq!(b.text(), "aXb");
        b.undo();
        assert_eq!((b.text(), b.selection()), ("ab", 1..1));
        b.undo();
        assert_eq!(b.text(), "");
        assert!(!b.undo());
    }

    #[test]
    fn a_new_edit_clears_redo_and_paste_is_its_own_step() {
        let mut b = typed("ab");
        b.undo();
        b.replace(None, "c", true);
        assert!(!b.redo());
        b.replace(None, "PASTE", false);
        b.replace(None, "d", true);
        b.undo();
        assert_eq!(b.text(), "cPASTE");
        b.undo();
        assert_eq!(b.text(), "c");
    }

    #[test]
    fn undo_restores_the_selection() {
        let mut b = Buffer::new("hello world");
        b.select(11, 6);
        b.replace(None, "", false);
        b.undo();
        assert_eq!((b.text(), b.selection(), b.caret()), ("hello world", 6..11, 6));
    }

    #[test]
    fn ime_composition_is_one_undo_step() {
        let mut b = Buffer::default();
        b.replace_and_mark(None, "k", None);
        b.replace_and_mark(None, "か", Some(3..3));
        assert_eq!(b.marked, Some(0..3));
        b.replace(None, "可", true);
        assert_eq!((b.text(), b.marked.clone()), ("可", None));
        b.undo();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn password_mask_maps_offsets() {
        let text = "pä55";
        assert_eq!(mask(text), "••••");
        // "ä" is two bytes, a bullet three.
        assert_eq!(to_mask(text, 3), 6);
        assert_eq!(from_mask(text, 6), 3);
        assert_eq!(from_mask(text, 12), text.len());
    }

    #[test]
    fn utf16_round_trip() {
        let text = "a😀b";
        assert_eq!(to_utf16(text, 5), 3);
        assert_eq!(from_utf16(text, 3), 5);
        assert_eq!(from_utf16(text, 99), text.len());
    }
}
