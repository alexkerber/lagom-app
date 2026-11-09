use super::*;

pub(super) struct MessageBody {
    id: usize,
    mail: Mail,
    main: gpui::WeakEntity<MainWindow>,
    focus: FocusHandle,
    web: html::Web,
    theme: Theme,
    overlay: bool,
}

impl html::WebHost for MessageBody {
    fn hints(&mut self, over: bool, command: bool, shift: bool, cx: &mut Context<Self>) {
        self.main.update(cx, |main, cx| main.update_hints(over, gpui::Modifiers { platform: command, shift, ..Default::default() }, cx)).ok();
    }
    fn mail_action(&mut self, action: Run, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.id;
        self.main.update(cx, |main, cx| main.mail_action_on(id, action, window, cx)).ok();
    }
    fn new_task(&mut self, cx: &mut Context<Self>) {
        let id = self.id;
        self.main.update(cx, |main, cx| main.task_from_mail(id, cx)).ok();
    }
    fn web(&mut self) -> &mut html::Web { &mut self.web }
    fn keys(&self) -> FocusHandle { self.focus.clone() }
}

impl MessageBody {
    pub(super) fn refresh(&mut self, mail: &Mail, cx: &mut Context<Self>) {
        let remote = self.mail.remote;
        self.mail = mail.clone();
        self.mail.remote = remote;
        cx.notify();
    }

    pub(super) fn prepare(&mut self, theme: Theme, overlay: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.theme.dark != theme.dark || self.overlay != overlay { cx.notify(); }
        self.theme = theme;
        self.overlay = overlay;
        self.web.frame(Some((self.id, &self.mail)), &theme, overlay, window, cx);
    }
}

impl Render for MessageBody {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (theme, overlay) = (self.theme, self.overlay);
        self.web.frame(Some((self.id, &self.mail)), &theme, overlay, window, cx);
        let id = self.id;
        let download = self.main.clone();
        let reply = self.main.clone();
        let main = self.main.clone();
        let open = main.upgrade().is_some_and(|main| main.read(cx).message_menu == Some(id));
        let menu = polish::reading_menu(&theme, id, open, move |action, window, cx| {
            main.update(cx, |main, cx| main.message_action(id, *action, window, cx)).ok();
        });
        let remote = self.web.has_remote && !self.mail.remote;
        // shortcut: expanded bodies have their own scroll area; use measured HTML heights when continuous body scrolling is needed.
        div().h(px(480.)).flex_shrink_0().flex().flex_col()
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.web.unfocus()))
            .when(remote, |d| d.child(button(&theme, "Load remote images", None).on_click(cx.listener(|this, _, _, cx| {
                this.mail.remote = true;
                cx.notify();
            }))))
            .child(reading_view(&theme, (id, &self.mail), overlay || !html::SUPPORTED || self.mail.html.is_none(), self.web.slot(), 0, menu,
                move |section, _, cx| { download.update(cx, |main, cx| main.download_attachment(id, section, cx)).ok(); },
                move |_, window, cx| { reply.update(cx, |main, cx| main.compose_for(Some(id), window, cx)).ok(); }))
    }
}

impl MainWindow {
    pub(super) fn toggle_message(&mut self, id: usize, mark_read: bool, cx: &mut Context<Self>) {
        if self.expanded.remove(&id).is_none() {
            self.open_message(id, mark_read, cx);
            let mail = self.inbox.mail[id].clone();
            let main = cx.entity().downgrade();
            let focus = self.focus.clone();
            self.expanded.insert(id, cx.new(|_| MessageBody { id, mail, main, focus, web: html::Web::default(), theme: self.theme, overlay: true }));
        }
        cx.notify();
    }

    pub(super) fn conversation(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme;
        div().id(("conversation", self.reading_thread.unwrap_or(0))).flex_1().min_h(px(0.)).overflow_y_scroll()
            .flex().flex_col().track_scroll(&self.reading_scroll)
            .children(self.reading_members.iter().map(|&id| {
                let m = &self.inbox.mail[id];
                let expanded = self.expanded.contains_key(&id);
                div().flex().flex_col().flex_shrink_0().border_b_1().border_color(t.line)
                    .child(pressable(div().id(("message-header", id)), &t, false)
                        .flex().flex_col().gap(px(4.)).px(px(24.)).py(px(12.)).cursor_pointer()
                        .child(div().flex().gap(px(8.)).items_center()
                            .child(icon(if expanded { "icons/chevron.svg" } else { "icons/chevron-right.svg" }, t.ink2, 16.))
                            .child(div().flex_1().min_w(px(0.)).one_line().font_weight(if m.unread { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).child(m.from.clone()))
                            .child(mono(mail::full_date(m.date), t.ink2).flex_shrink_0()))
                        .child(div().one_line().text_color(t.ink2).child(m.snippet.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_message(id, true, cx))))
                    .children(self.expanded.get(&id).cloned())
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn inline_completion_refreshes_expanded_mail_without_resetting_remote_consent(cx: &mut gpui::TestAppContext) {
        let (main, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
        main.update(cx, |main, cx| {
            let id = main.inbox.single().unwrap();
            main.toggle_message(id, false, cx);
            let expanded = main.expanded[&id].clone();
            let mut mail = main.inbox.mail[id].clone();
            mail.inline = std::sync::Arc::new(vec![lagom_core::inline::InlinePart { cid: "logo".into(), mime_type: "image/png".into(), data: vec![1] }]);
            expanded.update(cx, |body, cx| {
                body.mail.remote = true;
                body.refresh(&mail, cx);
                assert!(body.mail.remote);
                assert_eq!(body.mail.inline[0].data, [1]);
            });
        });
    }
}
