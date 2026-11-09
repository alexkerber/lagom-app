use super::*;

pub(super) enum TaskCmd {
    Edited,
    Sync(Account),
    Pause(String),
    Resume(String),
}

pub(super) struct TaskWorker {
    store: Store,
    tasks: tokio::task::JoinSet<(String, lagom_core::Result<()>)>,
    task_jobs: HashMap<String, (tokio::task::AbortHandle, bool)>,
    task_debounce: Option<tokio::time::Instant>,
    paused: HashSet<String>,
    tick: tokio::time::Interval,
    me: UnboundedSender<Cmd>,
    events: UnboundedSender<Event>,
    #[cfg(test)]
    pub(super) endpoint: Option<String>,
}

impl TaskWorker {
    pub(super) fn new(store: Store, me: UnboundedSender<Cmd>, events: UnboundedSender<Event>) -> Self {
        let mut tick = tokio::time::interval(SYNC_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self { store, tasks: tokio::task::JoinSet::new(), task_jobs: HashMap::new(), task_debounce: None,
            paused: HashSet::new(), tick, me, events, #[cfg(test)] endpoint: None }
    }

    fn emit(&self, event: Event) { let _ = self.events.send(event); }

    pub(super) fn pause(&mut self, id: &str) {
        self.paused.insert(id.into());
        if let Some((task, _)) = self.task_jobs.remove(id) { task.abort(); }
    }

    fn edited(&mut self) {
        self.task_debounce = Some(tokio::time::Instant::now() + Duration::from_millis(400));
    }

    pub(super) async fn step(&mut self, rx: &mut UnboundedReceiver<TaskCmd>) -> bool {
        tokio::select! {
            Some(result) = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => self.tasks_finished(result),
            _ = async {
                match self.task_debounce { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await }
            } => {
                self.task_debounce = None;
                for account in self.store.accounts().unwrap_or_default() { self.sync_tasks(&account); }
            },
            _ = self.tick.tick() => {
                for account in self.store.accounts().unwrap_or_default() { self.sync_tasks(&account); }
            },
            cmd = rx.recv() => match cmd {
                Some(TaskCmd::Edited) => self.edited(),
                Some(TaskCmd::Sync(account)) => self.sync_tasks(&account),
                Some(TaskCmd::Resume(id)) => { self.paused.remove(&id); },
                Some(TaskCmd::Pause(id)) => self.pause(&id),
                None => return false,
            },
        }
        true
    }

    fn sync_tasks(&mut self, account: &Account) {
        if self.paused.contains(&account.id)
            || account.auth != lagom_core::account::Auth::OAuth(Provider::Google)
            { return; }
        if let Some((_, again)) = self.task_jobs.get_mut(&account.id) {
            *again = true;
            return;
        }
        match self.store.account_scopes(&account.id) {
            Ok(scopes) if oauth::tasks_allowed(&scopes) => {}
            Ok(_) => return,
            Err(e) => { self.emit(Event::TasksSync(account.id.clone(), SyncState::Failed(e.to_string()))); return; }
        }
        let path = self.store.path().to_owned();
        let account = account.clone();
        let id = account.id.clone();
        #[cfg(test)]
        let endpoint = self.endpoint.clone();
        let task = self.tasks.spawn(async move {
            let result = match Store::open(path) {
                Ok(mut store) => {
                    #[cfg(test)]
                    if let Some(endpoint) = endpoint {
                        let result = lagom_core::google::sync_tasks_with(&mut store, &account.id, &endpoint,
                            |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await;
                        return (account.id, result);
                    }
                    lagom_core::google::sync_tasks(&mut store, &account).await
                }
                Err(e) => Err(e),
            };
            (account.id, result)
        });
        self.task_jobs.insert(id, (task, false));
    }

    fn tasks_finished(&mut self, completed: Result<(tokio::task::Id, (String, lagom_core::Result<()>)), tokio::task::JoinError>) {
        let task = match &completed { Ok((task, _)) => *task, Err(error) => error.id() };
        let Some(id) = self.task_jobs.iter().find(|(_, (handle, _))| handle.id() == task).map(|(id, _)| id.clone()) else { return };
        let (_, again) = self.task_jobs.remove(&id).unwrap();
        let Some(account) = self.store.accounts().unwrap_or_default().into_iter().find(|a| a.id == id) else { return };
        let result = match completed { Ok((_, (_, result))) => result, Err(_) => Err(Error::Network("Google Tasks sync task failed")) };
        if matches!(result, Err(Error::ReauthRequired)) {
            self.pause(&id);
            let _ = self.me.send(Cmd::TasksReauth(id.clone()));
        }
        self.emit(Event::TasksSync(id, match result { Ok(()) => SyncState::Synced(now()), Err(e) => SyncState::Failed(e.to_string()) }));
        self.emit(Event::Changed);
        if again { self.sync_tasks(&account); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn failed_account_removal_resumes_task_scheduling() {
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let mut account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        account.auth = lagom_core::account::Auth::OAuth(Provider::Google);
        let path = std::env::temp_dir().join(format!("lagom-task-removal-{}.db", account.id));
        let store = Store::open(&path).unwrap();
        store.save_authorized_account(&account, oauth::GOOGLE_TASKS_SCOPE).unwrap();
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let (task_tx, mut task_rx) = unbounded_channel();
        let mut worker = Worker { store, sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: Some(task_tx), reauth_required: HashSet::new(), discovery: None, me, events };
        let mut tasks = TaskWorker::new(Store::open(&path).unwrap(), worker.me.clone(), worker.events.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        tasks.endpoint = Some(format!("http://{}/", listener.local_addr().unwrap()));
        tasks.tick.tick().await;
        tasks.pause(&account.id);
        worker.remove_with(&account.id, |removing| {
            assert_eq!(removing.id, account.id);
            Err(Error::Secret)
        });
        while !task_rx.is_empty() { assert!(tasks.step(&mut task_rx).await); }
        assert_eq!(worker.store.accounts().unwrap(), [account.clone()]);
        assert!(matches!(out.try_recv().unwrap(), Event::Sync(id, SyncState::Failed(_)) if id == account.id));
        worker.sync_tasks(&account);
        assert!(tasks.step(&mut task_rx).await);
        assert!(tasks.task_jobs.contains_key(&account.id), "failed removal must allow manual task sync to start again");
        tasks.pause(&account.id);
        worker.remove_with(&account.id, |_| Ok(()));
        while !task_rx.is_empty() { assert!(tasks.step(&mut task_rx).await); }
        assert!(worker.store.accounts().unwrap().is_empty());
        assert!(tasks.paused.contains(&account.id), "successful removal must stay paused");
        drop(tasks);
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn task_edits_debounce_jobs_deduplicate_and_cancelled_results_stay_quiet() {
        let path = std::env::temp_dir().join(format!("lagom-task-jobs-{}-{}.db", std::process::id(), now()));
        let store = Store::open(&path).unwrap();
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = TaskWorker::new(store, me, events);
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let mut account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        account.auth = lagom_core::account::Auth::OAuth(Provider::Google);
        worker.store.save_authorized_account(&account, oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
        worker.edited();
        let first = worker.task_debounce.unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        worker.edited();
        assert!(worker.task_debounce.unwrap() > first);
        assert!(worker.tasks.is_empty());
        let id = account.id.clone();
        let handle = worker.tasks.spawn(async move { (id, Ok(())) });
        worker.task_jobs.insert(account.id.clone(), (handle, false));
        worker.sync_tasks(&account);
        assert_eq!(worker.tasks.len(), 1);
        assert!(worker.task_jobs[&account.id].1);
        worker.task_jobs.get_mut(&account.id).unwrap().1 = false;
        let completed = worker.tasks.join_next_with_id().await.unwrap();
        worker.tasks_finished(completed);
        assert!(matches!(out.try_recv().unwrap(), Event::TasksSync(_, SyncState::Synced(_))));
        assert!(matches!(out.try_recv().unwrap(), Event::Changed));
        let id = account.id.clone();
        let handle = worker.tasks.spawn(async move { (id, Err(Error::ReauthRequired)) });
        worker.task_jobs.insert(account.id.clone(), (handle, false));
        let stale = worker.tasks.join_next_with_id().await.unwrap();
        worker.pause(&account.id);
        worker.store.delete_account(&account.id).unwrap();
        worker.tasks_finished(stale);
        assert!(out.try_recv().is_err());
        assert!(worker.paused.contains(&account.id));
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }
}
