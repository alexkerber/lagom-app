//! The compact sidebar: one icon per section, the mailboxes in a popover, AI Agents and Settings at the foot.

use super::*;

pub(super) const COLLAPSE_TIP: &str = if cfg!(target_os = "macos") { "Compact sidebar · ⇧⌥⌘S" } else { "Compact sidebar · Ctrl+Alt+Shift+S" };
const EXPAND_TIP: &str = if cfg!(target_os = "macos") { "Full sidebar · ⇧⌥⌘S" } else { "Full sidebar · Ctrl+Alt+Shift+S" };
const SEARCH_TIP: &str = if cfg!(target_os = "macos") { "Search · ⌘K" } else { "Search · Ctrl+K" };
const BUTTON: f32 = 40.;
const POPOVER_W: f32 = 248.;

impl MainWindow {
    /// Full, rail or hidden. The sidebar slides from the width it shows now, so a change mid-slide turns smoothly.
    pub(super) fn set_sidebar(&mut self, hidden: bool, rail: bool, cx: &mut Context<Self>) {
        self.finish_resize(cx);
        if (hidden, rail) == (self.panes.sidebar_hidden, self.panes.sidebar_rail) { return; }
        self.panes.sidebar_hidden = hidden;
        self.panes.sidebar_rail = rail;
        self.sidebar_slide = (!crate::motion::reduced()).then(|| (Instant::now(), self.sidebar_shown_w));
        self.save_panes(cx);
        cx.notify();
    }

    pub(super) fn rail(&self, viewport_h: f32, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let (in_agents, in_settings) = self.footer_active();
        let unread = self.mail_unread();
        let on_mail = matches!(self.page, Page::Mail | Page::Drafts);
        let button = |id: &'static str, path: &'static str, tip: &'static str, active: bool| rail_button(t, id, path, tip, active, self.window_active);
        div()
            .debug_selector(|| "rail".into())
            .w(px(RAIL_W))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .pb(px(14.))
            // The title-bar row as in the full sidebar: the traffic lights on macOS, then Expand.
            .pt(px(if cfg!(target_os = "macos") { 11. + 28. + 8. } else { 12. }))
            .child(button("rail-expand", "icons/sidebar.svg", EXPAND_TIP, false)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleRail), cx)))
            .child(button("rail-search", "icons/search.svg", SEARCH_TIP, false)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenPalette), cx)))
            .child(div().h(px(12.)).flex_shrink_0())
            .children(SECTIONS.iter().enumerate().map(|(ix, &(_, path))| {
                let active = matches!((&self.page, ix), (Page::Home, 0) | (Page::Mail | Page::Drafts, 1) | (Page::Calendar, 2) | (Page::Todo, 3));
                button(["rail-today", "rail-mail", "rail-calendar", "rail-todo"][ix], path, SECTION_TIPS[ix], active)
                    .when(ix == 1 && unread > 0, |d| d.child(badge(t, unread).debug_selector(|| "rail-mail-badge".into())))
                    .on_click(cx.listener(move |this, _, window, cx| this.show_section(ix, window, cx)))
            }))
            // The full sidebar's mailboxes, folders and accounts, one click away on the Mail pages.
            .when(on_mail, |d| d.child(div().mt(px(8.)).relative().flex_shrink_0()
                .child(button("rail-mailboxes", "icons/inbox.svg", "Mailboxes, folders and accounts", false)
                    .when(self.mailbox_popover, |d| d.bg(t.sel))
                    .on_click(cx.listener(|this, _, _, cx| { this.mailbox_popover = !this.mailbox_popover; cx.notify(); })))
                .when(self.mailbox_popover, |d| d.child(gpui::deferred(
                    gpui::anchored().position_mode(gpui::AnchoredPositionMode::Local).position(gpui::point(px((RAIL_W + BUTTON) / 2. + 6.), px(-8.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(self.mailbox_popover_view(viewport_h, cx))).priority(1)))))
            .child(div().flex_1())
            .when(self.agents.sidebar, |d| d.child(button("rail-agents", "icons/connect.svg", "AI Agents", in_agents)
                .when(self.agents.active() > 0, |d| d.child(badge(t, self.agents.active())))
                .on_click(cx.listener(|this, _, window, cx| this.open_agents(window, cx)))))
            .child(button("rail-settings", "icons/settings.svg", if cfg!(target_os = "macos") { "Settings · ⌘," } else { "Settings · Ctrl+," }, in_settings)
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))))
    }

    fn mailbox_popover_view(&self, viewport_h: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = &self.theme;
        div().id("mailbox-popover").debug_selector(|| "mailbox-popover".into())
            .w(px(POPOVER_W)).max_h(px((viewport_h - 16.).max(120.))).overflow_y_scroll()
            .p(px(6.)).bg(t.raised).border_1().border_color(t.line).rounded(px(radius::CONTROL))
            .occlude()
            // A row that opens a mailbox, folder or account closes the popover; disclosures stop the click first.
            .on_click(cx.listener(|this, _, _, cx| { this.mailbox_popover = false; cx.notify(); }))
            .child(self.mail_nav(cx))
    }
}

/// A square icon button with the section fill when active, as the full sidebar's rows have.
fn rail_button(t: &Theme, id: &'static str, path: &'static str, tip: &'static str, active: bool, window_active: bool) -> Stateful<Div> {
    let theme = *t;
    let ink = if active { if window_active { t.sel_icon } else { t.ink2 } } else { t.ink2 };
    pressable(div().id(id).group(id), t, active)
        .debug_selector(move || format!("{id}-{}", if active { "filled" } else { "idle" }))
        .relative()
        .size(px(BUTTON))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::CONTROL))
        .when(active, |d| d.bg(t.sel))
        .child(icon(path, ink, 18.).when(!active, |icon| icon.group_hover(id, { let ink = theme.ink; move |s| s.text_color(ink) })))
        .tooltip(move |_, cx| cx.new(|_| Tip(theme, tip.into())).into())
}

/// A count on an icon: the rail has no room for the full sidebar's quiet number beside a label.
fn badge(t: &Theme, count: usize) -> Div {
    let text = if count > 99 { "99+".to_string() } else { count.to_string() };
    div().absolute().top(px(3.)).right(px(1.)).h(px(15.)).min_w(px(15.)).px(px(4.)).rounded_full()
        .flex().items_center().justify_center().bg(t.accent)
        .child(mono(text, t.accent_ink).text_size(px(10.)).font_weight(FontWeight::SEMIBOLD))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn settle(view: &Entity<MainWindow>, cx: &mut VisualTestContext) {
        view.update(cx, |v, cx| { if let Some((_, from)) = v.sidebar_slide { v.sidebar_slide = Some((Instant::now() - SLIDE, from)); } cx.notify(); });
        cx.run_until_parked();
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
    }

    #[gpui::test]
    fn rail_cycles_with_both_toggles_and_keeps_the_list_width(cx: &mut TestAppContext) {
        cx.update(|cx| bind_keys(cx));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
        cx.run_until_parked();
        let full = view.read_with(cx, |v, _| (v.sidebar_w, v.list_w));
        cx.dispatch_action(ToggleRail);
        // Between full and rail the sidebar cross-fades in place, narrowing from the full width.
        view.update(cx, |v, cx| { v.sidebar_slide = Some((Instant::now() - SLIDE / 4, full.0)); cx.notify(); });
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        let mid = view.read_with(cx, |v, _| v.sidebar_shown_w);
        assert!(mid > RAIL_W && mid < full.0, "{mid}");
        settle(&view, cx);
        view.read_with(cx, |v, _| {
            assert!(v.panes.sidebar_rail && !v.panes.sidebar_hidden);
            assert_eq!((v.sidebar_w, v.sidebar_shown_w, v.list_w), (RAIL_W, RAIL_W, full.1));
        });
        assert_eq!(cx.debug_bounds("rail").unwrap().size.width, px(RAIL_W));
        assert!(cx.debug_bounds("rail-mail-badge").is_some());
        assert!(cx.debug_bounds("rail-mail-filled").is_some() && cx.debug_bounds("rail-today-idle").is_some());
        // The show-or-hide toggle hides the rail and brings it back as the rail.
        for hidden in [true, false] {
            cx.dispatch_action(ToggleSidebar);
            settle(&view, cx);
            view.read_with(cx, |v, _| assert_eq!((v.panes.sidebar_hidden, v.panes.sidebar_rail, v.sidebar_shown_w), (hidden, true, if hidden { 0. } else { RAIL_W })));
        }
        // Hidden, the rail toggle shows the rail; the rail's Expand goes back to full.
        cx.dispatch_action(ToggleSidebar);
        cx.dispatch_action(ToggleRail);
        settle(&view, cx);
        view.read_with(cx, |v, _| assert_eq!((v.panes.sidebar_hidden, v.panes.sidebar_rail), (false, true)));
        let at = cx.debug_bounds("rail-expand-idle").unwrap().center();
        cx.simulate_click(at, Default::default());
        settle(&view, cx);
        view.read_with(cx, |v, _| assert_eq!((v.panes.full_sidebar(), v.sidebar_w, v.list_w), (true, full.0, full.1)));
        // The full sidebar's header button collapses it.
        let at = cx.debug_bounds("sidebar-collapse").unwrap().center();
        cx.simulate_click(at, Default::default());
        settle(&view, cx);
        assert!(view.read_with(cx, |v, _| v.panes.sidebar_rail));
    }

    #[gpui::test]
    fn rail_popover_keeps_every_mailbox_folder_and_account_in_reach(cx: &mut TestAppContext) {
        cx.update(|cx| bind_keys(cx));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "rail", window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("nav-Inbox").is_none());
        let click = |cx: &mut VisualTestContext, id: &'static str| {
            let at = cx.debug_bounds(id).unwrap_or_else(|| panic!("{id}")).center();
            cx.simulate_click(at, Default::default());
            cx.run_until_parked();
        };
        click(cx, "rail-mailboxes-idle");
        assert!(cx.debug_bounds("mailbox-popover").unwrap().left() >= px(RAIL_W));
        for id in ["nav-Inbox", "nav-Later", "nav-Sent", "nav-Done", "nav-Drafts", "nav-Trash", "folder-0", "add-folder"] {
            assert!(cx.debug_bounds(id).is_some(), "{id}");
        }
        // A disclosure keeps it open; a mailbox opens and closes it.
        click(cx, "disclosure-Done");
        assert!(view.read_with(cx, |v, _| v.mailbox_popover && v.mailbox_expanded[1]));
        click(cx, "nav-Trash");
        view.read_with(cx, |v, _| assert!(!v.mailbox_popover && v.inbox.open_kind() == Some(Kind::Trash)));
        // A click outside closes it and does nothing else; Esc closes it too.
        click(cx, "rail-mailboxes-idle");
        let outside = cx.debug_bounds("rail-today-idle").unwrap().center();
        cx.simulate_click(outside, Default::default());
        view.read_with(cx, |v, _| assert!(!v.mailbox_popover && matches!(v.page, Page::Mail)));
        click(cx, "rail-mailboxes-idle");
        cx.dispatch_action(Cancel);
        assert!(!view.read_with(cx, |v, _| v.mailbox_popover));
    }
}
