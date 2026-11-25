//! The listener belongs to the app, including while its main window is closed.
pub(crate) mod confirm;
pub(crate) mod read;
pub(crate) mod write;

use crate::sync::Bridge;
use lagom_core::{
    agent::{
        audit::{Address, AuditRow, Failure},
        id::AuditTarget,
        protocol::{Code, Request, Response, VERSION},
        token::Identity,
    },
    store::Store,
};
#[cfg(windows)]
pub(crate) mod windows;
#[cfg(windows)]
use self::windows::{clone_peer, peer_connected, Socket};
#[cfg(windows)]
pub(crate) use self::windows::{socket_path, write_frame};
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeServer;
#[cfg(windows)]
type Peer = std::os::windows::io::OwnedHandle;
#[cfg(unix)]
type Peer = std::os::unix::net::UnixStream;
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream as Stream};
#[cfg(unix)]
use std::{fs::{self, File}, os::{fd::AsFd, unix::fs::{FileTypeExt, PermissionsExt}}};

#[cfg(unix)]
fn clone_peer(stream: &Stream) -> io::Result<Peer> {
    Ok(stream.as_fd().try_clone_to_owned()?.into())
}

#[cfg(unix)]
fn peer_connected(mut peer: &Peer) -> bool {
    use std::io::Read;
    matches!(peer.read(&mut [0]), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
}
use std::{
    collections::{HashMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    sync::{oneshot, watch, Semaphore},
    task::{self, JoinSet},
    time::{interval, timeout, timeout_at},
};

const MAX_REQUEST: usize = 1024 * 1024;
const MAX_RESPONSE: usize = 32 * 1024 * 1024;
const MAX_PER_TOKEN: usize = 4;
const SETTINGS_POLL: Duration = Duration::from_millis(100);
const ANSWER: Duration = Duration::from_secs(30);
struct TokenLimit {
    times: VecDeque<(Instant, bool)>,
    permits: Arc<Semaphore>,
}
type Rates = Arc<Mutex<HashMap<String, TokenLimit>>>;

pub(crate) struct Server {
    #[cfg(windows)]
    pub bind_failed: watch::Receiver<bool>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn start(path: &Path, bridge: Bridge) -> Self {
        let path = path.to_owned();
        let (stop, rx) = oneshot::channel();
        #[cfg(windows)]
        let (status, bind_failed) = watch::channel(false);
        let thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("agent runtime");
            rt.block_on(serve(
                path, bridge, rx,
                #[cfg(windows)]
                status,
            ));
            rt.shutdown_timeout(Duration::ZERO);
        });
        Self {
            #[cfg(windows)]
            bind_failed,
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
pub(crate) fn socket_path(dir: &Path) -> Result<PathBuf, Code> {
    let path = dir.join("agent.sock");
    std::os::unix::net::SocketAddr::from_pathname(&path).map_err(|_| Code::SocketPathTooLong)?;
    Ok(path)
}

#[cfg(unix)]
pub(crate) async fn connect(path: &Path) -> io::Result<tokio::net::UnixStream> {
    // Enabling access takes effect on the listener's next settings poll.
    let mut retries = 5;
    loop {
        match tokio::net::UnixStream::connect(path).await {
            Err(e) if retries > 0 && matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
                retries -= 1;
                tokio::time::sleep(SETTINGS_POLL).await;
            }
            result => return result,
        }
    }
}

#[cfg(unix)]
struct Socket {
    listener: UnixListener,
    path: PathBuf,
    _lock: File,
    generation: i64,
}

#[cfg(unix)]
impl Socket {
    async fn accept(&mut self) -> io::Result<(Stream, bool)> {
        let (stream, _) = self.listener.accept().await?;
        let same_user = stream.peer_cred().is_ok_and(|peer| peer.uid() == unsafe { libc::geteuid() });
        Ok((stream, same_user))
    }

    fn clear_stale(path: &Path) -> io::Result<File> {
        let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let lock = File::open(dir)?;
        lock.try_lock().map_err(io::Error::from)?;
        lock.set_permissions(fs::Permissions::from_mode(0o700))?;
        if let Ok(meta) = fs::symlink_metadata(path) {
            if !meta.file_type().is_socket() {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            match std::os::unix::net::UnixStream::connect(path) {
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path)?,
                _ => return Err(io::ErrorKind::AddrInUse.into()),
            }
        }
        Ok(lock)
    }

    fn bind(path: &Path, generation: i64) -> io::Result<Self> {
        let lock = Self::clear_stale(path)?;
        let socket = Self {
            listener: UnixListener::bind(path)?,
            path: path.into(),
            _lock: lock,
            generation,
        };
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(socket)
    }
}

#[cfg(unix)]
impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

async fn serve(
    path: PathBuf, bridge: Bridge, mut stop: oneshot::Receiver<()>,
    #[cfg(windows)] status: watch::Sender<bool>,
) {
    let Ok(settings) = Store::agent_writer(&path) else {
        return;
    };
    let Ok(socket_path) = socket_path(path.parent().unwrap()) else { return };
    #[cfg(unix)]
    if socket_path.exists() && settings.agent_enabled() == Ok(false) {
        let _ = Socket::clear_stale(&socket_path);
    }
    let mut socket: Option<Socket> = None;
    let mut calls = JoinSet::new();
    let permits = Arc::new(Semaphore::new(8));
    let rates = Rates::default();
    let pending = Arc::new(Mutex::new(std::collections::BTreeMap::<i64, i64>::new()));
    let (wake, mut wakes) = tokio::sync::mpsc::channel(1);
    let recorder = task::spawn_blocking({
        let path = path.clone();
        let pending = pending.clone();
        move || {
            while wakes.blocking_recv().is_some() {
                let counts = std::mem::take(&mut *pending.lock().unwrap());
                for (minute, count) in counts {
                    if Store::agent_writer(&path)
                        .and_then(|s| s.count_anonymous_refusals(minute, count))
                        .is_err()
                    {
                        eprintln!("Could not record agent admission refusals");
                    }
                }
            }
        }
    });
    let mut tick = interval(SETTINGS_POLL);
    let (mut enabled, _) = watch::channel(());
    let (policy, _) = watch::channel(settings.agent_policy_generation().ok());
    let mut bind_error = None;
    let mut recovered = false;
    loop {
        tokio::select! {
            biased;
            _ = &mut stop => break,
            _ = tick.tick() => {
                let revision = settings.agent_policy_generation().ok();
                policy.send_if_modified(|old| { if *old == revision { false } else { *old=revision; true } });
                let generation = settings.agent_generation().ok().flatten();
                if socket.as_ref().is_some_and(|bound|Some(bound.generation)!=generation) {
                    socket.take();
                    enabled=watch::channel(()).0;
                }
                if let Some(generation)=generation.filter(|_|socket.is_none()) {
                    match Socket::bind(&socket_path,generation) {
                        Ok(bound) => {
                            if !recovered {
                                if settings.recover_agent_writes().is_err() { continue; }
                                recovered = true;
                            }
                            socket=Some(bound);
                            bind_error=None;
                        }
                        Err(e) if bind_error!=Some(e.kind()) => {
                            eprintln!("Could not open agent socket: {e}");
                            bind_error=Some(e.kind());
                        }
                        Err(_) => {}
                    }
                }
                #[cfg(windows)]
                {
                    let failed = generation.is_some() && bind_error.is_some();
                    if *status.borrow() != failed { status.send_replace(failed); }
                }
            }
            Some(_) = calls.join_next(), if !calls.is_empty() => {}
            accepted = async { socket.as_mut().unwrap().accept().await }, if socket.is_some() => {
                let Ok((stream,same_user)) = accepted else { continue };
                let permit = permits.clone().try_acquire_owned();
                let failure = if !same_user { Some(Code::Unauthorized) } else if permit.is_err() { Some(Code::Busy) } else { None };
                if let Some(code) = failure {
                    let minute = chrono::Utc::now().timestamp().div_euclid(60)*60;
                    *pending.lock().unwrap().entry(minute).or_default() += 1;
                    let _ = wake.try_send(());
                    let mut stream=stream;
                    let _ = timeout(Duration::from_millis(100),write_frame(&mut stream, &line(Response::Failure(code)))).await;
                } else {
                    calls.spawn(call(stream,path.clone(),bridge.clone(),rates.clone(),enabled.subscribe(),socket.as_ref().unwrap().generation,policy.subscribe(),permit.unwrap()));
                }
            }
        }
    }
    drop(socket);
    drop(enabled);
    drop(wake);
    let _ = recorder.await;
    while calls.join_next().await.is_some() {}
}

#[cfg(unix)]
pub(crate) async fn write_frame(stream: &mut Stream, bytes: &[u8]) -> io::Result<()> {
    tokio::io::AsyncWriteExt::write_all(stream, bytes).await
}

async fn frame(stream: &mut Stream) -> Result<Request, Code> {
    let mut buf = zeroize::Zeroizing::new(Vec::new());
    let mut reader = BufReader::new(stream.take((MAX_REQUEST + 1) as u64));
    reader
        .read_until(b'\n', &mut buf)
        .await
        .map_err(|_| Code::InvalidRequest)?;
    if buf.len() > MAX_REQUEST {
        return Err(Code::TooLarge);
    }
    if buf.pop() != Some(b'\n') {
        return Err(Code::InvalidRequest);
    }
    serde_json::from_slice(&buf).map_err(|_| Code::InvalidRequest)
}

#[derive(Default)]
struct Audit {
    account: Option<String>,
    target: Option<AuditTarget>,
    dispatched: bool,
    worker_audit: bool,
    recipients: Vec<Address>,
}

impl Audit {
    fn record(
        &self,
        store: &Store,
        req: Option<&Request>,
        who: Option<&Identity>,
        outcome: Result<(), Failure>,
    ) -> Result<(), Code> {
        let verified_refusal = match &outcome {
            Err(Failure::VerifiedRefusal { .. } | Failure::VerifiedBusy { .. }) => true,
            Err(Failure::Error(Code::Busy | Code::NotFound) | Failure::OutsideScope) => who.is_some(),
            _ => false,
        };
        store.record_agent_audit(AuditRow {
            identity: who,
            account: self.account.as_deref(),
            command: req.filter(|r| self.dispatched || (verified_refusal && r.cmd != "remote.refused"))
                .map_or("", |r| &r.cmd),
            target: self.target.as_ref(),
            outcome,
            source_ip: req.and_then(|r| r.source_ip.as_ref()?.parse().ok()),
            recipients: &self.recipients,
        })
    }
}

fn admit(rates: &Rates, token_id: &str, write: bool) -> Result<tokio::sync::OwnedSemaphorePermit, Code> {
    let mut rates = rates.lock().map_err(|_| Code::Internal)?;
    let now = Instant::now();
    rates.retain(|_, limit| {
        limit.permits.available_permits() < MAX_PER_TOKEN || limit.times
            .back()
            .is_some_and(|last| now.duration_since(last.0) < Duration::from_secs(60))
    });
    let limit = rates.entry(token_id.to_owned()).or_insert_with(|| TokenLimit {
        times: VecDeque::new(),
        permits: Arc::new(Semaphore::new(MAX_PER_TOKEN)),
    });
    let times = &mut limit.times;
    while times
        .front()
        .is_some_and(|at| now.duration_since(at.0) >= Duration::from_secs(60))
    {
        times.pop_front();
    }
    if times.len() >= 300 || write && times.iter().filter(|(_, w)| *w).count() >= 30 {
        return Err(Code::Busy);
    }
    times.push_back((now, write));
    limit.permits.clone().try_acquire_owned().map_err(|_| Code::Busy)
}

fn line(response: Response) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&response).expect("wire response serializes");
    bytes.push(b'\n');
    bytes
}

async fn call(
    mut stream: Stream,
    path: PathBuf,
    bridge: Bridge,
    rates: Rates,
    mut off: watch::Receiver<()>,
    generation: i64,
    policy: watch::Receiver<Option<i64>>,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let parsed = tokio::select! {
        biased;
        _ = off.changed() => Err(Code::Unavailable),
        parsed = timeout(Duration::from_secs(5),frame(&mut stream)) => parsed.unwrap_or(Err(Code::InvalidRequest)),
    };
    let confirming = parsed
        .as_ref()
        .is_ok_and(|r| r.cmd == "draft.send" || r.cmd.starts_with("event."));
    let deadline = tokio::time::Instant::now()
        + if confirming {
            Duration::from_secs(130)
        } else {
            ANSWER
        };
    let writing = parsed.as_ref().is_ok_and(|r| write::handles(&r.cmd));
    let peer = if confirming {
        clone_peer(&stream).ok().map(Arc::new)
    } else {
        None
    };
    let (cancel, cancelled) = watch::channel(());
    let mut job = task::spawn_blocking(move || {
        process(
            &path, parsed, &bridge, &rates, cancelled, deadline, generation, peer, policy,
        )
    });
    let mut extra = [0u8; 1];
    let (bytes, _token_permit) = tokio::select! {
        biased;
        _ = off.changed() => {
            drop(stream); drop(cancel);
            let _ = job.await;
            return;
        },
        _ = stream.read(&mut extra), if writing => {
            drop(cancel);
            let _=job.await;
            return;
        }
        result = timeout_at(deadline,&mut job) => match result {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => (line(Response::Failure(Code::Internal)), None),
            Err(_) => {
                drop(stream); drop(cancel);
                let _ = job.await;
                return;
            },
        }
    };
    drop(cancel);
    tokio::select! {
        biased;
        _ = off.changed() => {},
        _ = timeout_at(deadline,write_frame(&mut stream, &bytes)) => {},
    }
}

fn process(
    path: &Path,
    parsed: Result<Request, Code>,
    bridge: &Bridge,
    rates: &Rates,
    off: watch::Receiver<()>,
    deadline: tokio::time::Instant,
    generation: i64,
    peer: Option<Arc<Peer>>,
    policy: watch::Receiver<Option<i64>>,
) -> (Vec<u8>, Option<tokio::sync::OwnedSemaphorePermit>) {
    let mut permit = None;
    let mut handled_remote_refusal = false;
    let access = Arc::new(std::sync::OnceLock::<read::ReadPolicy>::new());
    let mut who = None;
    let mut audit = Audit::default();
    let cancelled = || off.has_changed().is_err() || tokio::time::Instant::now() >= deadline;
    let mut result = (|| -> read::Result<Vec<u8>> {
        let req = parsed.as_ref().map_err(|e| *e)?;
        let stop = off.clone();
        let query_deadline = if matches!(req.cmd.as_str(), "emails" | "search") {
            deadline.min(tokio::time::Instant::now() + Duration::from_secs(2))
        } else {
            deadline
        };
        let active = access.clone();
        let mut changes = policy.clone();
        let db_path = path.to_owned();
        let store = Store::agent_reader(path, move || {
            if stop.has_changed().is_err() || tokio::time::Instant::now() >= query_deadline {
                return true;
            }
            active.get().is_some_and(|grant| {
                if grant.expired() {
                    return true;
                }
                if matches!(changes.has_changed(), Ok(false)) {
                    return false;
                }
                changes.borrow_and_update();
                Store::agent_reader(&db_path, || false)
                    .map_or(true, |fresh| grant.check(&fresh).is_err())
            })
        })?;
        let identity = match store.authenticate(
            req.token.as_deref().map(String::as_str),
            req.source_ip.as_deref(),
        ) {
            Ok(identity) => who.insert(identity),
            Err(e) => {
                if let Failure::VerifiedRefusal { agent, token, .. } = &e {
                    match admit(rates, token, false).map(drop) {
                        Err(Code::Busy) => {
                            return Err(Failure::VerifiedBusy {
                                agent: agent.clone(),
                                token: token.clone(),
                            })
                        }
                        Err(code) => return Err(code.into()),
                        Ok(()) => {}
                    }
                }
                return Err(e);
            }
        };
        permit = Some(admit(rates, identity.token_id(), write::handles(&req.cmd))?);
        if store.agent_generation()? != Some(generation) {
            return Err(Code::Unauthorized.into());
        }
        if req.v != VERSION {
            return Err(Code::Unsupported.into());
        }
        if req.cmd == "remote.refused" {
            handled_remote_refusal = true;
            return Err(Code::InvalidRequest.into());
        }
        let value = if write::handles(&req.cmd) {
            write::dispatch(
                &store,
                identity,
                req,
                bridge,
                generation,
                &mut audit,
                off.clone(),
                peer.as_ref(),
            )?
        } else {
            read::dispatch(
                &store,
                identity,
                req,
                |request| bridge.send(crate::sync::Cmd::AgentAttachment(request)),
                &mut audit.account,
                &mut audit.target,
                &access,
                off.clone(),
                policy.clone(),
            )?
        };
        let mut bytes =
            serde_json::to_vec(&Response::Success(value)).map_err(|_| Code::Internal)?;
        if bytes.len() >= MAX_RESPONSE {
            return Err(Code::TooLarge.into());
        }
        bytes.push(b'\n');
        Ok(bytes)
    })();
    if audit.worker_audit { return (result.unwrap_or_else(|e| line(Response::Failure(e.into()))), permit); }
    let audited = Store::agent_writer(path).and_then(|store| {
        store.agent_audit_transaction(|| {
            if let Some(grant) = access.get() {
                if let Err(e) = grant.check(&store) {
                    result = Err(e);
                }
            }
            if !audit.dispatched && cancelled() {
                result = Err(Code::Unavailable.into());
            }
            audit.dispatched |= handled_remote_refusal || access.get().is_some();
            audit.record(
                &store,
                parsed.as_ref().ok(),
                who.as_ref(),
                result.as_ref().map(|_| ()).map_err(Clone::clone),
            )
        })
    });
    let bytes = audited
        .map_err(Failure::from)
        .and(result)
        .unwrap_or_else(|e| line(Response::Failure(e.into())));
    (bytes, permit)
}
pub mod detect;

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn four_calls_per_token_leave_room_for_another_token() {
        let rates = Rates::default();
        let mut held: Vec<_> = (0..4).map(|_| admit(&rates, "one", false).unwrap()).collect();
        assert!(matches!(admit(&rates, "one", false), Err(Code::Busy)));
        let _other = admit(&rates, "two", false).unwrap();
        held.pop();
        assert!(admit(&rates, "one", false).is_ok());
        for _ in 6..300 {
            drop(admit(&rates, "one", false).unwrap());
        }
        assert!(matches!(admit(&rates, "one", false), Err(Code::Busy)));
    }
}

#[test]
fn audit_checks_the_stored_non_uuid_account_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("mail.db")).unwrap();
    let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
    let mut account = lagom_core::account::Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    account.id = "imported account".into();
    store.save_account(&account).unwrap();
    let audit = Audit { account: Some(account.id.clone()), ..Default::default() };
    audit.record(&store, None, None, Ok(())).unwrap();
    assert_eq!(store.agent_audit(1).unwrap()[0].account.as_deref(), Some(account.id.as_str()));
    store.delete_account(&account.id).unwrap();
    audit.record(&store, None, None, Ok(())).unwrap();
    assert!(store.agent_audit(1).unwrap().is_empty());
}
