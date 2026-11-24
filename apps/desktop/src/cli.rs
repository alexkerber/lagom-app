mod mcp;
mod remote;
mod skill;

use lagom_core::agent::policy::Level::{self, Read, Send, Triage};
use lagom_core::agent::protocol::{Code, Request, Response, VERSION};
use serde_json::{Map, Value};
use std::{
    env,
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::timeout,
};

type Args = Map<String, Value>;
type Result<T> = std::result::Result<T, Code>;
const INVALID: Code = Code::InvalidRequest;
const MAX_REQUEST: usize = 1 << 20;
const MAX_RESPONSE: usize = 32 << 20;
const ACTIONS: &[&str] =
    &["archive", "move", "read", "unread", "star", "unstar", "snooze", "unsnooze", "trash"];

struct Tool {
    description: &'static str,
    name: &'static str,
    cmd: &'static str,
    positional: &'static [&'static str],
    flags: &'static [&'static str],
    resource: &'static str,
    level: Level,
}

const fn tool(
    description: &'static str,
    name: &'static str,
    cmd: &'static str,
    positional: &'static [&'static str],
    flags: &'static [&'static str],
    resource: &'static str,
    level: Level,
) -> Tool {
    Tool { description, name, cmd, positional, flags, resource, level }
}

const TOOLS: &[Tool] = &[
    tool(
        "List accessible accounts and their mail and calendar permissions.",
        "list_accounts",
        "accounts",
        &[],
        &[],
        "",
        Read,
    ),
    tool(
        "List folders and unread counts by account.",
        "list_folders",
        "folders",
        &[],
        &[],
        "mail",
        Read,
    ),
    tool(
        "List messages matching an optional filter.",
        "list_mail",
        "emails",
        &[],
        &["filter", "page", "limit"],
        "mail",
        Read,
    ),
    tool(
        "Search mail within the permitted accounts and folders.",
        "search_mail",
        "search",
        &["query"],
        &["in", "page", "limit"],
        "mail",
        Read,
    ),
    tool(
        "Read the messages and attachment metadata in a thread.",
        "read_thread",
        "thread",
        &["id"],
        &[],
        "mail",
        Read,
    ),
    tool(
        "Read a message attachment by ID.",
        "get_attachment",
        "attachment",
        &["id"],
        &["folder"],
        "mail",
        Read,
    ),
    tool(
        "List contacts for the selected accounts.",
        "list_contacts",
        "contacts",
        &[],
        &[],
        "mail",
        Read,
    ),
    tool(
        "List cached calendar events in a time range.",
        "list_events",
        "events",
        &[],
        &["start", "end"],
        "calendar",
        Read,
    ),
    tool(
        "Find free and busy intervals in the selected calendars.",
        "find_availability",
        "availability",
        &[],
        &["start", "end"],
        "calendar",
        Read,
    ),
    tool("List drafts.", "list_drafts", "draft.list", &[], &[], "mail", Read),
    tool("Read a draft by ID.", "read_draft", "draft.show", &["id"], &[], "mail", Read),
    tool(
        "Create a mail draft for review.",
        "create_draft",
        "draft.create",
        &[],
        &["to", "cc", "bcc", "subject", "body", "reply", "forward"],
        "mail",
        Triage,
    ),
    tool(
        "Update an agent-owned draft at its current revision.",
        "edit_draft",
        "draft.edit",
        &["id"],
        &["revision", "to", "cc", "bcc", "subject", "body"],
        "mail",
        Triage,
    ),
    tool(
        "Delete an agent-owned draft.",
        "delete_draft",
        "draft.delete",
        &["id"],
        &[],
        "mail",
        Triage,
    ),
    tool(
        "Request approval to send a draft.",
        "send_draft",
        "draft.send",
        &["id"],
        &[],
        "mail",
        Send,
    ),
    tool(
        "Apply a triage action to a message.",
        "mail_action",
        "action",
        &["action", "id"],
        &["folder", "until"],
        "mail",
        Triage,
    ),
    tool(
        "Request approval to create a calendar event.",
        "create_event",
        "event.create",
        &[],
        &["calendar", "title", "start", "end", "location", "description", "guests", "meet"],
        "calendar",
        Send,
    ),
    tool(
        "Request approval to update a calendar event.",
        "update_event",
        "event.update",
        &["id"],
        &["title", "start", "end", "location", "description", "guests", "meet"],
        "calendar",
        Send,
    ),
    tool(
        "Request approval to delete a calendar event.",
        "delete_event",
        "event.delete",
        &["id"],
        &[],
        "calendar",
        Send,
    ),
];

impl Tool {
    fn required(&self) -> impl Iterator<Item = &str> {
        self.positional.iter().copied().chain((self.cmd == "draft.edit").then_some("revision"))
    }

    fn command(&self, mut args: Args) -> Result<(String, Args)> {
        for key in self.required() {
            if !args.contains_key(key) {
                return Err(INVALID);
            }
        }
        for (key, value) in &args {
            if key != "account"
                && !self.flags.contains(&key.as_str())
                && !self.positional.contains(&key.as_str())
            {
                return Err(INVALID);
            }
            let valid = match key.as_str() {
                "start" | "end" if matches!(self.cmd, "availability" | "events") => value.is_i64(),
                "page" => value.as_u64().is_some_and(|n| n >= 1),
                "limit" => value.as_u64().is_some_and(|n| (1..=100).contains(&n)),
                "action" => value.as_str().is_some_and(|v| ACTIONS.contains(&v)),
                _ => value.is_string(),
            };
            if !valid {
                return Err(INVALID);
            }
        }
        let cmd = if self.cmd == "action" {
            let Some(Value::String(action)) = args.remove("action") else {
                return Err(INVALID);
            };
            format!("action.{action}")
        } else {
            self.cmd.into()
        };
        Ok((cmd, args))
    }
}

#[derive(Default)]
struct Options {
    words: Vec<String>,
    args: Args,
    identity: Option<(String, String)>,
    json: bool,
}

impl Options {
    fn parse(words: Vec<String>) -> Result<Self> {
        let mut opts = Self::default();
        let mut iter = words.into_iter();
        while let Some(word) = iter.next() {
            if matches!(word.as_str(), "--help" | "--version") {
                opts.words.push(word);
                continue;
            }
            let Some(flag) = word.strip_prefix("--") else {
                opts.words.push(word);
                continue;
            };
            if flag == "json" {
                opts.json = true;
                continue;
            }
            let (key, value) = match flag.split_once('=') {
                Some((k, v)) => (k, v.to_owned()),
                None => {
                    let value = iter.next().ok_or(INVALID)?;
                    if let Some(opt) = value.strip_prefix("--") {
                        let key = opt.split('=').next().unwrap();
                        if matches!(
                            key,
                            "help"
                                | "version"
                                | "json"
                                | "agent"
                                | "token"
                                | "account"
                                | "dir"
                                | "for"
                        ) || TOOLS
                            .iter()
                            .any(|t| t.flags.contains(&key) || t.positional.contains(&key))
                        {
                            return Err(INVALID);
                        }
                    }
                    (flag, value)
                }
            };
            match key {
                "token" | "agent" => {
                    opts.identity.get_or_insert((key.into(), value));
                }
                _ => {
                    let value = match key {
                        "limit" | "page" => Value::from(value.parse::<u64>().map_err(|_| INVALID)?),
                        _ => Value::String(value),
                    };
                    if opts.args.insert(key.into(), value).is_some() {
                        return Err(INVALID);
                    }
                }
            }
        }
        Ok(opts)
    }
}

struct Client {
    source_ip: Option<String>,
    dir: PathBuf,
    identity: Option<(String, String)>,
}

impl Client {
    fn token(&self) -> Result<String> {
        let token = match &self.identity {
            Some((kind, v)) if kind == "agent" => {
                if v.len() != 12
                    || !v.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                {
                    return Err(Code::Unauthorized);
                }
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(self.dir.join("agents").join(format!("{v}.token")))
                    .map_err(|_| Code::Unauthorized)?;
                let meta = file.metadata().map_err(|_| Code::Unauthorized)?;
                if !meta.is_file()
                    || meta.uid() != unsafe { libc::geteuid() }
                    || meta.mode() & 0o077 != 0
                    || meta.len() > 256
                {
                    return Err(Code::Unauthorized);
                }
                let mut text = zeroize::Zeroizing::new(String::new());
                io::Read::read_to_string(&mut io::Read::take(file, 256), &mut text)
                    .map_err(|_| Code::Unauthorized)?;
                text.trim_end().into()
            }
            Some((_, v)) => v.clone(),
            None => env::var("LAGOM_TOKEN").map_err(|_| Code::Unauthorized)?,
        };
        if token.is_empty() {
            return Err(Code::Unauthorized);
        }
        Ok(token)
    }

    async fn request(&self, cmd: &str, args: Args) -> Result<(Vec<u8>, Response)> {
        let request = Request {
            v: VERSION,
            token: Some(self.token()?.into()),
            cmd: cmd.into(),
            args,
            source_ip: self.source_ip.clone(),
        };
        let mut bytes = zeroize::Zeroizing::new(serde_json::to_vec(&request).map_err(|_| INVALID)?);
        bytes.push(b'\n');
        if bytes.len() > MAX_REQUEST {
            return Err(Code::TooLarge);
        }
        let mut stream = timeout(Duration::from_secs(5), async {
            let mut stream = UnixStream::connect(crate::agent::socket_path(&self.dir)?)
                .await
                .map_err(|_| Code::AppUnreachable)?;
            if stream.peer_cred().map_err(|_| Code::Unauthorized)?.uid()
                != unsafe { libc::geteuid() }
            {
                return Err(Code::Unauthorized);
            }
            stream.write_all(&bytes).await.map_err(|_| Code::Unavailable)?;
            Ok::<_, Code>(stream)
        })
        .await
        .map_err(|_| Code::Unavailable)??;
        let confirm = cmd == "draft.send" || cmd.starts_with("event.");
        let lost = if cmd.starts_with("action.")
            || cmd.starts_with("event.")
            || matches!(cmd, "draft.create" | "draft.edit" | "draft.delete" | "draft.send")
        {
            Code::Uncertain
        } else {
            Code::Unavailable
        };
        let mut frame = Vec::new();
        timeout(
            Duration::from_secs(if confirm { 130 } else { 30 }),
            BufReader::new((&mut stream).take(MAX_RESPONSE as u64 + 1))
                .read_until(b'\n', &mut frame),
        )
        .await
        .map_err(|_| lost)?
        .map_err(|_| lost)?;
        if frame.len() > MAX_RESPONSE {
            return Err(if lost == Code::Uncertain { lost } else { Code::TooLarge });
        }
        if frame.last() != Some(&b'\n') {
            return Err(lost);
        }
        let response = serde_json::from_slice(&frame).map_err(|_| {
            if lost == Code::Uncertain {
                return lost;
            }
            // The shared decoder rejects newer versions before reading their payload.
            match serde_json::from_slice::<Value>(&frame).ok().and_then(|v| v["v"].as_u64()) {
                Some(v) if v != u64::from(VERSION) => Code::Unsupported,
                _ => INVALID,
            }
        })?;
        Ok((frame, response))
    }
}

fn field(v: &Value) -> String {
    escape(&v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_control()
            || matches!(c,'\u{061c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
        {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

fn table(out: &mut impl Write, value: &Value) -> io::Result<()> {
    match value {
        Value::Object(obj) => {
            for (k, v) in obj {
                if v.is_array() {
                    table(out, v)?;
                } else {
                    writeln!(out, "{}\t{}", escape(k), field(v))?;
                }
            }
        }
        Value::Array(rows) if rows.iter().all(Value::is_object) => {
            let cols: std::collections::BTreeSet<_> = rows
                .iter()
                .filter_map(Value::as_object)
                .flat_map(|o| o.keys().map(String::as_str))
                .collect();
            if !cols.is_empty() {
                writeln!(out, "{}", cols.iter().map(|k| escape(k)).collect::<Vec<_>>().join("\t"))?;
                for row in rows {
                    writeln!(
                        out,
                        "{}",
                        cols.iter()
                            .map(|k| row.get(*k).map(field).unwrap_or_default())
                            .collect::<Vec<_>>()
                            .join("\t")
                    )?;
                }
            }
        }
        Value::Array(rows) => {
            for row in rows {
                table(out, row)?;
            }
        }
        _ => writeln!(out, "{}", field(value))?,
    }
    Ok(())
}

fn list_table(out: &mut impl Write, cmd: &str, value: &Value, client: &Client,
              rt: &tokio::runtime::Runtime) -> Result<()> {
    let (key, columns): (_, &[&str]) = match cmd {
        "emails" | "search" => ("messages", &["date", "from", "subject", "id"]),
        "folders" => (cmd, &["name", "role", "unread"]),
        "accounts" => (cmd, &["email", "mail", "calendar", "id"]),
        "contacts" => (cmd, &["name", "address"]),
        "events" => (cmd, &["start", "end", "title"]),
        _ => return table(out, value).map_err(|_| Code::Internal),
    };
    let rows = value[key].as_array().ok_or(INVALID)?;
    let mut ids = rows.iter().filter_map(|row| row["account"].as_str());
    let first = ids.next();
    let mut columns = columns.to_vec();
    let mut emails = std::collections::HashMap::new();
    if ids.any(|id| Some(id) != first) {
        let (_, response) = rt.block_on(client.request("accounts", Args::new()))?;
        let accounts = match response {
            Response::Success(data) => data,
            Response::Failure(code) => return Err(code),
        };
        for row in accounts["accounts"].as_array().ok_or(INVALID)? {
            emails.insert(row["id"].as_str().ok_or(INVALID)?.to_owned(), field(&row["email"]));
        }
        columns.push("account");
    }
    if !rows.is_empty() {
        writeln!(out, "{}", columns.join("\t")).map_err(|_| Code::Internal)?;
    }
    for row in rows {
        let mut cells = Vec::new();
        for &column in &columns {
            let cell = match column {
                "account" => emails.get(row[column].as_str().ok_or(INVALID)?)
                    .ok_or(Code::NotFound)?.clone(),
                "date" => row[column].as_str().and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
                    .map(|date| date.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default(),
                _ => row.get(column).map(field).unwrap_or_default(),
            };
            cells.push(cell);
        }
        writeln!(out, "{}", cells.join("\t")).map_err(|_| Code::Internal)?;
    }
    if let Some(next) = value.get("next_page") {
        writeln!(out, "next_page\t{}", field(next)).map_err(|_| Code::Internal)?;
    }
    Ok(())
}

fn run(mut opts: Options) -> Result<i32> {
    let Some((name, rest)) = opts.words.split_first() else {
        return Err(INVALID);
    };
    if name == "contacts" && rest == ["--help"] {
        writeln!(io::stdout().lock(), "lagom contacts [--account ID|EMAIL] [--agent ID] [--json]\nLists all contacts. Only emails and search accept --page and --limit.")
            .map_err(|_| Code::Internal)?;
        return Ok(0);
    }
    match name.as_str() {
        "skill" => {
            skill::install(rest, &opts.args)?;
            if opts.json {
                writeln!(io::stdout().lock(), "{}", serde_json::json!({"installed":true}))
                    .map_err(|_| Code::Internal)?;
            }
            return Ok(0);
        }
        "--help" if rest.is_empty() && opts.args.is_empty() => {
            writeln!(io::stdout().lock(), "lagom <command> [--account ID] [--agent ID] [--json]\nUse --agent ID or LAGOM_TOKEN. --token is visible in process listings.\naccounts, folders, emails [--filter FILTER] [--page N] [--limit N], search QUERY [--in SCOPE] [--page N] [--limit N], thread ID, attachment ID, events [--start UNIX] [--end UNIX], availability [--start UNIX] [--end UNIX], contacts\nOnly emails and search accept --page and --limit.\ndraft list|show|create|edit|delete|send, action archive|move|read|unread|star|unstar|snooze|unsnooze|trash, event create|update|delete\nmcp, skill install [--for KIND] [--dir PATH]").map_err(|_| Code::Internal)?;
            return Ok(0);
        }
        "--version" if rest.is_empty() && opts.args.is_empty() => {
            writeln!(io::stdout().lock(), "lagom {}", version()).map_err(|_| Code::Internal)?;
            return Ok(0);
        }
        _ => {}
    }
    let dir = lagom_core::store::Store::data_dir().map_err(|_| INVALID)?;
    let client = Client { dir, identity: opts.identity.take(), source_ip: None };
    run_client(client, Ok(opts))
}

fn prepare(opts: Options) -> Result<(String, Args, bool)> {
    let (name, mut rest) = opts.words.split_first().ok_or(INVALID)?;
    if name == "mcp" {
        if !rest.is_empty() || !opts.args.is_empty() {
            return Err(INVALID);
        }
        return Ok((name.clone(), opts.args, opts.json));
    }
    let mut cmd = name.clone();
    if matches!(name.as_str(), "draft" | "event") {
        let (sub, tail) = rest.split_first().ok_or(INVALID)?;
        cmd = format!("{name}.{sub}");
        rest = tail;
    }
    if cmd == "draft.schedule" { return Err(Code::Unsupported); }
    let tool = TOOLS.iter().find(|t| t.cmd == cmd).ok_or(INVALID)?;
    if rest.len() != tool.positional.len() {
        return Err(INVALID);
    }
    let mut args = opts.args;
    for (key, v) in tool.positional.iter().zip(rest) {
        if args.insert((*key).into(), Value::from(v.clone())).is_some() {
            return Err(INVALID);
        }
    }
    if matches!(cmd.as_str(), "events" | "availability") {
        for key in ["start", "end"] {
            if let Some(value) = args.get_mut(key) {
                *value = Value::from(
                    value.as_str().ok_or(INVALID)?.parse::<i64>().map_err(|_| INVALID)?,
                );
            }
        }
    }
    let (cmd, args) = tool.command(args)?;
    Ok((cmd, args, opts.json))
}

fn run_client(client: Client, options: Result<Options>) -> Result<i32> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all().build().map_err(|_| Code::Internal)?;
    let (cmd, args, json) = options.and_then(prepare).inspect_err(|_| {
        if client.source_ip.is_some() && client.identity.is_some() {
            let _ = rt.block_on(client.request("remote.refused", Args::new()));
        }
    })?;
    if cmd == "mcp" {
        if client.source_ip.is_some() {
            drop(zeroize::Zeroizing::new(client.token()?));
        }
        return mcp::serve(client, &rt).map(|_| 0);
    }
    let (frame, response) = rt.block_on(client.request(&cmd, args))?;
    let mut out = io::stdout().lock();
    if json {
        out.write_all(&frame).map_err(|_| Code::Internal)?;
        return Ok(match response {
            Response::Success(_) => 0,
            Response::Failure(code) => status(code),
        });
    }
    match response {
        Response::Success(value) => list_table(&mut out, &cmd, &value, &client, &rt).map(|_| 0),
        Response::Failure(code) => Err(code),
    }
}

fn version() -> &'static str {
    option_env!("LAGOM_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn status(code: Code) -> i32 {
    if code == INVALID {
        2
    } else {
        1
    }
}

pub fn dispatch() -> Option<i32> {
    let mut probe = env::args_os().skip(1);
    let mut first = probe.next()?;
    while matches!(
        first.to_str()?.split('=').next(),
        Some("--agent" | "--token" | "--account" | "--json")
    ) {
        if first != "--json" && !first.to_str()?.contains('=') {
            probe.next()?;
        }
        first = probe.next()?;
    }
    let command = first.to_str()?;
    if !matches!(command, "--help" | "--version" | "mcp" | "skill" | "remote")
        && !TOOLS.iter().any(|t| t.cmd.split('.').next() == Some(command))
    {
        return None;
    }
    let words: Vec<String> = match env::args_os().skip(1).map(|w| w.into_string()).collect() {
        Ok(v) => v,
        Err(_) => {
            let _ = writeln!(io::stderr().lock(), "{}", INVALID.message());
            return Some(status(INVALID));
        }
    };
    let json = words.iter().any(|w| w == "--json");
    let result = if command == "remote" {
        remote::run(&words)
    } else {
        Options::parse(words).and_then(run)
    };
    Some(match result {
        Ok(status) => status,
        Err(code) => {
            if json {
                let mut out = io::stdout().lock();
                let _ = serde_json::to_writer(&mut out, &Response::Failure(code));
                let _ = writeln!(out);
            } else {
                let _ = writeln!(io::stderr().lock(), "{}", code.message());
            }
            status(code)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_escapes_controls_in_keys_and_values() {
        let text = "\x1b\x07\r\n\t\x7f\u{9b}\u{202e}";
        let mut out = Vec::new();
        table(&mut out, &serde_json::json!([{(text):text}])).unwrap();
        let escaped = r"\u{1b}\u{7}\r\n\t\u{7f}\u{9b}\u{202e}";
        assert_eq!(String::from_utf8(out).unwrap(), format!("{escaped}\n{escaped}\n"));
    }
}
