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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub agent: Agent,
    cli: Option<PathBuf>,
    config_dir: Option<PathBuf>,
}

fn executable(home: &Path, path: &OsStr, command: &str) -> Option<PathBuf> {
    env::split_paths(path)
        .map(|dir| home.join(dir).join(command))
        .find(|file| {
            fs::metadata(file).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

impl Agent {
    fn config_env(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode => Some("CLAUDE_CONFIG_DIR"),
            Self::Codex => Some("CODEX_HOME"),
            _ => None,
        }
    }

    fn probe(self, home: &Path, path: &OsStr, config_dir: Option<PathBuf>) -> Option<Detected> {
        let cli = self
            .commands()
            .iter()
            .find_map(|cmd| executable(home, path, cmd));
        let dir = config_dir.clone().unwrap_or_else(|| home.join(self.dir()));
        (dir.is_dir() || cli.is_some()).then_some(Detected {
            agent: self,
            cli,
            config_dir,
        })
    }
}

pub fn detect(home: &Path, path: &OsStr) -> Vec<Detected> {
    use Agent::*;
    [ClaudeCode, Codex, Cursor, Hermes, OpenClaw, ClaudeDesktop]
        .into_iter()
        .filter_map(|a| {
            a.probe(
                home,
                path,
                a.config_env().and_then(env::var_os).map(PathBuf::from),
            )
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
    #[serde(default)]
    pub link_receipt: Option<PathBuf>,
    pub bin_dir_on_path: bool,
    #[serde(default)]
    cli: Option<PathBuf>,
    #[serde(default)]
    config_dir: Option<PathBuf>,
    home: PathBuf,
    binary: PathBuf,
    id: String,
    agent: Agent,
}

impl Plan {
    pub fn new(
        client: Detected,
        id: &str,
        home: &Path,
        path: &OsStr,
        binary: &Path,
        previous: Option<&Connection>,
    ) -> Result<Self> {
        let mut plan = Self::in_dir(
            client,
            id,
            home,
            path,
            binary,
            &lagom_core::store::Store::data_dir()?,
        )?;
        // Older receipts kept link ownership on the individual connection.
        if let Some(old) = previous.filter(|c| {
            c.plan.agent == plan.agent
                && c.plan.home == home
                && c.linked
                && c.plan.link_receipt.is_none()
        }) {
            let link = home.join(".local/bin/lagom");
            if installed(home, binary)
                && old.plan.link.as_ref() == Some(&link)
                && fs::read_link(&link).ok().as_ref() == Some(&old.plan.binary)
            {
                plan.link = Some(link);
            }
        }
        Ok(plan)
    }

    fn in_dir(
        client: Detected,
        id: &str,
        home: &Path,
        path: &OsStr,
        binary: &Path,
        data: &Path,
    ) -> Result<Self> {
        ensure!(valid_id(id), "invalid agent ID");
        ensure!(
            home.is_absolute() && binary.is_absolute() && binary.to_str().is_some(),
            "expected absolute paths and a UTF-8 binary path"
        );
        let agent = client.agent;
        let dir = client
            .config_dir
            .clone()
            .unwrap_or_else(|| home.join(agent.dir()));
        let config = match agent {
            Agent::ClaudeCode => Some(
                client
                    .config_dir
                    .as_deref()
                    .unwrap_or(home)
                    .join(".claude.json"),
            ),
            Agent::Codex => Some(dir.join("config.toml")),
            Agent::Cursor => Some(dir.join("mcp.json")),
            Agent::ClaudeDesktop => Some(dir.join("claude_desktop_config.json")),
            _ => None,
        };
        let dir = if agent == Agent::Codex {
            home.join(".agents")
        } else {
            dir
        };
        let skill = (!matches!(agent, Agent::Cursor | Agent::ClaudeDesktop))
            .then(|| dir.join("skills/lagom/SKILL.md"));
        let bin_dir = home.join(".local/bin");
        let link = bin_dir.join("lagom");
        let receipt = data.join("agents/link-target");
        let recorded = read(&receipt)?;
        let link = (cfg!(target_os = "macos")
            && installed(home, binary)
            && (fs::read_link(&link).is_ok_and(|target| {
                Some(target.as_path()) == recorded.as_deref().map(Path::new)
                    && (target == binary || !target.exists())
            }) || (executable(home, path, "lagom").is_none()
                && fs::symlink_metadata(&link).is_err_and(|e| e.kind() == ErrorKind::NotFound))))
        .then_some(link);
        Ok(Self {
            agent,
            cli: client.cli,
            config_dir: client.config_dir,
            home: home.into(),
            binary: binary.into(),
            id: id.into(),
            config,
            skill,
            link_receipt: cfg!(target_os = "macos").then_some(receipt),
            link,
            bin_dir_on_path: env::split_paths(path).any(|dir| home.join(dir) == bin_dir),
            token: data.join("agents").join(format!("{id}.token")),
        })
    }

    // Serialize app operations without creating a lock file in the home directory.
    fn lock(&self) -> Result<fs::File> {
        let dir = fs::File::open(&self.home)?;
        dir.lock()?;
        Ok(dir)
    }

    /// The caller supplies the newly issued local credential and revokes it if installation fails.
    pub fn connect(self, token: &str, previous: Option<&Connection>) -> Result<Connection> {
        self.connect_with(token, previous, &run_cli)
    }

    fn connect_with(
        self,
        token: &str,
        previous: Option<&Connection>,
        runner: &Runner,
    ) -> Result<Connection> {
        let _lock = self.lock()?;
        if self.config.is_some() {
            ensure!(
                installed(&self.home, &self.binary) || debug_binary(&self.binary),
                "Move lagom to Applications first"
            );
        }
        let previous = previous.filter(|c| c.plan.agent == self.agent && c.plan.home == self.home);
        let mut connection = Connection {
            plan: self,
            dirs: vec![],
            config: None,
            skill_text: None,
            linked: false,
        };
        connection.install(token, previous, runner)?;
        Ok(connection)
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn installed(home: &Path, binary: &Path) -> bool {
    let Ok(real) = binary.canonicalize() else {
        return false;
    };
    binary.is_file()
        && (binary.starts_with("/Applications") || binary.starts_with(home.join("Applications")))
        && (real.starts_with("/Applications")
            || home
                .canonicalize()
                .is_ok_and(|p| real.starts_with(p.join("Applications"))))
}

fn debug_binary(binary: &Path) -> bool {
    // OUT_DIR is under the build profile, including custom Cargo target directories.
    cfg!(debug_assertions)
        && Path::new(env!("OUT_DIR"))
            .ancestors()
            .nth(4)
            .is_some_and(|target| {
                binary.is_file()
                    && binary.canonicalize().is_ok_and(|p| p.starts_with(target))
                    && !binary
                        .components()
                        .any(|c| c == std::path::Component::ParentDir)
            })
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
    #[serde(default)]
    entry: Option<Value>,
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
    atomic_write(path, contents, previous, None)
}

fn atomic_write(
    path: &Path,
    contents: &str,
    previous: Option<&str>,
    mode: Option<u32>,
) -> Result<()> {
    let mut tmp = NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.write_all(contents.as_bytes())?;
    if let Some(previous) = previous {
        ensure!(
            read(path)?.as_deref() == Some(previous),
            "file changed: {}",
            path.display()
        );
        let permissions = match mode {
            Some(mode) => fs::Permissions::from_mode(mode),
            None => fs::metadata(path)?.permissions(),
        };
        tmp.as_file().set_permissions(permissions)?;
        tmp.as_file().sync_all()?;
        tmp.persist(path)?;
    } else {
        tmp.as_file().sync_all()?;
        tmp.persist_noclobber(path)?;
    }
    if let Ok(dir) = fs::File::open(path.parent().unwrap()) {
        let _ = dir.sync_all();
    }
    Ok(())
}

type Runner = dyn Fn(&Plan, &Path, &[&str]) -> Result<()>;

fn run_cli(plan: &Plan, exe: &Path, args: &[&str]) -> Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).env("HOME", &plan.home);
    let key = plan.agent.config_env().unwrap();
    if let Some(dir) = &plan.config_dir {
        cmd.env(key, dir);
    } else {
        cmd.env_remove(key);
    }
    let result = cmd
        .output()
        .with_context(|| format!("running {}", exe.display()))?;
    ensure!(
        result.status.success(),
        "{} mcp {} failed ({}): {}",
        exe.display(),
        args[1],
        result.status,
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(())
}

impl Plan {
    fn entry(&self, id: &str) -> Value {
        let mut entry = json!({"command":self.binary,"args":["mcp","--agent",id]});
        if matches!(self.agent, Agent::ClaudeCode | Agent::Cursor) {
            entry["type"] = "stdio".into();
        }
        if self.agent == Agent::ClaudeCode {
            entry["env"] = json!({});
        }
        entry
    }

    fn owned_id<'a>(&self, entry: &'a Value) -> Option<&'a str> {
        let id = entry["args"].get(2)?.as_str()?;
        let mut expected = self.entry(id);
        // Claude adds an empty environment. Earlier Cursor registrations omitted type.
        if self.agent == Agent::ClaudeCode && entry.get("env").is_none() {
            expected.as_object_mut()?.remove("env");
        }
        if self.agent == Agent::Cursor && entry.get("type").is_none() {
            expected.as_object_mut()?.remove("type");
        }
        (valid_id(id) && expected == *entry).then_some(id)
    }

    fn command(&self, runner: &Runner, add: Option<&Value>) -> Result<()> {
        let exe = self
            .cli
            .as_deref()
            .filter(|p| p.is_absolute())
            .context("agent CLI missing; detect the agent again")?;
        let mut args = vec!["mcp", if add.is_some() { "add" } else { "remove" }];
        if self.agent == Agent::ClaudeCode {
            args.extend(["--scope", "user"]);
        }
        args.push("lagom");
        if let Some(entry) = add {
            args.extend([
                "--",
                entry["command"].as_str().unwrap(),
                "mcp",
                "--agent",
                entry["args"][2].as_str().unwrap(),
            ]);
        }
        runner(self, exe, &args)
    }

    fn skill(&self) -> String {
        let quoted = format!("'{}'", self.binary.to_str().unwrap().replace('\'', "'\\''"));
        include_str!("../../assets/skill/SKILL.md")
            .replace("{{agent_id}}", &self.id)
            .replace("{{binary}}", &quoted)
    }
}

fn preserve_endings(path: &Path, original: Option<&str>) -> Result<()> {
    let Some(original) = original else {
        return Ok(());
    };
    let Some(current) = read(path)? else {
        return Ok(());
    };
    let mut text = current.replace("\r\n", "\n");
    if original.contains("\r\n") {
        text = text.replace('\n', "\r\n");
    }
    if !original.ends_with('\n') {
        text.truncate(text.trim_end_matches(['\r', '\n']).len());
    } else if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(if original.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        });
    }
    if text != current {
        write(path, &text, Some(&current))?;
    }
    Ok(())
}

// Undo only completed writes, and only while their contents still match.
enum Undo {
    File(PathBuf, Option<String>, String),
    Link(PathBuf, Option<PathBuf>, Option<PathBuf>),
    Registration {
        before: Option<Value>,
        after: Option<Value>,
        original: Option<String>,
    },
}

impl Undo {
    fn write(
        log: &mut Vec<Self>,
        path: &Path,
        before: Option<String>,
        after: String,
    ) -> Result<()> {
        write(path, &after, before.as_deref())?;
        log.push(Self::File(path.into(), before, after));
        Ok(())
    }

    fn revert(self, plan: &Plan, runner: &Runner) -> Result<()> {
        match self {
            Self::File(path, before, after) => {
                if let Some(before) = before {
                    write(&path, &before, Some(&after))?;
                } else {
                    ensure!(
                        read(&path)?.as_ref() == Some(&after),
                        "file changed: {}",
                        path.display()
                    );
                    fs::remove_file(path)?;
                }
            }
            Self::Link(path, before, after) => {
                if let Some(after) = after {
                    ensure!(
                        fs::read_link(&path)? == after,
                        "link changed: {}",
                        path.display()
                    );
                    fs::remove_file(&path)?;
                }
                if let Some(before) = before {
                    symlink(before, path)?;
                }
            }
            Self::Registration {
                before,
                after,
                original,
            } => {
                let path = plan.config.as_ref().unwrap();
                ensure!(
                    existing(plan.agent, read(path)?.as_deref())? == after,
                    "registration changed: {}",
                    path.display()
                );
                plan.command(runner, before.as_ref())?;
                preserve_endings(path, original.as_deref())?;
            }
        }
        Ok(())
    }
}

impl Connection {
    fn install_config(
        &mut self,
        previous: Option<&Connection>,
        runner: &Runner,
        undo: &mut Vec<Undo>,
    ) -> Result<()> {
        let plan = &self.plan;
        let Some(path) = &plan.config else {
            return Ok(());
        };
        let original = read(path)?;
        let current = existing(plan.agent, original.as_deref())?;
        if let Some(entry) = &current {
            ensure!(
                plan.owned_id(entry).is_some()
                    || previous
                        .is_some_and(|c| c.plan.config == plan.config && c.matches_entry(entry)),
                "lagom already configured at {}",
                path.display()
            );
        }
        let desired = plan.entry(&plan.id);
        let mut edit = ConfigEdit {
            entry: None,
            new_file: original.is_none(),
            new_group: false,
        };
        if plan.agent.config_env().is_some() {
            ensure!(
                plan.cli.as_ref().is_some_and(|p| p.is_absolute()
                    && fs::metadata(p)
                        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)),
                "agent CLI missing; detect the agent again"
            );
            if current.as_ref() != Some(&desired) {
                if current.is_some() {
                    plan.command(runner, None)?;
                    undo.push(Undo::Registration {
                        before: current,
                        after: None,
                        original: original.clone(),
                    });
                }
                plan.command(runner, Some(&desired))?;
                undo.push(Undo::Registration {
                    before: None,
                    after: Some(desired.clone()),
                    original: original.clone(),
                });
                ensure!(
                    existing(plan.agent, read(path)?.as_deref())?.as_ref() == Some(&desired),
                    "registration missing or changed at {}",
                    path.display()
                );
                preserve_endings(path, original.as_deref())?;
            }
        } else if current.as_ref() != Some(&desired) {
            let (text, metadata) = put_json(original.as_deref(), &desired)?;
            edit = metadata;
            parents(path.parent().unwrap(), &mut self.dirs)?;
            Undo::write(undo, path, original, text)?;
        }
        edit.entry = Some(desired);
        self.config = Some(edit);
        Ok(())
    }

    fn install(
        &mut self,
        token: &str,
        previous: Option<&Connection>,
        runner: &Runner,
    ) -> Result<()> {
        let skill = self.plan.skill.as_ref().map(|_| self.plan.skill());
        let old_skill = self.plan.skill.as_deref().map(read).transpose()?.flatten();
        ensure!(
            old_skill.is_none()
                || old_skill == skill
                || previous
                    .is_some_and(|c| c.plan.skill == self.plan.skill && c.skill_text == old_skill),
            "skill already configured at {}",
            self.plan.skill.as_ref().unwrap().display()
        );
        let old_token = read(&self.plan.token)?;
        let mut undo = vec![];
        let result = (|| -> Result<()> {
            self.install_config(previous, runner, &mut undo)?;
            let plan = &self.plan;
            if let (Some(path), Some(text)) = (&plan.skill, &skill) {
                if old_skill != skill {
                    parents(path.parent().unwrap(), &mut self.dirs)?;
                    Undo::write(&mut undo, path, old_skill, text.clone())?;
                }
            }
            parents(plan.token.parent().unwrap(), &mut self.dirs)?;
            if let Some(link) = &plan.link {
                let receipt = plan
                    .link_receipt
                    .as_ref()
                    .context("missing link receipt path")?;
                let recorded = read(receipt)?;
                let target = fs::read_link(link).ok();
                if let Some(old) = &target {
                    ensure!(
                        Some(old.as_path()) == recorded.as_deref().map(Path::new)
                            || previous.is_some_and(|c| c.linked
                                && c.plan.link.as_ref() == Some(link)
                                && c.plan.binary == *old),
                        "link already configured at {}",
                        link.display()
                    );
                }
                if target.as_ref() != Some(&plan.binary) {
                    parents(link.parent().unwrap(), &mut self.dirs)?;
                    if let Some(old) = target {
                        ensure!(
                            !old.exists()
                                || previous.is_some_and(|c| c.linked && c.plan.binary == old),
                            "link already configured at {}",
                            link.display()
                        );
                        fs::remove_file(link)?;
                        undo.push(Undo::Link(link.clone(), Some(old), None));
                    }
                    symlink(&plan.binary, link)?;
                    undo.push(Undo::Link(link.clone(), None, Some(plan.binary.clone())));
                }
                if recorded.as_deref() != plan.binary.to_str() {
                    Undo::write(
                        &mut undo,
                        receipt,
                        recorded,
                        plan.binary.to_str().unwrap().into(),
                    )?;
                }
            }
            atomic_write(&plan.token, token, old_token.as_deref(), Some(0o600))
        })();
        if let Err(err) = result {
            let mut failures: Vec<_> = undo
                .into_iter()
                .rev()
                .filter_map(|u| u.revert(&self.plan, runner).err())
                .collect();
            failures.extend(self.remove_dirs().err());
            if !failures.is_empty() {
                anyhow::bail!(
                    "{err:#}; rollback failed: {}",
                    failures
                        .iter()
                        .map(|e| format!("{e:#}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            return Err(err);
        }
        let plan = &self.plan;
        if let Some(previous) = previous {
            self.dirs.extend_from_slice(&previous.dirs);
            self.dirs.sort();
            self.dirs.dedup();
            if let (Some(edit), Some(old)) = (&mut self.config, &previous.config) {
                if previous.plan.config == plan.config {
                    edit.new_file |= old.new_file;
                    edit.new_group |= old.new_group;
                }
            }
        }
        self.skill_text = skill;
        self.linked = plan.link.is_some();
        Ok(())
    }

    pub fn reconnect(&self, token: &str) -> Result<()> {
        let _lock = self.plan.lock()?;
        let path = &self.plan.token;
        atomic_write(path, token, read(path)?.as_deref(), Some(0o600))
    }

    fn remove_skill(&self) -> Result<()> {
        if let (Some(path), Some(text)) = (&self.plan.skill, &self.skill_text) {
            if let Some(current) = read(path)? {
                ensure!(current == *text, "skill was edited");
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    fn matches_entry(&self, entry: &Value) -> bool {
        if let Some(written) = self.config.as_ref().and_then(|edit| edit.entry.as_ref()) {
            return entry == written;
        }
        let mut legacy = self.plan.entry(&self.plan.id);
        if self.plan.agent == Agent::ClaudeCode {
            legacy.as_object_mut().unwrap().remove("env");
        }
        if self.plan.agent == Agent::Cursor {
            legacy.as_object_mut().unwrap().remove("type");
        }
        entry == &legacy
    }

    fn remove_config(&self, runner: &Runner) -> Result<()> {
        let plan = &self.plan;
        let (Some(path), Some(edit)) = (&plan.config, &self.config) else {
            return Ok(());
        };
        let Some(current) = read(path)? else {
            return Ok(());
        };
        let Some(entry) = existing(plan.agent, Some(&current))? else {
            return Ok(());
        };
        ensure!(self.matches_entry(&entry), "MCP entry was edited");
        if plan.agent.config_env().is_some() {
            plan.command(runner, None)?;
            preserve_endings(path, Some(&current))?;
        } else {
            let (text, empty) = remove_json(&current, edit)?;
            if edit.new_file && empty {
                fs::remove_file(path)?;
            } else {
                write(path, &text, Some(&current))?;
            }
        }
        Ok(())
    }

    fn remove_dirs(&self) -> Result<()> {
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

    /// Revoke in the store first. Return paths that the person may need to clean up.
    pub fn disconnect(&self) -> Result<Vec<String>> {
        self.disconnect_with(&run_cli)
    }

    fn disconnect_with(&self, runner: &Runner) -> Result<Vec<String>> {
        let _lock = self.plan.lock()?;
        let plan = &self.plan;
        match fs::remove_file(&plan.token) {
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            result => result?,
        }
        let mut left = vec![];
        for (path, result) in [
            (&plan.config, self.remove_config(runner)),
            (&plan.skill, self.remove_skill()),
        ] {
            if let Err(err) = result {
                left.push(format!("{}: {err:#}", path.as_ref().unwrap().display()));
            }
        }
        if !other_tokens(&plan.token)? {
            let recorded = plan
                .link_receipt
                .as_deref()
                .map(read)
                .transpose()?
                .flatten();
            let target = recorded.as_deref().map(Path::new).or_else(|| {
                (plan.link_receipt.is_none() && self.linked).then_some(plan.binary.as_path())
            });
            if let Some(target) = target {
                let path = plan.home.join(".local/bin/lagom");
                match fs::read_link(&path) {
                    Ok(current) if current == target => fs::remove_file(&path)?,
                    Err(e) if e.kind() == ErrorKind::NotFound => (),
                    _ => left.push(format!("{}: link was changed", path.display())),
                }
                if let Some(receipt) = &plan.link_receipt {
                    fs::remove_file(receipt)?;
                }
            }
        }
        self.remove_dirs()?;
        Ok(left)
    }
}

fn other_tokens(token: &Path) -> Result<bool> {
    let entries = match fs::read_dir(token.parent().unwrap()) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        result => result?,
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension() == Some(OsStr::new("token"))
            && path
                .file_stem()
                .and_then(OsStr::to_str)
                .is_some_and(valid_id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
// Borrow the original JSON values so edits preserve unrelated bytes, including large numbers.
type Object<'a> = std::collections::BTreeMap<String, &'a RawValue>;

fn object(text: &str) -> Result<Object<'_>> {
    Ok(serde_json::from_str(text)?)
}

fn offset(text: &str, part: &str) -> usize {
    part.as_ptr() as usize - text.as_ptr() as usize
}

fn existing(agent: Agent, original: Option<&str>) -> Result<Option<Value>> {
    let Some(text) = original else {
        return Ok(None);
    };
    if agent == Agent::Codex {
        let doc: toml_edit::DocumentMut = text.parse()?;
        let Some(servers) = doc.get("mcp_servers") else {
            return Ok(None);
        };
        let servers = servers.as_table_like().context("invalid mcp_servers")?;
        return servers
            .get("lagom")
            .cloned()
            .map(|item| {
                let table = item
                    .into_table()
                    .map_err(|_| anyhow::anyhow!("lagom already configured"))?;
                toml_edit::de::from_document(toml_edit::DocumentMut::from(table))
                    .map_err(Into::into)
            })
            .transpose();
    }
    let root = object(text)?;
    let Some(group) = root.get("mcpServers") else {
        return Ok(None);
    };
    object(group.get())?
        .get("lagom")
        .map(|value| Ok(serde_json::from_str(value.get())?))
        .transpose()
}

fn put_json(original: Option<&str>, entry: &Value) -> Result<(String, ConfigEdit)> {
    let mut edit = ConfigEdit {
        entry: None,
        new_file: original.is_none(),
        new_group: false,
    };
    let mut text = original.unwrap_or("{}").to_owned();
    let root = object(&text)?;
    let (group, mut field) = if let Some(servers) = root.get("mcpServers") {
        if let Some(old) = object(servers.get())?.get("lagom") {
            let pos = offset(&text, old.get());
            text.replace_range(pos..pos + old.get().len(), &entry.to_string());
            return Ok((text, edit));
        }
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

fn remove_json(current: &str, edit: &ConfigEdit) -> Result<(String, bool)> {
    let root = object(current)?;
    let group = root["mcpServers"].get();
    let servers = object(group)?;
    let server = servers["lagom"];
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
    Ok((text, remove_group && root.len() == 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Agent::*;

    const ID: &str = "abcdef123456";
    const BIN: &str = "Applications/lagom.app/Contents/MacOS/lagom";

    fn plan(home: &Path, agent: Agent, id: &str) -> Result<Plan> {
        let binary = home.join(BIN);
        fs::create_dir_all(binary.parent().unwrap())?;
        fs::write(&binary, "app")?;
        let dir = home.join(agent.dir());
        fs::create_dir_all(dir)?;
        let path = home.join("bin");
        let client = agent.probe(home, path.as_os_str(), None).unwrap();
        Plan::in_dir(
            client,
            id,
            home,
            path.as_os_str(),
            &home.join(BIN),
            &home.join("custom-data"),
        )
    }

    fn executable_fixture(home: &Path, name: &str) -> Result<PathBuf> {
        let dir = home.join("bin");
        fs::create_dir_all(&dir)?;
        let file = dir.join(name);
        fs::write(&file, "fixture")?;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755))?;
        Ok(file)
    }

    fn fake(plan: &Plan, exe: &Path, args: &[&str]) -> Result<()> {
        let claude = plan.agent == ClaudeCode;
        assert_eq!(
            exe,
            plan.home
                .join("bin")
                .join(if claude { "claude" } else { "codex" })
        );
        let mut expected = vec!["mcp", args[1]];
        if claude {
            expected.extend(["--scope", "user"]);
        }
        expected.push("lagom");
        let add = args[1] == "add";
        if add {
            expected.extend([
                "--",
                plan.binary.to_str().unwrap(),
                "mcp",
                "--agent",
                args.last().unwrap(),
            ]);
        }
        assert_eq!(args, expected);
        let path = plan.config.as_ref().unwrap();
        let text = read(path)?.unwrap_or_default();
        let text = if claude {
            let mut doc: Value = serde_json::from_str(if text.is_empty() { "{}" } else { &text })?;
            if add {
                let mut entry = plan.entry(args.last().unwrap());
                entry["env"] = json!({});
                doc["mcpServers"]["lagom"] = entry;
            } else {
                doc["mcpServers"].as_object_mut().unwrap().remove("lagom");
            }
            serde_json::to_string_pretty(&doc)?
        } else {
            let mut doc: toml_edit::DocumentMut = text.parse()?;
            if add {
                let entry = &mut doc["mcp_servers"]["lagom"];
                entry["command"] = toml_edit::value(plan.binary.to_str().unwrap());
                entry["args"] = toml_edit::value(
                    ["mcp", "--agent", args.last().unwrap()]
                        .into_iter()
                        .collect::<toml_edit::Array>(),
                );
            } else {
                doc["mcp_servers"]
                    .as_table_like_mut()
                    .unwrap()
                    .remove("lagom");
            }
            doc.to_string()
        };
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, format!("{}\n", text.trim_end()))?;
        Ok(())
    }

    #[test]
    fn detection_only_reads_and_keeps_cli_and_config_paths() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        let path = home.join("bin");
        for agent in [ClaudeCode, Codex, Cursor, Hermes, OpenClaw, ClaudeDesktop] {
            assert!(agent.probe(home, path.as_os_str(), None).is_none());
            let custom = home.join("profiles").join(agent.dir());
            fs::create_dir_all(&custom)?;
            let client = agent
                .probe(home, path.as_os_str(), Some(custom.clone()))
                .unwrap();
            assert!(client.cli.is_none());
            let exe = executable_fixture(home, agent.commands().last().unwrap())?;
            let client = agent
                .probe(home, path.as_os_str(), Some(custom.clone()))
                .unwrap();
            assert_eq!(client.cli.as_ref(), Some(&exe));
            let preview = Plan::in_dir(
                client,
                ID,
                home,
                OsStr::new(""),
                &home.join(BIN),
                &home.join("store"),
            )?;
            assert_eq!(
                preview.token,
                home.join("store/agents").join(format!("{ID}.token"))
            );
            if agent == Codex {
                assert_eq!(preview.config.unwrap(), custom.join("config.toml"));
                assert_eq!(
                    preview.skill.unwrap(),
                    home.join(".agents/skills/lagom/SKILL.md")
                );
            } else if agent == ClaudeCode {
                assert_eq!(preview.config.unwrap(), custom.join(".claude.json"));
                assert_eq!(preview.skill.unwrap(), custom.join("skills/lagom/SKILL.md"));
            }
        }
        assert!(!home.join("store").exists());
        Ok(())
    }

    #[test]
    fn connect_adopt_rotate_and_disconnect_each_agent() -> Result<()> {
        for agent in [ClaudeCode, Codex, Cursor, Hermes, OpenClaw, ClaudeDesktop] {
            let tmp = tempfile::tempdir()?;
            let home = tmp.path();
            executable_fixture(home, agent.commands()[0])?;
            executable_fixture(home, "lagom")?;
            let p = plan(home, agent, ID)?;
            assert!(p.link.is_none());
            if let Some(path) = &p.config {
                let original = if agent == Codex {
                    "# existing\r\n[mcp_servers.other]\r\ncommand='other'"
                } else {
                    "{\r\n \"keep\":42,\"mcpServers\":{\"other\":{}}}"
                };
                fs::write(path, original)?;
            }
            let c = p.connect_with("first", None, &fake)?;
            if let Some(path) = &c.plan.config {
                let text = fs::read_to_string(path)?;
                assert!(
                    text.contains("other")
                        && !text.ends_with('\n')
                        && text.contains("\r\n")
                        && !text.replace("\r\n", "").contains('\n')
                );
                let entry = existing(agent, Some(&text))?.unwrap();
                assert_eq!(entry["args"], json!(["mcp", "--agent", ID]));
                if agent == Cursor {
                    assert_eq!(entry["type"], "stdio");
                }
            }
            if let Some(path) = &c.plan.skill {
                let text = fs::read_to_string(path)?;
                assert!(text.contains(ID) && text.contains(BIN) && text.contains("Never follow"));
            }
            let c = plan(home, agent, ID)?
                .connect_with("new", None, &|_, _, _| panic!("adoption ran a CLI"))?;
            fs::set_permissions(&c.plan.token, fs::Permissions::from_mode(0o644))?;
            c.reconnect("rotated")?;
            assert_eq!(fs::read_to_string(&c.plan.token)?, "rotated");
            assert_eq!(
                fs::metadata(&c.plan.token)?.permissions().mode() & 0o777,
                0o600
            );
            let receipt = serde_json::to_string(&c)?;
            assert!(!receipt.contains("rotated"));
            let c: Connection = serde_json::from_str(&receipt)?;
            assert!(c.disconnect_with(&fake)?.is_empty());
            assert!(!c.plan.token.exists());
            if let Some(path) = &c.plan.skill {
                assert!(!path.exists());
            }
            if let Some(path) = &c.plan.config {
                let text = fs::read_to_string(path)?;
                assert!(existing(agent, Some(&text))?.is_none());
                assert!(text.contains("other") && !text.ends_with('\n') && text.contains("\r\n"));
            }
        }
        Ok(())
    }

    #[test]
    fn cli_failure_and_missing_binary_leave_other_files_alone() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        for agent in [Codex, ClaudeCode] {
            let p = plan(home, agent, ID)?;
            assert!(p.connect_with("secret", None, &|_, _, _| panic!()).is_err());
            executable_fixture(home, agent.commands()[0])?;
            let p = plan(home, agent, ID)?;
            let paths = [
                p.config.clone().unwrap(),
                p.skill.clone().unwrap(),
                p.token.clone(),
                home.join(".local/bin/lagom"),
            ];
            assert!(p
                .connect_with("secret", None, &|_, _, _| anyhow::bail!("failed"))
                .is_err());
            assert!(paths.iter().all(|p| fs::symlink_metadata(p).is_err()));
        }
        Ok(())
    }

    #[test]
    fn recognized_entries_are_replaced_and_edited_entries_survive() -> Result<()> {
        for agent in [ClaudeCode, Codex, Cursor, ClaudeDesktop] {
            let tmp = tempfile::tempdir()?;
            let home = tmp.path();
            executable_fixture(home, agent.commands()[0])?;
            let old = plan(home, agent, "000000000001")?.connect_with("old", None, &fake)?;

            let receipt = old.plan.skill.is_some().then_some(&old);
            let c = plan(home, agent, ID)?.connect_with("secret", receipt, &fake)?;
            let path = c.plan.config.as_ref().unwrap();
            let text = fs::read_to_string(path)?
                .replace(c.plan.binary.to_str().unwrap(), "/elsewhere/user-command");
            fs::write(path, &text)?;
            let err = plan(home, agent, ID)?
                .connect_with("secret", None, &fake)
                .err()
                .unwrap();
            assert!(err.to_string().contains("already configured"));
            if let Some(skill) = &c.plan.skill {
                fs::write(skill, "user edit")?;
            }
            let left = c.disconnect_with(&fake)?;
            assert_eq!(left.len(), 1 + usize::from(c.plan.skill.is_some()));
            assert_eq!(fs::read_to_string(path)?, text);
            assert!(!c.plan.token.exists());
        }
        Ok(())
    }

    #[test]
    fn skill_conflict_prevents_registration_and_late_failure_restores_json() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        executable_fixture(home, "codex")?;
        let p = plan(home, Codex, ID)?;
        let skill = p.skill.clone().unwrap();
        fs::create_dir_all(skill.parent().unwrap())?;
        fs::write(&skill, "user skill")?;
        let err = p
            .connect_with("secret", None, &|_, _, _| panic!("skill conflict ran CLI"))
            .err()
            .unwrap();
        assert!(err.to_string().contains("already configured"));
        assert_eq!(fs::read_to_string(skill)?, "user skill");
        let p = plan(home, Cursor, ID)?;
        let path = p.config.clone().unwrap();
        let original = "{\r\n \"keep\":42}";
        fs::write(&path, original)?;
        if let Some(link) = &p.link {
            fs::create_dir_all(link.parent().unwrap())?;
            fs::write(link, "created after preview")?;
            assert!(p.connect("secret", None).is_err());
            assert_eq!(fs::read_to_string(path)?, original);
            assert!(!home.join("custom-data/agents").exists());
        }
        Ok(())
    }

    #[test]
    fn failed_replacement_reports_failed_rollback() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        executable_fixture(home, "codex")?;
        let old = plan(home, Codex, "000000000001")?.connect_with("old", None, &fake)?;
        let p = plan(home, Codex, ID)?;
        let token = p.token.clone();
        let err = p
            .connect_with("new", Some(&old), &|p, exe, args| {
                if args[1] == "add" {
                    anyhow::bail!("add failed for {}", args.last().unwrap());
                }
                fake(p, exe, args)
            })
            .err()
            .unwrap();
        let message = err.to_string();
        assert!(
            message.contains("rollback failed")
                && message.contains(ID)
                && message.contains("000000000001")
        );
        assert!(!token.exists());
        assert_eq!(
            fs::read_to_string(old.plan.skill.as_ref().unwrap())?,
            old.skill_text.unwrap()
        );
        Ok(())
    }

    #[test]
    fn json_disconnect_restores_original_bytes_and_foreign_links() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        let link = home.join(".local/bin/lagom");
        fs::create_dir_all(link.parent().unwrap())?;
        symlink(home.join("missing-user-command"), &link)?;
        for original in [
            "{\r\n \"number\":123456789012345678901234567890}",
            "{\"mcpServers\":{ \"other\":42 }}\r\n",
        ] {
            let p = plan(home, Cursor, ID)?;
            let path = p.config.clone().unwrap();
            fs::write(&path, original)?;
            assert!(p.link.is_none());
            assert!(p.connect("secret", None)?.disconnect()?.is_empty());
            assert_eq!(fs::read_to_string(path)?, original);
            assert_eq!(fs::read_link(&link)?, home.join("missing-user-command"));
        }
        Ok(())
    }

    #[test]
    fn malformed_config_reports_leftover_on_disconnect_and_retry() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        executable_fixture(tmp.path(), "codex")?;
        let c = plan(tmp.path(), Codex, ID)?.connect_with("secret", None, &fake)?;
        let path = c.plan.config.as_ref().unwrap();
        fs::write(path, "[broken")?;
        for _ in 0..2 {
            let left = c.disconnect_with(&fake)?;
            assert_eq!(left.len(), 1);
            assert!(left[0].contains(path.to_str().unwrap()));
            assert_eq!(fs::read_to_string(path)?, "[broken");
            assert!(!c.plan.skill.as_ref().unwrap().exists() && !c.plan.token.exists());
        }
        Ok(())
    }

    #[test]
    fn refuse_uninstalled_mcp_binary_before_writing() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        for bin in [
            home.join("Downloads/lagom"),
            home.join("Applications/../Downloads/lagom"),
        ] {
            let mut p = plan(home, Cursor, ID)?;
            p.binary = bin;
            let path = p.config.clone().unwrap();
            assert_eq!(
                p.connect("secret", None).err().unwrap().to_string(),
                "Move lagom to Applications first"
            );
            assert!(!path.exists());
        }
        let mut p = plan(home, Cursor, ID)?;
        p.binary = env::current_exe()?;
        if cfg!(debug_assertions) {
            assert!(p.connect("secret", None)?.disconnect()?.is_empty());
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dangling_link_is_replaced_and_shared_until_last_disconnect() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let home = tmp.path();
        let link = home.join(".local/bin/lagom");
        fs::create_dir_all(link.parent().unwrap())?;
        let mut initial = plan(home, Hermes, ID)?;
        initial.binary = home.join("Applications/old-lagom");
        fs::write(&initial.binary, "app")?;
        let first = initial.connect("one", None)?;
        fs::remove_file(&first.plan.binary)?;
        let second = plan(home, Cursor, "000000000002")?.connect("two", None)?;
        first.disconnect()?;
        assert_eq!(fs::read_link(&link)?, home.join(BIN));
        second.disconnect()?;
        assert!(fs::symlink_metadata(link).is_err());
        Ok(())
    }
}
