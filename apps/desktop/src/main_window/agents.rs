use super::*;
use crate::settings::agents::{Agents, AGENTS};
use lagom_core::agent::{policy::{Access, Setting}, protocol::Code};
use lagom_core::store::Store;

/// A click on Settings > AI Agents.
pub(crate) enum AgentChange {
    Toggle(Setting),
    /// The first Connect click: list the files, write nothing.
    Preview(String),
    Cancel,
    Connect(String),
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
        detect(&a.home, &a.path).into_iter().map(|agent| (agent.kind(), agent.name())).collect()
    }

    fn agent(kind: &str) -> anyhow::Result<Agent> {
        Agent::ALL.into_iter().find(|a| a.kind() == kind).ok_or_else(|| anyhow::anyhow!("Unknown agent"))
    }

    fn plan(a: &Agents, agent: Agent, id: &str) -> anyhow::Result<Plan> {
        Plan::new(agent, id, &a.home, &a.path, &std::env::current_exe()?)
    }

    /// The paths Connect will write, with the home directory as ~.
    pub fn preview(a: &Agents, kind: &str) -> anyhow::Result<Vec<String>> {
        let plan = plan(a, agent(kind)?, "000000000000")?;
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
        let plan = plan(a, agent, &id)?;
        let token = core(store.issue_token(&id, TokenScope::Local))?;
        let connection = plan.connect(&token).inspect_err(|_| { let _ = store.revoke_agent_tokens(&id); })?;
        let saved = serde_json::to_string(&connection).map_err(anyhow::Error::from)
            .and_then(|receipt| core(store.set_agent_connection(&id, Some(&receipt))));
        if let Err(e) = saved {
            let _ = store.revoke_agent_tokens(&id);
            let _ = connection.disconnect();
            return Err(e);
        }
        Ok(format!("{} connected", agent.name()))
    }

    pub fn remove(store: &Store, id: &str) -> anyhow::Result<()> {
        if let Some(receipt) = core(store.agent_connection(id))? {
            serde_json::from_str::<Connection>(&receipt)?.disconnect()?;
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
    pub fn preview(_: &Agents, _: &str) -> anyhow::Result<Vec<String>> { anyhow::bail!("Not supported on this system") }
    pub fn connect(_: &Store, _: &Agents, _: &str) -> anyhow::Result<String> { anyhow::bail!("Not supported on this system") }
    pub fn remove(_: &Store, _: &str) -> anyhow::Result<()> { Ok(()) }
}

impl AgentChange {
    /// The toast after a change, if it needs one.
    fn apply(self, store: &Store, a: &Agents) -> anyhow::Result<Option<String>> {
        Ok(match self {
            Self::Toggle(setting) => {
                core(store.set_agent_setting(setting, !core(store.agent_setting(setting))?))?;
                None
            }
            Self::Level(agent, access) => {
                core(store.set_agent_access(&agent, &access))?;
                None
            }
            Self::Connect(kind) => Some(files::connect(store, a, &kind)?),
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
            AgentChange::Preview(kind) => match files::preview(&self.agents, &kind) {
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
