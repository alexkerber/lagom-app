use super::*;

impl MainWindow {
    pub(super) fn reload_draft_rows(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let path = live.store.path().to_owned();
        let read = cx.background_executor().spawn(async move { Store::open(path)?.draft_summaries() });
        self.draft_read = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.draft_read = None;
                match result {
                    Ok(rows) => this.draft_rows = rows,
                    Err(error) => this.notify(format!("Could not read drafts: {error}"), cx),
                }
                cx.notify();
            }).ok();
        }));
    }

    pub(super) fn show_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) { return; }
        self.release(false, cx);
        self.clear_motion();
        self.set_page(Page::Drafts, cx);
        self.reload_draft_rows(cx);
        window.focus(&self.focus);
        cx.notify();
    }

    pub(super) fn open_draft(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) { return; }
        let compose = if let Some(index) = self.drafts.iter().position(|c| c.read(cx).draft_id() == id) {
            self.drafts.remove(index)
        } else {
            let Some(live) = &self.live else { return };
            let draft = match live.store.take_draft(id) {
                Ok(draft) => draft,
                Err(error) => { self.notify(format!("Could not open draft: {error}"), cx); return; }
            };
            let Some(account) = live.accounts.iter().find(|a| a.id == draft.account).cloned() else { return };
            let store = live.store.clone();
            let compose = cx.new(|cx| Compose::restore(draft, account, store, window, cx));
            self.watch_compose(&compose, window, cx);
            compose
        };
        compose.update(cx, |compose, cx| compose.reopen(window, cx));
        self.compose = Some(compose);
        cx.notify();
    }

    pub(super) fn drafts_view(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        div().flex_1().min_w(px(0.)).flex().flex_col()
            .child(toolbar(t).px(px(20.)).child("Drafts"))
            .child(div().id("draft-list").debug_selector(|| "draft-list".into()).flex_1().min_h(px(0.)).overflow_y_scroll()
                .p(px(20.)).flex().flex_col().gap(px(8.))
                .when(self.draft_rows.is_empty(), |d| d.child(div().text_color(t.ink2).child("No drafts")))
                .children(self.draft_rows.iter().rev().map(|draft| {
                    let id = draft.id.clone();
                    let account = self.live.as_ref().and_then(|live| live.accounts.iter().find(|a| a.id == draft.account))
                        .map(|a| a.email.clone()).unwrap_or_default();
                    pressable(div().id(SharedString::from(format!("draft-{}", draft.id))), t, false)
                        .debug_selector(|| "draft-row".into()).p(px(12.)).rounded(px(radius::CONTROL)).border_1().border_color(t.line)
                        .flex().flex_col().gap(px(4.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).one_line().child(if draft.subject.is_empty() { "(no subject)".into() } else { draft.subject.clone() }))
                        .child(div().text_color(t.ink2).one_line().child(format!("{account} · To: {}", draft.to)))
                        .child(div().text_color(t.ink2).one_line().child(lagom_core::mime::clean_snippet(&draft.snippet)))
                        .child(button(t, "Edit draft", None).on_click(cx.listener(move |this, _, window, cx| this.open_draft(&id, window, cx))))
                })))
    }
}
