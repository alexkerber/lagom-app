//! Filesystem side of local agent setup. The policy store issues and revokes the token.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue, Value};
use std::{
    env,
    ffi::OsStr,
    fs,
    io::{ErrorKind, Write},
    os::unix::fs::{symlink, DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use toml_edit::{Array, DocumentMut, Item, Table};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Cursor,
    Hermes,
    OpenClaw,
    ClaudeDesktop,
}

impl Agent {
    pub const ALL: [Self; 6] = [
        Self::ClaudeCode,
        Self::Codex,
        Self::Cursor,
        Self::Hermes,
        Self::OpenClaw,
        Self::ClaudeDesktop,
    ];

    /// The store's `kind` for this agent.
    pub fn kind(self) -> String {
        format!("{self:?}")
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Hermes => "Hermes",
            Self::OpenClaw => "OpenClaw",
            Self::ClaudeDesktop => "Claude Desktop",
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Self::ClaudeCode => ".claude",
            Self::Codex => ".codex",
            Self::Cursor => ".cursor",
            Self::Hermes => ".hermes",
            Self::OpenClaw => ".openclaw",
            Self::ClaudeDesktop if cfg!(target_os = "macos") => {
                "Library/Application Support/Claude"
            }
            Self::ClaudeDesktop => ".config/Claude",
        }
    }

    fn commands(self) -> &'static [&'static str] {
        match self {
            Self::ClaudeCode => &["claude"],
            Self::Codex => &["codex"],
            Self::Cursor => &["cursor", "cursor-agent"],
            Self::Hermes => &["hermes"],
            Self::OpenClaw => &["openclaw"],
            // shortcut: path not verified against Claude Desktop docs; also check its config directory.
            Self::ClaudeDesktop => &["claude-desktop"],
        }
    }
}

fn on_path(home: &Path, path: &OsStr, command: &str) -> bool {
    env::split_paths(path).any(|dir| {
        fs::metadata(home.join(dir).join(command))
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

pub fn detect(home: &Path, path: &OsStr) -> Vec<Agent> {
    Agent::ALL
        .into_iter()
        .filter(|a| {
            home.join(a.dir()).is_dir() || a.commands().iter().any(|cmd| on_path(home, path, cmd))
        })
        .collect()
}

/// Preview these destinations before connecting. Keep the returned Connection in the app store.
#[derive(Serialize, Deserialize)]
pub struct Plan {
    pub token: PathBuf,
    pub skill: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub link: Option<PathBuf>,
    pub bin_dir_on_path: bool,
    home: PathBuf,
    binary: PathBuf,
    id: String,
    agent: Agent,
}

impl Plan {
    pub fn new(agent: Agent, id: &str, home: &Path, path: &OsStr, binary: &Path) -> Result<Self> {
        ensure!(
            id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid agent ID"
        );
        ensure!(
            home.is_absolute() && binary.is_absolute() && binary.to_str().is_some(),
            "expected absolute paths and a UTF-8 binary path"
        );
        let dir = home.join(agent.dir());
        let config = match agent {
            Agent::ClaudeCode => Some(home.join(".claude.json")),
            Agent::Codex => Some(dir.join("config.toml")),
            Agent::Cursor => Some(dir.join("mcp.json")),
            Agent::ClaudeDesktop => Some(dir.join("claude_desktop_config.json")),
            _ => None,
        };
        // shortcut: path not verified against Codex docs; use the plan’s ~/.codex/skills location.
        let skill = (!matches!(agent, Agent::Cursor | Agent::ClaudeDesktop))
            .then(|| dir.join("skills/lagom/SKILL.md"));
        let data = if cfg!(target_os = "macos") {
            "Library/Application Support"
        } else {
            ".local/share"
        };
        let bin_dir = home.join(".local/bin");
        let link = bin_dir.join("lagom");
        let installed =
            binary.starts_with("/Applications") || binary.starts_with(home.join("Applications"));
        let link = (cfg!(target_os = "macos")
            && installed
            && !on_path(home, path, "lagom")
            && fs::symlink_metadata(&link).is_err_and(|e| e.kind() == ErrorKind::NotFound))
        .then_some(link);
        Ok(Self {
            agent,
            home: home.into(),
            binary: binary.into(),
            id: id.into(),
            config,
            skill,
            link,
            bin_dir_on_path: env::split_paths(path).any(|dir| home.join(dir) == bin_dir),
            token: home
                .join(data)
                .join("lagom/agents")
                .join(format!("{id}.token")),
        })
    }

    // Serialize app operations without creating a lock file in the home directory.
    fn lock(&self) -> Result<fs::File> {
        let dir = fs::File::open(&self.home)?;
        dir.lock()?;
        Ok(dir)
    }

    /// The caller supplies the newly issued local credential and revokes it if installation fails.
    pub fn connect(self, token: &str) -> Result<Connection> {
        let _lock = self.lock()?;
        let mut connection = Connection {
            plan: self,
            dirs: vec![],
            config: None,
            skill_text: None,
            linked: false,
        };
        if let Err(err) = connection.install(token) {
            connection
                .cleanup()
                .context(format!("rolling back after {err}"))?;
            return Err(err);
        }
        Ok(connection)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Connection {
    plan: Plan,
    dirs: Vec<PathBuf>,
    config: Option<ConfigEdit>,
    skill_text: Option<String>,
    linked: bool,
}

#[derive(Serialize, Deserialize)]
struct ConfigEdit {
    new_file: bool,
    new_group: bool,
}

fn read(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(m) => ensure!(m.is_file(), "expected a regular file at {}", path.display()),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    Ok(Some(fs::read_to_string(path)?))
}

fn parents(dir: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        parents(dir.parent().context("missing parent")?, created)?;
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => created.push(dir.into()),
            Err(e) if e.kind() == ErrorKind::AlreadyExists && dir.is_dir() => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn write(path: &Path, contents: &str, previous: Option<&str>) -> Result<()> {
    let mut tmp = NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.write_all(contents.as_bytes())?;
    if let Some(previous) = previous {
        ensure!(
            read(path)?.as_deref() == Some(previous),
            "file changed: {}",
            path.display()
        );
        tmp.as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
        tmp.persist(path)?;
    } else {
        tmp.persist_noclobber(path)?;
    }
    Ok(())
}

impl Connection {
    fn entry(&self) -> Value {
        let mut entry = json!({"command":self.plan.binary,"args":["mcp","--agent",self.plan.id]});
        if self.plan.agent == Agent::ClaudeCode {
            entry["type"] = "stdio".into();
        }
        entry
    }

    fn install(&mut self, token: &str) -> Result<()> {
        let plan = &self.plan;
        if let Some(path) = &plan.config {
            let original = read(path)?;
            let (contents, edit) = add_config(plan.agent, original.as_deref(), &self.entry())?;
            parents(path.parent().unwrap(), &mut self.dirs)?;
            write(path, &contents, original.as_deref())?;
            self.config = Some(edit);
        }
        if let Some(path) = &plan.skill {
            let quoted = format!("'{}'", plan.binary.to_str().unwrap().replace('\'', "'\\''"));
            let text = include_str!("../../assets/skill/SKILL.md")
                .replace("{{agent_id}}", &plan.id)
                .replace("{{binary}}", &quoted);
            parents(path.parent().unwrap(), &mut self.dirs)?;
            write(path, &text, None)?;
            self.skill_text = Some(text);
        }
        if let Some(path) = &plan.link {
            parents(path.parent().unwrap(), &mut self.dirs)?;
            symlink(&plan.binary, path)?;
            self.linked = true;
        }
        parents(plan.token.parent().unwrap(), &mut self.dirs)?;
        write(&plan.token, token, None)
    }

    /// Rotate a connected agent's token after the store replaces its credential.
    pub fn reconnect(&self, token: &str) -> Result<()> {
        let _lock = self.plan.lock()?;
        let path = &self.plan.token;
        let mut tmp = NamedTempFile::new_in(path.parent().unwrap())?;
        tmp.write_all(token.as_bytes())?;
        tmp.persist(path)?;
        Ok(())
    }

    /// Revoke in the store first. Cleanup can be retried with the same receipt.
    pub fn disconnect(&self) -> Result<()> {
        let _lock = self.plan.lock()?;
        match fs::remove_file(&self.plan.token) {
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            result => result?,
        }
        self.cleanup()
    }

    fn cleanup(&self) -> Result<()> {
        let plan = &self.plan;
        if let (Some(path), Some(text)) = (&plan.skill, &self.skill_text) {
            if fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
                && fs::read(path)? == text.as_bytes()
            {
                fs::remove_file(path)?;
            }
        }
        if let Some(path) = &plan.link {
            if self.linked && fs::read_link(path).ok().as_ref() == Some(&plan.binary) {
                fs::remove_file(path)?;
            }
        }
        if let (Some(path), Some(edit)) = (&plan.config, &self.config) {
            if let Some(current) = read(path)? {
                if let Some((text, empty)) =
                    remove_config(plan.agent, &current, &self.entry(), edit)?
                {
                    if edit.new_file && empty {
                        fs::remove_file(path)?;
                    } else {
                        write(path, &text, Some(&current))?;
                    }
                }
            }
        }
        for dir in self.dirs.iter().rev() {
            match fs::remove_dir(dir) {
                Err(e)
                    if matches!(e.kind(), ErrorKind::DirectoryNotEmpty | ErrorKind::NotFound) =>
                {
                    ()
                }
                result => result?,
            }
        }
        Ok(())
    }
}

// Borrow the original JSON values so edits preserve unrelated bytes, including large numbers.
type Object<'a> = std::collections::BTreeMap<String, &'a RawValue>;

fn object(text: &str) -> Result<Object<'_>> {
    Ok(serde_json::from_str(text)?)
}

fn offset(text: &str, part: &str) -> usize {
    part.as_ptr() as usize - text.as_ptr() as usize
}

fn add_config(agent: Agent, original: Option<&str>, entry: &Value) -> Result<(String, ConfigEdit)> {
    let mut edit = ConfigEdit {
        new_file: original.is_none(),
        new_group: false,
    };
    if agent == Agent::Codex {
        let mut doc: DocumentMut = original.unwrap_or("").parse()?;
        edit.new_group = !doc.contains_key("mcp_servers");
        let servers = doc
            .entry("mcp_servers")
            .or_insert(Item::Table(Table::new()))
            .as_table_like_mut()
            .context("invalid mcp_servers")?;
        ensure!(
            !servers.contains_key("lagom"),
            "lagom MCP entry already exists"
        );
        let mut server = Table::new();
        server["command"] = toml_edit::value(entry["command"].as_str().unwrap());
        let args: Array = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        server["args"] = toml_edit::value(args);
        servers.insert("lagom", Item::Table(server));
        return Ok((doc.to_string(), edit));
    }
    let mut text = original.unwrap_or("{}").to_owned();
    let root = object(&text)?;
    let (group, mut field) = if let Some(servers) = root.get("mcpServers") {
        ensure!(
            !object(servers.get())?.contains_key("lagom"),
            "lagom MCP entry already exists"
        );
        (servers.get(), format!("\"lagom\":{entry}"))
    } else {
        edit.new_group = true;
        (text.trim(), format!("\"mcpServers\":{{\"lagom\":{entry}}}"))
    };
    if !object(group)?.is_empty() {
        field.push(',');
    }
    let pos = offset(&text, group) + 1;
    text.insert_str(pos, &field);
    Ok((text, edit))
}

fn remove_config(
    agent: Agent,
    current: &str,
    entry: &Value,
    edit: &ConfigEdit,
) -> Result<Option<(String, bool)>> {
    if agent == Agent::Codex {
        let mut doc: DocumentMut = current.parse()?;
        let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_like_mut) else {
            return Ok(None);
        };
        let Some(server) = servers.get("lagom").and_then(Item::as_table_like) else {
            return Ok(None);
        };
        let args = server.get("args").and_then(Item::as_array);
        if server.len() != 2
            || server.get("command").and_then(Item::as_str) != entry["command"].as_str()
            || !args.is_some_and(|a| {
                a.iter().map(|v| v.as_str()).eq(entry["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(Value::as_str))
            })
        {
            return Ok(None);
        }
        servers.remove("lagom");
        if edit.new_group && servers.is_empty() {
            doc.remove("mcp_servers");
        }
        let text = doc.to_string();
        let empty = text.trim().is_empty();
        return Ok(Some((text, empty)));
    }
    let root = object(current)?;
    let Some(group) = root.get("mcpServers") else {
        return Ok(None);
    };
    let group = group.get();
    let servers = object(group)?;
    let Some(server) = servers.get("lagom") else {
        return Ok(None);
    };
    if serde_json::from_str::<Value>(server.get())? != *entry {
        return Ok(None);
    }

    let remove_group = edit.new_group && servers.len() == 1;
    let (map, container, value) = if remove_group {
        (&root, current.trim(), group)
    } else {
        (&servers, group, server.get())
    };
    let value_start = offset(current, value);
    let mut end = value_start + value.len();
    // Take the preceding comma, or the following comma for the first member.
    let start = map
        .values()
        .map(|raw| offset(current, raw.get()) + raw.get().len())
        .filter(|&pos| pos <= value_start)
        .max();
    let start = if let Some(pos) = start {
        pos + current[pos..value_start]
            .find(',')
            .context("missing JSON separator")?
    } else {
        if map.len() > 1 {
            end += current[end..].find(',').context("missing JSON separator")? + 1;
        }
        offset(current, container) + 1
    };
    let mut text = current.to_owned();
    text.replace_range(start..end, "");
    Ok(Some((text, remove_group && root.len() == 1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut found = BTreeMap::new();
        for child in fs::read_dir(dir).unwrap() {
            let child = child.unwrap();
            if child.file_type().unwrap().is_dir() {
                found.extend(files(&child.path()));
            } else {
                found.insert(child.path(), fs::read(child.path()).unwrap());
            }
        }
        found
    }

    #[test]
    fn detect_connect_disconnect_in_temp_home() -> Result<()> {
        use Agent::*;
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        let bin = home.join("bin");
        fs::create_dir(&bin)?;
        let path = bin.as_os_str();
        assert!(detect(home, path).is_empty());
        assert_eq!(fs::read_dir(home)?.count(), 1);
        let agents = [ClaudeCode, Codex, Cursor, Hermes, OpenClaw, ClaudeDesktop];
        for agent in agents {
            let exe = bin.join(agent.commands().last().unwrap());
            fs::write(&exe, "agent")?;
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))?;
        }
        let binaries = files(home);
        assert_eq!(detect(home, path), agents);
        assert_eq!(files(home), binaries);
        let empty_path = home.join("empty-bin");
        for agent in agents {
            fs::create_dir_all(home.join(agent.dir()))?;
        }
        assert_eq!(detect(home, empty_path.as_os_str()), agents);
        let id = "abcdef123456";
        let binary = Path::new("/Applications/lagom.app/Contents/MacOS/lagom");
        fs::write(bin.join("lagom"), "foreign")?;
        fs::set_permissions(bin.join("lagom"), fs::Permissions::from_mode(0o755))?;
        let before = files(home);
        assert_eq!(detect(home, path), agents);
        assert_eq!(files(home), before);
        for agent in agents {
            let plan = || Plan::new(agent, id, home, path, binary);
            let preview = plan()?;
            assert!(preview.link.is_none());
            assert_eq!(
                preview.config.is_some(),
                matches!(agent, ClaudeCode | Codex | Cursor | ClaudeDesktop)
            );
            assert_eq!(
                preview.skill.is_some(),
                matches!(agent, ClaudeCode | Codex | Hermes | OpenClaw)
            );
            if let Some(file) = &preview.config {
                fs::create_dir_all(file.parent().unwrap())?;
                fs::write(
                    file,
                    if agent == Codex {
                        "# keep\n[mcp_servers.other]\ncommand = 'other'\n"
                    } else {
                        "{ \"keep\" : 42, \"mcpServers\": { \"other\": {} } }\n"
                    },
                )?;
            }
            let before = files(home);
            let c = plan()?.connect("first-secret")?;
            let written = files(home);
            let mut expected = before.clone();
            expected.insert(preview.token.clone(), b"first-secret".into());
            if let Some(file) = &preview.skill {
                let text = fs::read_to_string(file)?;
                assert!(text.contains(&format!("lagom --agent {id}")));
                assert!(text.contains(binary.to_str().unwrap()));
                expected.insert(file.clone(), text.into_bytes());
            }
            if let Some(file) = &preview.config {
                let text = fs::read_to_string(file)?;
                assert!(
                    text.contains(binary.to_str().unwrap())
                        && text.contains(id)
                        && text.contains("--agent")
                );
                if agent == Codex {
                    let doc: DocumentMut = text.parse()?;
                    let entry = &doc["mcp_servers"]["lagom"];
                    assert_eq!(entry["command"].as_str(), binary.to_str());
                    assert!(entry["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|arg| arg.as_str().unwrap())
                        .eq(["mcp", "--agent", id]));
                } else {
                    let doc: Value = serde_json::from_str(&text)?;
                    assert_eq!(doc["mcpServers"]["lagom"]["command"], json!(binary));
                    assert_eq!(
                        doc["mcpServers"]["lagom"]["args"],
                        json!(["mcp", "--agent", id])
                    );
                }
                assert!(!text.contains("secret"));
                expected.insert(file.clone(), text.into_bytes());
            }
            assert_eq!(written, expected);
            c.reconnect("second-secret")?;
            assert_eq!(fs::read_to_string(&preview.token)?, "second-secret");
            assert_eq!(
                fs::metadata(&preview.token)?.permissions().mode() & 0o777,
                0o600
            );
            let receipt = serde_json::to_string(&c)?;
            assert!(!receipt.contains("secret"));
            serde_json::from_str::<Connection>(&receipt)?.disconnect()?;
            assert_eq!(files(home), before);
            if let Some(skill) = preview.skill {
                let c = plan()?.connect("secret")?;
                fs::write(&skill, "user edit")?;
                c.disconnect()?;
                assert_eq!(fs::read_to_string(&skill)?, "user edit");
            }
        }
        let plan = Plan::new(Cursor, id, home, path, binary)?;
        let config = plan.config.clone().unwrap();
        fs::remove_file(&config)?;
        let c = plan.connect("secret")?;
        let current =
            fs::read_to_string(&config)?.replace("{\"lagom\":", "{\"keep\":42,\"lagom\":");
        fs::write(&config, current)?;
        c.disconnect()?;
        assert_eq!(
            fs::read_to_string(&config)?,
            r#"{"mcpServers":{"keep":42}}"#
        );
        let plan = Plan::new(Cursor, id, home, path, binary)?;
        fs::write(&config, "broken")?;
        let before = files(home);
        assert!(plan.connect("secret").is_err());
        assert_eq!(files(home), before);
        if cfg!(target_os = "macos") {
            let plan = Plan::new(Cursor, id, home, empty_path.as_os_str(), binary)?;
            let link = plan.link.clone().unwrap();
            fs::remove_file(&config)?;
            let c = plan.connect("secret")?;
            assert_eq!(fs::read_link(&link)?, binary);
            c.disconnect()?;
            assert!(!config.exists() && fs::symlink_metadata(link).is_err());
        }
        Ok(())
    }
}
