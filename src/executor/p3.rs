//! WASI Preview 3 executor: task spawning delegated to the host runtime.

use std::sync::OnceLock;

use any_spawner::CustomExecutor;

use super::ExecutorError;

/// A custom executor that delegates WASI Preview 3 tasks to the host.
#[derive(Clone, Copy)]
pub struct Wasip3Executor;

impl CustomExecutor for Wasip3Executor {
    fn spawn(&self, future: any_spawner::PinnedFuture<()>) {
        wasip3::spawn(future);
    }

    fn spawn_local(&self, future: any_spawner::PinnedLocalFuture<()>) {
        wasip3::spawn(future);
    }

    fn poll_local(&self) {
        // WASI Preview 3 host tasks are driven by the component runtime.
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitState {
    Initialized,
    Conflict,
}

static INITIALIZED: OnceLock<InitState> = OnceLock::new();

fn initialize_with(
    state: &OnceLock<InitState>,
    initialize: impl FnOnce() -> Result<(), any_spawner::ExecutorError>,
) -> Result<(), ExecutorError> {
    match state.get_or_init(|| match initialize() {
        Ok(()) => InitState::Initialized,
        Err(_) => InitState::Conflict,
    }) {
        InitState::Initialized => Ok(()),
        InitState::Conflict => Err(ExecutorError::SpawnerAlreadyInitialized),
    }
}

/// Initializes the global task spawner for WASI Preview 3.
///
/// Repeated calls return the result of the first initialization attempt.
///
/// # Errors
///
/// Returns
/// [`ExecutorError::SpawnerAlreadyInitialized`](crate::ExecutorError::SpawnerAlreadyInitialized)
/// when another global executor was installed before this function's first
/// call.
pub fn init_wasip3_spawner() -> Result<(), ExecutorError> {
    initialize_with(&INITIALIZED, || {
        any_spawner::Executor::init_local_custom_executor(Wasip3Executor)
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn repeated_success_returns_success_without_reinitializing() {
        let state = OnceLock::new();
        let calls = Cell::new(0);
        let first = initialize_with(&state, || {
            calls.set(calls.get() + 1);
            Ok(())
        });
        let second = initialize_with(&state, || {
            calls.set(calls.get() + 1);
            Ok(())
        });

        assert_eq!((first, second, calls.get()), (Ok(()), Ok(()), 1));
    }

    #[test]
    fn repeated_conflict_returns_conflict_without_reinitializing() {
        let state = OnceLock::new();
        let calls = Cell::new(0);
        let first = initialize_with(&state, || {
            calls.set(calls.get() + 1);
            Err(any_spawner::ExecutorError::AlreadySet)
        });
        let second = initialize_with(&state, || {
            calls.set(calls.get() + 1);
            Ok(())
        });

        assert_eq!(
            (first, second, calls.get()),
            (
                Err(ExecutorError::SpawnerAlreadyInitialized),
                Err(ExecutorError::SpawnerAlreadyInitialized),
                1,
            )
        );
    }
}
