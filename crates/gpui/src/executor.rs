use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::mem;
use std::mem::ManuallyDrop;
use std::panic::Location;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use std::task::Wake;
use std::task::Waker;
use std::thread;
use std::thread::ThreadId;
use std::time::Duration;
use std::time::Instant;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_util::TryFutureExt as _;

use crate::App;
use crate::PlatformDispatcher;
use crate::RunnableVariant;

/// Task priority for background tasks, mapped to the platform's queue priorities.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Priority {
    /// Use for tasks critical to user experience/responsiveness.
    High,
    /// Suitable for most use cases.
    #[default]
    Medium,
    /// Use for background work that can be deprioritized.
    Low,
}

/// Metadata attached to runnables for debugging.
#[derive(Clone, Debug)]
pub struct RunnableMeta {
    /// The source location where the task was spawned.
    pub location: &'static Location<'static>,
}

/// Task is a primitive that allows work to happen in the background.
///
/// It implements [`Future`] so you can `.await` on it.
///
/// If you drop a task it will be cancelled immediately. Calling [`Task::detach`] allows
/// the task to continue running, but with no way to return a value.
#[must_use]
pub struct Task<T>(TaskState<T>);

enum TaskState<T> {
    Ready(Option<T>),
    Spawned(async_task::Task<T, RunnableMeta>),
}

impl<T> Task<T> {
    /// Creates a new task that will resolve with the value.
    pub const fn ready(val: T) -> Self {
        Task(TaskState::Ready(Some(val)))
    }

    /// Whether the task has finished running.
    pub fn is_ready(&self) -> bool {
        match &self.0 {
            TaskState::Ready(_) => true,
            TaskState::Spawned(task) => task.is_finished(),
        }
    }

    /// Detaching a task runs it to completion in the background.
    pub fn detach(self) {
        if let TaskState::Spawned(task) = self.0 {
            task.detach();
        }
    }
}

impl<T> Debug for Task<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            TaskState::Ready(_) => f.debug_tuple("Task::Ready").finish(),
            TaskState::Spawned(task) => f.debug_tuple("Task::Spawned").field(task).finish(),
        }
    }
}

impl<T> Unpin for Task<T> {}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        match &mut self.0 {
            TaskState::Ready(val) => Poll::Ready(val.take().expect("Task polled after completion")),
            TaskState::Spawned(task) => Pin::new(task).poll(cx),
        }
    }
}

/// A pointer to the executor that is currently running,
/// for spawning background tasks.
#[derive(Clone)]
pub struct BackgroundExecutor {
    dispatcher: Arc<dyn PlatformDispatcher>,
}

/// A pointer to the executor that is currently running,
/// for spawning tasks on the main thread.
#[derive(Clone)]
pub struct ForegroundExecutor {
    dispatcher: Arc<dyn PlatformDispatcher>,
    not_send: PhantomData<Rc<()>>,
}

/// Extension trait for `Task<Result<T, E>>` that adds `detach_and_log_err` with an `&App` context.
///
/// This trait is automatically implemented for all `Task<Result<T, E>>` types.
pub trait TaskExt<T, E> {
    /// Run the task to completion in the background and log any errors that occur.
    fn detach_and_log_err(self, cx: &App);
}

impl<T, E> TaskExt<T, E> for Task<Result<T, E>>
where
    T: 'static,
    E: 'static + Display,
{
    #[track_caller]
    fn detach_and_log_err(self, cx: &App) {
        let location = Location::caller();
        cx.foreground_executor().spawn(self.log_tracked_err(*location)).detach();
    }
}

impl BackgroundExecutor {
    /// Creates a new BackgroundExecutor from the given PlatformDispatcher.
    pub fn new(dispatcher: Arc<dyn PlatformDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Enqueues the given future to be run to completion on a background thread.
    #[track_caller]
    pub fn spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.spawn_with_priority(Priority::default(), future)
    }

    /// Enqueues the given future to be run to completion on a background thread with the given priority.
    #[track_caller]
    pub fn spawn_with_priority<R>(
        &self,
        priority: Priority,
        future: impl Future<Output = R> + Send + 'static,
    ) -> Task<R>
    where
        R: Send + 'static,
    {
        let dispatcher = Arc::clone(&self.dispatcher);
        let (runnable, task) = async_task::Builder::new()
            .metadata(RunnableMeta { location: Location::caller() })
            .spawn(move |_| future, move |runnable| dispatcher.dispatch(runnable, priority));
        runnable.schedule();
        Task(TaskState::Spawned(task))
    }

    /// Runs background tasks that may borrow from their environment and waits for all of them to complete.
    ///
    /// Dropping the returned future cancels its tasks and synchronously waits for their futures to
    /// be destroyed before returning.
    pub async fn scoped<'scope, F>(&self, scheduler: F)
    where
        F: FnOnce(&mut Scope<'scope>),
    {
        let mut scope = Scope::new(self.clone(), Priority::default());
        (scheduler)(&mut scope);
        let spawned = mem::take(&mut scope.futures)
            .into_iter()
            .map(|f| self.spawn_with_priority(scope.priority, f))
            .collect::<Vec<_>>();
        for task in spawned {
            task.await;
        }
    }

    /// Runs prioritized background tasks that may borrow from their environment and waits for all
    /// of them to complete.
    ///
    /// Dropping the returned future cancels its tasks and synchronously waits for their futures to
    /// be destroyed before returning.
    pub async fn scoped_priority<'scope, F>(&self, priority: Priority, scheduler: F)
    where
        F: FnOnce(&mut Scope<'scope>),
    {
        let mut scope = Scope::new(self.clone(), priority);
        (scheduler)(&mut scope);
        let spawned = mem::take(&mut scope.futures)
            .into_iter()
            .map(|f| self.spawn_with_priority(scope.priority, f))
            .collect::<Vec<_>>();
        for task in spawned {
            task.await;
        }
    }

    /// Get the current time.
    pub fn now(&self) -> Instant {
        Instant::now()
    }

    /// Returns a task that will complete after the given duration.
    /// Depending on other concurrent tasks the elapsed duration may be longer
    /// than requested.
    #[track_caller]
    pub fn timer(&self, duration: Duration) -> Task<()> {
        if duration.is_zero() {
            return Task::ready(());
        }
        // The future never pends, so the schedule callback runs exactly once: as the delayed dispatch.
        let dispatcher = Arc::clone(&self.dispatcher);
        let (runnable, task) = async_task::Builder::new()
            .metadata(RunnableMeta { location: Location::caller() })
            .spawn(|_| async {}, move |runnable| dispatcher.dispatch_after(duration, runnable));
        runnable.schedule();
        Task(TaskState::Spawned(task))
    }

    /// How many CPUs are available to the dispatcher.
    pub fn num_cpus(&self) -> usize {
        thread::available_parallelism().map_or(1, std::num::NonZero::get)
    }

    /// Whether we're on the main thread.
    pub fn is_main_thread(&self) -> bool {
        self.dispatcher.is_main_thread()
    }
}

impl ForegroundExecutor {
    /// Creates a new ForegroundExecutor from the given PlatformDispatcher.
    pub fn new(dispatcher: Arc<dyn PlatformDispatcher>) -> Self {
        Self { dispatcher, not_send: PhantomData }
    }

    /// Enqueues the given Task to run on the main thread. Tasks run in spawn order.
    #[track_caller]
    pub fn spawn<R>(&self, future: impl Future<Output = R> + 'static) -> Task<R>
    where
        R: 'static,
    {
        let dispatcher = Arc::clone(&self.dispatcher);
        let (runnable, task) = spawn_local_with_source_location(
            future,
            move |runnable| dispatcher.dispatch_on_main_thread(runnable),
            RunnableMeta { location: Location::caller() },
        );
        runnable.schedule();
        Task(TaskState::Spawned(task))
    }

    /// Block the current thread until the given future resolves.
    /// Consider using `block_with_timeout` instead.
    pub fn block_on<R>(&self, future: impl Future<Output = R>) -> R {
        let future = std::pin::pin!(future);
        block(future, None).expect("block_on future did not complete")
    }

    /// Block the current thread until the given future resolves or the timeout elapses.
    pub fn block_with_timeout<R, Fut: Future<Output = R>>(
        &self,
        duration: Duration,
        future: Fut,
    ) -> Result<R, impl Future<Output = R> + use<R, Fut>> {
        let mut future = Box::pin(future);
        block(future.as_mut(), Some(Instant::now() + duration)).ok_or(future)
    }
}

/// Polls `future` on the current thread, parking between wakes, until it completes or `deadline` passes.
pub(crate) fn block<F: Future>(mut future: Pin<&mut F>, deadline: Option<Instant>) -> Option<F::Output> {
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return Some(output);
        }
        // `park` may wake spuriously or from an unpark meant for an earlier
        // poll; either way the loop just polls again.
        match deadline {
            Some(deadline) => {
                let now = Instant::now();
                if deadline <= now {
                    return None;
                }
                thread::park_timeout(deadline - now);
            }
            None => thread::park(),
        }
    }
}

/// Wakes a thread blocked in [`block`].
struct ThreadWaker(thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Variant of `async_task::spawn_local` that includes the source location of the spawn in panics.
fn spawn_local_with_source_location<Fut, S>(
    future: Fut,
    schedule: S,
    metadata: RunnableMeta,
) -> (RunnableVariant, async_task::Task<Fut::Output, RunnableMeta>)
where
    Fut: Future + 'static,
    Fut::Output: 'static,
    S: async_task::Schedule<RunnableMeta> + Send + Sync + 'static,
{
    #[inline]
    fn thread_id() -> ThreadId {
        std::thread_local! {
            static ID: ThreadId = thread::current().id();
        }
        ID.try_with(|id| *id).unwrap_or_else(|_| thread::current().id())
    }

    struct Checked<F> {
        id: ThreadId,
        inner: ManuallyDrop<F>,
        location: &'static Location<'static>,
    }

    impl<F> Drop for Checked<F> {
        fn drop(&mut self) {
            assert_eq!(
                self.id,
                thread_id(),
                "local task dropped by a thread that didn't spawn it. Task spawned at {}",
                self.location
            );
            // SAFETY: `inner` is wrapped in `ManuallyDrop`, so this is the only
            // place it is dropped. The thread check above ensures local futures
            // are dropped on the thread that created them.
            unsafe {
                ManuallyDrop::drop(&mut self.inner);
            }
        }
    }

    impl<F: Future> Future for Checked<F> {
        type Output = F::Output;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            // SAFETY: We don't move any fields out of `self`; this mutable
            // reference is only used to check metadata and to project the pin to
            // `inner` below.
            let this = unsafe { self.get_unchecked_mut() };
            assert!(
                this.id == thread_id(),
                "local task polled by a thread that didn't spawn it. Task spawned at {}",
                this.location
            );
            // SAFETY: `inner` is structurally pinned by `Checked`; after
            // `Checked` is pinned, `inner` is never moved. The thread check
            // above ensures the local future is only polled by its spawning
            // thread.
            unsafe { Pin::new_unchecked(&mut *this.inner).poll(cx) }
        }
    }

    let location = metadata.location;
    let future = move |_| Checked { id: thread_id(), inner: ManuallyDrop::new(future), location };

    // SAFETY: `Checked` enforces the invariants required by `spawn_unchecked`:
    // the non-`Send` future is only polled and dropped on the thread that
    // spawned it.
    unsafe { async_task::Builder::new().metadata(metadata).spawn_unchecked(future, schedule) }
}

/// Scope manages a set of tasks that are enqueued and waited on together. See [`BackgroundExecutor::scoped`].
pub struct Scope<'a> {
    executor: BackgroundExecutor,
    priority: Priority,
    futures: Vec<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    tx: Option<mpsc::Sender<()>>,
    rx: mpsc::Receiver<()>,
    lifetime: PhantomData<&'a ()>,
}

impl<'a> Scope<'a> {
    fn new(executor: BackgroundExecutor, priority: Priority) -> Self {
        let (tx, rx) = mpsc::channel(1);
        Self { executor, priority, tx: Some(tx), rx, futures: Vec::default(), lifetime: PhantomData }
    }

    /// How many CPUs are available to the dispatcher.
    pub fn num_cpus(&self) -> usize {
        self.executor.num_cpus()
    }

    /// Spawn a future into this scope.
    #[track_caller]
    pub fn spawn<F>(&mut self, f: F)
    where
        F: Future<Output = ()> + Send + 'a,
    {
        let tx = self.tx.clone().expect("scope is not dropped");

        // SAFETY: The 'a lifetime is guaranteed to outlive any of these futures because
        // dropping this `Scope` blocks until all of the futures have resolved.
        let f = unsafe {
            mem::transmute::<
                Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
                Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
            >(Box::pin(async move {
                f.await;
                drop(tx);
            }))
        };
        self.futures.push(f);
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        self.tx.take().expect("scope is dropped once");

        // Wait until the channel is closed, which means that all of the spawned
        // futures have resolved.
        let future = async {
            self.rx.next().await;
        };
        block(std::pin::pin!(future), None);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use futures::channel::oneshot;

    use super::*;

    #[test]
    fn block_returns_ready_future() {
        assert_eq!(block(std::pin::pin!(async { 7 }), None), Some(7));
    }

    #[test]
    fn block_wakes_on_cross_thread_completion() {
        let (tx, rx) = oneshot::channel();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            tx.send(5).unwrap();
        });
        assert_eq!(block(std::pin::pin!(rx), None), Some(Ok(5)));
        sender.join().unwrap();
    }

    #[test]
    fn block_times_out_on_pending_future() {
        let start = Instant::now();
        let result = block(std::pin::pin!(std::future::pending::<()>()), Some(start + Duration::from_millis(20)));
        assert_eq!(result, None);
        assert!(start.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn util_block_on_completes() {
        assert_eq!(crate::util::block_on(async { 3 }), 3);
    }

    /// Runs background work on fresh threads and queues main-thread work until `run_main`.
    #[derive(Default)]
    struct TestDispatcher {
        main: Mutex<VecDeque<RunnableVariant>>,
    }

    impl TestDispatcher {
        fn run_main(&self) {
            loop {
                let Some(runnable) = self.main.lock().unwrap().pop_front() else { break };
                runnable.run();
            }
        }
    }

    impl PlatformDispatcher for TestDispatcher {
        fn is_main_thread(&self) -> bool {
            true
        }

        fn dispatch(&self, runnable: RunnableVariant, _priority: Priority) {
            thread::spawn(move || runnable.run());
        }

        fn dispatch_on_main_thread(&self, runnable: RunnableVariant) {
            self.main.lock().unwrap().push_back(runnable);
        }

        fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
            thread::spawn(move || {
                thread::sleep(duration);
                runnable.run();
            });
        }
    }

    fn executors() -> (Arc<TestDispatcher>, BackgroundExecutor, ForegroundExecutor) {
        let dispatcher = Arc::new(TestDispatcher::default());
        (
            Arc::clone(&dispatcher),
            BackgroundExecutor::new(Arc::<TestDispatcher>::clone(&dispatcher)),
            ForegroundExecutor::new(dispatcher),
        )
    }

    #[test]
    fn ready_task_resolves_immediately() {
        let (_, _, foreground) = executors();
        let task = Task::ready(7);
        assert!(task.is_ready());
        assert_eq!(foreground.block_on(task), 7);
        Task::ready(()).detach();
    }

    #[test]
    fn background_spawn_runs_off_thread() {
        let (_, background, foreground) = executors();
        let task = background.spawn(async { thread::current().id() });
        assert_ne!(foreground.block_on(task), thread::current().id());
        let task = background.spawn_with_priority(Priority::Low, async { 1 + 1 });
        assert_eq!(foreground.block_on(task), 2);
        assert!(background.is_main_thread());
    }

    #[test]
    fn foreground_spawn_runs_in_order_when_main_queue_drains() {
        let (dispatcher, _, foreground) = executors();
        let order = Rc::new(RefCell::new(Vec::new()));
        let tasks = (0..3)
            .map(|i| {
                let order = Rc::clone(&order);
                foreground.spawn(async move { order.borrow_mut().push(i) })
            })
            .collect::<Vec<_>>();
        assert!(order.borrow().is_empty());
        assert!(!tasks[0].is_ready());

        dispatcher.run_main();
        assert_eq!(*order.borrow(), [0, 1, 2]);
        assert!(tasks.iter().all(Task::is_ready));
    }

    #[test]
    fn dropped_task_is_cancelled_and_detached_task_runs() {
        let (dispatcher, _, foreground) = executors();
        let ran = Rc::new(RefCell::new(Vec::new()));
        drop(foreground.spawn({
            let ran = Rc::clone(&ran);
            async move { ran.borrow_mut().push("dropped") }
        }));
        foreground
            .spawn({
                let ran = Rc::clone(&ran);
                async move { ran.borrow_mut().push("detached") }
            })
            .detach();

        dispatcher.run_main();
        assert_eq!(*ran.borrow(), ["detached"]);
    }

    #[test]
    fn timer_fires_after_duration() {
        let (_, background, foreground) = executors();
        assert!(background.timer(Duration::ZERO).is_ready());

        let start = background.now();
        let timer = background.timer(Duration::from_millis(20));
        assert!(!timer.is_ready());
        foreground.block_on(timer);
        assert!(start.elapsed() >= Duration::from_millis(20));

        // A dropped timer is cancelled; its delayed runnable is a no-op.
        drop(background.timer(Duration::from_millis(1)));
    }

    #[test]
    fn block_with_timeout_returns_unfinished_future() {
        let (_, background, foreground) = executors();
        assert_eq!(foreground.block_with_timeout(Duration::from_secs(5), async { 3 }).ok(), Some(3));

        let slow = background.timer(Duration::from_millis(50));
        let Err(slow) = foreground.block_with_timeout(Duration::from_millis(1), slow) else {
            panic!("timer finished before the timeout");
        };
        foreground.block_on(slow);
    }

    #[test]
    fn scoped_tasks_borrow_from_their_environment() {
        let (_, background, foreground) = executors();
        let total = AtomicUsize::new(0);
        foreground.block_on(background.scoped(|scope| {
            for i in 1..=4 {
                let total = &total;
                scope.spawn(async move {
                    total.fetch_add(i, Ordering::SeqCst);
                });
            }
        }));
        foreground.block_on(background.scoped_priority(Priority::Low, |scope| {
            scope.spawn(async {
                total.fetch_add(10, Ordering::SeqCst);
            });
        }));
        assert_eq!(total.load(Ordering::SeqCst), 20);
    }
}
