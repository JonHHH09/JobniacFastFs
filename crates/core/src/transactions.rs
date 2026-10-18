//! Unit-of-work semantics: transaction nesting plus after-commit, after-rollback and
//! after-completion hooks.
//!
//! The current transaction lives in an explicit [`TransactionContext`] that the use case owns and
//! passes to repositories, so no ambient state exists. The SQLite implementation stores its
//! connection next to each frame.
//!
//! Entry points are closure-scoped, so a frame is always popped on every return path of the body:
//! * [`TransactionContext::transaction`] joins an active transaction (`REQUIRED`) and otherwise
//!   starts one. `requires_new` always starts a nested transaction (`REQUIRES_NEW`). The scope is
//!   committed when the body returns `Ok` and rolled back when it returns `Err`. A joined body that
//!   returns `Err` hands the error to its caller, and the enclosing
//!   scope decides the outcome.
//! * [`TransactionContext::without_transaction`] runs the body outside any transaction
//!   (`NOT_SUPPORTED`).
//! * Outside a transaction, `after_commit` and `after_completion` run immediately and
//!   `after_rollback` is dropped.
//! * When a scope ends, its after-commit hooks run only when it committed, then its completion hooks
//!   run in registration order (after-rollback hooks only when it did not commit). Hooks run after
//!   the scope is popped, outside the transaction they belong to, and are awaited one after another.
//! * A failing hook is logged with its error kind and never propagates. A panicking hook is treated
//!   as a failure, so later hooks still run.
//!
//! Hooks are `FnOnce` closures that capture their own state and receive no context. Dropping the
//! future of [`TransactionContext::transaction`] before it completes (cancellation) leaves the frame
//! open; the context must then be discarded. Release builds use `panic = "abort"`, so a panicking
//! body also ends the process there.

use std::fmt;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

use tracing::warn;

/// The future a hook produces. It is boxed so hooks of different shapes share one type.
pub type HookFuture = Pin<Box<dyn Future<Output = Result<(), HookError>> + Send + 'static>>;

/// A deferred action run once after a transaction boundary. Its error is logged, never propagated.
pub type Hook = Box<dyn FnOnce() -> HookFuture + Send + 'static>;

/// Failure class reported for a hook that panicked.
const PANIC_KIND: &str = "Panic";

/// A hook failure. `kind` names the failure class (for example the exception class name).
/// It must never carry input values, SQL or secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookError {
    kind: &'static str,
}

impl HookError {
    /// A failure of the given class.
    pub const fn new(kind: &'static str) -> Self {
        Self { kind }
    }

    /// The failure class.
    pub const fn kind(&self) -> &'static str {
        self.kind
    }
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transaction hook failed: {}", self.kind)
    }
}

impl std::error::Error for HookError {}

/// A hook that awaits asynchronous work (for example a file cleanup or an event delivery).
pub fn deferred<F, Fut>(run: F) -> Hook
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), HookError>> + Send + 'static,
{
    Box::new(move || -> HookFuture { Box::pin(run()) })
}

/// A hook that does synchronous work only.
pub fn deferred_sync<F>(run: F) -> Hook
where
    F: FnOnce() -> Result<(), HookError> + Send + 'static,
{
    deferred(move || async move { run() })
}

/// Options for [`TransactionContext::transaction`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransactionOptions {
    /// Advisory only for storage-free contexts; SQLite uses it to pick a reader or the writer.
    pub read_only: bool,
    /// Start a nested transaction even when one is active.
    pub requires_new: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Completion {
    AfterRollback,
    AfterCompletion,
}

#[derive(Default)]
struct HookSet {
    after_commit: Vec<Hook>,
    completion: Vec<(Completion, Hook)>,
}

impl HookSet {
    async fn run(self, committed: bool) {
        if committed {
            for hook in self.after_commit {
                run_hook(hook).await;
            }
        }
        for (kind, hook) in self.completion {
            if kind == Completion::AfterCompletion || !committed {
                run_hook(hook).await;
            }
        }
    }
}

/// Polls a hook future and reports a panic raised by the poll as `Err(())`.
struct CatchPanic(HookFuture);

impl Future for CatchPanic {
    type Output = Result<Result<(), HookError>, ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let future = &mut self.0;
        match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Ready(outcome)) => Poll::Ready(Ok(outcome)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(Err(())),
        }
    }
}

/// Run one hook. A returned error or a panic is logged with its class only; the caller continues
/// with the next hook.
async fn run_hook(hook: Hook) {
    let outcome = match catch_unwind(AssertUnwindSafe(hook)) {
        Ok(future) => CatchPanic(future).await,
        Err(_) => Err(()),
    };
    let error = match outcome {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error,
        Err(()) => HookError::new(PANIC_KIND),
    };
    warn!(
        error_kind = error.kind(),
        "transaction hook failed: {}",
        error.kind()
    );
}

enum Frame {
    Transaction(HookSet),
    Suspended,
}

/// The explicit unit-of-work state of one request.
#[derive(Default)]
pub struct TransactionContext {
    frames: Vec<Frame>,
    commits: u64,
    rollbacks: u64,
}

impl TransactionContext {
    /// A context with no open transaction.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a transaction is active at the innermost level (not suspended).
    pub fn in_transaction(&self) -> bool {
        matches!(self.frames.last(), Some(Frame::Transaction(_)))
    }

    /// Number of started transaction scopes that committed (joined scopes are not counted).
    pub fn commits(&self) -> u64 {
        self.commits
    }

    /// Number of started transaction scopes that rolled back.
    pub fn rollbacks(&self) -> u64 {
        self.rollbacks
    }

    /// Run `body` in a transaction scope. The scope joins the active transaction unless
    /// `options.requires_new` is set. A joined body runs inside the enclosing scope, so its `Err`
    /// is returned to the caller unchanged and the enclosing scope decides the outcome.
    ///
    /// A started scope commits when `body` returns `Ok` and rolls back when it returns `Err`. Its
    /// hooks run before this method returns, and the frame is popped on every path.
    pub async fn transaction<T, E>(
        &mut self,
        options: TransactionOptions,
        body: impl AsyncFnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        if self.in_transaction() && !options.requires_new {
            return body(&mut *self).await;
        }
        self.frames.push(Frame::Transaction(HookSet::default()));
        let result = body(&mut *self).await;
        self.finish_frame(result.is_ok()).await;
        result
    }

    /// Run `body` outside any transaction (`NOT_SUPPORTED`). The frame is popped when `body`
    /// returns. Transactions opened inside `body` are complete before it returns.
    pub async fn without_transaction<T>(&mut self, body: impl AsyncFnOnce(&mut Self) -> T) -> T {
        self.frames.push(Frame::Suspended);
        let value = body(&mut *self).await;
        self.frames.pop();
        value
    }

    /// Register a hook for the outermost commit. Outside a transaction it runs now.
    pub async fn after_commit(&mut self, hook: Hook) {
        match self.frames.last_mut() {
            Some(Frame::Transaction(hooks)) => hooks.after_commit.push(hook),
            _ => run_hook(hook).await,
        }
    }

    /// Register a hook that runs only after a rollback. Outside a transaction it is dropped.
    pub fn after_rollback(&mut self, hook: Hook) {
        if let Some(Frame::Transaction(hooks)) = self.frames.last_mut() {
            hooks.completion.push((Completion::AfterRollback, hook));
        }
    }

    /// Register a hook that runs after either outcome. Outside a transaction it runs now.
    pub async fn after_completion(&mut self, hook: Hook) {
        match self.frames.last_mut() {
            Some(Frame::Transaction(hooks)) => {
                hooks.completion.push((Completion::AfterCompletion, hook))
            }
            _ => run_hook(hook).await,
        }
    }

    /// Pop the frame of a started scope, count the outcome, then run its hooks.
    async fn finish_frame(&mut self, committed: bool) {
        // `transaction` pushes this frame and only returns after every nested frame is popped, so the
        // top frame is always a transaction here.
        let Some(Frame::Transaction(hooks)) = self.frames.pop() else {
            return;
        };
        if committed {
            self.commits += 1;
        } else {
            self.rollbacks += 1;
        }
        hooks.run(committed).await;
    }
}

impl fmt::Debug for TransactionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransactionContext")
            .field("depth", &self.frames.len())
            .field("commits", &self.commits)
            .field("rollbacks", &self.rollbacks)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    type Events = Arc<Mutex<Vec<&'static str>>>;

    fn events() -> Events {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn push(events: &Events, label: &'static str) -> Result<(), HookError> {
        events
            .lock()
            .map_err(|_| HookError::new("PoisonedLock"))?
            .push(label);
        Ok(())
    }

    fn record(events: &Events, label: &'static str) -> Hook {
        let events = Arc::clone(events);
        deferred_sync(move || push(&events, label))
    }

    fn snapshot(events: &Events) -> Vec<&'static str> {
        events.lock().map(|guard| guard.clone()).unwrap_or_default()
    }

    fn required() -> TransactionOptions {
        TransactionOptions::default()
    }

    fn requires_new() -> TransactionOptions {
        TransactionOptions {
            read_only: false,
            requires_new: true,
        }
    }

    #[tokio::test]
    async fn commit_hooks_run_in_phase_order_after_the_transaction_ends() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(record(&events, "commit")).await;
                uow.after_rollback(record(&events, "rollback"));
                uow.after_commit(record(&events, "second")).await;
                uow.after_completion(record(&events, "completion")).await;
                assert!(snapshot(&events).is_empty());
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(snapshot(&events), ["commit", "second", "completion"]);
        assert_eq!((uow.commits(), uow.rollbacks()), (1, 0));
    }

    #[tokio::test]
    async fn rollback_hooks_run_before_completion_and_error_propagates() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), &str> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(record(&events, "commit")).await;
                uow.after_rollback(record(&events, "rollback"));
                uow.after_completion(record(&events, "completion")).await;
                Err("abort")
            })
            .await;
        assert_eq!(result, Err("abort"));
        assert_eq!(snapshot(&events), ["rollback", "completion"]);
        assert_eq!((uow.commits(), uow.rollbacks()), (0, 1));
    }

    #[tokio::test]
    async fn completion_hooks_keep_registration_order_around_rollback_hooks() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), &str> = uow
            .transaction(required(), async |uow| {
                uow.after_completion(record(&events, "first completion"))
                    .await;
                uow.after_rollback(record(&events, "rollback"));
                uow.after_completion(record(&events, "last completion"))
                    .await;
                Err("abort")
            })
            .await;
        assert!(result.is_err());
        assert_eq!(
            snapshot(&events),
            ["first completion", "rollback", "last completion"]
        );
    }

    #[tokio::test]
    async fn required_nesting_joins_the_outer_transaction() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                let inner: Result<(), ()> = uow
                    .transaction(
                        TransactionOptions {
                            read_only: true,
                            requires_new: false,
                        },
                        async |uow| {
                            uow.after_commit(record(&events, "nested")).await;
                            Ok(())
                        },
                    )
                    .await;
                assert!(inner.is_ok());
                assert!(snapshot(&events).is_empty());
                assert!(uow.in_transaction());
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(snapshot(&events), ["nested"]);
        assert_eq!(uow.commits(), 1);
    }

    #[tokio::test]
    async fn a_failed_joined_scope_rolls_back_the_enclosing_transaction() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), &str> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(record(&events, "commit")).await;
                uow.after_rollback(record(&events, "rollback"));
                uow.transaction(
                    required(),
                    async |_: &mut TransactionContext| -> Result<(), &str> { Err("inner") },
                )
                .await?;
                Ok(())
            })
            .await;
        assert_eq!(result, Err("inner"));
        assert_eq!(snapshot(&events), ["rollback"]);
        assert_eq!((uow.commits(), uow.rollbacks()), (0, 1));
    }

    #[tokio::test]
    async fn an_inner_error_the_body_handles_does_not_roll_back_the_outer_scope() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), &str> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(record(&events, "commit")).await;
                let handled = uow
                    .transaction(
                        required(),
                        async |_: &mut TransactionContext| -> Result<(), &str> { Err("inner") },
                    )
                    .await;
                assert!(handled.is_err());
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(snapshot(&events), ["commit"]);
        assert_eq!((uow.commits(), uow.rollbacks()), (1, 0));
    }

    #[tokio::test]
    async fn requires_new_hooks_run_before_the_resumed_outer_hooks() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(record(&events, "outer")).await;
                let inner: Result<(), ()> = uow
                    .transaction(requires_new(), async |uow| {
                        uow.after_commit(record(&events, "inner")).await;
                        Ok(())
                    })
                    .await;
                assert!(inner.is_ok());
                assert_eq!(snapshot(&events), ["inner"]);
                assert!(uow.in_transaction());
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(snapshot(&events), ["inner", "outer"]);
        assert_eq!(uow.commits(), 2);
    }

    #[tokio::test]
    async fn without_transaction_runs_outside_the_transaction_and_restores_the_outer_one() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                uow.without_transaction(async |uow| {
                    assert!(!uow.in_transaction());
                    uow.after_commit(record(&events, "now")).await;
                    let nested: Result<(), ()> = uow
                        .transaction(required(), async |uow| {
                            uow.after_commit(record(&events, "suspended-new")).await;
                            Ok(())
                        })
                        .await;
                    assert!(nested.is_ok());
                })
                .await;
                assert_eq!(snapshot(&events), ["now", "suspended-new"]);
                assert!(uow.in_transaction());
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(uow.commits(), 2);
    }

    #[tokio::test]
    async fn hooks_registered_outside_a_transaction_run_now_and_rollback_hooks_are_dropped() {
        let mut uow = TransactionContext::new();
        let events = events();
        uow.after_commit(record(&events, "commit")).await;
        uow.after_rollback(record(&events, "rollback"));
        uow.after_completion(record(&events, "completion")).await;
        assert_eq!(snapshot(&events), ["commit", "completion"]);
    }

    #[tokio::test]
    async fn a_failing_hook_does_not_stop_later_hooks() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(deferred_sync(|| Err(HookError::new("RuntimeError"))))
                    .await;
                uow.after_commit(record(&events, "next")).await;
                uow.after_completion(record(&events, "completed")).await;
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(snapshot(&events), ["next", "completed"]);
    }

    #[tokio::test]
    async fn a_panicking_hook_is_isolated_and_later_hooks_still_run() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(Box::new(|| -> HookFuture { panic!("hook panicked") }))
                    .await;
                uow.after_commit(deferred(|| async {
                    assert!(std::hint::black_box(false), "poll panicked");
                    Ok(())
                }))
                .await;
                uow.after_commit(record(&events, "after the panics")).await;
                uow.after_completion(record(&events, "completed")).await;
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(snapshot(&events), ["after the panics", "completed"]);
        assert_eq!(uow.commits(), 1);
    }

    #[tokio::test]
    async fn an_async_hook_is_awaited_before_the_transaction_returns() {
        let mut uow = TransactionContext::new();
        let events = events();
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let hook_events = Arc::clone(&events);
        let result: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(deferred(move || async move {
                    receiver.await.map_err(|_| HookError::new("RecvError"))?;
                    push(&hook_events, "hook finished")
                }))
                .await;
                tokio::spawn(async move {
                    tokio::task::yield_now().await;
                    let _ = sender.send(());
                });
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(snapshot(&events), ["hook finished"]);
    }

    #[tokio::test]
    async fn an_early_error_return_pops_the_frame_and_counts_a_rollback() {
        let mut uow = TransactionContext::new();
        let events = events();
        let result: Result<(), &str> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(record(&events, "commit")).await;
                Err("early")
            })
            .await;
        assert_eq!(result, Err("early"));
        assert!(!uow.in_transaction());
        assert_eq!((uow.commits(), uow.rollbacks()), (0, 1));
        assert!(snapshot(&events).is_empty());

        let later: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                uow.after_commit(record(&events, "later")).await;
                Ok(())
            })
            .await;
        assert!(later.is_ok());
        assert_eq!(snapshot(&events), ["later"]);
        assert_eq!((uow.commits(), uow.rollbacks()), (1, 1));
    }

    #[tokio::test]
    async fn an_early_error_inside_a_suspension_leaves_the_outer_transaction_active() {
        let mut uow = TransactionContext::new();
        let result: Result<(), ()> = uow
            .transaction(required(), async |uow| {
                let suspended: Result<(), &str> = uow
                    .without_transaction(async |uow| {
                        assert!(!uow.in_transaction());
                        Err("early")
                    })
                    .await;
                assert_eq!(suspended, Err("early"));
                assert!(uow.in_transaction());
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        assert_eq!((uow.commits(), uow.rollbacks()), (1, 0));
    }

    #[test]
    fn hook_error_display_names_only_the_kind() {
        let error = HookError::new("RuntimeError");
        assert_eq!(error.to_string(), "transaction hook failed: RuntimeError");
        assert_eq!(error.kind(), "RuntimeError");
    }
}
