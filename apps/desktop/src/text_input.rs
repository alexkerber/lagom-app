//! A text field: single-line, or multi-line with wrapping. Selection by mouse and keys, word and
//! line movement, clipboard, undo and IME (marked text). Built on GPUI's `examples/input.rs`.
//! The editing model, `Buffer`, is plain Rust so tests can drive it without a window.

use std::ops::Range;
use std::time::Duration;

use gpui::{
    div, fill, point, prelude::*, px, relative, size, App, Bounds, BoxShadow, ClipboardItem, Context,
    CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Rgba, ScrollWheelEvent,
    SharedString, Style, Subscription, Task, TextAlign, TextRun, UTF16Selection, UnderlineStyle,
    Window, WrappedLine,
};
use unicode_segmentation::GraphemeCursor;

use crate::main_window::Undo;
use crate::theme::{radius, Theme, DEFAULT_ACCENT};

gpui::actions!(
    text_input,
    [
        Backspace, Delete, DeleteWordLeft, DeleteWordRight, Left, Right, Up, Down, SelectLeft,
        SelectRight, SelectUp, SelectDown, WordLeft, WordRight, SelectWordLeft, SelectWordRight,
        LineStart, LineEnd, SelectLineStart, SelectLineEnd, DocStart, DocEnd, SelectDocStart,
        SelectDocEnd, SelectAll, Copy, Cut, Paste, Redo, Enter, Escape, FocusNext, FocusPrev,
        ShowCharacterPalette
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
    cx.bind_keys([
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

/// One undo step: `new` replaced `old` at `at`. `sel` is the selection before it.
#[derive(Debug)]
struct Edit {
    at: usize,
    old: String,
    new: String,
    sel: Range<usize>,
    reversed: bool,
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
        self.sel = caret..caret;
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

/// Shaped text from the last frame, for hit tests, ↑ ↓ and the IME. Offsets are display offsets.
struct Layout {
    lines: Vec<WrappedLine>,
    /// Start offset and top of each line (a paragraph, wrapped).
    starts: Vec<usize>,
    tops: Vec<Pixels>,
    line_height: Pixels,
    /// Window position of the text's top left, scroll applied.
    origin: Point<Pixels>,
    height: Pixels,
}

impl Layout {
    fn position(&self, offset: usize) -> Point<Pixels> {
        let i = self.starts.partition_point(|s| *s <= offset).saturating_sub(1);
        let Some(line) = self.lines.get(i) else { return Point::default() };
        let p = line.position_for_index(offset - self.starts[i], self.line_height).unwrap_or_default();
        point(p.x, p.y + self.tops[i])
    }

    /// The offset closest to `p`, in text coordinates.
    fn index(&self, p: Point<Pixels>) -> usize {
        let i = self.tops.partition_point(|y| *y <= p.y).saturating_sub(1);
        let Some(line) = self.lines.get(i) else { return 0 };
        let bottom = line.size(self.line_height).height - self.line_height * 0.5;
        let y = (p.y - self.tops[i]).clamp(px(0.), bottom.max(px(0.)));
        let ix = line.closest_index_for_position(point(p.x.max(px(0.)), y), self.line_height);
        self.starts[i] + ix.unwrap_or_else(|ix| ix)
    }
}

const CARET_WIDTH: f32 = 2.;
/// The caret blinks for 5 s after the last key or click, then stays solid: idle means no timers.
const BLINK: Duration = Duration::from_millis(500);
const BLINKS: usize = 10;

pub struct TextInput {
    buffer: Buffer,
    focus: FocusHandle,
    placeholder: SharedString,
    multiline: bool,
    masked: bool,
    bordered: bool,
    layout: Option<Layout>,
    /// Horizontal scroll for one line, vertical for many.
    scroll: Pixels,
    autoscroll: bool,
    /// The x that ↑ and ↓ keep, so the caret returns to its column past short lines.
    goal_x: Option<Pixels>,
    selecting: bool,
    focused: bool,
    caret_on: bool,
    /// The text as painted (bullets in password mode), rebuilt only after an edit, not per frame.
    shown: Option<SharedString>,
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
            masked: false,
            bordered: false,
            layout: None,
            scroll: px(0.),
            autoscroll: true,
            goal_x: None,
            selecting: false,
            caret_on: true,
            shown: None,
            _blink: None,
            _focus: [on, off],
        }
    }

    /// Wraps, and Enter inserts a newline.
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Password mode: bullets, and no copy or cut.
    #[allow(dead_code)] // The add-account form uses it next.
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
        self
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Replaces everything and clears the undo history.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.buffer = Buffer::new(&self.clean(text));
        self.edited(cx);
    }

    pub fn select(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.buffer.select(range.start, range.end);
        self.moved(cx);
    }

    /// One line keeps one line: pasted or IME newlines become spaces.
    fn clean(&self, text: &str) -> String {
        if self.multiline { text.replace("\r\n", "\n") } else { text.replace(['\r', '\n'], " ") }
    }

    fn display(&self, offset: usize) -> usize {
        if self.masked { to_mask(self.text(), offset) } else { offset }
    }

    fn offset(&self, display: usize) -> usize {
        if self.masked { from_mask(self.text(), display) } else { display.min(self.text().len()) }
    }

    fn offset_at(&self, position: Point<Pixels>) -> usize {
        self.layout.as_ref().map_or(0, |l| self.offset(l.index(position - l.origin)))
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
        self.autoscroll = true;
        self.wake(cx);
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        self.shown = None;
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
        let p = l.position(self.display(self.buffer.caret()));
        let x = *self.goal_x.get_or_insert(p.x);
        let y = p.y + l.line_height * if down { 1.5 } else { -0.5 };
        let target = if y < px(0.) {
            0
        } else if y >= l.height {
            self.text().len()
        } else {
            self.offset(l.index(point(x, y)))
        };
        self.go_to(target, select);
        self.moved(cx);
    }

    /// ⌘← ⌘→: the visual line when wrapped, as in native text views.
    fn line_edge(&mut self, end: bool, select: bool, cx: &mut Context<Self>) {
        let Some(l) = self.layout.as_ref().filter(|_| self.multiline) else {
            return self.go(if end { Motion::LineEnd } else { Motion::LineStart }, select, cx);
        };
        let p = l.position(self.display(self.buffer.caret()));
        let x = if end { px(f32::MAX / 2.) } else { px(0.) };
        let target = self.offset(l.index(point(x, p.y + l.line_height * 0.5)));
        self.go_to(target, select);
        self.goal_x = None;
        self.moved(cx);
    }

    fn delete(&mut self, m: Motion, cx: &mut Context<Self>) {
        if self.buffer.delete(m) {
            self.edited(cx);
        }
    }

    fn insert(&mut self, text: &str, join: bool, cx: &mut Context<Self>) {
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
        if self.multiline { self.insert("\n", true, cx) } else { cx.emit(TextInputEvent::Enter) }
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
            _ => self.go_to(offset, e.modifiers.shift),
        }
        self.selecting = e.click_count < 2;
        self.goal_x = None;
        self.moved(cx);
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.buffer.select_to(self.offset_at(position));
        self.moved(cx);
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
        let start = l.position(self.display(from_utf16(t, range.start)));
        let end = l.position(self.display(from_utf16(t, range.end)));
        let end = if end.y == start.y { end.x } else { start.x };
        Some(Bounds::from_corners(
            l.origin + start,
            l.origin + point(end, start.y + l.line_height),
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
        let shown = match &self.shown {
            Some(s) => s.clone(),
            None => {
                let t = self.buffer.text();
                let s: SharedString = if self.masked { mask(t).into() } else { t.to_string().into() };
                self.shown.insert(s).clone()
            }
        };
        div()
            .key_context(if self.multiline { "TextInput multiline" } else { "TextInput" })
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
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(TextInputEvent::Escape)))
            .on_action(cx.listener(|_, _: &FocusNext, window, _| window.focus_next()))
            .on_action(cx.listener(|_, _: &FocusPrev, window, _| window.focus_prev()))
            .on_action(cx.listener(|_, _: &ShowCharacterPalette, window, _| window.show_character_palette()))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .when(self.multiline, |d| d.size_full().on_scroll_wheel(cx.listener(Self::on_scroll)))
            .when(!self.multiline, |d| d.w_full())
            .when(self.bordered, |d| {
                d.flex()
                    .items_center()
                    .h(px(30.))
                    .px(px(10.))
                    .border_1()
                    .border_color(if focused { t.accent } else { t.line })
                    .rounded(px(radius::CONTROL))
                    .when(focused, |d| {
                        d.shadow(vec![BoxShadow {
                            color: Rgba { a: 0.25, ..t.accent }.into(),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: px(3.),
                        }])
                    })
            })
            .child(TextElement {
                input: cx.entity(),
                shown,
                placeholder: t.ink2.into(),
                caret: t.accent.into(),
                // Native fields grey the selection of an unfocused field.
                selection: if focused { Rgba { a: 0.25, ..t.accent } } else { t.sel }.into(),
            })
    }
}

struct TextElement {
    input: Entity<TextInput>,
    shown: SharedString,
    placeholder: Hsla,
    caret: Hsla,
    selection: Hsla,
}

struct Prepaint {
    layout: Layout,
    scroll: Pixels,
    caret: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
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
        let input = self.input.read(cx);
        let style = window.text_style();
        let line_height = window.line_height();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let text = input.text();
        let (shown, color) = if text.is_empty() {
            (input.placeholder.clone(), self.placeholder)
        } else {
            (self.shown.clone(), style.color)
        };

        let run = TextRun { len: shown.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
        let runs = match input.buffer.marked.clone().filter(|m| !m.is_empty() && !text.is_empty()) {
            Some(m) => {
                let m = input.display(m.start)..input.display(m.end);
                let underline = UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false };
                [
                    TextRun { len: m.start, ..run.clone() },
                    TextRun { len: m.len(), underline: Some(underline), ..run.clone() },
                    TextRun { len: shown.len() - m.end, ..run },
                ]
                .into_iter()
                .filter(|r| r.len > 0)
                .collect()
            }
            None => vec![run],
        };
        let wrap = input.multiline.then_some(bounds.size.width);
        let lines = window.text_system().shape_text(shown, font_size, &runs, wrap, None).map(|l| l.into_vec()).unwrap_or_default();

        let (mut starts, mut tops) = (Vec::with_capacity(lines.len()), Vec::with_capacity(lines.len()));
        let (mut start, mut height, mut width) = (0, px(0.), px(0.));
        for line in &lines {
            starts.push(start);
            tops.push(height);
            start += line.len() + 1;
            height += line.size(line_height).height;
            width = width.max(line.width());
        }
        let mut layout = Layout { lines, starts, tops, line_height, origin: bounds.origin, height };

        // Keep the caret in view: sideways for one line, up and down for many.
        let sel = input.buffer.selection();
        let caret = layout.position(input.display(input.buffer.caret()));
        let mut scroll = input.scroll;
        if input.multiline {
            if input.autoscroll {
                scroll = scroll.min(caret.y).max(caret.y + line_height - bounds.size.height);
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
        let selection = if sel.is_empty() || text.is_empty() {
            Vec::new()
        } else {
            let (a, b) = (layout.position(input.display(sel.start)), layout.position(input.display(sel.end)));
            if a.y == b.y {
                vec![rect(a.x, b.x, a.y, a.y + line_height, self.selection)]
            } else {
                // First row to the right edge, full rows between, last row from the left edge.
                let right = bounds.size.width.max(width);
                let mut quads = vec![rect(a.x, right, a.y, a.y + line_height, self.selection)];
                if b.y > a.y + line_height {
                    quads.push(rect(px(0.), right, a.y + line_height, b.y, self.selection));
                }
                quads.push(rect(px(0.), b.x, b.y, b.y + line_height, self.selection));
                quads
            }
        };
        // The caret spans the glyphs, not the whole line height.
        let caret_h = (font_size * 1.25).min(line_height);
        // GPUI blurs a field when its window deactivates, so `focused` also hides the caret then.
        let shows = input.focused && input.caret_on && sel.is_empty();
        let caret = shows.then(|| {
            let top = caret.y + (line_height - caret_h) * 0.5;
            fill(Bounds::new(o + point(caret.x, top), size(px(CARET_WIDTH), caret_h)), self.caret)
        });
        Prepaint { layout, scroll, caret, selection }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), state: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let layout = std::mem::replace(
            &mut state.layout,
            Layout { lines: Vec::new(), starts: Vec::new(), tops: Vec::new(), line_height: px(0.), origin: bounds.origin, height: px(0.) },
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in state.selection.drain(..) {
                window.paint_quad(quad);
            }
            for (line, top) in layout.lines.iter().zip(&layout.tops) {
                let at = point(layout.origin.x, layout.origin.y + *top);
                line.paint(at, layout.line_height, TextAlign::Left, None, window, cx).ok();
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

        let scroll = state.scroll;
        self.input.update(cx, |input, _| {
            input.layout = Some(layout);
            input.scroll = scroll;
            input.autoscroll = false;
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
