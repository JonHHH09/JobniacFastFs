//! A failing transaction hook is logged once, with its failure class and nothing else, and later
//! hooks still run.
//!
//! This test runs in its own test binary on purpose: `tracing` caches the interest of each log call
//! site process-wide, so a parallel unit test that hits the hook warning first (with no subscriber)
//! would hide the event from this test.

use std::io;
use std::sync::{Arc, Mutex};

use jobniac_core::transactions::{
    HookError, TransactionContext, TransactionOptions, deferred_sync,
};

/// An in-memory log writer shared with the subscriber.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.0.lock() {
            Ok(mut guard) => guard.extend_from_slice(buf),
            Err(_) => return Err(io::Error::other("poisoned log buffer")),
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_failing_hook_is_logged_once_with_its_class_and_nothing_else() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .without_time()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut uow = TransactionContext::new();
    let result: Result<(), ()> = uow
        .transaction(TransactionOptions::default(), async |uow| {
            uow.after_commit(deferred_sync(|| Err(HookError::new("RuntimeError"))))
                .await;
            Ok(())
        })
        .await;
    assert!(result.is_ok());

    let text = match captured.0.lock() {
        Ok(guard) => String::from_utf8_lossy(&guard).into_owned(),
        Err(_) => String::new(),
    };
    assert_eq!(text.matches("transaction hook failed").count(), 1, "{text}");
    assert!(
        text.contains("error_kind=\"RuntimeError\"") || text.contains("error_kind=RuntimeError"),
        "{text}"
    );
    assert!(text.contains("WARN"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
}
