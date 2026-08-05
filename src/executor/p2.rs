//! WASI Preview 2 executor: a single-threaded futures executor that
//! bridges `wasi::io::poll::Pollable` resources into Rust futures.

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::VecDeque,
    future::Future,
    mem,
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
};

use any_spawner::CustomExecutor;
use futures::{
    executor::{LocalPool, LocalSpawner},
    future::{AbortHandle, Abortable},
    task::{LocalSpawnExt, SpawnExt},
};
use parking_lot::Mutex;
use wasi::{
    clocks::monotonic_clock::{Duration, subscribe_duration},
    io::poll::{Pollable, poll},
};

use super::ExecutorError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegistrationId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegistrationStatus {
    Pending,
    Ready,
    Canceled,
}

struct RegistrationInner {
    status: RegistrationStatus,
    task_waker: Waker,
}

struct WaitRegistration {
    id: RegistrationId,
    inner: Mutex<RegistrationInner>,
}

impl WaitRegistration {
    fn new(id: RegistrationId, waker: &Waker) -> Self {
        Self {
            id,
            inner: Mutex::new(RegistrationInner {
                status: RegistrationStatus::Pending,
                task_waker: waker.clone(),
            }),
        }
    }

    fn id(&self) -> RegistrationId {
        self.id
    }

    fn poll(&self, waker: &Waker) -> Poll<Result<(), ExecutorError>> {
        let mut inner = self.inner.lock();
        match inner.status {
            RegistrationStatus::Ready => Poll::Ready(Ok(())),
            RegistrationStatus::Canceled => {
                Poll::Ready(Err(ExecutorError::PollableCanceled))
            }
            RegistrationStatus::Pending => {
                if !inner.task_waker.will_wake(waker) {
                    inner.task_waker.clone_from(waker);
                }
                Poll::Pending
            }
        }
    }

    fn mark_ready(&self) -> Option<Waker> {
        let mut inner = self.inner.lock();
        if inner.status != RegistrationStatus::Pending {
            return None;
        }

        inner.status = RegistrationStatus::Ready;
        Some(inner.task_waker.clone())
    }

    fn cancel(&self) {
        let mut inner = self.inner.lock();
        if inner.status == RegistrationStatus::Pending {
            inner.status = RegistrationStatus::Canceled;
        }
    }

    fn is_pending(&self) -> bool {
        self.inner.lock().status == RegistrationStatus::Pending
    }

    #[cfg(test)]
    fn status(&self) -> RegistrationStatus {
        self.inner.lock().status
    }
}

struct TableEntry<P> {
    pollable: P,
    registration: Arc<WaitRegistration>,
}

struct WaitQueue<P> {
    next_id: AtomicU64,
    entries: Mutex<VecDeque<TableEntry<P>>>,
}

impl<P> Default for WaitQueue<P> {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            entries: Mutex::new(VecDeque::new()),
        }
    }
}

impl<P> WaitQueue<P> {
    fn register(
        &self,
        pollable: P,
        waker: &Waker,
    ) -> Result<Arc<WaitRegistration>, ExecutorError> {
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                id.checked_add(1)
            })
            .map(RegistrationId)
            .map_err(|_| ExecutorError::PollableRegistrationExhausted)?;
        let registration = Arc::new(WaitRegistration::new(id, waker));

        let queue_depth = {
            let mut entries = self.entries.lock();
            entries.push_back(TableEntry {
                pollable,
                registration: registration.clone(),
            });
            entries.len()
        };
        #[cfg(feature = "tracing")]
        tracing::trace!(
            preview = "p2",
            pollable_id = id.0,
            queue_depth,
            "registered WASIp2 pollable"
        );
        #[cfg(not(feature = "tracing"))]
        let _ = queue_depth;

        Ok(registration)
    }

    fn cancel(&self, id: RegistrationId) {
        let (removed, queue_depth) = {
            let mut entries = self.entries.lock();
            let removed = entries
                .iter()
                .position(|entry| entry.registration.id() == id)
                .and_then(|index| entries.remove(index));
            (removed, entries.len())
        };
        #[cfg(feature = "tracing")]
        if removed.is_some() {
            tracing::trace!(
                preview = "p2",
                pollable_id = id.0,
                queue_depth,
                cancellation = true,
                "canceled WASIp2 pollable"
            );
        }
        #[cfg(not(feature = "tracing"))]
        let _ = queue_depth;
        drop(removed);
    }

    fn take_pending(&self) -> Vec<TableEntry<P>> {
        let drained = self.entries.lock().drain(..).collect::<Vec<_>>();
        drained
            .into_iter()
            .filter(|entry| entry.registration.is_pending())
            .collect()
    }

    fn requeue_pending(
        &self,
        entries: impl IntoIterator<Item = TableEntry<P>>,
    ) {
        let pending = entries
            .into_iter()
            .filter(|entry| entry.registration.is_pending())
            .collect::<VecDeque<_>>();
        self.entries.lock().extend(pending);
    }

    fn len(&self) -> usize {
        self.entries.lock().len()
    }
}

static POLLABLE_QUEUE: OnceLock<Arc<WaitQueue<Pollable>>> = OnceLock::new();

fn initialize_wait_queue(
    registry: &OnceLock<Arc<WaitQueue<Pollable>>>,
) -> Arc<WaitQueue<Pollable>> {
    registry
        .get_or_init(|| Arc::new(WaitQueue::default()))
        .clone()
}

/// Suspends execution for `duration` nanoseconds.
///
/// # Errors
///
/// Returns
/// [`ExecutorError::Preview2NotInitialized`](crate::ExecutorError::Preview2NotInitialized)
/// if no Preview 2 executor has been created, or another executor error if
/// timer registration fails.
///
/// # Example
///
/// ```ignore
/// use leptos_wasi::wasip2::sleep;
///
/// # async fn run() -> Result<(), leptos_wasi::ExecutorError> {
/// sleep(1_000_000_000).await?;
/// # Ok(())
/// # }
/// ```
pub async fn sleep(duration: Duration) -> Result<(), ExecutorError> {
    WaitPoll::new(subscribe_duration(duration)).await
}

/// Resolves when a WASI Preview 2 [`wasi::io::poll::Pollable`] becomes ready.
///
/// Dropping this future cancels its registration and removes queued host
/// resources before the next blocking poll.
pub struct WaitPoll(WaitPollInner);

enum WaitPollInner {
    Unregistered(Pollable),
    Registered(Arc<WaitRegistration>),
    Failed(ExecutorError),
}

impl WaitPoll {
    /// Creates a future for a WASI Preview 2 pollable resource.
    pub fn new(pollable: Pollable) -> Self {
        Self(WaitPollInner::Unregistered(pollable))
    }
}

impl Future for WaitPoll {
    type Output = Result<(), ExecutorError>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output> {
        let this = self.get_mut();
        let current = mem::replace(
            &mut this.0,
            WaitPollInner::Failed(ExecutorError::PollableCanceled),
        );

        match current {
            WaitPollInner::Unregistered(pollable) => {
                let Some(queue) = POLLABLE_QUEUE.get() else {
                    let error = ExecutorError::Preview2NotInitialized;
                    this.0 = WaitPollInner::Failed(error);
                    return Poll::Ready(Err(error));
                };

                match queue.register(pollable, cx.waker()) {
                    Ok(registration) => {
                        this.0 = WaitPollInner::Registered(registration);
                        Poll::Pending
                    }
                    Err(error) => {
                        this.0 = WaitPollInner::Failed(error);
                        Poll::Ready(Err(error))
                    }
                }
            }
            WaitPollInner::Registered(registration) => {
                let result = registration.poll(cx.waker());
                this.0 = WaitPollInner::Registered(registration);
                result
            }
            WaitPollInner::Failed(error) => {
                this.0 = WaitPollInner::Failed(error);
                Poll::Ready(Err(error))
            }
        }
    }
}

impl Drop for WaitPoll {
    fn drop(&mut self) {
        let WaitPollInner::Registered(registration) = &self.0 else {
            return;
        };

        registration.cancel();
        if let Some(queue) = POLLABLE_QUEUE.get() {
            queue.cancel(registration.id());
        }
    }
}

/// Controls how eagerly [`crate::wasip2::Executor`] checks WASI pollables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// Run one guest task before each host poll, favoring I/O responsiveness.
    Preemptive,

    /// Run all immediately available guest work before blocking on the host.
    Stalled,
}

/// A single-threaded cooperative executor for WASI Preview 2.
#[derive(Clone)]
pub struct Executor(Rc<ExecutorInner>);

thread_local! {
    static PREVIEW2_EXECUTOR: RefCell<Option<Executor>> = const {
        RefCell::new(None)
    };

    static PREVIEW2_SPAWNER_STATE: OnceCell<Preview2InitState> = const {
        OnceCell::new()
    };
}

enum Preview2InitState {
    Initialized(Executor),
    Conflict,
}

struct ExecutorInner {
    pool: RefCell<LocalPool>,
    spawner: LocalSpawner,
    queue: Arc<WaitQueue<Pollable>>,
    mode: Mode,
    spawn_failed: Cell<bool>,
}

trait Poller<P> {
    fn poll(&self, pollables: &[&P]) -> Vec<u32>;
}

struct HostPoller;

impl Poller<Pollable> for HostPoller {
    fn poll(&self, pollables: &[&Pollable]) -> Vec<u32> {
        poll(pollables)
    }
}

/// Polls every pending registration once and wakes the ready tasks.
///
/// Returns `true` when at least one registration was polled, which the
/// executor reads as "the host may still make progress".
fn dispatch_ready<P>(queue: &WaitQueue<P>, poller: &impl Poller<P>) -> bool {
    let entries = queue.take_pending();
    if entries.is_empty() {
        return false;
    }

    let pollables = entries
        .iter()
        .map(|entry| &entry.pollable)
        .collect::<Vec<_>>();
    let ready = poller.poll(&pollables);
    let mut ready_flags = vec![false; entries.len()];
    for index in ready {
        if let Some(is_ready) = usize::try_from(index)
            .ok()
            .and_then(|index| ready_flags.get_mut(index))
        {
            *is_ready = true;
        }
    }

    let mut pending = Vec::new();
    let mut task_wakers = Vec::new();
    for (entry, is_ready) in entries.into_iter().zip(ready_flags) {
        if is_ready {
            if let Some(waker) = entry.registration.mark_ready() {
                task_wakers.push(waker);
            }
        } else if entry.registration.is_pending() {
            pending.push(entry);
        }
    }

    queue.requeue_pending(pending);

    // Waking can synchronously poll user futures, so no executor or
    // registration mutex may be held here.
    for waker in task_wakers {
        waker.wake();
    }

    true
}

impl Executor {
    /// Creates a cooperative Preview 2 executor.
    ///
    /// # Errors
    ///
    /// Repeated calls on the same thread return the same executor so they stay
    /// aligned with `any_spawner`'s thread-local executor instance.
    ///
    /// Returns
    /// [`ExecutorError::Preview2ModeMismatch`](crate::ExecutorError::Preview2ModeMismatch)
    /// if a reused executor is requested with a different mode, or
    /// [`ExecutorError::Preview2InitializationReentrant`](crate::ExecutorError::Preview2InitializationReentrant)
    /// if initialization is called recursively.
    pub fn new(mode: Mode) -> Result<Self, ExecutorError> {
        PREVIEW2_EXECUTOR.with(|slot| {
            let mut slot = slot
                .try_borrow_mut()
                .map_err(|_| ExecutorError::Preview2InitializationReentrant)?;

            if let Some(executor) = slot.as_ref() {
                if executor.0.mode != mode {
                    return Err(ExecutorError::Preview2ModeMismatch);
                }
                return Ok(executor.clone());
            }

            let queue = initialize_wait_queue(&POLLABLE_QUEUE);
            let executor = Self::with_queue(mode, queue);
            *slot = Some(executor.clone());
            Ok(executor)
        })
    }

    fn with_queue(mode: Mode, queue: Arc<WaitQueue<Pollable>>) -> Self {
        let pool = LocalPool::new();
        let spawner = pool.spawner();
        Self(Rc::new(ExecutorInner {
            pool: RefCell::new(pool),
            spawner,
            queue,
            mode,
            spawn_failed: Cell::new(false),
        }))
    }

    /// Drives `future` to completion on the current thread.
    ///
    /// # Errors
    ///
    /// Returns an error when the future cannot be spawned, the executor is
    /// polled recursively, its result channel closes, or it stalls without a
    /// live pollable capable of waking the future.
    pub fn run_until<T>(&self, future: T) -> Result<T::Output, ExecutorError>
    where
        T: Future + 'static,
    {
        let (sender, mut receiver) =
            futures::channel::oneshot::channel::<T::Output>();
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        self.0
            .spawner
            .spawn_local(Box::pin(async move {
                let _ = Abortable::new(
                    async move {
                        let output = future.await;
                        let _ = sender.send(output);
                    },
                    abort_registration,
                )
                .await;
            }))
            .map_err(|_| ExecutorError::TaskSpawnFailed)?;

        let result = self.drive_until_output(&mut receiver);
        if result.is_err() {
            abort_handle.abort();
            self.drop_aborted_tasks();
        }
        result
    }

    fn drive_until_output<T>(
        &self,
        receiver: &mut futures::channel::oneshot::Receiver<T>,
    ) -> Result<T, ExecutorError> {
        loop {
            if let Some(output) = receiver
                .try_recv()
                .map_err(|_| ExecutorError::RunUntilCanceled)?
            {
                return Ok(output);
            }

            let can_make_progress = self.poll_once()?;

            if let Some(output) = receiver
                .try_recv()
                .map_err(|_| ExecutorError::RunUntilCanceled)?
            {
                return Ok(output);
            }

            if !can_make_progress {
                return Err(ExecutorError::Stalled);
            }
        }
    }

    fn drop_aborted_tasks(&self) {
        if let Ok(mut pool) = self.0.pool.try_borrow_mut() {
            pool.run_until_stalled();
        }
    }

    fn poll_once(&self) -> Result<bool, ExecutorError> {
        if self.0.spawn_failed.replace(false) {
            return Err(ExecutorError::TaskSpawnFailed);
        }

        let mut pool = self
            .0
            .pool
            .try_borrow_mut()
            .map_err(|_| ExecutorError::ReentrantPoll)?;
        let guest_progress = match self.0.mode {
            Mode::Preemptive => pool.try_run_one(),
            Mode::Stalled => {
                pool.run_until_stalled();
                false
            }
        };
        drop(pool);

        if self.0.spawn_failed.replace(false) {
            return Err(ExecutorError::TaskSpawnFailed);
        }

        let pollable_progress = self.poll_registered(&HostPoller);
        Ok(guest_progress || pollable_progress)
    }

    fn poll_registered(&self, poller: &impl Poller<Pollable>) -> bool {
        dispatch_ready(&self.0.queue, poller)
    }
}

impl Preview2InitState {
    fn result(&self, requested_mode: Mode) -> Result<Executor, ExecutorError> {
        match self {
            Self::Initialized(executor)
                if executor.0.mode == requested_mode =>
            {
                Ok(executor.clone())
            }
            Self::Initialized(_) => Err(ExecutorError::Preview2ModeMismatch),
            Self::Conflict => Err(ExecutorError::SpawnerAlreadyInitialized),
        }
    }
}

fn initialize_preview2_with(
    state: &OnceCell<Preview2InitState>,
    mode: Mode,
    create: impl FnOnce(Mode) -> Result<Executor, ExecutorError>,
    install: impl FnOnce(Executor) -> Result<(), any_spawner::ExecutorError>,
) -> Result<Executor, ExecutorError> {
    if let Some(state) = state.get() {
        return state.result(mode);
    }

    let executor = create(mode)?;
    let initialized = match install(executor.clone()) {
        Ok(()) => Preview2InitState::Initialized(executor),
        Err(_) => Preview2InitState::Conflict,
    };

    if state.set(initialized).is_err() {
        return state
            .get()
            .ok_or(ExecutorError::Preview2InitializationReentrant)?
            .result(mode);
    }

    state
        .get()
        .ok_or(ExecutorError::Preview2InitializationReentrant)?
        .result(mode)
}

/// Initializes and returns the thread-local WASI Preview 2 executor.
///
/// The first call installs the returned executor in `any_spawner`. Later
/// calls with the same [`Mode`](crate::wasip2::Mode) return that exact executor
/// without attempting another installation.
///
/// # Errors
///
/// Returns
/// [`ExecutorError::SpawnerAlreadyInitialized`](crate::ExecutorError::SpawnerAlreadyInitialized)
/// consistently when another executor was installed before the first call. It
/// also propagates executor construction and scheduling-mode errors.
pub fn init_wasip2_executor(mode: Mode) -> Result<Executor, ExecutorError> {
    PREVIEW2_SPAWNER_STATE.with(|state| {
        initialize_preview2_with(
            state,
            mode,
            Executor::new,
            any_spawner::Executor::init_local_custom_executor,
        )
    })
}

pub(crate) fn pollable_queue_depth() -> usize {
    POLLABLE_QUEUE.get().map_or(0, |queue| queue.len())
}

impl CustomExecutor for Executor {
    fn spawn(&self, future: any_spawner::PinnedFuture<()>) {
        if self.0.spawner.spawn(future).is_err() {
            self.0.spawn_failed.set(true);
        }
    }

    fn spawn_local(&self, future: any_spawner::PinnedLocalFuture<()>) {
        if self.0.spawner.spawn_local(future).is_err() {
            self.0.spawn_failed.set(true);
        }
    }

    fn poll_local(&self) {
        let _ = self.poll_once();
    }
}

#[expect(
    clippy::panic,
    reason = "a failed invariant in a test should abort the test"
)]
#[cfg(test)]
mod tests {
    use std::task::{Wake, Waker};
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{
            Arc, OnceLock,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use futures::task::noop_waker;

    use super::*;

    struct CountingWake(AtomicUsize);

    struct SetOnDrop(Arc<AtomicBool>);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl Drop for SetOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn initialize_wait_queue_reuses_the_process_queue() {
        let registry: OnceLock<Arc<WaitQueue<Pollable>>> = OnceLock::new();
        let first = initialize_wait_queue(&registry);
        let second = initialize_wait_queue(&registry);

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn executor_new_supports_a_reused_component_instance() {
        let first = Executor::new(Mode::Stalled)
            .expect("first initialization should succeed");
        let second = Executor::new(Mode::Stalled)
            .expect("reused initialization should succeed");

        assert!(Rc::ptr_eq(&first.0, &second.0));
    }

    #[test]
    fn executor_new_rejects_a_mode_change_on_reuse() {
        Executor::new(Mode::Stalled)
            .expect("first initialization should succeed");

        let Err(error) = Executor::new(Mode::Preemptive) else {
            panic!("mode change unexpectedly succeeded")
        };

        assert_eq!(error, ExecutorError::Preview2ModeMismatch);
    }

    #[test]
    fn executor_new_reports_recursive_initialization() {
        let error = PREVIEW2_EXECUTOR.with(|slot| {
            let _borrow = slot.borrow_mut();
            match Executor::new(Mode::Stalled) {
                Ok(_) => {
                    panic!("recursive initialization unexpectedly succeeded")
                }
                Err(error) => error,
            }
        });

        assert_eq!(error, ExecutorError::Preview2InitializationReentrant);
    }

    #[test]
    fn cancel_marks_a_pending_registration_as_canceled() {
        let registration =
            WaitRegistration::new(RegistrationId(1), &noop_waker());

        registration.cancel();

        assert_eq!(registration.status(), RegistrationStatus::Canceled);
    }

    #[test]
    fn cancel_does_not_replace_a_ready_registration() {
        let registration =
            WaitRegistration::new(RegistrationId(1), &noop_waker());
        let _ = registration.mark_ready();

        registration.cancel();

        assert_eq!(registration.status(), RegistrationStatus::Ready);
    }

    #[test]
    fn mark_ready_returns_the_task_waker_for_waking_after_unlock() {
        let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let registration = WaitRegistration::new(RegistrationId(1), &waker);

        registration
            .mark_ready()
            .expect("pending registration should return its waker")
            .wake();

        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn take_pending_filters_canceled_registrations_before_host_polling() {
        let queue = WaitQueue::default();
        let canceled = queue
            .register("long", &noop_waker())
            .expect("registration should succeed");
        queue
            .register("short", &noop_waker())
            .expect("registration should succeed");
        canceled.cancel();

        let pending = queue
            .take_pending()
            .into_iter()
            .map(|entry| entry.pollable)
            .collect::<Vec<_>>();

        assert_eq!(pending, vec!["short"]);
    }

    #[test]
    fn canceling_many_registrations_returns_queue_to_baseline() {
        let queue = WaitQueue::default();
        let registrations = (0..256)
            .map(|pollable| {
                queue
                    .register(pollable, &noop_waker())
                    .expect("registration should succeed")
            })
            .collect::<Vec<_>>();

        for registration in registrations {
            registration.cancel();
            queue.cancel(registration.id());
        }

        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn run_until_returns_the_ready_future_output() {
        let executor =
            Executor::with_queue(Mode::Stalled, Arc::new(WaitQueue::default()));

        let output = executor
            .run_until(async { 42 })
            .expect("ready future should complete");

        assert_eq!(output, 42);
    }

    #[test]
    fn run_until_cancels_a_future_that_cannot_be_woken() {
        let executor =
            Executor::with_queue(Mode::Stalled, Arc::new(WaitQueue::default()));
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = SetOnDrop(dropped.clone());

        let error = executor
            .run_until(async move {
                let _guard = guard;
                futures::future::pending::<()>().await;
            })
            .expect_err("pending future without a pollable should stall");

        assert_eq!(
            (error, dropped.load(Ordering::Relaxed)),
            (ExecutorError::Stalled, true)
        );
    }

    struct ScriptedPoller(Vec<u32>);

    impl<P> Poller<P> for ScriptedPoller {
        fn poll(&self, _: &[&P]) -> Vec<u32> {
            self.0.clone()
        }
    }

    #[test]
    fn ready_registrations_wake_their_task_and_leave_the_queue() {
        let queue = WaitQueue::<&'static str>::default();
        let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let first = queue
            .register("first", &waker)
            .expect("registration should succeed");
        let second = queue
            .register("second", &waker)
            .expect("registration should succeed");

        let progressed = dispatch_ready(&queue, &ScriptedPoller(vec![0]));

        assert!(progressed);
        assert_eq!(first.status(), RegistrationStatus::Ready);
        assert_eq!(second.status(), RegistrationStatus::Pending);
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn out_of_range_ready_indices_are_ignored() {
        let queue = WaitQueue::<&'static str>::default();
        let registration = queue
            .register("only", &noop_waker())
            .expect("registration should succeed");

        let progressed =
            dispatch_ready(&queue, &ScriptedPoller(vec![7, u32::MAX]));

        assert!(progressed);
        assert_eq!(registration.status(), RegistrationStatus::Pending);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn an_empty_queue_reports_no_host_progress() {
        let queue = WaitQueue::<&'static str>::default();

        assert!(!dispatch_ready(&queue, &ScriptedPoller(vec![0])));
    }

    #[test]
    fn canceled_registrations_are_never_woken_or_requeued() {
        let queue = WaitQueue::<&'static str>::default();
        let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let registration = queue
            .register("canceled", &waker)
            .expect("registration should succeed");
        registration.cancel();

        let progressed = dispatch_ready(&queue, &ScriptedPoller(vec![0]));

        assert!(!progressed);
        assert_eq!(registration.status(), RegistrationStatus::Canceled);
        assert_eq!(counter.0.load(Ordering::Relaxed), 0);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn preemptive_mode_runs_queued_guest_work_one_task_at_a_time() {
        let executor = Executor::with_queue(
            Mode::Preemptive,
            Arc::new(WaitQueue::default()),
        );
        let completed = Rc::new(Cell::new(0_usize));
        for _ in 0..2 {
            let completed = Rc::clone(&completed);
            executor.spawn_local(Box::pin(async move {
                completed.set(completed.get() + 1);
            }));
        }

        let first = executor
            .poll_once()
            .expect("the first task should be runnable");
        let after_first = completed.get();
        let second = executor
            .poll_once()
            .expect("the second task should be runnable");

        assert_eq!(
            (first, after_first, second, completed.get()),
            (true, 1, true, 2)
        );
    }

    #[test]
    fn stalled_mode_drains_every_runnable_task_in_one_poll() {
        let executor =
            Executor::with_queue(Mode::Stalled, Arc::new(WaitQueue::default()));
        let completed = Rc::new(Cell::new(0_usize));
        for _ in 0..3 {
            let completed = Rc::clone(&completed);
            executor.spawn_local(Box::pin(async move {
                completed.set(completed.get() + 1);
            }));
        }

        let progressed = executor
            .poll_once()
            .expect("draining the pool should not fail");

        assert_eq!((progressed, completed.get()), (false, 3));
    }

    #[test]
    fn preview2_initializer_reuses_the_exact_installed_executor() {
        let state = OnceCell::new();
        let install_calls = Cell::new(0);
        let installed = RefCell::new(None);
        let first = initialize_preview2_with(
            &state,
            Mode::Stalled,
            |mode| {
                Ok(Executor::with_queue(mode, Arc::new(WaitQueue::default())))
            },
            |executor| {
                install_calls.set(install_calls.get() + 1);
                installed.replace(Some(executor));
                Ok(())
            },
        )
        .expect("first initialization should succeed");
        let second = initialize_preview2_with(
            &state,
            Mode::Stalled,
            |_| panic!("cached initialization should not create an executor"),
            |_| panic!("cached initialization should not install an executor"),
        )
        .expect("cached initialization should succeed");
        let installed = installed
            .borrow()
            .as_ref()
            .expect("installer should receive an executor")
            .clone();

        assert!(
            Rc::ptr_eq(&first.0, &second.0)
                && Rc::ptr_eq(&first.0, &installed.0)
                && install_calls.get() == 1
        );
    }

    #[test]
    fn preview2_initializer_persists_a_conflicting_first_result() {
        let state = OnceCell::new();
        let install_calls = Cell::new(0);
        let first = initialize_preview2_with(
            &state,
            Mode::Stalled,
            |mode| {
                Ok(Executor::with_queue(mode, Arc::new(WaitQueue::default())))
            },
            |_| {
                install_calls.set(install_calls.get() + 1);
                Err(any_spawner::ExecutorError::AlreadySet)
            },
        );
        let second = initialize_preview2_with(
            &state,
            Mode::Stalled,
            |_| panic!("cached conflict should not create an executor"),
            |_| panic!("cached conflict should not install an executor"),
        );

        assert_eq!(
            (first.err(), second.err(), install_calls.get()),
            (
                Some(ExecutorError::SpawnerAlreadyInitialized),
                Some(ExecutorError::SpawnerAlreadyInitialized),
                1,
            )
        );
    }
}
