//! The compact sidebar: one icon per section, the mailboxes in a popover, AI Agents and Settings at the foot.
//! One sidebar button beside the window controls goes between full and rail, and brings a hidden sidebar back.

use super::*;

/// Rail and hidden keep this row on top for the window controls and the sidebar button.
pub(super) const TITLE_ROW: f32 = if cfg!(target_os = "macos") { 36. } else { 44. };
const SEARCH_TIP: &str = if cfg!(target_os = "macos") { "Search · ⌘K" } else { "Search · Ctrl+K" };
const BUTTON: f32 = 36.;
const POPOVER_W: f32 = 248.;

impl MainWindow {
    /// Full, rail or hidden. The sidebar slides from the width it shows now, so a change mid-slide turns smoothly.
    pub(super) fn set_sidebar(&mut self, hidden: bool, rail: bool, cx: &mut Context<Self>) {
        self.finish_resize(cx);
        if (hidden, rail) == (self.panes.sidebar_hidden, self.panes.sidebar_rail) { return; }
        // A slide to or from hidden stays a slide when reversed mid-way, so its content keeps sliding.
        let hiding = hidden || self.panes.sidebar_hidden || self.sidebar_slide.is_some_and(|s| s.2);
        self.panes.sidebar_hidden = hidden;
        self.panes.sidebar_rail = rail;
        self.sidebar_slide = (!crate::motion::reduced()).then(|| (Instant::now(), self.sidebar_shown_w, hiding));
        self.save_panes(cx);
        cx.notify();
    }

    /// The sidebar button: in the title-bar row after the traffic lights (macOS) or at the top left, in every state.
    pub(super) fn sidebar_button(&self) -> Stateful<Div> {
        let mac = cfg!(target_os = "macos");
        let hidden = self.panes.sidebar_hidden;
        let tip = match (hidden, self.panes.sidebar_rail, mac) {
            (true, _, true) => "Show sidebar · ⌥⌘S",
            (true, _, false) => "Show sidebar · Ctrl+Alt+S",
            (false, false, true) => "Compact sidebar · ⇧⌥⌘S",
            (false, false, false) => "Compact sidebar · Ctrl+Alt+Shift+S",
            (false, true, true) => "Full sidebar · ⇧⌥⌘S",
            (false, true, false) => "Full sidebar · Ctrl+Alt+Shift+S",
        };
        // Occlude: a search button sliding or fading past must not take its click.
        icon_button(&self.theme, "sidebar-toggle", "icons/sidebar.svg", tip)
            .absolute()
            .occlude()
            // macOS: 8 px after the traffic lights, which span x 18..78, on their centre line.
            .left(px(if mac { 86. } else { 12. }))
            .top(px(if mac { 11. } else { 12. }))
            .on_click(move |_, window, cx| if hidden { window.dispatch_action(Box::new(ToggleSidebar), cx) } else { window.dispatch_action(Box::new(ToggleRail), cx) })
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
            .gap(px(2.))
            .pt(px(10.))
            .pb(px(10.))
            .child(button("rail-search", "icons/search.svg", SEARCH_TIP, false)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenPalette), cx)))
            .child(div().h(px(8.)).flex_shrink_0())
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
                    gpui::anchored().position_mode(gpui::AnchoredPositionMode::Local).position(gpui::point(px((RAIL_W + BUTTON) / 2. + 6.), px(-6.)))
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

/// A square icon button. Active, it matches the full sidebar's selected row: the grey selection under the selection icon colour.
fn rail_button(t: &Theme, id: &'static str, path: &'static str, tip: &'static str, active: bool, window_active: bool) -> Stateful<Div> {
    let theme = *t;
    let (fill, ink) = match (active, window_active) {
        (true, true) => (Some(t.sel), t.sel_icon),
        (true, false) => (Some(t.sel), t.ink2),
        (false, _) => (None, t.ink2),
    };
    pressable(div().id(id).group(id), t, active)
        .debug_selector(move || format!("{id}-{}", if active { "filled" } else { "idle" }))
        .relative()
        .size(px(BUTTON))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::CONTROL))
        .when_some(fill, |d, fill| d.bg(fill))
        .child(icon(path, ink, 18.).when(!active, |icon| icon.group_hover(id, { let ink = theme.ink; move |s| s.text_color(ink) })))
        .tooltip(move |_, cx| cx.new(|_| Tip(theme, tip.into())).into())
}

/// A count on an icon: the rail has no room for the full sidebar's quiet number beside a label.
/// A ring in the sidebar's colour cuts it out of the selected square.
fn badge(t: &Theme, count: usize) -> Div {
    let text = if count > 99 { "99+".to_string() } else { count.to_string() };
    let ring = if t.dark { gpui::rgb(crate::theme::SIDEBAR_DARK) } else { gpui::rgb(crate::theme::SIDEBAR_LIGHT) };
    div().absolute().top(px(-3.)).right(px(-4.)).h(px(15.)).min_w(px(15.)).px(px(3.)).rounded_full()
        .border_2().border_color(ring)
        .flex().items_center().justify_center().bg(t.accent)
        .child(mono(text, t.accent_ink).text_size(px(9.)).font_weight(FontWeight::SEMIBOLD))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn settle(view: &Entity<MainWindow>, cx: &mut VisualTestContext) {
        view.update(cx, |v, cx| { if let Some((_, from, hiding)) = v.sidebar_slide { v.sidebar_slide = Some((Instant::now() - SLIDE, from, hiding)); } cx.notify(); });
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
        view.update(cx, |v, cx| { v.sidebar_slide = Some((Instant::now() - SLIDE / 4, full.0, false)); cx.notify(); });
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
        // A hide reversed mid-slide keeps sliding the rail; it does not turn into a cross-fade.
        cx.dispatch_action(ToggleSidebar);
        view.update(cx, |v, cx| { v.sidebar_slide = Some((Instant::now() - SLIDE / 4, RAIL_W, true)); cx.notify(); });
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        cx.dispatch_action(ToggleSidebar);
        assert!(view.read_with(cx, |v, _| v.sidebar_slide.is_some_and(|s| s.2)));
        settle(&view, cx);
        // Hidden, the rail toggle shows the rail.
        cx.dispatch_action(ToggleSidebar);
        cx.dispatch_action(ToggleRail);
        settle(&view, cx);
        view.read_with(cx, |v, _| assert_eq!((v.panes.sidebar_hidden, v.panes.sidebar_rail), (false, true)));
        // The sidebar button stays in one place: rail to full, full to rail, and hidden back to what it was.
        let button = cx.debug_bounds("sidebar-toggle").unwrap();
        let click = |cx: &mut VisualTestContext| {
            assert_eq!(cx.debug_bounds("sidebar-toggle").unwrap(), button);
            cx.simulate_click(button.center(), Default::default());
            settle(&view, cx);
        };
        click(cx);
        view.read_with(cx, |v, _| assert_eq!((v.panes.full_sidebar(), v.sidebar_w, v.list_w), (true, full.0, full.1)));
        click(cx);
        assert!(view.read_with(cx, |v, _| v.panes.sidebar_rail));
        cx.dispatch_action(ToggleSidebar);
        settle(&view, cx);
        click(cx);
        view.read_with(cx, |v, _| assert_eq!((v.panes.sidebar_hidden, v.panes.sidebar_rail), (false, true)));
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
        // While it is open, mail keys do not reach the message behind it.
        let (rows, tasks, toast) = view.read_with(cx, |v, _| (v.inbox.list().len(), v.today.tasks.len(), v.toast.as_ref().map(|t| t.0.clone())));
        cx.dispatch_action(DeleteMessage);
        cx.dispatch_action(NewTask);
        assert_eq!(view.read_with(cx, |v, _| (v.inbox.list().len(), v.today.tasks.len(), v.toast.as_ref().map(|t| t.0.clone()))), (rows, tasks, toast));
        // The palette opens on top and closes the popover.
        cx.dispatch_action(OpenPalette);
        assert!(!view.read_with(cx, |v, _| v.mailbox_popover));
        cx.dispatch_action(Cancel);
        click(cx, "rail-mailboxes-idle");
        cx.dispatch_action(Cancel);
        assert!(!view.read_with(cx, |v, _| v.mailbox_popover));
    }
}
