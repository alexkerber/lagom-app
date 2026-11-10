use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use lagom_core::{
    account::{Account, Security, ServerConfig},
    discovery::{discover, Discovery},
    imap_sync::{ImapSync, DEFAULT_LIMIT},
    oauth::{self, Provider},
    presets,
    smtp::{self, Outgoing},
    store::{Folder, MessageSummary, Store},
    tasks::{Task, TaskList},
};
use std::{
    io::{IsTerminal, Write},
    path::PathBuf,
};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(name = "lagom-cli", about = "Local-first lagom mail core test client")]
struct Args {
    #[arg(
        long,
        global = true,
        help = "Account UUID or email; required for ambiguous account lists"
    )]
    account: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Override the platform data-directory database"
    )]
    database: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Clone, Copy, ValueEnum)]
enum Transport {
    Tls,
    StartTls,
    Plain,
}
impl From<Transport> for Security {
    fn from(t: Transport) -> Self {
        match t {
            Transport::Tls => Self::Tls,
            Transport::StartTls => Self::StartTls,
            Transport::Plain => Self::Plain,
        }
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum SignInProvider {
    Google,
    Microsoft,
}
impl From<SignInProvider> for Provider {
    fn from(value: SignInProvider) -> Self {
        match value {
            SignInProvider::Google => Self::Google,
            SignInProvider::Microsoft => Self::Microsoft,
        }
    }
}
#[derive(Subcommand)]
enum Command {
    Tasks {
        #[command(subcommand)]
        command: TasksCommand,
    },
    SignIn {
        #[arg(value_enum)]
        provider: SignInProvider,
    },
    Discover {
        email: String,
    },
    AddAccount {
        email: String,
        #[arg(long)]
        imap: Option<String>,
        #[arg(long)]
        smtp: Option<String>,
        #[arg(long, value_enum)]
        imap_security: Option<Transport>,
        #[arg(long, value_enum)]
        smtp_security: Option<Transport>,
    },
    Accounts,
    Sync {
        #[arg(value_name = "ACCOUNT")]
        target: Option<String>,
    },
    Folders,
    List {
        folder: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    Show {
        id: i64,
    },
    Search {
        query: String,
    },
    Send {
        #[arg(long, value_delimiter = ',')]
        to: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        cc: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        bcc: Vec<String>,
        #[arg(long)]
        subject: String,
        #[arg(long, required_unless_present = "body_file", conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long, required_unless_present = "body")]
        body_file: Option<PathBuf>,
        #[arg(long)]
        in_reply_to: Option<String>,
        #[arg(long)]
        references: Vec<String>,
    },
    Archive {
        id: i64,
    },
    Trash {
        id: i64,
    },
    Move {
        id: i64,
        folder: String,
    },
}
#[derive(Subcommand)]
enum TasksCommand {
    Lists,
    List {
        list: String,
        #[arg(long, help = "Include completed tasks")]
        done: bool,
    },
    Add { list: String, title: String },
    Done { id: i64 },
    FromMail {
        id: i64,
        #[arg(long)]
        list: Option<String>,
    },
}

fn selected_task_list(store: &Store, selector: Option<&str>) -> Result<TaskList> {
    let lists = store.task_lists()?;
    let Some(selector) = selector else {
        let default = store.default_task_list()?;
        return lists.into_iter().find(|list| list.id == default).context("No default task list");
    };
    if let Some(list) = lists.iter().find(|l| l.id.to_string() == selector) {
        return Ok(list.clone());
    }
    let mut found = lists.into_iter().filter(|l| l.name.to_lowercase() == selector.to_lowercase());
    let list = found.next().context("Task list missing; run tasks lists")?;
    if found.next().is_some() {
        bail!("Task list ambiguous; run tasks lists and use its numeric id");
    }
    Ok(list)
}
fn print_task(task: &Task) {
    println!("{}\t{}\t{}", task.id, if task.done_at.is_some() { "x" } else { " " }, clean(&task.title));
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn selected(store: &Store, selector: Option<&str>) -> Result<Account> {
    let accounts = store.accounts()?;
    let matches: Vec<_> = accounts
        .into_iter()
        .filter(|a| selector.is_none_or(|s| a.id == s || a.email.eq_ignore_ascii_case(s)))
        .collect();
    match matches.len() {
        0 => bail!("No matching account; use add-account first"),
        1 => Ok(matches.into_iter().next().context("account missing")?),
        _ => bail!("Choose one account with --account <UUID or email>"),
    }
}
fn selected_folder(store: &Store, account: &Account, selector: &str) -> Result<Folder> {
    let folders = store.folders(&account.id, now())?;
    let found: Vec<_> = folders
        .into_iter()
        .filter(|f| {
            f.id.to_string() == selector
                || f.name == selector
                || (selector.eq_ignore_ascii_case("INBOX") && f.name.eq_ignore_ascii_case("INBOX"))
        })
        .collect();
    if found.len() != 1 {
        bail!("Folder missing or ambiguous; run folders and use its numeric id");
    }
    found.into_iter().next().context("folder missing")
}
fn endpoint(value: &str, username: &str, imap: bool) -> Result<ServerConfig> {
    let (host, port) = value
        .rsplit_once(':')
        .context("server must be host:port (IPv6: [::1]:port)")?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let port: u16 = port.parse().context("invalid server port")?;
    let security = if (imap && port == 993) || (!imap && port == 465) {
        Security::Tls
    } else {
        Security::StartTls
    };
    Ok(ServerConfig {
        host: host.into(),
        port,
        security,
        username: username.into(),
    })
}
fn config(
    email: &str,
    imap: Option<&str>,
    smtp: Option<&str>,
    is: Option<Transport>,
    ss: Option<Transport>,
) -> Result<Account> {
    let preset = presets::servers(email);
    let mut i = match imap {
        Some(v) => endpoint(v, email, true)?,
        None => preset
            .as_ref()
            .map(|p| p.0.clone())
            .context("Unknown provider: specify --imap and --smtp")?,
    };
    let mut s = match smtp {
        Some(v) => endpoint(v, email, false)?,
        None => preset
            .as_ref()
            .map(|p| p.1.clone())
            .context("Unknown provider: specify --imap and --smtp")?,
    };
    if let Some(t) = is {
        i.security = t.into();
    }
    if let Some(t) = ss {
        s.security = t.into();
    }
    Ok(Account::new(email.into(), i, s)?)
}
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}
fn print_messages(messages: Vec<MessageSummary>) {
    for m in messages {
        println!(
            "{}\t{}\t{}\t{}",
            m.id,
            if m.seen { " " } else { "*" },
            clean(&m.from_address),
            clean(&m.subject)
        );
    }
}
async fn connection(account: &Account) -> Result<ImapSync> {
    let password = account.password()?;
    Ok(ImapSync::login(account, &password).await?)
}
async fn sign_in(provider: Provider) -> Result<oauth::SignedIn> {
    // A browser launched with the URL as an argument would show its state and PKCE challenge to other local users in `ps`.
    Ok(oauth::sign_in(provider, |url| eprintln!("Open this URL in your browser to sign in:\n{url}")).await?)
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut store = match &args.database {
        Some(path) => Store::open(path)?,
        None => Store::open_default()?,
    };
    match args.command {
        Command::Tasks { command } => match command {
            TasksCommand::Lists => {
                for list in store.task_lists()? {
                    println!("{}\t{}", list.id, clean(&list.name));
                }
            }
            TasksCommand::List { list, done } => {
                let list = selected_task_list(&store, Some(&list))?;
                for task in store.tasks(list.id, done)? { print_task(&task); }
            }
            TasksCommand::Add { list, title } => {
                let list = selected_task_list(&store, Some(&list))?;
                print_task(&store.add_task(list.id, &title, "", None)?);
            }
            TasksCommand::Done { id } => {
                store.set_done(id, true)?;
                println!("Updated {id}");
            }
            TasksCommand::FromMail { id, list } => {
                let list = selected_task_list(&store, list.as_deref())?;
                let mail = store.message(id)?.summary;
                print_task(&store.task_from_mail(list.id, &mail)?);
            }
        },
        Command::SignIn { provider } => {
            let signed = sign_in(provider.into()).await?;
            if let Err(error) = store.save_account(&signed.account) {
                let _ = signed.account.delete_password();
                return Err(error.into());
            }
            println!("Added {} ({})", clean(&signed.email), signed.account.id);
        }
        Command::Discover { email } => match discover(&email).await {
            Discovery::OAuth(provider) => println!(
                "OAuth: {}",
                match provider {
                    Provider::Google => "google",
                    Provider::Microsoft => "microsoft",
                }
            ),
            Discovery::Password { preset, help_url } => {
                println!(
                    "Password: {}\nIMAP: {}:{} {:?} ({})\nSMTP: {}:{} {:?} ({})",
                    clean(&preset.name),
                    preset.imap.host,
                    preset.imap.port,
                    preset.imap.security,
                    clean(&preset.imap.username),
                    preset.smtp.host,
                    preset.smtp.port,
                    preset.smtp.security,
                    clean(&preset.smtp.username)
                );
                if let Some(url) = help_url {
                    println!("App password: {url}");
                }
            }
            Discovery::Manual => println!("Manual configuration required"),
        },
        Command::AddAccount {
            email,
            imap,
            smtp,
            imap_security,
            smtp_security,
        } => {
            let account = config(
                &email,
                imap.as_deref(),
                smtp.as_deref(),
                imap_security,
                smtp_security,
            )?;
            eprint!("App password: ");
            std::io::stderr().flush()?;
            let password = Zeroizing::new(if std::io::stdin().is_terminal() {
                rpassword::read_password()?
            } else {
                rpassword::read_password_with_config(
                    rpassword::ConfigBuilder::new()
                        .input_reader(std::io::stdin())
                        .output_discard()
                        .build(),
                )?
            });
            if password.is_empty() {
                bail!("Password cannot be empty");
            }
            ImapSync::login(&account, &password).await?.logout().await?;
            account.set_password(&password)?;
            if let Err(error) = store.save_account(&account) {
                let _ = account.delete_password();
                return Err(error.into());
            }
            println!("Added {} ({})", account.email, account.id);
        }
        Command::Accounts => {
            for account in store.accounts()? {
                println!(
                    "{}\t{}\t{}",
                    clean(&account.id),
                    clean(&account.email),
                    clean(&account.display_name)
                );
            }
        }
        Command::Sync { target } => {
            let selector = target.as_deref().or(args.account.as_deref());
            let accounts = if selector.is_some() {
                vec![selected(&store, selector)?]
            } else {
                store.accounts()?
            };
            if accounts.is_empty() {
                bail!("No accounts; use add-account first");
            }
            for account in accounts {
                let mut imap = connection(&account).await?;
                imap.sync(&mut store, DEFAULT_LIMIT).await?;
                imap.logout().await?;
                println!("Synced {}", account.email);
            }
        }
        Command::Folders => {
            let account = selected(&store, args.account.as_deref())?;
            for f in store.folders(&account.id, now())? {
                println!(
                    "{}\t{}\t{}\t{} unread",
                    f.id,
                    clean(&f.name),
                    f.role.as_str(),
                    f.unread
                );
            }
        }
        Command::List { folder, limit } => {
            let account = selected(&store, args.account.as_deref())?;
            let folder = selected_folder(&store, &account, folder.as_deref().unwrap_or("INBOX"))?;
            print_messages(store.list_messages(folder.id, now(), limit, 0)?);
        }
        Command::Show { id } => {
            let m = store.message(id)?;
            println!(
                "From: {} <{}>\nTo: {}\nCc: {}\nSubject: {}\n\n{}",
                clean(&m.summary.from_name),
                clean(&m.summary.from_address),
                clean(&m.summary.to),
                clean(&m.summary.cc),
                clean(&m.summary.subject),
                clean(&m.body_text)
            );
        }
        Command::Search { query } => {
            let account = selected(&store, args.account.as_deref())?;
            print_messages(store.search(&account.id, &query, now(), 50, 0)?);
        }
        Command::Send {
            to,
            cc,
            bcc,
            subject,
            body,
            body_file,
            in_reply_to,
            references,
        } => {
            let body = match body_file {
                Some(path) => std::fs::read_to_string(&path).with_context(|| format!("Could not read {} as UTF-8", path.display()))?,
                None => body.context("Provide --body or --body-file")?,
            };
            let account = selected(&store, args.account.as_deref())?;
            let password = account.password()?;
            smtp::send(
                &account,
                &password,
                &Outgoing {
                    to,
                    cc,
                    bcc,
                    subject,
                    text: body,
                    in_reply_to,
                    references,
                    message_id: None,
                    attachments: Vec::new(),
                },
                &mut store,
            )
            .await?;
            println!("Sent");
        }
        command @ (Command::Archive { .. } | Command::Trash { .. } | Command::Move { .. }) => {
            let id = match &command {
                Command::Archive { id } | Command::Trash { id } | Command::Move { id, .. } => *id,
                _ => unreachable!(),
            };
            let message = store.message(id)?.summary;
            let selector = args.account.as_deref().unwrap_or(&message.account);
            let account = selected(&store, Some(selector))?;
            if account.id != message.account {
                bail!("Message belongs to another account");
            }
            let mut imap = connection(&account).await?;
            match command {
                Command::Archive { .. } => imap.archive(&mut store, id).await?,
                Command::Trash { .. } => imap.trash(&mut store, id).await?,
                Command::Move { folder, .. } => {
                    let target = selected_folder(&store, &account, &folder)?;
                    imap.move_message(&mut store, id, target.id).await?;
                }
                _ => unreachable!(),
            }
            imap.logout().await?;
            println!("Updated {id}");
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_and_plain_guard() {
        assert!(config("a@gmail.com", None, None, None, None).is_ok());
        assert!(config("a@custom.test", None, None, None, None).is_err());
        let a = config(
            "test@lagom.test",
            Some("127.0.0.1:3143"),
            Some("localhost:3025"),
            Some(Transport::Plain),
            Some(Transport::Plain),
        )
        .unwrap();
        assert_eq!(a.imap.security, Security::Plain);
        assert!(config(
            "a@custom.test",
            Some("mail.evil:143"),
            Some("localhost:3025"),
            Some(Transport::Plain),
            Some(Transport::Plain)
        )
        .is_err());
        assert!(endpoint("bad", "a@test", true).is_err());
        assert_eq!(endpoint("[::1]:143", "a@test", true).unwrap().host, "::1");
    }
    #[test]
    fn parser_and_terminal_output() {
        use clap::CommandFactory;
        Args::command().debug_assert();
        assert!(Args::try_parse_from(["lagom-cli", "sign-in", "google"]).is_ok());
        assert!(Args::try_parse_from(["lagom-cli", "sign-in", "microsoft"]).is_ok());
        assert!(Args::try_parse_from(["lagom-cli", "sign-in", "unknown"]).is_err());
        assert!(Args::try_parse_from(["lagom-cli", "discover", "a@example.com"]).is_ok());
        assert!(Args::try_parse_from(["lagom-cli", "list", "INBOX", "--limit", "10"]).is_ok());
        assert!(Args::try_parse_from([
            "lagom-cli",
            "send",
            "--to",
            "a@example.com",
            "--subject",
            "Hi",
            "--body",
            "Hello"
        ])
        .is_ok());
        let send = ["lagom-cli", "send", "--to", "a@example.com", "--subject", "Hi"];
        assert!(Args::try_parse_from(send).is_err());
        assert!(Args::try_parse_from(send.into_iter().chain(["--body-file", "mail.md"])).is_ok());
        assert!(Args::try_parse_from(send.into_iter().chain(["--body", "x", "--body-file", "mail.md"])).is_err());
        assert_eq!(clean("Hi\x1b[31m"), "Hi\\u{1b}[31m");
    }
}
