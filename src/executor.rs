//! Async executors for WASI Preview 2 and Preview 3.
//!
//! WASI Preview 2 uses a single-threaded [`futures`] executor and bridges
//! `wasi::io::poll::Pollable` resources into Rust futures. WASI Preview 3
//! delegates task spawning to the host runtime.

/// Errors produced while configuring or driving a WASI executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ExecutorError {
    /// A WASI Preview 2 pollable was awaited before its executor was initialized.
    #[error(
        "a WASI Preview 2 pollable was awaited before initializing the executor"
    )]
    Preview2NotInitialized,

    /// A reused Preview 2 executor was requested with a different scheduling mode.
    #[error(
        "the WASI Preview 2 executor was initialized with a different mode"
    )]
    Preview2ModeMismatch,

    /// Preview 2 executor initialization was called recursively on the same thread.
    #[error("the WASI Preview 2 executor cannot be initialized recursively")]
    Preview2InitializationReentrant,

    /// The executor exhausted the identifier space used for pollable registrations.
    #[error(
        "the WASI Preview 2 pollable registration identifier space is exhausted"
    )]
    PollableRegistrationExhausted,

    /// A pollable registration was canceled before it became ready.
    #[error("the WASI Preview 2 pollable registration was canceled")]
    PollableCanceled,

    /// A future could not be queued on the local executor.
    #[error("the local executor rejected a task")]
    TaskSpawnFailed,

    /// The result channel used by a Preview 2 executor closed unexpectedly.
    #[error("the future passed to Executor::run_until was canceled")]
    RunUntilCanceled,

    /// The executor was polled recursively while it was already driving tasks.
    #[error("the WASI Preview 2 executor cannot be polled recursively")]
    ReentrantPoll,

    /// The root future is pending and no runnable task or live pollable can
    /// wake it. Unrelated to the Preview 2 scheduling mode of the same name.
    #[error("the WASI Preview 2 executor stalled with no live pollables")]
    Stalled,

    /// The global `any_spawner` executor was initialized before the `WASIp3` executor.
    #[error("the global task spawner has already been initialized")]
    SpawnerAlreadyInitialized,
}

#[cfg(feature = "wasip2")]
mod p2;
#[cfg(feature = "wasip2")]
pub use p2::{Executor, Mode, WaitPoll, init_wasip2_executor, sleep};
#[cfg(feature = "wasip2")]
pub(crate) use p2::{bench_dispatch_ready, pollable_queue_depth};

#[cfg(feature = "wasip3")]
mod p3;
#[cfg(feature = "wasip3")]
pub use p3::{Wasip3Executor, init_wasip3_spawner};
