use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::mem;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_channel::Receiver;
use async_channel::Sender;
use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::channel::oneshot;
use futures::future;
use futures::future::Shared;
use futures::stream::FuturesUnordered;
use git::Oid;
use git::repository::CommitData;
use git::repository::FileHistoryChangedFileSets;
use git::repository::GitRepository;
use git::repository::InitialGraphCommitData;
use git::repository::LogOrder;
use git::repository::LogSource;
use git::repository::RepoPath;
use git::repository::SearchCommitArgs;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::BackgroundExecutor;
use gpui::Context;
use gpui::SharedString;
use gpui::Task;
use gpui::WeakEntity;
use rustc_hash::FxHashMap;
use util::ResultExt as _;

use super::Repository;
use super::RepositoryEvent;
use super::RepositoryState;

#[derive(Clone)]
pub enum CommitDataState {
    Loading(Option<Shared<oneshot::Receiver<Arc<CommitData>>>>),
    Loaded(Arc<CommitData>),
}

pub(super) struct CommitDataHandler {
    _task: Task<()>,
    commit_data_request: Sender<Oid>,
    completion_senders: FxHashMap<Oid, oneshot::Sender<Arc<CommitData>>>,
    pending_requests: HashSet<Oid>,
}

/// Represents the handler of a git cat-file --batch process within ed
/// It's used to lazily fetch commit data as needed (whatever a user is viewing).
pub(super) enum CommitDataHandlerState {
    /// The handler is open and processing requests.
    Open(CommitDataHandler),
    /// The handler closed because it didn't receive any requests in the last 10s
    /// or hasn't been open before.
    Closed,
}

pub struct InitialGitGraphData {
    fetch_task: Task<()>,
    pub error: Option<SharedString>,
    pub commit_data: Vec<Arc<InitialGraphCommitData>>,
    pub commit_oid_to_index: FxHashMap<Oid, usize>,
    subscribers: Vec<Sender<Result<Vec<Arc<InitialGraphCommitData>>, SharedString>>>,
}

pub struct GraphDataResponse<'a> {
    pub commits: &'a [Arc<InitialGraphCommitData>],
    pub is_loading: bool,
    pub error: Option<SharedString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitGraphEvent {
    CountUpdated(usize),
    FullyLoaded,
    LoadingError,
}

impl Repository {
    pub fn file_history_changed_files(
        &mut self,
        paths: Vec<RepoPath>,
        commit_limit: usize,
    ) -> oneshot::Receiver<Result<Vec<FileHistoryChangedFileSets>>> {
        self.send_job(None, move |git_repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = git_repo;
                backend.file_history_changed_files(paths, commit_limit).await
            }
        })
    }

    pub fn get_graph_data(&self, log_source: LogSource, log_order: LogOrder) -> Option<&InitialGitGraphData> {
        self.initial_graph_data.get(&(log_source, log_order))
    }

    /// Drops the cached commits of one graph so the next request reads them
    /// from git again.
    pub fn forget_graph_data(&mut self, log_source: &LogSource, log_order: LogOrder) {
        self.initial_graph_data.remove(&(log_source.clone(), log_order));
    }

    pub fn search_commits(
        &mut self,
        log_source: LogSource,
        search_args: SearchCommitArgs,
        request_tx: Sender<Oid>,
        cx: &mut Context<Self>,
    ) {
        let repository_state = self.repository_state.clone();

        cx.background_spawn(async move {
            let repo_state = repository_state.await;

            match repo_state {
                Ok(RepositoryState { backend, .. }) => {
                    backend.search_commits(log_source, search_args, request_tx).await.log_err();
                }

                Err(error) => {
                    log::error!("failed to get repository state for commit search: {error}");
                }
            }
        })
        .detach();
    }

    pub fn graph_data(
        &mut self,
        log_source: LogSource,
        log_order: LogOrder,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> GraphDataResponse<'_> {
        let initial_commit_data = self.initial_graph_data.entry((log_source.clone(), log_order)).or_insert_with(|| {
            let state = self.repository_state.clone();
            let log_source = log_source.clone();

            let fetch_task = cx.spawn(async move |repository, cx| {
                let state = state.await;
                let result = match state {
                    Ok(RepositoryState { backend, .. }) => {
                        Self::local_git_graph_data(repository.clone(), backend, log_source.clone(), log_order, cx).await
                    }
                    Err(e) => Err(SharedString::from(e)),
                };

                repository
                    .update(cx, |repository, cx| {
                        if let Some(data) = repository.initial_graph_data.get_mut(&(log_source.clone(), log_order)) {
                            match &result {
                                Ok(()) => {
                                    cx.emit(RepositoryEvent::GraphEvent(
                                        (log_source.clone(), log_order),
                                        GitGraphEvent::FullyLoaded,
                                    ));
                                }
                                Err(fetch_task_error) => {
                                    data.subscribers
                                        .retain(|sender| sender.try_send(Err(fetch_task_error.clone())).is_ok());
                                    data.error = Some(fetch_task_error.clone());
                                    cx.emit(RepositoryEvent::GraphEvent(
                                        (log_source.clone(), log_order),
                                        GitGraphEvent::LoadingError,
                                    ));
                                }
                            }
                            data.subscribers.clear();
                        } else {
                            log::error!("This task would be dropped if this entry doesn't exist");
                        }
                    })
                    .log_err();
            });

            InitialGitGraphData {
                fetch_task,
                error: None,
                commit_data: Vec::new(),
                commit_oid_to_index: FxHashMap::default(),
                subscribers: Vec::new(),
            }
        });

        let max_start = initial_commit_data.commit_data.len().saturating_sub(1);
        let max_end = initial_commit_data.commit_data.len();

        GraphDataResponse {
            commits: &initial_commit_data.commit_data[range.start.min(max_start)..range.end.min(max_end)],
            is_loading: !initial_commit_data.fetch_task.is_ready(),
            error: initial_commit_data.error.clone(),
        }
    }

    fn append_initial_graph_commits(
        this: &WeakEntity<Self>,
        graph_data_key: &(LogSource, LogOrder),
        initial_graph_commit_data: Vec<Arc<InitialGraphCommitData>>,
        cx: &mut AsyncApp,
    ) {
        this.update(cx, |repository, cx| {
            let graph_data = repository.initial_graph_data.entry(graph_data_key.clone()).and_modify(|graph_data| {
                graph_data.subscribers.retain(|sender| sender.try_send(Ok(initial_graph_commit_data.clone())).is_ok());

                for commit_data in initial_graph_commit_data {
                    graph_data.commit_oid_to_index.insert(commit_data.sha, graph_data.commit_data.len());
                    graph_data.commit_data.push(commit_data);
                }
                cx.emit(RepositoryEvent::GraphEvent(
                    graph_data_key.clone(),
                    GitGraphEvent::CountUpdated(graph_data.commit_data.len()),
                ));
            });

            match &graph_data {
                Entry::Occupied(_) => {}
                Entry::Vacant(_) => {
                    log::error!("This task should be dropped if data doesn't exist");
                }
            }
        })
        .log_err();
    }

    async fn local_git_graph_data(
        this: WeakEntity<Self>,
        backend: Arc<dyn GitRepository>,
        log_source: LogSource,
        log_order: LogOrder,
        cx: &mut AsyncApp,
    ) -> Result<(), SharedString> {
        let (request_tx, request_rx) = async_channel::unbounded::<Vec<Arc<InitialGraphCommitData>>>();

        let task = cx.background_executor().spawn({
            let log_source = log_source.clone();
            async move {
                backend
                    .initial_graph_data(log_source, log_order, request_tx)
                    .await
                    .map_err(|err| SharedString::from(err.to_string()))
            }
        });

        let graph_data_key = (log_source, log_order);

        while let Ok(initial_graph_commit_data) = request_rx.recv().await {
            Self::append_initial_graph_commits(&this, &graph_data_key, initial_graph_commit_data, cx);
        }

        task.await?;
        Ok(())
    }

    pub fn fetch_commit_data(&mut self, sha: Oid, await_result: bool, cx: &mut Context<Self>) -> &CommitDataState {
        if self.commit_data.contains_key(&sha) {
            let data = &self.commit_data[&sha];

            if matches!(data, CommitDataState::Loading(None)) && await_result {
                let (tx, rx) = oneshot::channel();
                self.commit_data.insert(sha, CommitDataState::Loading(Some(rx.shared())));

                let handler = self.get_handler(cx);
                handler.completion_senders.insert(sha, tx);
            }

            return &self.commit_data[&sha];
        }

        let (state, completer) = if await_result {
            let (tx, rx) = oneshot::channel();
            (CommitDataState::Loading(Some(rx.shared())), Some(tx))
        } else {
            (CommitDataState::Loading(None), None)
        };

        self.commit_data.insert(sha, state);

        let handler = self.get_handler(cx);
        if let Some(tx) = completer {
            handler.completion_senders.insert(sha, tx);
        }
        if handler.commit_data_request.try_send(sha).is_ok() {
            handler.pending_requests.insert(sha);
        } else {
            handler.completion_senders.remove(&sha);
        }

        self.commit_data.get(&sha).unwrap_or_else(|| &CommitDataState::Loading(None))
    }

    fn get_handler(&mut self, cx: &Context<Self>) -> &mut CommitDataHandler {
        if matches!(self.commit_data_handler, CommitDataHandlerState::Closed) {
            self.commit_data_handler = CommitDataHandlerState::Open(self.open_commit_data_handler(cx));
        }

        match &mut self.commit_data_handler {
            CommitDataHandlerState::Open(handler) => handler,
            CommitDataHandlerState::Closed => unreachable!(),
        }
    }

    fn open_commit_data_handler(&self, cx: &Context<Self>) -> CommitDataHandler {
        let state = self.repository_state.clone();
        let (result_tx, result_rx) = async_channel::bounded::<(Oid, CommitData)>(64);
        let (request_tx, request_rx) = async_channel::unbounded::<Oid>();

        let foreground_task = cx.spawn(async move |this, cx| {
            while let Ok((sha, commit_data)) = result_rx.recv().await {
                let result = this.update(cx, |this, cx| {
                    let data = Arc::new(commit_data);

                    if let CommitDataHandlerState::Open(handler) = &mut this.commit_data_handler {
                        handler.pending_requests.remove(&sha);
                        if let Some(completion_sender) = handler.completion_senders.remove(&sha) {
                            completion_sender.send(Arc::clone(&data)).ok();
                        }
                    } else {
                        log::error!("The handler state has to be open for this task to exist");
                    }

                    this.commit_data.insert(sha, CommitDataState::Loaded(data));

                    cx.notify();
                });
                if result.is_err() {
                    break;
                }
            }

            this.update(cx, |this, _cx| {
                let CommitDataHandlerState::Open(handler) =
                    mem::replace(&mut this.commit_data_handler, CommitDataHandlerState::Closed)
                else {
                    log::error!("The handler state has to be open for this task to exist");
                    return;
                };

                for sha in handler.pending_requests {
                    this.commit_data.remove(&sha);
                }
            })
            .ok();
        });

        let request_tx_for_handler = request_tx;
        let background_executor = cx.background_executor().clone();

        cx.background_spawn(async move {
            match state.await {
                Ok(RepositoryState { backend, .. }) => {
                    Self::local_commit_data_reader(backend, request_rx, result_tx, background_executor).await;
                }
                Err(error) => {
                    log::error!("failed to get repository state: {error}");
                }
            }
        })
        .detach();

        CommitDataHandler {
            _task: foreground_task,
            commit_data_request: request_tx_for_handler,
            completion_senders: FxHashMap::default(),
            pending_requests: HashSet::default(),
        }
    }

    async fn local_commit_data_reader(
        backend: Arc<dyn GitRepository>,
        request_rx: Receiver<Oid>,
        result_tx: Sender<(Oid, CommitData)>,
        background_executor: BackgroundExecutor,
    ) {
        async fn receive_commit_data_request(request_rx: &Receiver<Oid>) -> Option<Oid> {
            if request_rx.is_closed() && request_rx.is_empty() {
                future::pending().await
            } else {
                request_rx.recv().await.ok()
            }
        }

        let reader = match backend.commit_data_reader() {
            Ok(reader) => reader,
            Err(error) => {
                log::error!("failed to create commit data reader: {error:?}");
                return;
            }
        };

        let read_commit_data = |sha| reader.read(sha).map(move |result| (sha, result));
        let mut read_futures = FuturesUnordered::new();

        loop {
            if read_futures.is_empty() {
                let timeout = background_executor.timer(Duration::from_secs(10));

                futures::select_biased! {
                    sha = futures::FutureExt::fuse(receive_commit_data_request(&request_rx)) => {
                        if let Some(sha) = sha {
                            read_futures.push(read_commit_data(sha));
                        }
                    }
                    () = futures::FutureExt::fuse(timeout) => {
                        break;
                    }
                }
            }

            let next_read = read_futures.next().fuse();
            futures::pin_mut!(next_read);

            futures::select_biased! {
                result = next_read => {
                    let Some((sha, result)) = result else {
                        continue;
                    };

                    match result {
                        Ok(commit_data) => {
                            if result_tx.send((sha, commit_data)).await.is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            log::error!("failed to read commit data for {sha}: {error:?}");
                        }
                    }
                }
                sha = futures::FutureExt::fuse(receive_commit_data_request(&request_rx)) => {
                    if let Some(sha) = sha {
                        read_futures.push(read_commit_data(sha));
                    }
                }
            }
        }

        drop(result_tx);
    }
}
