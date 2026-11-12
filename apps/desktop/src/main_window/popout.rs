//! A message in its own window, as in Mail: a double-click on a row, the toolbar's pop-out
//! button or ⌘O. The main window owns the inbox: this window reads the message from it and
//! sends every action back to it, so both windows always show the same state.

use gpui::{Bounds, Subscription, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions};

use super::*;

pub struct MessageWindow {
    main: WindowHandle<MainWindow>,
    /// The message: an id into `Inbox::mail`, stable across store reads.
    id: usize,
    pub(super) focus: FocusHandle,
    web: html::Web,
    _main: [Subscription; 2],
}

impl html::WebHost for MessageWindow {
    fn hints(&mut self, over: bool, command: bool, shift: bool, cx: &mut Context<Self>) {
        self.main.update(cx, |main, _, cx| main.update_hints(over, gpui::Modifiers { platform: command, shift, ..Default::default() }, cx)).ok();
    }
    fn mail_action(&mut self, action: Run, _: &mut Window, cx: &mut Context<Self>) {
        let id = self.id;
        self.main.update(cx, |main, window, cx| {
            window.activate_window();
            main.mail_action_on(id, action, window, cx);
        }).ok();
    }
    fn new_task(&mut self, cx: &mut Context<Self>) { self.task(cx); }
    fn web(&mut self) -> &mut html::Web {
        &mut self.web
    }

    fn keys(&self) -> FocusHandle {
        self.focus.clone()
    }
}

impl MessageWindow {
    fn task(&mut self, cx: &mut Context<Self>) {
        let id = self.id;
        self.main.update(cx, |main, window, cx| {
            main.task_from_mail(id, cx);
            window.activate_window();
        }).ok();
    }
    fn new(main: WindowHandle<MainWindow>, inbox: &Entity<MainWindow>, id: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            main,
            id,
            focus,
            web: html::Web::default(),
            _main: [
                // A move, a store read or a theme change in the main window shows here too.
                cx.observe(inbox, |_, _, cx| cx.notify()),
                // Without the main window there is no inbox to act on.
                cx.observe_release_in(inbox, window, |_, _, window, _| window.remove_window()),
            ],
        }
    }

    pub(super) fn act(&mut self, what: Move, cx: &mut Context<Self>) {
        let id = self.id;
        self.main.update(cx, |main, window, cx| main.act_on(id, what, window, cx)).ok();
    }

    /// The reply sheet opens in the main window, which owns drafts and sending.
    fn reply(&mut self, cx: &mut App) {
        self.compose(ComposeKind::Reply, cx);
    }

    fn compose(&mut self, kind: ComposeKind, cx: &mut App) {
        let id = self.id;
        self.main
            .update(cx, |main, window, cx| {
                window.activate_window();
                main.compose_kind(Some(id), kind, window, cx);
            })
            .ok();
    }
}

impl Render for MessageWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Ok(main) = self.main.read(cx) else { return div() };
        let t = main.theme;
        // One message's text and HTML: cheap to clone, and the main window stays free to change.
        let Some(m) = main.inbox.mail.get(self.id).cloned() else { return div() };
        let id = self.id;
        let in_trash = if m.store.is_some() {
            main.inbox.folders.iter().any(|f| f.kind == Kind::Trash && !f.gone && f.store.map(mail::Place::Folder).as_ref() == Some(&m.folder))
        } else {
            main.inbox.trash.contains(&id)
        };
        let menu_open = main.message_menu == Some(id);
        self.web.frame(Some((id, &m)), &t, menu_open, window, cx);
        let web = html::SUPPORTED && m.html.is_some();
        let this = cx.entity().downgrade();
        let on_reply = move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.reply(cx)).ok();
        };
        let main_window = self.main;
        let menu = polish::reading_menu(&t, id, menu_open, move |action, _, cx| {
            main_window.update(cx, |main, window, cx| {
                if matches!(action, polish::MessageAction::ReplyAll | polish::MessageAction::Forward | polish::MessageAction::Move) { window.activate_window(); }
                main.message_action(id, *action, window, cx);
            }).ok();
        });
        let on_download = move |section, _: &mut Window, cx: &mut App| {
            main_window.update(cx, |main, _, cx| main.download_attachment(id, section, cx)).ok();
        };
        div()
            .key_context("MessageWindow")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.web.unfocus()))
            .on_action(cx.listener(|this, _: &NewTask, _, cx| this.task(cx)))
            .on_action(cx.listener(|this, _: &Done, _, cx| this.act(Move::Done, cx)))
            .on_action(cx.listener(|this, _: &Later, _, cx| this.act(Move::Later, cx)))
            .on_action(cx.listener(|this, _: &DeleteMessage, _, cx| this.act(Move::Delete, cx)))
            .on_action(cx.listener(|this, _: &Forward, _, cx| this.compose(ComposeKind::Forward, cx)))
            .on_action(cx.listener(|this, _: &ReplyAll, _, cx| this.compose(ComposeKind::ReplyAll, cx)))
            .on_action(cx.listener(|this, _: &ToggleRead, _, cx| {
                let id = this.id;
                this.main.update(cx, |main, window, cx| main.toggle_read_on(id, window, cx)).ok();
            }))
            .on_action(cx.listener(|this, _: &OpenMovePalette, _, cx| {
                let id = this.id;
                this.main.update(cx, |main, window, cx| {
                    window.activate_window();
                    main.message_action(id, polish::MessageAction::Move, window, cx);
                }).ok();
            }))
            .on_action(cx.listener(|this, _: &Reply, _, cx| this.reply(cx)))
            .map(|d| quit_through_main(d, cx))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .text_color(t.ink)
            .font_family(font::ui())
            .text_size(px(size::BASE))
            .line_height(relative(LEADING))
            .child(
                toolbar(&t)
                    .flex_wrap().h_auto().min_h(px(HEADER_H)).py(px(8.))
                    // macOS: the traffic lights sit in the toolbar, as in the main window's sidebar.
                    .pl(px(if cfg!(target_os = "macos") { 84. } else { 16. }))
                    .pr(px(16.))
                    .child(button(&t, "Reply", None).on_click(cx.listener(|this, _, _, cx| this.reply(cx))))
                    .child(icon_button(&t, "reply-all", "icons/reply-all.svg", "Reply all · Shift+R").on_click(cx.listener(|this, _, _, cx| this.compose(ComposeKind::ReplyAll, cx))))
                    .child(button(&t, "Forward", None).on_click(cx.listener(|this, _, _, cx| this.compose(ComposeKind::Forward, cx))))
                    .when(!in_trash, |d| d
                        .child(button(&t, "Later", None).on_click(cx.listener(|this, _, _, cx| this.act(Move::Later, cx)))))
                    .when(web && self.web.has_remote && !m.remote, |d| {
                        d.child(button(&t, "Load remote images", None).on_click(cx.listener(move |this, _, _, cx| {
                            this.main.update(cx, |main, _, cx| {
                                main.inbox.mail[id].remote = true;
                                cx.notify();
                            })
                            .ok();
                        })))
                    })
                    // Done last in the accent, as in the main window.
                    .child(div().flex_1())
                    .child(button(&t, if in_trash { "Move to Inbox" } else { "Done" }, Some(t.accent))
                        .on_click(cx.listener(move |this, _, _, cx| this.act(if in_trash { Move::Inbox } else { Move::Done }, cx)))),
            )
            .child(reading_view(&t, (id, &m), !web || menu_open && self.web.cover().is_none(), self.web.surface(), 0, menu, on_download, on_reply))
    }
}

/// ⌘Q flushes local edits in the main window; queued sends survive the exit.
fn quit_through_main<E: InteractiveElement>(el: E, cx: &mut Context<MessageWindow>) -> E {
    #[cfg(target_os = "macos")]
    let el = el.on_action(cx.listener(|this, _: &crate::menus::Quit, _, cx| {
        let held = this.main.update(cx, |main, window, cx| {
            let held = main.hold_exit(cx);
            if held {
                window.activate_window();
            }
            held
        });
        if !held.unwrap_or(false) {
            cx.propagate();
        }
    }));
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
    el
}

/// Offset of each new message window from the main window, as macOS cascades windows.
const CASCADE: f32 = 24.;

impl MainWindow {
    /// The selected message in its own window, or that window to the front if it is open.
    pub(super) fn open_popout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.inbox.single() else { return };
        // The body and HTML come from the store here; the new window only reads them.
        self.open_message(id, true, cx);
        self.popouts.retain(|(_, h)| h.read(cx).is_ok());
        if let Some((_, h)) = self.popouts.iter().find(|p| p.0 == id) {
            h.update(cx, |_, window, _| window.activate_window()).ok();
            return;
        }
        let Some(main) = window.window_handle().downcast::<MainWindow>() else { return };
        let inbox = cx.entity();
        let step = CASCADE * (self.popouts.len() + 1) as f32;
        let origin = window.bounds().origin + gpui::point(px(step), px(step));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, gpui::size(px(720.), px(800.))))),
            window_min_size: Some(gpui::size(px(420.), px(320.))),
            titlebar: Some(TitlebarOptions {
                title: Some(self.inbox.mail[id].subject.clone().into()),
                appears_transparent: cfg!(target_os = "macos"),
                traffic_light_position: cfg!(target_os = "macos").then(|| gpui::point(px(18.), px(20.))),
            }),
            ..Default::default()
        };
        match cx.open_window(options, |window, cx| cx.new(|cx| MessageWindow::new(main, &inbox, id, window, cx))) {
            Ok(handle) => self.popouts.push((id, handle)),
            Err(e) => self.notify(format!("Could not open a window: {e}"), cx),
        }
    }

    /// A move of message `id` from its window. If another list is on screen, the list holding
    /// the message opens first, as Undo does, so the move plays where it can be seen.
    pub(super) fn act_on(&mut self, id: usize, what: Move, window: &mut Window, cx: &mut Context<Self>) {
        // The palette acts on the selection, which this move changes.
        if self.palette.is_some() {
            self.close_palette(window, cx);
        }
        let store = self.inbox.mail[id].store;
        if self.inbox.account_filter.as_ref().is_some_and(|a| *a != self.inbox.mail[id].account) {
            self.inbox.account_filter = None;
            self.inbox.regroup();
        }
        if !self.inbox.raw_list().contains(&id) {
            match (self.inbox.mail[id].store.filter(|_| self.live.is_some()), self.inbox.items.contains(&id)) {
                // Real mail: find and pin it by store id, as ⌘K does. A folder's cached rows may
                // hold it while its next read, a page from the top, would not (an older ⌘K hit).
                (Some(store), _) => self.open_found(store, cx),
                (None, true) => self.open_box(None, cx),
                (None, false) if self.inbox.trash.contains(&id) => self.open_kind(Kind::Trash, cx),
                (None, false) => {
                    if let Some(f) = self.inbox.folders.iter().position(|f| f.items.contains(&id)) {
                        self.open_box(Some(f), cx);
                    }
                }
            }
        }
        let id = match store {
            Some(store) => match self.inbox.by_store.get(&store) {
                Some(&id) => id,
                None => { self.notify("That message is no longer available locally", cx); return; }
            },
            None => id,
        };
        if self.inbox.unread_only { self.filter_unread(false, cx); }
        match self.inbox.list().iter().position(|&rep| self.inbox.members(rep).contains(&id)) {
            Some(ix) => self.act_at(ix, what, 0., cx),
            None => self.notify("That message is not in a loaded list. Try again after the next sync.", cx),
        }
    }

    /// The window of message `id` (`None`: every message window) closes: the message left its
    /// list, or the inbox was replaced and its ids now name other messages. Deferred, as the
    /// move may come from that window, which GPUI is updating now.
    pub(super) fn close_popouts(&mut self, id: Option<usize>, cx: &mut Context<Self>) {
        self.popouts.retain(|&(i, h)| {
            let close = id.is_none_or(|id| id == i);
            if close {
                cx.defer(move |cx| {
                    h.update(cx, |_, window, _| window.remove_window()).ok();
                });
            }
            !close
        });
    }
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    #[gpui::test]
    fn icon_toolbar_keeps_every_action_inside_a_narrow_message_window(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
        view.update_in(cx, |v, window, cx| v.open_popout(window, cx));
        let popout = view.read_with(cx, |v, _| v.popouts[0].1);
        let mut popup = gpui::VisualTestContext::from_window(popout.into(), cx);
        popup.simulate_resize(gpui::size(px(420.), px(500.)));
        popup.run_until_parked();
        let bounds: Vec<_> = ["Reply", "reply-all", "Forward", "Later", "Done"].into_iter().map(|label| popup.debug_bounds(label).unwrap()).collect();
        for (i, a) in bounds.iter().enumerate() {
            assert!(a.left() >= px(0.) && a.right() <= px(420.), "action {i} must fit");
            for b in bounds.iter().skip(i + 1) { assert!(!a.intersects(b), "toolbar actions must not overlap"); }
        }
    }
}
