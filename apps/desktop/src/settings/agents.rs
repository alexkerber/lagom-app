//! Settings > AI Agents: the master switch, the agents on this computer, the access matrices,
//! confirmations and the audit log. The core enforces every level this page shows.

#[cfg(unix)]
mod remote;
#[cfg(unix)]
pub(crate) use remote::{source_ranges, RemoteForm};

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use gpui::{div, prelude::*, px, relative, Context, Div, FontWeight, Rgba, SharedString};
use lagom_core::account::Account;
use lagom_core::agent::{audit::AuditEntry, policy::{Access, Level, Resource, Setting}, protocol::Code, token::AgentRow};
use lagom_core::store::Store;

use super::{plate, section, stop, toggle_row, REPOSITORY};
use crate::main_window::{button, icon, page_title, icon_button, mono, pressable, AgentChange, MainWindow, OneLine};
use crate::theme::{radius, size, Theme};

/// `Page::Settings(Some(AGENTS))` opens this page. Account IDs are UUIDs, so it cannot clash.
pub(crate) const AGENTS: &str = "ai-agents";

const MAIL: [Level; 4] = [Level::None, Level::Read, Level::Triage, Level::Send];
const CALENDAR: [Level; 3] = [Level::None, Level::Read, Level::Send];
const SEGMENT: f32 = 52.;

/// What the page shows, read from the store when it opens and after each change.
#[derive(Default)]
pub struct Agents {
    #[cfg(unix)]
    pub remote: Option<RemoteForm>,
    #[cfg(unix)]
    pub issued: Option<(zeroize::Zeroizing<String>, String)>,
    #[cfg(unix)]
    pub show_token: bool,
    pub enabled: bool,
    pub socket_error: Option<Code>,
    pub confirm: bool,
    pub sidebar: bool,
    /// Kind and name of each agent found on this computer when the page opened.
    pub detected: Vec<(String, &'static str)>,
    /// This computer's name, stored with a local agent when it connects.
    pub host: String,
    pub agents: Vec<AgentRow>,
    /// Each agent's own access rows, for what its list row says it may do.
    pub matrices: HashMap<String, Vec<Access>>,
    pub global: Vec<Access>,
    /// The agent whose own matrix shows; `None` shows the global matrix.
    pub selected: Option<String>,
    pub own: Vec<Access>,
    pub activity: Vec<AuditEntry>,
    /// Connect waits for a second click: the kind and the files it will write.
    pub pending: Option<(String, Vec<String>)>,
    /// Where detection and Connect look. Tests point them at a temporary home.
    pub home: PathBuf,
    pub path: OsString,
}

impl Agents {
    pub fn new() -> Self {
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
            path: std::env::var_os("PATH").unwrap_or_default(),
            ..Default::default()
        }
    }

    pub fn load(&mut self, store: &Store) -> Result<(), Code> {
        self.enabled = store.agent_setting(Setting::Enabled)?;
        #[cfg(unix)]
        {
            self.socket_error = store.path().parent().filter(|_| self.enabled)
                .and_then(|dir| crate::agent::socket_path(dir).err());
        }
        self.confirm = store.agent_setting(Setting::Confirm)?;
        self.sidebar = store.agent_setting(Setting::Sidebar)?;
        self.agents = store.agents()?;
        if self.selected.as_ref().is_some_and(|id| !self.agents.iter().any(|a| &a.id == id)) {
            self.selected = None;
        }
        self.global = store.agent_access("")?;
        self.matrices = self.agents.iter().map(|a| Ok((a.id.clone(), store.agent_access(&a.id)?))).collect::<Result<_, Code>>()?;
        self.own = match &self.selected {
            Some(id) => store.agent_access(id)?,
            None => Vec::new(),
        };
        self.activity = store.agent_audit(50)?;
        Ok(())
    }

    pub fn active(&self) -> usize {
        self.agents.iter().filter(|a| a.active).count()
    }

    pub fn agent(&self, id: &str) -> Option<&AgentRow> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Detected agents with their store row, then connected agents that detection no longer finds.
    fn list(&self) -> Vec<(String, String, Option<&AgentRow>)> {
        let local = |kind: &str| self.agents.iter().find(|a| a.kind == kind && !a.remote);
        let mut rows: Vec<_> = self.detected.iter().map(|(kind, name)| (kind.clone(), name.to_string(), local(kind))).collect();
        rows.extend(self.agents.iter()
            .filter(|a| a.remote || (a.active && !self.detected.iter().any(|(k, _)| *k == a.kind)))
            .map(|a| (a.kind.clone(), a.name.clone(), Some(a))));
        rows
    }

    /// The highest level an agent reaches on any account: the lower of its own row and the global one.
    fn reach(&self, id: &str, resource: Resource, accounts: &[Account]) -> Level {
        let own = self.matrices.get(id).map_or(&[][..], Vec::as_slice);
        let ids: Vec<&str> = if accounts.is_empty() { vec![""] } else { accounts.iter().map(|a| a.id.as_str()).collect() };
        ids.into_iter().map(|account| stored(own, resource, account).min(stored(&self.global, resource, account))).max().unwrap_or(Level::None)
    }

    /// What a connected agent may do, for example "Triage mail · Read calendar".
    fn may(&self, id: &str, accounts: &[Account]) -> String {
        let mail = match self.reach(id, Resource::Mail, accounts) {
            Level::None => None,
            Level::Read => Some("Read mail"),
            Level::Triage => Some("Triage mail"),
            Level::Send => Some("Send mail"),
        };
        let calendar = match self.reach(id, Resource::Calendar, accounts) {
            Level::None => None,
            Level::Read => Some("Read calendar"),
            Level::Triage | Level::Send => Some("Change calendar"),
        };
        let parts: Vec<_> = [mail, calendar].into_iter().flatten().collect();
        if parts.is_empty() { "No access".into() } else { parts.join(" · ") }
    }

    /// The level on screen. An agent's own level never shows above the global one.
    pub fn shown(&self, resource: Resource, account: &str) -> Level {
        let global = stored(&self.global, resource, account);
        match self.selected {
            Some(_) => stored(&self.own, resource, account).min(global),
            None => global,
        }
    }

    /// The level a click would replace: the selected agent's own row, else the global one.
    fn stored(&self, resource: Resource, account: &str) -> Level {
        stored(if self.selected.is_some() { &self.own } else { &self.global }, resource, account)
    }

    /// Shown, stored and the highest level a segment may offer: the global level while an agent is selected.
    fn levels(&self, resource: Resource, account: &str) -> (Level, Level, Level) {
        let cap = if self.selected.is_some() { stored(&self.global, resource, account) } else { Level::Send };
        (self.shown(resource, account), self.stored(resource, account), cap)
    }

    /// Fixed sample rows for `LAGOM_DEMO=1`; no store, no file system.
    pub fn demo() -> Self {
        let now = chrono::Utc::now().timestamp();
        let access = |resource, account: &str, level| Access { resource, account: account.into(), level };
        let own = vec![
            access(Resource::Mail, "", Level::Read),
            access(Resource::Calendar, "", Level::Read),
            access(Resource::Mail, "personal", Level::Send),
        ];
        let remote = vec![
            access(Resource::Mail, "", Level::None),
            access(Resource::Calendar, "", Level::None),
            access(Resource::Mail, "personal", Level::Read),
            access(Resource::Calendar, "personal", Level::Read),
        ];
        let entry = |ago: i64, agent: &str, account: &str, command: &str, result: &str| AuditEntry {
            at: now - ago, agent: Some(agent.into()), account: Some(account.into()), command: command.into(), result: result.into(),
        };
        Self {
            enabled: true,
            confirm: true,
            sidebar: true,
            detected: vec![
                ("ClaudeCode".into(), "Claude Code"), ("Codex".into(), "Codex"), ("Cursor".into(), "Cursor"),
                ("Hermes".into(), "Hermes"), ("OpenClaw".into(), "OpenClaw"), ("ClaudeDesktop".into(), "Claude Desktop"),
            ],
            host: "studio".into(),
            agents: vec![
                AgentRow { id: "3f9a1c0b77de".into(), kind: "ClaudeCode".into(), name: "Claude Code".into(), host: "studio".into(),
                    remote: false, active: true, expires: None, last_used: Some(now - 120) },
                AgentRow { id: "8c21d0e4a913".into(), kind: "remote".into(), name: "build-server".into(), host: "build-server".into(),
                    remote: true, active: true, expires: Some(now + 30 * 86_400), last_used: Some(now - 3 * 3600) },
            ],
            matrices: HashMap::from([("3f9a1c0b77de".into(), own.clone()), ("8c21d0e4a913".into(), remote)]),
            global: vec![
                access(Resource::Mail, "", Level::Read),
                access(Resource::Calendar, "", Level::Read),
                access(Resource::Mail, "personal", Level::Triage),
                access(Resource::Calendar, "personal", Level::Send),
                access(Resource::Mail, "work", Level::None),
            ],
            // What a selected demo agent holds: Send on Personal mail shows as the global Triage.
            own,
            activity: vec![
                entry(90, "3f9a1c0b77de", "personal", "action.archive", "ok"),
                entry(240, "3f9a1c0b77de", "personal", "thread", "ok"),
                entry(260, "3f9a1c0b77de", "work", "search", "not_found:outside_scope"),
                entry(300, "3f9a1c0b77de", "personal", "search", "ok"),
                entry(3600, "8c21d0e4a913", "personal", "events", "ok"),
            ],
            ..Self::new()
        }
    }
}

/// The account's own row, else the group default, else None.
fn stored(rows: &[Access], resource: Resource, account: &str) -> Level {
    let find = |a: &str| rows.iter().find(|r| r.resource == resource && r.account == a).map(|r| r.level);
    find(account).or_else(|| find("")).unwrap_or(Level::None)
}

fn word(level: Level) -> &'static str {
    match level {
        Level::None => "None",
        Level::Read => "Read",
        Level::Triage => "Triage",
        Level::Send => "Send",
    }
}

fn note(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::SNIPPET)).text_color(t.ink2).line_height(relative(1.4)).child(text.into())
}

/// The agent's own mark (sources in assets/logos/README.md), or its initial where none is vendored.
/// Single-colour marks are drawn as a mask in their colour; the others as their own image.
fn mark(t: &Theme, kind: &str, name: &str) -> Div {
    let tile = div().size(px(32.)).flex_shrink_0().flex().items_center().justify_center()
        .rounded(px(radius::CONTROL)).bg(t.bg).border_1().border_color(t.line);
    let image = |path: &'static str, s: f32| gpui::img(gpui::ImageSource::Resource(gpui::Resource::Embedded(path.into()))).size(px(s)).flex_shrink_0();
    match kind {
        // Claude Code's mascot in Claude's brand colour, as code.claude.com shows it.
        "ClaudeCode" => tile.child(icon("logos/claude-code.svg", gpui::rgb(0xD97757), 22.)),
        "ClaudeDesktop" => tile.child(image("logos/claude.svg", 22.)),
        // Cursor's flat cube is single-colour: black on light, white on dark.
        "Cursor" => tile.child(icon("logos/cursor.svg", t.ink, 20.)),
        // The file keeps OpenAI's clear space around the Blossom, so it draws larger than the others.
        "Codex" => tile.child(image(if t.dark { "logos/openai-white.svg" } else { "logos/openai-black.svg" }, 38.)),
        "OpenClaw" => tile.child(image("logos/openclaw.svg", 24.)),
        _ => tile.text_size(px(size::BASE)).font_weight(FontWeight::SEMIBOLD).text_color(t.ink2)
            .child(name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default()),
    }
}

/// A small pill after an agent's name: a dot and a word, tinted with its colour.
fn tag(text: &'static str, color: Rgba) -> Div {
    div().flex_shrink_0().h(px(18.)).px(px(7.)).flex().items_center().gap(px(5.)).rounded(px(radius::KEY))
        .bg(Rgba { a: 0.14, ..color }).text_color(color).text_size(px(size::SMALL)).font_weight(FontWeight::MEDIUM)
        .child(div().size(px(6.)).rounded_full().bg(color))
        .child(text)
}

/// "Used 5 min ago" while recent, then the activity log's date words.
fn used(at: i64, now: chrono::DateTime<chrono::Local>) -> String {
    match now.timestamp() - at {
        ..=59 => "Used just now".into(),
        s @ ..=3599 => format!("Used {} min ago", s / 60),
        s @ ..=86_399 => format!("Used {} h ago", s / 3600),
        _ => format!("Used {}", crate::mail::when(at, now).replace("Yesterday", "yesterday")),
    }
}

/// A row of segments, one per level. Segments above `cap` are dimmed and do nothing.
/// `saved`: the stored level; a click on the shown level still pins an agent whose row is higher.
fn levels(t: &Theme, key: String, levels: &[Level], (on, saved, cap): (Level, Level, Level), agent: &str, resource: Resource, account: &str, cx: &mut Context<MainWindow>) -> Div {
    let mut row = div().flex().flex_shrink_0().gap(px(2.)).p(px(2.)).rounded(px(radius::CONTROL)).border_1().border_color(t.line).bg(t.bg);
    for &level in levels {
        let id = SharedString::from(format!("{key}-{}", word(level)));
        let selected = level == on;
        let allowed = level <= cap;
        let segment = div().id(id.clone()).debug_selector(move || id.to_string())
            .w(px(SEGMENT)).h(px(22.)).flex().items_center().justify_center().rounded(px(radius::KEY))
            .text_size(px(size::SNIPPET))
            .text_color(if selected { t.ink } else { t.ink2 })
            .when(selected, |s| s.bg(t.sel).font_weight(FontWeight::MEDIUM))
            .when(!allowed, |s| s.opacity(0.35))
            .child(word(level));
        let (agent, account) = (agent.to_string(), account.to_string());
        row = row.child(if allowed && level != saved {
            stop(t, pressable(segment, t, false)).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                this.agent_change(AgentChange::Level(agent.clone(), Access { resource, account: account.clone(), level }), cx)
            }))
        } else {
            segment
        });
    }
    row
}

fn matrix(t: &Theme, a: &Agents, accounts: &[Account], colors: &[Rgba], cx: &mut Context<MainWindow>) -> Div {
    let agent = a.selected.clone().unwrap_or_default();
    let mut row = |label: Div, account: &str, key: &str, first: bool| {
        div().flex().items_center().gap(px(12.)).px(px(14.)).py(px(8.))
            .when(!first, |r| r.border_t_1().border_color(t.line))
            .child(label.flex_1().min_w(px(0.)))
            .child(levels(t, format!("mail-{key}"), &MAIL, a.levels(Resource::Mail, account), &agent, Resource::Mail, account, cx))
            .child(levels(t, format!("calendar-{key}"), &CALENDAR, a.levels(Resource::Calendar, account), &agent, Resource::Calendar, account, cx))
    };
    let column = |label: &'static str, n: usize| div().w(px(SEGMENT * n as f32 + 2. * (n + 1) as f32 + 2.)).flex_shrink_0()
        .text_size(px(size::SMALL)).font_weight(FontWeight::MEDIUM).text_color(t.ink2).child(label);
    let mut rows = plate(t)
        .child(div().flex().gap(px(12.)).px(px(14.)).pt(px(10.)).pb(px(2.))
            .child(div().flex_1())
            .child(column("Mail", MAIL.len()))
            .child(column("Calendar", CALENDAR.len())))
        .child(row(div().flex().flex_col()
            .child(div().font_weight(FontWeight::MEDIUM).one_line().child("Personal accounts"))
            .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).one_line().child("Default for each account")), "", "group", true));
    for (ix, account) in accounts.iter().enumerate() {
        let label = div().flex().items_center().gap(px(10.))
            .child(super::avatar(account, colors[ix], 24.))
            .child(div().min_w(px(0.)).one_line().child(account.email.clone()));
        rows = rows.child(row(label, &account.id, &account.id, false));
    }
    rows
}

/// Every Settings page for AI Agents.
pub fn view(t: &Theme, a: &Agents, accounts: &[Account], colors: &[Rgba], cx: &mut Context<MainWindow>) -> Div {
    let (enabled, confirm, sidebar) = (a.enabled, a.confirm, a.sidebar);
    let header = div().flex().items_center().gap(px(8.))
        .child(page_title("AI Agents").flex_1())
        .child(stop(t, button(t, "Guide", None))
            .on_click(cx.listener(|_, _, _, cx| cx.open_url(&format!("{REPOSITORY}/blob/main/docs/ai-agents.md")))))
        .child(stop(t, button(t, "Disconnect all", None)).text_color(t.danger)
            .on_click(cx.listener(|this, _, _, cx| this.agent_change(AgentChange::DisconnectAll, cx))));
    let access = div().flex().flex_col().gap(px(12.))
        .child(plate(t).px(px(14.)).py(px(6.))
            .child(stop(t, toggle_row(t, "Enable agent access", a.enabled))
                .on_click(cx.listener(move |this, _, _, cx| this.agent_change(AgentChange::Toggle(Setting::Enabled, !enabled), cx))))
            .child(div().flex().flex_col().gap(px(6.)).pb(px(8.))
                .when_some(a.socket_error, |row, code| row.child(div().text_color(t.danger).child(code.message())))
                .child(note(t, "Connected agents can read and act on your mail and calendar, within the levels below. lagom never gives them your password. An agent can send what it reads to the AI service it uses."))
                .child(div().text_size(px(size::SNIPPET)).text_color(t.ink).line_height(relative(1.4))
                    .child("Connect only agents you trust, on a computer you control. A mail can hold instructions written for the agent."))));

    let mut list = plate(t);
    let rows = a.list();
    if rows.is_empty() {
        list = list.child(div().px(px(14.)).py(px(12.)).child(note(t, "No agents found. lagom looks for Claude Code, Codex, Cursor, Hermes and OpenClaw.")));
    }
    let now = chrono::Local::now();
    for (ix, (kind, name, row)) in rows.into_iter().enumerate() {
        let active = row.is_some_and(|r| r.active);
        let pending = a.pending.as_ref().filter(|(k, _)| *k == kind && !active);
        let used = row.and_then(|r| r.last_used).map(|at| used(at, now));
        // Every request fails while agent access is off, so a connected agent is paused, not active.
        let paused = active && !a.enabled;
        let detail = match row {
            Some(r) if r.remote => match (paused, used) {
                (true, _) => "Remote over SSH · Agent access is off".into(),
                (false, Some(used)) => format!("Remote over SSH · {used}"),
                (false, None) => "Remote over SSH".into(),
            },
            Some(_) if paused => "Agent access is off".into(),
            Some(r) if active => [a.may(&r.id, accounts)].into_iter().chain(used).collect::<Vec<_>>().join(" · "),
            _ => "Installed".into(),
        };
        let status = active.then(|| if paused { tag("Paused", t.ink2) } else { tag("Active", t.swipe_done) });
        let mut line = div().flex().items_center().gap(px(12.)).px(px(14.)).min_h(px(56.)).py(px(10.))
            .child(mark(t, &kind, &name))
            .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(1.))
                .child(div().flex().items_center().gap(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).one_line().child(name))
                    .children(status))
                .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).one_line().child(detail)));
        if let Some(remote) = row.filter(|r| r.remote) {
            let id = remote.id.clone();
            let expiry = remote.expires.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));
            let status = expiry.map_or_else(|| "Revoked".into(), |end| format!("{} {}", if end > chrono::Utc::now() { "Expires" } else { "Expired" }, end.date_naive()));
            line = line.child(note(t, status).flex_shrink_0())
                .child(stop(t, button(t, if expiry.is_some() { "Revoke" } else { "Remove" }, None).id(("remote-action", ix)).text_color(t.ink2))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let change = if expiry.is_some() { AgentChange::Disconnect(id.clone()) } else { AgentChange::RemoveRemote(id.clone()) };
                        this.agent_change(change, cx)
                    })));
        } else if let Some(id) = row.filter(|_| active).map(|r| r.id.clone()) {
            line = line.child(stop(t, button(t, "Disconnect", None).id(SharedString::from(format!("disconnect-{kind}")))
                    .debug_selector({ let kind = kind.clone(); move || format!("disconnect-{kind}") })
                    .text_color(t.ink2))
                    .on_click(cx.listener(move |this, _, _, cx| this.agent_change(AgentChange::Disconnect(id.clone()), cx))));
        } else if pending.is_none() {
            let k = kind.clone();
            line = line.child(stop(t, button(t, "Connect", None).id(SharedString::from(format!("connect-{kind}")))
                .debug_selector({ let kind = kind.clone(); move || format!("connect-{kind}") })
                .bg(t.bg).border_1().border_color(t.line))
                .on_click(cx.listener(move |this, _, _, cx| this.agent_change(AgentChange::Preview(k.clone()), cx))));
        }
        let mut block = div().flex().flex_col().when(ix > 0, |b| b.border_t_1().border_color(t.line)).child(line);
        if let Some((_, files)) = pending {
            let (yes, no, listed) = (kind.clone(), kind.clone(), files.clone());
            block = block.child(div().flex().flex_col().gap(px(8.)).pl(px(58.)).pr(px(14.)).pb(px(12.))
                .child(note(t, "Connect writes these files. Disconnect removes them again."))
                .children(files.iter().map(|f| mono(f.clone(), t.ink).one_line()))
                .child(div().flex().gap(px(8.)).pt(px(4.))
                    .child(stop(t, button(t, "Connect", Some(t.accent)).id(SharedString::from(format!("confirm-connect-{yes}")))
                        .debug_selector(move || format!("confirm-connect-{no}")))
                        .on_click(cx.listener(move |this, _, _, cx| this.agent_change(AgentChange::Connect(yes.clone(), listed.clone()), cx))))
                    .child(stop(t, button(t, "Cancel", None))
                        .on_click(cx.listener(|this, _, _, cx| this.agent_change(AgentChange::Cancel, cx))))));
        }
        list = list.child(block);
    }
    let agents = div().flex().flex_col().gap(px(12.))
        .child(section(t, "Agents", Some("Connect a local agent or add one over SSH.")))
        .child(list);

    #[cfg(unix)]
    let agents = agents.child(remote::view(t, a, accounts, cx));

    let selected = a.selected.as_ref().and_then(|id| a.agent(id));
    let mut scope = div().flex().flex_wrap().gap(px(2.)).p(px(2.)).rounded(px(radius::CONTROL)).border_1().border_color(t.line).bg(t.bg);
    for (ix, (id, name)) in std::iter::once((None, "All agents".to_string()))
        .chain(a.agents.iter().map(|r| (Some(r.id.clone()), r.name.clone()))).enumerate() {
        let on = a.selected == id;
        let segment = div().id(("agent-scope", ix)).debug_selector(move || format!("agent-scope-{ix}"))
            .h(px(22.)).px(px(10.)).flex().items_center().rounded(px(radius::KEY)).text_size(px(size::SNIPPET))
            .text_color(if on { t.ink } else { t.ink2 })
            .when(on, |s| s.bg(t.sel).font_weight(FontWeight::MEDIUM))
            .child(name);
        scope = scope.child(stop(t, pressable(segment, t, on)).cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.agent_change(AgentChange::Select(id.clone()), cx))));
    }
    let levels_note = "Read: search and read. Triage: also archive, move and mark. Send: also send mail and change events.";
    let narrowing = match selected {
        Some(agent) => format!("Only for {}. It never gets more than All agents allows; higher levels are dimmed.", agent.name),
        None => "For every agent. Select an agent to give it less.".into(),
    };
    let access_matrix = div().flex().flex_col().gap(px(12.))
        .child(section(t, "Access", Some(levels_note)))
        .when(!a.agents.is_empty(), |m| m.child(div().flex().child(scope)))
        .child(note(t, narrowing))
        .child(matrix(t, a, accounts, colors, cx));

    let options = plate(t).px(px(14.)).py(px(6.))
        .child(stop(t, toggle_row(t, "Confirmation pop-ups", a.confirm))
            .on_click(cx.listener(move |this, _, _, cx| this.agent_change(AgentChange::Toggle(Setting::Confirm, !confirm), cx))))
        .child(note(t, "Ask before an agent sends mail or changes an event.").pb(px(8.)))
        .child(div().border_t_1().border_color(t.line).pt(px(4.))
            .child(stop(t, toggle_row(t, "Show AI Agents in sidebar", a.sidebar))
                .on_click(cx.listener(move |this, _, _, cx| this.agent_change(AgentChange::Toggle(Setting::Sidebar, !sidebar), cx)))));

    div()
        .w_full()
        .max_w(px(600.))
        .flex()
        .flex_col()
        .gap(px(32.))
        .child(div().flex().flex_col().gap(px(12.))
            .child(div().flex().items_center().gap(px(4.)).ml(px(-6.))
                .child(stop(t, icon_button(t, "back", "icons/previous.svg", "Back to Settings · Esc"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_account_settings(window, cx))))
                .child(div().text_color(t.ink2).child("Settings")))
            .child(header))
        .child(access)
        .child(agents)
        .child(access_matrix)
        .child(options)
        .child(activity(t, a, accounts, cx))
}

/// The audit log by agent, newest first, with Revoke on each connected agent.
fn activity(t: &Theme, a: &Agents, accounts: &[Account], cx: &mut Context<MainWindow>) -> Div {
    let mut groups: Vec<(Option<&str>, Vec<&AuditEntry>)> = Vec::new();
    for e in &a.activity {
        match groups.iter_mut().find(|(agent, _)| *agent == e.agent.as_deref()) {
            Some((_, rows)) => rows.push(e),
            None => groups.push((e.agent.as_deref(), vec![e])),
        }
    }
    let now = chrono::Local::now();
    let mut log = div().flex().flex_col().gap(px(12.))
        .child(section(t, "Activity", Some("The latest 50 activity rows per agent. Refusals are grouped by minute. lagom keeps activity for 90 days.")));
    if groups.is_empty() {
        log = log.child(plate(t).px(px(14.)).py(px(12.)).child(note(t, "No requests yet.")));
    }
    for (agent, rows) in groups {
        let row = agent.and_then(|id| a.agent(id));
        let name = match (agent, row) {
            (_, Some(r)) => format!("{} · {}", r.name, r.host),
            (Some(_), None) => "Removed agent".into(),
            (None, None) => "Unknown key".into(),
        };
        let mut group = plate(t)
            .child(div().flex().items_center().gap(px(8.)).px(px(14.)).min_h(px(40.))
                .child(div().flex_1().min_w(px(0.)).font_weight(FontWeight::MEDIUM).one_line().child(name))
                .children(row.filter(|r| r.active).map(|r| {
                    let id = r.id.clone();
                    stop(t, button(t, "Revoke", None).id(SharedString::from(format!("revoke-{id}")))
                        .debug_selector({ let id = id.clone(); move || format!("revoke-{id}") }))
                        .text_color(t.danger)
                        .on_click(cx.listener(move |this, _, _, cx| this.agent_change(AgentChange::Disconnect(id.clone()), cx)))
                })));
        for e in rows {
            let account = e.account.as_deref().map(|id| accounts.iter().find(|x| x.id == id).map_or(id, |x| x.email.as_str())).unwrap_or("");
            let ok = e.result == "ok";
            group = group.child(div().flex().items_center().gap(px(12.)).px(px(14.)).min_h(px(30.)).border_t_1().border_color(t.line)
                .child(mono(crate::mail::when(e.at, now), t.ink2).w(px(64.)).flex_shrink_0())
                .child(mono(e.command.clone(), t.ink).w(px(112.)).flex_shrink_0().one_line())
                .child(div().flex_1().min_w(px(0.)).text_size(px(size::SNIPPET)).text_color(t.ink2).one_line().child(account.to_string()))
                .child(mono(e.result.replace('_', " ").replace(':', " · "), if ok { t.ink2 } else { t.danger }).flex_shrink_0()));
        }
        log = log.child(group);
    }
    log
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn enabled_access_shows_the_socket_path_error_in_settings() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("x".repeat(120));
        std::fs::create_dir(&dir).unwrap();
        let store = Store::open(dir.join("lagom.sqlite3")).unwrap();
        let mut settings = Agents::new();
        for enabled in [true, false] {
            store.set_agent_setting(Setting::Enabled, enabled).unwrap();
            settings.load(&store).unwrap();
            assert_eq!(settings.socket_error, enabled.then_some(Code::SocketPathTooLong));
        }
    }

    #[test]
    fn a_connected_row_says_what_the_agent_may_do_under_both_matrices() {
        let a = Agents::demo();
        let accounts = crate::mail::demo_accounts();
        // Its own Send on Personal mail stays at the global Triage; Work mail is None for everyone.
        assert_eq!(a.may("3f9a1c0b77de", &accounts), "Triage mail · Read calendar");
        assert_eq!(a.may("8c21d0e4a913", &accounts), "Read mail · Read calendar");
        let none = Agents { global: Vec::new(), ..Agents::demo() };
        assert_eq!(none.may("3f9a1c0b77de", &accounts), "No access");
    }

    #[test]
    fn last_use_reads_as_minutes_then_hours_then_dates() {
        let now = chrono::Local::now();
        let at = |ago: i64| used(now.timestamp() - ago, now);
        assert_eq!(at(-5), "Used just now");
        assert_eq!(at(150), "Used 2 min ago");
        assert_eq!(at(3 * 3600), "Used 3 h ago");
        assert!(at(30 * 86_400).starts_with("Used ") && !at(30 * 86_400).contains("ago"));
    }
}
