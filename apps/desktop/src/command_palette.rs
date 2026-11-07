//! Command palette (⌘K), concept B structure in Concept D tokens.
//! Source: CommandPalette on the Claude design canvas. Enter runs the selected command: the
//! palette emits `Run` or `MoveTo` and the main window does the work.
//! The query is a `TextInput`; ↑ and ↓ reach the palette because a single-line field leaves them.
//! "move <query>" lists folders (Figma "Lagom · Folders", frame 6). Enter there emits `MoveTo`.
//! A query that matches no command searches the local store and lists messages.

use std::{rc::Rc, time::Duration};

use gpui::{
    div, ease_out_quint, prelude::*, px, Animation, AnimationExt, App, Context, Div, DismissEvent,
    Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, KeyDownEvent,
    MouseButton, Rgba, ScrollHandle, SharedString, StyledText, Subscription, Window,
};

use crate::mail::LaterWhen;
use crate::motion::spring;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT};

enum Hint {
    Key(&'static str),
    Note(&'static str),
}

/// A command the main window runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Run {
    Done,
    Later(LaterWhen),
    Delete,
    Reply,
    Compose,
    Sync,
    Settings,
    AddAccount,
    /// A search hit: the store id of the message.
    Open(i64),
}

/// What Enter on an item does.
#[derive(Clone, Debug, PartialEq)]
enum Action {
    Run(Run),
    Move(MoveTo),
    /// Replaces the query, for "Move to…".
    Fill(&'static str),
}

struct Command {
    section: &'static str,
    label: &'static str,
    hint: Hint,
    action: Action,
}

const COMMANDS: &[Command] = &[
    Command { section: "Actions", label: "Done", hint: Hint::Key("E"), action: Action::Run(Run::Done) },
    Command { section: "Actions", label: LaterWhen::Evening.label(), hint: Hint::Note("18:00"), action: Action::Run(Run::Later(LaterWhen::Evening)) },
    Command { section: "Actions", label: LaterWhen::Tomorrow.label(), hint: Hint::Key("L"), action: Action::Run(Run::Later(LaterWhen::Tomorrow)) },
    Command { section: "Actions", label: LaterWhen::NextWeek.label(), hint: Hint::Note("Mon 08:00"), action: Action::Run(Run::Later(LaterWhen::NextWeek)) },
    Command { section: "Actions", label: "Move to…", hint: Hint::Note("move"), action: Action::Fill("move ") },
    Command { section: "Actions", label: "Delete", hint: Hint::Key("⌫"), action: Action::Run(Run::Delete) },
    Command { section: "Actions", label: "Reply", hint: Hint::Key("R"), action: Action::Run(Run::Reply) },
    Command { section: "Actions", label: "New message", hint: Hint::Key("C"), action: Action::Run(Run::Compose) },
    Command { section: "Mail", label: "Sync now", hint: Hint::Note("IMAP"), action: Action::Run(Run::Sync) },
    Command { section: "Mail", label: "Settings", hint: Hint::Key("⌘,"), action: Action::Run(Run::Settings) },
    Command { section: "Mail", label: "Add account", hint: Hint::Note("IMAP"), action: Action::Run(Run::AddAccount) },
];

/// Where "Move to" sends the selected message.
#[derive(Clone, Debug, PartialEq)]
pub enum MoveTo {
    Folder(usize),
    New(String),
}

/// One row in the list: a command, a folder target in "move" mode, or a message found.
struct Item {
    section: &'static str,
    label: String,
    hint: Option<&'static Hint>,
    range: Option<std::ops::Range<usize>>,
    action: Action,
}

const MOVE_TO: &str = "Move to ";

/// The folder query when `query` starts with "move", in any case.
fn move_query(query: &str) -> Option<&str> {
    query.get(..4).filter(|p| p.eq_ignore_ascii_case("move")).map(|_| query[4..].trim_start())
}

fn commands(query: &str) -> Vec<Item> {
    COMMANDS
        .iter()
        .filter_map(|c| {
            find(c.label, query).map(|range| Item { section: c.section, label: c.label.into(), hint: Some(&c.hint), range, action: c.action.clone() })
        })
        .collect()
}

/// `folders` is (folder index, "Travel / Trains"); `hits` is (store id, "Sender · Subject").
fn matches(query: &str, folders: &[(usize, String)], hits: &[(i64, String)]) -> Vec<Item> {
    let Some(q) = move_query(query) else {
        let items = commands(query);
        if !items.is_empty() {
            return items;
        }
        return hits
            .iter()
            .map(|(id, label)| Item { section: "Messages", label: label.clone(), hint: None, range: None, action: Action::Run(Run::Open(*id)) })
            .collect();
    };
    let mut items: Vec<Item> = folders
        .iter()
        .filter_map(|(f, path)| {
            find(path, q).map(|r| Item {
                section: "Move to",
                label: format!("{MOVE_TO}{path}"),
                hint: None,
                range: r.map(|r| r.start + MOVE_TO.len()..r.end + MOVE_TO.len()),
                action: Action::Move(MoveTo::Folder(*f)),
            })
        })
        .collect();
    if !q.is_empty() && !folders.iter().any(|(_, p)| p.eq_ignore_ascii_case(q)) {
        items.push(Item {
            section: "Create",
            label: format!("New folder “{q}”"),
            hint: None,
            range: None,
            action: Action::Move(MoveTo::New(q.into())),
        });
    }
    items
}

/// Byte range of `query` in `label`, ignoring ASCII case. An empty query matches with no range.
fn find(label: &str, query: &str) -> Option<Option<std::ops::Range<usize>>> {
    if query.is_empty() {
        return Some(None);
    }
    let start = label.to_ascii_lowercase().find(&query.to_ascii_lowercase())?;
    Some(Some(start..start + query.len()))
}

/// Full-text search in the store: (store id, label) for a query.
pub type Search = Rc<dyn Fn(&str) -> Vec<(i64, String)>>;

pub struct CommandPalette {
    input: Entity<TextInput>,
    selected: usize,
    folders: Vec<(usize, String)>,
    // Subject of the message a move would take.
    subject: Option<String>,
    search: Option<Search>,
    /// Search results for the current query, found once per edit, not per frame.
    hits: Vec<(i64, String)>,
    /// The list scrolls past `LIST_MAX`; the arrow keys keep the selection in view.
    scroll: ScrollHandle,
    _input: Subscription,
}

/// The list's height cap: about seven rows, so a long folder list never pushes the footer off.
const LIST_MAX: f32 = 360.;

/// The scroll child of item `i`: section headers are children too, one before each section.
fn child_index(items: &[Item], i: usize) -> usize {
    let headers = items[..=i.min(items.len().saturating_sub(1))].iter().enumerate().filter(|(j, it)| *j == 0 || items[j - 1].section != it.section).count();
    i + headers
}

impl EventEmitter<DismissEvent> for CommandPalette {}
impl EventEmitter<MoveTo> for CommandPalette {}
impl EventEmitter<Run> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl CommandPalette {
    pub fn new(
        query: &str,
        folders: Vec<(usize, String)>,
        subject: Option<String>,
        search: Option<Search>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new(window, cx).placeholder("Type a command or search").with_text(query));
        let sub = cx.subscribe(&input, |this, _, e: &TextInputEvent, cx| match e {
            TextInputEvent::Changed => {
                this.selected = 0;
                this.scroll.set_offset(gpui::point(px(0.), px(0.)));
                this.find_hits(cx);
                cx.notify();
            }
            TextInputEvent::Enter => this.run(this.matches(cx), cx),
            TextInputEvent::Escape => cx.emit(DismissEvent),
        });
        let mut this = Self { input, selected: 0, folders, subject, search, hits: Vec::new(), scroll: ScrollHandle::new(), _input: sub };
        this.find_hits(cx);
        this
    }

    /// Searches only when no command matches: a typed command never waits on the store.
    fn find_hits(&mut self, cx: &App) {
        let query = self.input.read(cx).text();
        self.hits = match &self.search {
            Some(search) if !query.trim().is_empty() && move_query(query).is_none() && commands(query).is_empty() => search(query),
            _ => Vec::new(),
        };
    }

    #[cfg(test)]
    pub(crate) fn query(&self, cx: &App) -> String {
        self.input.read(cx).text().to_string()
    }

    fn matches(&self, cx: &App) -> Vec<Item> {
        matches(self.input.read(cx).text(), &self.folders, &self.hits)
    }

    /// Enter or a click: the selected item runs and the palette closes.
    fn run(&mut self, matches: Vec<Item>, cx: &mut Context<Self>) {
        match matches.into_iter().nth(self.selected).map(|i| i.action) {
            Some(Action::Fill(text)) => {
                self.input.update(cx, |input, cx| input.set_text(text, cx));
                return;
            }
            Some(Action::Move(target)) => cx.emit(target),
            Some(Action::Run(run)) => cx.emit(run),
            None => {}
        }
        cx.emit(DismissEvent);
    }

    fn on_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.matches(cx);
        match event.keystroke.key.as_str() {
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(matches.len().saturating_sub(1)),
            _ => return,
        }
        if !matches.is_empty() {
            self.scroll.scroll_to_item(child_index(&matches, self.selected));
        }
        cx.stop_propagation();
        cx.notify();
    }
}

fn mono(text: impl Into<SharedString>, color: Rgba) -> Div {
    div().font_family(font::MONO).text_size(px(size::MONO)).text_color(color).child(text.into())
}

fn key_cap(t: &Theme, key: &'static str, selected: bool) -> Div {
    mono(key, if selected { t.ink } else { t.ink2 })
        .px(px(6.))
        .py(px(1.))
        .border_1()
        .border_color(if selected { t.ink2 } else { t.line })
        .rounded(px(radius::KEY))
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
        let matches = self.matches(cx);
        let moving = move_query(self.input.read(cx).text()).is_some();
        let bold = HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() };

        let mut list = div().id("palette-list").track_scroll(&self.scroll).max_h(px(LIST_MAX)).overflow_y_scroll().flex().flex_col().p(px(8.));
        let mut section = "";
        for (i, item) in matches.iter().enumerate() {
            if item.section != section {
                section = item.section;
                list = list.child(
                    div()
                        .px(px(12.))
                        .pt(px(if i == 0 { 10. } else { 14. }))
                        .pb(px(6.))
                        .text_size(px(size::SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.ink2)
                        .child(section),
                );
            }
            let selected = i == self.selected;
            list = list.child(
                div()
                    .id(("item", i))
                    // The pointer moves the selection, as the arrow keys do, so the highlight is the hover.
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.selected != i {
                            this.selected = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = i;
                        this.run(this.matches(cx), cx);
                    }))
                    .cursor_pointer()
                    .active(|s| s.bg(t.press))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(44.))
                    .px(px(12.))
                    .rounded(px(radius::CONTROL))
                    .text_size(px(size::BODY))
                    .when(selected, |d| d.bg(t.sel))
                    .child(div().size(px(6.)).rounded_full().when(selected, |d| d.bg(t.accent)))
                    .child(
                        div()
                            .flex_1()
                            .child(StyledText::new(item.label.clone()).with_highlights(item.range.clone().map(|r| (r, bold)))),
                    )
                    .children(match item.hint {
                        Some(Hint::Key(key)) => Some(key_cap(&t, key, selected)),
                        Some(Hint::Note(note)) => Some(mono(*note, t.ink2)),
                        // Folder targets show ↵ on the selected row only.
                        None => selected.then(|| key_cap(&t, "↵", true)),
                    }),
            );
        }

        let input = div()
            .flex()
            .items_center()
            .gap(px(14.))
            .h(px(64.))
            .px(px(20.))
            .border_b_1()
            .border_color(t.line)
            .child(mono("›", t.accent).text_size(px(size::BODY)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .text_size(px(size::TITLE))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.input.clone()),
            )
            .child(key_cap(&t, "esc", false));

        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(40.))
            .px(px(20.))
            .border_t_1()
            .border_color(t.line)
            .bg(t.subtle)
            .child(mono(if moving { "↑ ↓ move · ↵ move to · esc close" } else { "↑ ↓ move · ↵ run · esc close" }, t.ink2))
            .child(mono(
                match matches.len() {
                    1 => "1 result".to_string(),
                    n => format!("{n} results"),
                },
                t.ink2,
            ));

        let dialog = div()
            .occlude()
            .w(px(640.))
            .flex()
            .flex_col()
            .bg(t.raised)
            .border_1()
            .border_color(t.line)
            .rounded(px(radius::PANEL))
            .shadow_xl()
            .overflow_hidden()
            .text_color(t.ink)
            .font_family(font::ui())
            .child(input)
            .when_some(self.subject.clone().filter(|_| moving), |d, s| {
                d.child(mono(format!("1 message · {s}"), t.ink2).truncate().px(px(20.)).pt(px(10.)))
            })
            .child(list)
            .child(footer);

        div()
            .id("command-palette")
            .on_key_down(cx.listener(Self::on_key))
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(112.))
            .bg(gpui::black().opacity(if t.dark { 0.5 } else { 0.18 }))
            .child(dialog)
            // docs/motion.md asks for scale 0.98 → 1, but GPUI divs have no scale transform:
            // the dialog drops 8 px into place on a spring instead, and fades in.
            .with_animation("command-palette-in", Animation::new(Duration::from_millis(120)), |el, d| {
                el.opacity(ease_out_quint()(d)).pt(px(104. + 8. * spring(d)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ignores_case_and_returns_byte_range() {
        assert_eq!(find("Undo done", "DONE"), Some(Some(5..9)));
        assert_eq!(find("Done mailbox", ""), Some(None));
        assert_eq!(find("Done mailbox", "later"), None);
    }

    #[test]
    fn later_offers_three_times_and_search_follows_commands() {
        let labels: Vec<_> = matches("later", &[], &[]).iter().map(|i| i.label.clone()).collect();
        assert_eq!(labels, ["Later: this evening 18:00", "Later: tomorrow 08:00", "Later: next week, Monday 08:00"]);
        assert_eq!(matches("tomorrow", &[], &[])[0].action, Action::Run(Run::Later(LaterWhen::Tomorrow)));
        // A query with no command lists the messages found; a command query never shows them.
        let hits = vec![(42, "Ingrid · Board".to_string())];
        let found = matches("ingrid", &[], &hits);
        assert_eq!((found.len(), found[0].section, &found[0].action), (1, "Messages", &Action::Run(Run::Open(42))));
        assert!(matches("done", &[], &hits).iter().all(|i| i.section != "Messages"));
        // "mov" still lists the command; four letters switch to folder mode.
        assert_eq!(matches("mov", &[], &[])[0].action, Action::Fill("move "));
    }

    #[test]
    fn move_lists_matching_folders_then_create() {
        let folders = vec![(0, "Receipts".into()), (1, "Travel".into()), (2, "Travel / Trains".into())];
        let items = matches("Move tr", &folders, &[]);
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Move to Travel", "Move to Travel / Trains", "New folder “tr”"]);
        // The highlight covers the query inside the folder path, not "Move to".
        assert_eq!(items[0].range, Some(8..10));
        assert_eq!(items[1].action, Action::Move(MoveTo::Folder(2)));
        assert_eq!(items[2].action, Action::Move(MoveTo::New("tr".into())));
        // An exact name offers no duplicate folder; bare "move" lists all folders.
        assert!(matches("move travel", &folders, &[]).iter().all(|i| i.section == "Move to"));
        assert_eq!(matches("move", &folders, &[]).len(), 3);
        // Other queries stay commands.
        assert!(matches("done", &folders, &[]).iter().all(|i| matches!(i.action, Action::Run(_))));
    }

    #[test]
    fn scroll_target_counts_section_headers() {
        // All commands: one section per group, so the row index shifts by each header above it.
        let items = matches("", &[], &[]);
        let sections: Vec<_> = items.iter().map(|i| i.section).collect();
        for i in 0..items.len() {
            let headers = 1 + (1..=i).filter(|&j| sections[j] != sections[j - 1]).count();
            assert_eq!(child_index(&items, i), i + headers);
        }
        // Many folders: one header, so the last row is its index plus one.
        let folders: Vec<_> = (0..40).map(|i| (i, format!("Folder {i}"))).collect();
        let items = matches("move", &folders, &[]);
        assert_eq!(child_index(&items, items.len() - 1), items.len());
    }
}
