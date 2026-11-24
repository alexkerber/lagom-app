use super::*;
use crate::settings::agents::{Agents, AGENTS};
use lagom_core::agent::{policy::{Access, Setting}, protocol::Code};
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
    Select(Option<String>),
    /// Agent "" is the global matrix.
    Level(String, Access),
}

fn core<T>(result: Result<T, Code>) -> anyhow::Result<T> {
    result.map_err(|code| anyhow::anyhow!(code.message()))
}

/// This computer's name, as agents list it. Read when the page opens.
fn host() -> String {
    std::process::Command::new("hostname").arg("-s").output().ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "This computer".into())
}

#[cfg(unix)]
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
        Plan::in_dir(client, id, &a.home, &a.path, &std::env::current_exe()?, &data)
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
        remove(store, &id)?;
        let plan = plan(a, Some(store), agent, &id)?;
        let token = core(store.issue_token(&id, TokenScope::Local))?;
        plan.connect_saved(&token, |receipt| {
            core(store.set_agent_connection(&id, Some(&serde_json::to_string(receipt)?)))
        }).inspect_err(|_| { let _ = store.revoke_agent_tokens(&id); })?;
        Ok(format!("{} connected", agent.name()))
    }

    pub fn remove(store: &Store, id: &str) -> anyhow::Result<()> {
        if let Some(receipt) = core(store.agent_connection(id))? {
            let left = serde_json::from_str::<Connection>(&receipt)?.disconnect()?;
            anyhow::ensure!(left.is_empty(), "{}", left.join(", "));
            core(store.set_agent_connection(id, None))?;
        }
        Ok(())
    }
}

// shortcut: Windows has no detection or Connect yet (plan step g); the page revokes in the store only.
#[cfg(not(unix))]
mod files {
    use super::*;
    pub fn detected(_: &Agents) -> Vec<(String, &'static str)> { Vec::new() }
    pub fn preview(_: &Agents, _: Option<&Store>, _: &str) -> anyhow::Result<Vec<String>> { anyhow::bail!("Not supported on this system") }
    pub fn connect(_: &Store, _: &Agents, _: &str) -> anyhow::Result<String> { anyhow::bail!("Not supported on this system") }
    pub fn remove(_: &Store, _: &str) -> anyhow::Result<()> { Ok(()) }
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
                files::remove(store, &id)?;
                Some(format!("{} disconnected", a.agent(&id).map_or("Agent", |r| r.name.as_str())))
            }
            Self::DisconnectAll => {
                core(store.revoke_all_agents())?;
                // Every key is gone already; clean up each agent's files and report the first failure.
                let mut failed = None;
                for r in core(store.agents())? {
                    if let Err(e) = files::remove(store, &r.id) { failed.get_or_insert(e); }
                }
                if let Some(e) = failed { return Err(e); }
                Some("All agents disconnected".into())
            }
            Self::Preview(_) | Self::Cancel | Self::Select(_) => None,
        })
    }
}

impl MainWindow {
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
        match change {
            AgentChange::Select(id) => {
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

#[cfg(unix)]
impl MainWindow {
    pub(super) fn watch_agent_sheets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_sheets.0.retain(|s| !s.answer.is_closed());
        for sheet in &mut self.agent_sheets.0 {
            if let Some(done) = sheet.closed.take() {
                cx.spawn(async move |this, cx| {
                    let _ = done.await;
                    this.update(cx, |_, cx| cx.notify()).ok();
                })
                .detach();
            }
        }
        if !self.agent_sheets.0.is_empty() {
            self.agent_deny.focus(window);
        }
    }

    fn answer_agent(&mut self, serial: usize, approve: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent_sheets.0.front().is_none_or(|s| s.serial != serial) {
            return;
        }
        if let Some(sheet) = self.agent_sheets.0.pop_front() {
            sheet.decide(approve);
        }
        if self.agent_sheets.0.is_empty() {
            self.focus.focus(window);
        } else {
            self.agent_deny.focus(window);
        }
        cx.notify();
    }

    pub(super) fn agent_sheet(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet = &self.agent_sheets.0[0];
        let serial = sheet.serial;
        let t = self.theme;
        let truncated = !sheet.expanded && sheet.body.chars().nth(2000).is_some();
        let body = if truncated {
            sheet.body.chars().take(2000).collect()
        } else {
            sheet.body.clone()
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.subtle)
            .text_color(t.ink)
            .on_key_down(cx.listener(move |this, e: &gpui::KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if k.key == "escape" {
                    this.answer_agent(serial, false, window, cx);
                }
                if crate::shortcuts::submit().matches(k) && !e.is_held {
                    this.answer_agent(serial, true, window, cx);
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .id("agent-confirmation")
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .bg(t.raised)
                    .w(px(560.))
                    .max_h_full()
                    .child(sheet.heading.clone())
                    .child(
                        div()
                            .id("agent-preview")
                            .overflow_y_scroll()
                            .flex_col()
                            .child(sheet.details.clone())
                            .child(body)
                            .when(truncated, |d| {
                                d.child(button(&t, "Show all", None).on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(s) = this.agent_sheets.0.front_mut() {
                                        s.expanded = true;
                                    }
                                    cx.notify();
                                })))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                button(&t, "Deny", None)
                                    .track_focus(&self.agent_deny)
                                    .border_1()
                                    .border_color(t.accent)
                                    .on_click(
                                        cx.listener(move |this, _, w, cx| this.answer_agent(serial, false, w, cx)),
                                    ),
                            )
                            .child(
                                button(
                                    &t,
                                    sheet.approve_label,
                                    Some(t.accent),
                                )
                                .id(("approve-agent", serial))
                                .on_click(
                                    cx.listener(move |this, _, w, cx| this.answer_agent(serial, true, w, cx)),
                                ),
                            ),
                    ),
            )
    }
}
