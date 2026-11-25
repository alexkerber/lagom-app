use super::*;
use crate::settings::agents::{Agents, AGENTS};
use lagom_core::agent::{policy::{Access, Setting}, protocol::Code};
use crate::agent::confirm::Tag;
use lagom_core::store::Store;

/// A click on Settings > AI Agents.
pub(crate) enum AgentChange {
    /// The value the switch shows after the click, so a second window cannot flip it back.
    Toggle(Setting, bool),
    /// The first Connect click: list the files, write nothing.
    Preview(String),
    Cancel,
    /// The kind, and the files the preview listed.
    Connect(String, Vec<String>),
    /// Revoke in the store, then remove what Connect wrote.
    Disconnect(String),
    DisconnectAll,
    RemoveRemote(String),
    Select(Option<String>),
    /// Agent "" is the global matrix.
    Level(String, Access),
}

fn core<T>(result: Result<T, Code>) -> anyhow::Result<T> {
    result.map_err(|code| anyhow::anyhow!(code.message()))
}

/// This computer's name, as agents list it. Read when the page opens.
#[cfg(windows)]
fn host() -> String { std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This computer".into()) }

#[cfg(unix)]
fn host() -> String {
    std::process::Command::new("hostname").arg("-s").output().ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "This computer".into())
}

mod files {
    use super::*;
    use crate::agent::detect::{detect, Agent, Connection, Plan};
    use lagom_core::agent::token::TokenScope;

    pub fn detected(a: &Agents) -> Vec<(String, &'static str)> {
        detect(&a.home, &a.path).into_iter().map(|client| (client.agent.kind(), client.agent.name())).collect()
    }

    fn agent(kind: &str) -> anyhow::Result<Agent> {
        Agent::ALL.into_iter().find(|a| a.kind() == kind).ok_or_else(|| anyhow::anyhow!("Unknown agent"))
    }

    fn plan(a: &Agents, store: Option<&Store>, agent: Agent, id: &str) -> anyhow::Result<Plan> {
        let client = detect(&a.home, &a.path).into_iter().find(|c| c.agent == agent)
            .ok_or_else(|| anyhow::anyhow!("Agent is no longer installed"))?;
        let data = match store {
            Some(store) => store.path().parent().ok_or_else(|| anyhow::anyhow!("Missing store directory"))?.to_owned(),
            None => Store::data_dir()?,
        };
        #[cfg(windows)]
        let binary = crate::agent::windows::installed_binary()?;
        #[cfg(unix)]
        let binary = std::env::current_exe()?;
        Plan::in_dir(client, id, &a.home, &a.path, &binary, &data)
    }

    /// The paths Connect will write, with the home directory as ~.
    pub fn preview(a: &Agents, store: Option<&Store>, kind: &str) -> anyhow::Result<Vec<String>> {
        let plan = plan(a, store, agent(kind)?, "000000000000")?;
        let short = |p: &std::path::Path| match p.strip_prefix(&a.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => p.display().to_string(),
        };
        let token = plan.token.parent().map(|dir| format!("{}/<agent>.token", short(dir)));
        Ok(token.into_iter()
            .chain([&plan.skill, &plan.config, &plan.link].into_iter().flatten().map(|p| short(p)))
            .collect())
    }

    pub fn connect(store: &Store, a: &Agents, kind: &str) -> anyhow::Result<String> {
        let agent = agent(kind)?;
        let id = match a.agents.iter().find(|r| r.kind == kind && !r.remote) {
            Some(r) => r.id.clone(),
            None => core(store.create_agent(kind, agent.name(), &a.host, false))?,
        };
        // A Disconnect that failed half-way leaves its receipt; finish it before writing again.
        remove(store, &id, false)?;
        let plan = plan(a, Some(store), agent, &id)?;
        let token = core(store.issue_token(&id, TokenScope::Local))?;
        plan.connect_saved(&token, |receipt| {
            core(store.set_agent_connection(&id, Some(&serde_json::to_string(receipt)?)))
        }).inspect_err(|_| { let _ = store.revoke_agent_tokens(&id); })?;
        Ok(format!("{} connected", agent.name()))
    }

    fn token_path(store: &Store, id: &str) -> anyhow::Result<std::path::PathBuf> {
        Ok(store.path().parent().ok_or_else(|| anyhow::anyhow!("Missing store directory"))?
            .join("agents").join(format!("{id}.token")))
    }

    #[cfg(unix)]
    pub(super) fn create_remote(store: &Store, fields: [&str; 4], accounts: &[String], level: lagom_core::agent::policy::Level)
        -> anyhow::Result<(zeroize::Zeroizing<String>, String)> {
        use anyhow::{ensure, Context as _};
        use std::io::Write;
        use std::process::{Command, Stdio};
        let [host, sources, date, key] = fields.map(str::trim);
        ensure!(!host.is_empty(), "Enter a host name");
        ensure!(!accounts.is_empty(), "Select at least one account");
        let ranges = crate::settings::agents::source_ranges(sources)?;
        let expires = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").context("Use YYYY-MM-DD for expiry")?.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let now = chrono::Utc::now();
        ensure!(expires > now && expires <= now + chrono::Days::new(365), "Choose an expiry in the next 365 days");
        ensure!(!key.chars().any(char::is_control), "Paste one SSH public key");
        let mut parts = key.split_whitespace();
        let (kind, blob) = parts.next().zip(parts.next()).context("Paste an SSH public key")?;
        ensure!(kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-"), "Paste a plain SSH public key");
        let key = format!("{kind} {blob}");
        let mut check = Command::new("/usr/bin/ssh-keygen").args(["-l", "-f", "-"]).stdin(Stdio::piped())
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().context("Could not check the SSH key")?;
        writeln!(check.stdin.take().unwrap(), "{key}")?;
        ensure!(check.wait()?.success(), "Invalid SSH public key");
        let quote = |path: &std::path::Path| -> anyhow::Result<String> {
            let path = path.to_str().context("Unsupported SSH command path")?;
            ensure!(!path.chars().any(char::is_control), "Unsupported SSH command path");
            Ok(format!("'{}'", path.replace('\'', r"'\''")))
        };
        let executable = std::env::current_exe()?;
        let mut command = quote(&executable)?;
        let data = store.path().parent().context("Missing store directory")?;
        if dirs::data_dir().map(|p| p.join("lagom")).as_deref() != Some(data) {
            check_executable(&executable)?;
            command = format!("/usr/bin/env LAGOM_DATA_DIR={} {command}", quote(&data.canonicalize()?)?);
        }
        let (id, token) = core(store.create_remote_agent(host, accounts, level, &ranges, expires.timestamp()))?;
        if let Err(e) = token_path(store, &id).and_then(|path| crate::agent::detect::write_remote_token(&path, &token)) {
            core(store.remove_remote_agent(&id))?;
            return Err(e);
        }
        let from = ranges.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        // sshd unquotes the option before the login shell reads the forced command.
        let command = command.replace('"', r#"\""#);
        let line = format!(r#"restrict,from="{from}",expiry-time="{}",command="{command} remote --agent {id}" {key}"#, expires.format("%Y%m%dZ"));
        Ok((token, line))
    }

    pub(super) fn check_executable(path: &std::path::Path) -> anyhow::Result<()> {
        anyhow::ensure!(path.is_absolute() && path.as_os_str().as_encoded_bytes().iter()
            .all(|b| b.is_ascii_alphanumeric() || b"/._- ".contains(b)), "Unsupported SSH command path");
        Ok(())
    }

    pub fn remove(store: &Store, id: &str, remote: bool) -> anyhow::Result<()> {
        if remote {
            match std::fs::remove_file(token_path(store, id)?) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => return Ok(()),
            }
        }
        if let Some(receipt) = core(store.agent_connection(id))? {
            let left = serde_json::from_str::<Connection>(&receipt)?.disconnect()?;
            anyhow::ensure!(left.is_empty(), "{}", left.join(", "));
            core(store.set_agent_connection(id, None))?;
        }
        Ok(())
    }
}

impl AgentChange {
    /// The toast after a change, if it needs one.
    fn apply(self, store: &Store, a: &Agents) -> anyhow::Result<Option<String>> {
        Ok(match self {
            Self::Toggle(setting, on) => {
                core(store.set_agent_setting(setting, on))?;
                None
            }
            Self::Level(agent, access) => {
                core(store.set_agent_access(&agent, &access))?;
                None
            }
            Self::Connect(kind, listed) => {
                // Write only what the person saw: a changed plan goes back to the preview.
                if files::preview(a, Some(store), &kind)? != listed {
                    anyhow::bail!("the files to write changed. Check the list and connect again");
                }
                Some(files::connect(store, a, &kind)?)
            }
            Self::Disconnect(id) => {
                core(store.revoke_agent_tokens(&id))?;
                files::remove(store, &id, a.agent(&id).is_some_and(|r| r.remote))?;
                Some(format!("{} disconnected", a.agent(&id).map_or("Agent", |r| r.name.as_str())))
            }
            Self::RemoveRemote(id) => {
                files::remove(store, &id, true)?;
                core(store.remove_remote_agent(&id))?;
                None
            }
            Self::DisconnectAll => {
                core(store.revoke_all_agents())?;
                // Every key is gone already; clean up each agent's files and report the first failure.
                let mut failed = None;
                for r in core(store.agents())? {
                    if let Err(e) = files::remove(store, &r.id, r.remote) { failed.get_or_insert(e); }
                }
                if let Some(e) = failed { return Err(e); }
                Some("All agents disconnected".into())
            }
            Self::Preview(_) | Self::Cancel | Self::Select(_) => None,
        })
    }
}

impl MainWindow {
    #[cfg(unix)]
    pub(crate) fn create_remote(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.agents.remote else { return };
        let Some(live) = &self.live else { return self.notify("Start without LAGOM_DEMO to create an agent", cx) };
        let fields = form.fields.each_ref().map(|f| f.read(cx).text());
        match files::create_remote(&live.store, fields, &form.accounts, form.level) {
            Ok(issued) => {
                self.agents.remote = None;
                self.agents.show_token = false;
                self.agents.issued = Some(issued);
                self.reload_agents(cx);
            }
            Err(e) => self.notify(format!("Could not create agent: {e:#}"), cx),
        }
        cx.notify();
    }

    pub(crate) fn open_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.agents.pending = None;
        self.reload_agents(cx);
        if self.live.is_some() {
            self.agents.detected = files::detected(&self.agents);
            self.agents.host = host();
        }
        self.set_page(Page::Settings(Some(AGENTS.into())), cx);
        cx.notify();
    }

    /// The store's state; the demo has fixed rows instead.
    pub(super) fn reload_agents(&mut self, cx: &mut Context<Self>) {
        let result = match &self.live {
            Some(live) => self.agents.load(&live.store),
            None => {
                if self.demo && self.agents.agents.is_empty() { self.agents = Agents::demo(); }
                Ok(())
            }
        };
        if let Err(code) = result {
            self.notify(format!("Could not read AI Agents settings: {}", code.message()), cx);
        }
    }

    pub(crate) fn agent_change(&mut self, change: AgentChange, cx: &mut Context<Self>) {
        #[cfg(unix)]
        { self.agents.issued = None; }
        match change {
            AgentChange::Select(id) => {
                // The demo has no store to reload the selected agent's own rows from.
                if self.live.is_none() {
                    self.agents.own = id.as_ref().and_then(|id| self.agents.matrices.get(id)).cloned().unwrap_or_default();
                }
                self.agents.selected = id;
                self.agents.pending = None;
            }
            AgentChange::Cancel => self.agents.pending = None,
            AgentChange::Connect(kind, _) if self.live.is_none() => {
                self.agents.pending = None;
                self.notify(format!("Demo mode: start without LAGOM_DEMO to connect {kind}"), cx);
            }
            AgentChange::Preview(kind) => match files::preview(&self.agents, self.live.as_ref().map(|live| live.store.as_ref()), &kind) {
                Ok(files) => self.agents.pending = Some((kind, files)),
                Err(e) => self.notify(format!("Could not connect: {e:#}"), cx),
            },
            change => {
                let Some(live) = &self.live else {
                    return self.notify("Demo mode: start without LAGOM_DEMO to change AI Agents", cx);
                };
                self.agents.pending = None;
                let store = live.store.clone();
                match change.apply(&store, &self.agents) {
                    Ok(Some(done)) => self.notify(done, cx),
                    Ok(None) => {}
                    Err(e) => self.notify(format!("Could not save: {e:#}"), cx),
                }
            }
        }
        self.reload_agents(cx);
        cx.notify();
    }
}

impl MainWindow {
    pub(super) fn watch_agent_sheets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_sheets.prune(cx.default_global::<crate::agent::confirm::Denials>());
        for sheet in &mut self.agent_sheets.sheets {
            if let Some(done) = sheet.closed.take() {
                cx.spawn(async move |this, cx| {
                    let _ = done.await;
                    this.update(cx, |_, cx| cx.notify()).ok();
                })
                .detach();
            }
        }
        if !self.agent_sheets.sheets.is_empty() {
            self.agent_deny.focus(window);
        }
    }

    fn answer_agent(&mut self, serial: usize, approve: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent_sheets.sheets.front().is_none_or(|s| s.serial != serial) {
            return;
        }
        if let Some(sheet) = self.agent_sheets.sheets.pop_front() {
            let agent = sheet.agent.clone();
            let approved = sheet.decide(approve);
            self.agent_sheets.scroll = Default::default();
            self.agent_sheets.answered(approve && !approved);
            if !approved { cx.default_global::<crate::agent::confirm::Denials>().deny(agent); }
        }
        if self.agent_sheets.sheets.is_empty() {
            self.focus.focus(window);
        } else {
            self.agent_deny.focus(window);
        }
        cx.notify();
    }

    pub(super) fn agent_sheet(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet = &self.agent_sheets.sheets[0];
        let serial = sheet.serial;
        let t = self.theme;
        let overflow = |scroll: &gpui::ScrollHandle| {
            let max = scroll.max_offset().height;
            (max > px(0.), max + scroll.offset().y > px(0.5))
        };
        let status = self.agent_sheets.scroll.each_ref().map(overflow);
        cx.on_next_frame(window, move |this, _, cx| {
            if this.agent_sheets.sheets.front().is_some_and(|s| s.serial == serial)
                && this.agent_sheets.scroll.each_ref().map(overflow) != status { cx.notify(); }
        });
        // Show the arming state only; the click handlers check it again.
        let armed = self.agent_sheets.armed(serial, false, false, window.is_window_active());
        if let Some(left) = self.agent_sheets.front_since.filter(|_| !armed)
            .and_then(|(_, at)| self.agent_sheets.delay().checked_sub(at.elapsed())) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(left + std::time::Duration::from_millis(10)).await;
                this.update(cx, |_, cx| cx.notify()).ok();
            }).detach();
        }
        // Who and from where, then who receives it, then what it is.
        let group = |label: &str| match label {
            "Agent" | "From" | "Account" => 0,
            "To" | "Cc" | "Bcc" | "Guest" => 1,
            _ => 2,
        };
        // Tags are outlined, not tinted: a tint drops red text below 4.5:1 on the plate.
        let mut plates: Vec<Div> = Vec::new();
        let mut last = None;
        for r in &sheet.rows {
            let g = group(r.label);
            let first = last != Some(g);
            if first {
                plates.push(div().flex_shrink_0().flex().flex_col().rounded(px(radius::PANEL)).bg(t.subtle).border_1().border_color(t.line).overflow_hidden());
            }
            last = Some(g);
            let text = |v: &String| div().min_w(px(0.)).child(v.clone());
            let value = div().flex_1().min_w(px(0.)).flex().flex_wrap().text_size(px(size::BASE))
                .when(matches!(r.label, "Agent" | "Title" | "Subject"), |v| v.font_weight(FontWeight::MEDIUM))
                .children(r.old.as_ref().map(|old| text(old).text_color(t.ink2).line_through()))
                .children(r.old.as_ref().map(|_| div().flex_shrink_0().mx(px(6.)).text_color(t.ink2).child("→")))
                .child(text(&r.value));
            let tag = r.tag.map(|tag| match tag {
                Tag::Added => ("Added", t.accent),
                Tag::Removed => ("Removed", t.danger),
            });
            let row = div().flex().items_start().gap(px(12.)).px(px(14.)).py(px(7.)).line_height(px(20.))
                .when(!first, |r| r.border_t_1().border_color(t.line))
                .child(div().w(px(112.)).flex_shrink_0().text_size(px(size::SNIPPET)).text_color(t.ink2).child(r.label))
                .child(value)
                .children(tag.map(|(word, color)| div().flex_shrink_0().px(px(6.)).rounded(px(radius::KEY)).border_1().border_color(color)
                    .text_size(px(size::SNIPPET)).font_weight(FontWeight::MEDIUM).text_color(color).child(word)));
            if let Some(plate) = plates.pop() {
                plates.push(plate.child(row));
            }
        }
        let caption = |text: &'static str| div().flex_shrink_0().pt(px(4.)).text_size(px(size::SMALL)).font_weight(FontWeight::MEDIUM).text_color(t.ink2).child(text);
        let text_box = |index: usize, id: &'static str, text: String, cap: f32, ink: Rgba| div().flex_shrink_0().child(div().id((id, serial)).flex_shrink_0().max_h(px(cap)).overflow_y_scroll().track_scroll(&self.agent_sheets.scroll[index])
            .px(px(14.)).py(px(10.)).rounded(px(radius::PANEL)).bg(t.subtle).border_1().border_color(t.line)
            .text_size(px(size::BASE)).line_height(relative(1.45)).text_color(ink).child(text))
            .when(status[index].1, |d| d.child(caption("More below ↓").debug_selector(move || format!("more-{id}"))));
        let delete = sheet.approve_label == "Delete event";
        let approve = if armed {
            button(&t, sheet.approve_label, Some(if delete { t.danger } else { t.accent })).border_1().border_color(gpui::transparent_black())
        } else {
            // Before arming it is no stronger than Deny.
            button(&t, sheet.approve_label, None).border_1().border_color(t.line).text_color(t.ink2)
        };
        let guarded = |button: gpui::Stateful<gpui::Div>, approve: bool| button.id((if approve { "approve-agent" } else { "revoke-agent" }, serial))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, event: &gpui::MouseDownEvent, w, _| {
                this.agent_sheets.pressed = this.agent_sheets.armed(serial, false, event.first_mouse, w.is_window_active()).then_some((serial, approve));
            }))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                if this.agent_sheets.pressed.take() != Some((serial, approve))
                    || !this.agent_sheets.armed(serial, event.is_keyboard(), event.first_focus(), w.is_window_active()) {
                    return;
                }
                if let Some(sheet) = this.agent_sheets.sheets.front().filter(|s| s.serial == serial) {
                    let agent = sheet.agent.clone();
                    this.answer_agent(serial, approve, w, cx);
                    if !approve { this.agent_change(AgentChange::Disconnect(agent), cx); }
                }
            }));
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.))
            .bg(t.subtle)
            .text_color(t.ink)
            .on_key_down(cx.listener(move |this, e: &gpui::KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if k.key == "escape" {
                    this.answer_agent(serial, false, window, cx);
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .id("agent-confirmation")
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .p(px(20.))
                    .rounded(px(radius::CARD))
                    .bg(t.bg)
                    .border_1()
                    .border_color(t.line)
                    .w(px(560.))
                    .max_h_full()
                    .child(page_title(sheet.heading))
                    .when(self.agent_sheets.withdrawn, |d| d.child("The previous request was withdrawn"))
                    .child(
                        div()
                            .id("agent-preview")
                            .track_scroll(&self.agent_sheets.scroll[2])
                            .overflow_y_scroll()
                            .flex_col()
                            .flex()
                            .gap(px(8.))
                            .children(plates)
                            .when_some(sheet.previous_description.clone(), |d, description| d.child(caption("Current description"))
                                .child(text_box(1, "current-description", description, if sheet.expanded { 320. } else { 120. }, t.ink2)))
                            .child(caption(if sheet.approve_label == "Send" { "Message" } else { "Description" }))
                            .child(text_box(0, "confirmation-body", sheet.body.clone(), if sheet.expanded { 320. } else { 180. }, t.ink))
                            .when(!sheet.expanded && status[..2].iter().any(|s| s.0), |d| {
                                d.child(div().flex().child(button(&t, "Expand", None).ml(px(-12.)).text_color(t.ink2).on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(s) = this.agent_sheets.sheets.front_mut() {
                                        s.expanded = true;
                                    }
                                    cx.notify();
                                }))))
                            }),
                    )
                    .when(status[2].1, |d| d.child(caption("More below ↓").debug_selector(|| "more-agent-preview".into())))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                guarded(button(&t, "Deny and revoke", None).ml(px(-12.)).text_color(if armed { t.danger } else { t.ink2 }), false),
                            )
                            .child(div().flex_1())
                            .child(
                                button(&t, "Deny", None)
                                    .track_focus(&self.agent_deny)
                                    .border_1()
                                    .border_color(t.line)
                                    .focus(move |s| s.border_color(t.accent))
                                    .on_click(
                                        cx.listener(move |this, _, w, cx| this.answer_agent(serial, false, w, cx)),
                                    ),
                            )
                            .child(
                                guarded(approve, true),
                            ),
                    ),
            )
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use lagom_core::{account::{Security, ServerConfig}, agent::policy::Level};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn forced_command_requires_a_safe_absolute_executable() {
        use std::path::Path;
        for path in ["/Applications/mail=app/lagom", "/tmp/mail;app", "/tmp/mail\napp", "lagom"] {
            assert_eq!(files::check_executable(Path::new(path)).unwrap_err().to_string(), "Unsupported SSH command path");
        }
        assert!(files::check_executable(Path::new("/Applications/Lagom Mail.app/Contents/MacOS/lagom")).is_ok());
    }

    #[test]
    fn ssh_line_matches_the_issued_scope_and_retires_with_its_agent() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = tempfile::Builder::new().prefix("agent's mail ").tempdir_in(temp.path())?;
        let store = Store::open(dir.path().join("lagom.sqlite3"))?;
        let config = ServerConfig { host: "localhost".into(), port: 993, username: "test".into(), security: Security::Tls };
        let account = Account::new("test@example.org".into(), config.clone(), config)?;
        store.save_account(&account)?;
        let date = chrono::Utc::now().date_naive() + chrono::Days::new(30);
        let expiry = date.to_string();
        let key = dir.path().join("key");
        assert!(std::process::Command::new("/usr/bin/ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&key).status()?.success());
        let public = std::fs::read_to_string(key.with_extension("pub"))?;
        let fields = ["build host", "203.0.113.0/24,2001:db8::1", &expiry, &public];
        let create = || files::create_remote(&store, fields, std::slice::from_ref(&account.id), Level::Triage);
        let (token, line) = create()?;
        let source = Some("203.0.113.1");
        core(store.set_agent_setting(Setting::Enabled, true))?;
        let global = Access { account: account.id.clone(), resource: lagom_core::agent::policy::Resource::Mail, level: Level::Send };
        core(store.set_agent_access("", &global))?;
        assert_eq!(core(store.effective(Some(&token), source, global.resource, &account.id))?, Level::Triage);

        let row = core(store.agents())?.pop().unwrap();
        let file = dir.path().join("agents").join(format!("{}.token", row.id));
        assert_eq!(std::fs::metadata(&file)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&file)?, *token);
        let (options, rest) = line.split_once(",command=\"").unwrap();
        let (command, key) = rest.split_once("\" ").unwrap();
        let (bin, id) = command.split_once(" remote --agent ").unwrap();
        assert_eq!(bin, format!("/usr/bin/env LAGOM_DATA_DIR='{}' '{}'", dir.path().canonicalize()?.display().to_string().replace('\'', r"'\''"), std::env::current_exe()?.display()));
        assert_eq!(row.id, id);
        assert_eq!(key, public.split_whitespace().take(2).collect::<Vec<_>>().join(" "));
        assert_eq!(options, format!("restrict,from=\"203.0.113.0/24,2001:db8::1/128\",expiry-time=\"{}\"", date.format("%Y%m%dZ")));
        assert_eq!(row.expires, Some(date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()));
        let mut a = Agents::new();
        core(a.load(&store))?;
        AgentChange::Disconnect(id.into()).apply(&store, &a)?;
        assert!(!file.exists());
        AgentChange::RemoveRemote(id.into()).apply(&store, &a)?;
        create()?;
        assert_ne!(core(store.agents())?[0].id, id);
        assert!(!file.exists());
        assert!(store.authenticate(Some(&token), Some("203.0.113.1")).is_err());
        Ok(())
    }
}
