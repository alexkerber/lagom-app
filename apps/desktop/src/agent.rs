//! The listener belongs to the app, including while its main window is closed.
pub(crate) mod read;

use crate::sync::Bridge;
use lagom_core::{
    agent::{
        audit::{AuditRow, Failure},
        id::ResolvedMessage,
        protocol::{Code, Request, Response, VERSION},
        token::Identity,
    },
    store::Store,
};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File},
    io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{oneshot, watch, Semaphore},
    task::{self, JoinSet},
    time::{interval, timeout, timeout_at},
};

const MAX_REQUEST: usize = 1024 * 1024;
const MAX_RESPONSE: usize = 32 * 1024 * 1024;
const ANSWER: Duration = Duration::from_secs(30);
type Rates = Arc<Mutex<HashMap<String, VecDeque<Instant>>>>;

pub(crate) struct Server {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn start(path: &Path, bridge: Bridge) -> Self {
        let path = path.to_owned();
        let (stop, rx) = oneshot::channel();
        let thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("agent runtime");
            rt.block_on(serve(path, bridge, rx));
            rt.shutdown_timeout(Duration::ZERO);
        });
        Self {
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

struct Socket {
    listener: UnixListener,
    path: PathBuf,
    _lock: File,
    generation: i64,
}

impl Socket {
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

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

async fn serve(path: PathBuf, bridge: Bridge, mut stop: oneshot::Receiver<()>) {
    let Ok(settings) = Store::agent_writer(&path) else {
        return;
    };
    let socket_path = path.with_file_name("agent.sock");
    if socket_path.exists() && settings.agent_enabled() == Ok(false) {
        let _ = Socket::clear_stale(&socket_path);
    }
    let mut socket: Option<Socket> = None;
    let mut calls = JoinSet::new();
    let permits = Arc::new(Semaphore::new(8));
    let rates = Rates::default();
    let mut tick = interval(Duration::from_millis(100));
    let (mut enabled, _) = watch::channel(());
    let mut bind_error = None;
    loop {
        tokio::select! {
            biased;
            _ = &mut stop => break,
            _ = tick.tick() => {
                let generation = settings.agent_generation().ok().flatten();
                if socket.as_ref().is_some_and(|bound|Some(bound.generation)!=generation) {
                    socket.take();
                    enabled=watch::channel(()).0;
                }
                if let Some(generation)=generation.filter(|_|socket.is_none()) {
                    match Socket::bind(&socket_path,generation) {
                        Ok(bound) => { socket=Some(bound); bind_error=None; }
                        Err(e) if bind_error!=Some(e.kind()) => {
                            eprintln!("Could not open agent socket: {e}");
                            bind_error=Some(e.kind());
                        }
                        Err(_) => {}
                    }
                }
            }
            Some(_) = calls.join_next(), if !calls.is_empty() => {}
            accepted = async { socket.as_ref().unwrap().listener.accept().await }, if socket.is_some() => {
                let Ok((stream,_)) = accepted else { continue };
                // SAFETY: geteuid has no arguments or preconditions.
                let same_user = stream.peer_cred().is_ok_and(|p|p.uid()==unsafe {libc::geteuid()});
                let permit = permits.clone().try_acquire_owned();
                let failure = if !same_user { Some(Code::Unauthorized) } else if permit.is_err() { Some(Code::Busy) } else { None };
                if let Some(code) = failure {
                    let code = audit(&settings,None,None,None,None,Err(code.into())).err().unwrap_or(code);
                    let mut stream=stream;
                    let _ = timeout(Duration::from_millis(100),stream.write_all(&line(Response::Failure(code)))).await;
                } else {
                    calls.spawn(call(stream,path.clone(),bridge.clone(),rates.clone(),enabled.subscribe(),socket.as_ref().unwrap().generation,permit.unwrap()));
                }
            }
        }
    }
    drop(socket);
    drop(enabled);
    while calls.join_next().await.is_some() {}
}

async fn frame(stream: &mut UnixStream) -> Result<Request, Code> {
    let mut buf = Vec::new();
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

fn audit(
    store: &Store,
    req: Option<&Request>,
    who: Option<&Identity>,
    account: Option<&str>,
    target: Option<&ResolvedMessage>,
    outcome: Result<(), Failure>,
) -> Result<(), Code> {
    store.record_agent_audit(AuditRow {
        identity: who,
        account: account.and_then(|s| s.parse().ok()),
        command: req.map_or("", |r| &r.cmd),
        target,
        outcome,
        source_ip: req.and_then(|r| r.source_ip.as_ref()?.parse().ok()),
        recipients: &[],
    })
}

fn rate(rates: &Rates, who: &Identity) -> Result<(), Code> {
    let mut rates = rates.lock().map_err(|_| Code::Internal)?;
    let now = Instant::now();
    rates.retain(|_, times| {
        times
            .back()
            .is_some_and(|last| now.duration_since(*last) < Duration::from_secs(60))
    });
    let times = rates.entry(who.token_id().to_owned()).or_default();
    while times
        .front()
        .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
    {
        times.pop_front();
    }
    if times.len() >= 300 {
        return Err(Code::Busy);
    }
    times.push_back(now);
    Ok(())
}

fn line(response: Response) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&response).expect("wire response serializes");
    bytes.push(b'\n');
    bytes
}

async fn call(
    mut stream: UnixStream,
    path: PathBuf,
    bridge: Bridge,
    rates: Rates,
    mut off: watch::Receiver<()>,
    generation: i64,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let parsed = tokio::select! {
        biased;
        _ = off.changed() => Err(Code::Unavailable),
        parsed = timeout(Duration::from_secs(5),frame(&mut stream)) => parsed.unwrap_or(Err(Code::InvalidRequest)),
    };
    let deadline = tokio::time::Instant::now() + ANSWER;
    let (cancel, cancelled) = watch::channel(());
    let mut job = task::spawn_blocking(move || {
        process(
            &path, parsed, &bridge, &rates, cancelled, deadline, generation,
        )
    });
    let bytes = tokio::select! {
        biased;
        _ = off.changed() => {
            drop(stream); drop(cancel);
            let _ = job.await;
            return;
        },
        result = timeout_at(deadline,&mut job) => match result {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => line(Response::Failure(Code::Internal)),
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
        _ = timeout_at(deadline,stream.write_all(&bytes)) => {},
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
) -> Vec<u8> {
    let mut who = None;
    let mut account = None;
    let mut target = None;
    let cancelled = || off.has_changed().is_err() || tokio::time::Instant::now() >= deadline;
    let mut result = (|| -> read::Result<Vec<u8>> {
        let req = parsed.as_ref().map_err(|e| *e)?;
        let stop = off.clone();
        let store = Store::agent_reader(path, move || {
            stop.has_changed().is_err() || tokio::time::Instant::now() >= deadline
        })?;
        let identity =
            who.insert(store.authenticate(req.token.as_deref().map(String::as_str), req.source_ip.as_deref())?);
        if store.agent_generation()? != Some(generation) {
            return Err(Code::Unauthorized.into());
        }
        rate(rates, identity)?;
        if req.v != VERSION {
            return Err(Code::Unsupported.into());
        }
        let value = read::dispatch(
            &store,
            identity,
            req,
            |request| bridge.send(crate::sync::Cmd::AgentAttachment(request)),
            &mut account,
            &mut target,
            off.clone(),
        )?;
        let mut bytes =
            serde_json::to_vec(&Response::Success(value)).map_err(|_| Code::Internal)?;
        if bytes.len() >= MAX_RESPONSE {
            return Err(Code::TooLarge.into());
        }
        bytes.push(b'\n');
        Ok(bytes)
    })();
    let audited = Store::agent_writer(path).and_then(|store| {
        if cancelled() || (result.is_ok() && store.agent_generation()? != Some(generation)) {
            result = Err(Code::Unavailable.into());
        }
        audit(
            &store,
            parsed.as_ref().ok(),
            who.as_ref(),
            account.as_deref(),
            target.as_ref(),
            result.as_ref().map(|_| ()).map_err(|e| *e),
        )
    });
    audited
        .map_err(Failure::from)
        .and(result)
        .unwrap_or_else(|e| line(Response::Failure(e.into())))
}
