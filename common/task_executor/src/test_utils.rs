use crate::TaskExecutor;
pub use logging::test_logger;
use slog::Logger;
use std::sync::Arc;
use tokio::runtime;

/// Whilst the `TestRuntime` is not necessarily useful in itself, it provides the necessary
/// components for creating a `TaskExecutor` during tests.
///
/// May create its own runtime or use an existing one.
///
/// ## Warning
///
/// This struct should never be used in production, only testing.
pub struct TestRuntime {
    runtime: Option<Arc<tokio::runtime::Runtime>>,
    _runtime_shutdown: async_channel::Sender<()>,
    pub task_executor: TaskExecutor,
    pub log: Logger,
}

impl Default for TestRuntime {
    /// If called *inside* an existing runtime, instantiates `Self` using a handle to that runtime. If
    /// called *outside* any existing runtime, create a new `Runtime` and keep it alive until the
    /// `Self` is dropped.
    fn default() -> Self {
        let (runtime_shutdown, exit) = async_channel::bounded(1);
        let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
        let log = test_logger();

        let (runtime, handle) = if let Ok(handle) = runtime::Handle::try_current() {
            (None, handle)
        } else {
            let runtime = Arc::new(
                runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            );
            let handle = runtime.handle().clone();
            (Some(runtime), handle)
        };

        let task_executor = TaskExecutor::new(handle, exit, log.clone(), shutdown_tx);

        Self {
            runtime,
            _runtime_shutdown: runtime_shutdown,
            task_executor,
            log,
        }
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            Arc::try_unwrap(runtime).unwrap().shutdown_background()
        }
    }
}

impl TestRuntime {
    pub fn set_logger(&mut self, log: Logger) {
        self.log = log.clone();
        self.task_executor.log = log;
    }
}
