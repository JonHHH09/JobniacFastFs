//! The hidden `__worker <kind>` mode: the sandboxed child of the worker supervisor.
//!
//! Both binaries that spawn workers (the console binary and the desktop binary) link this module,
//! so each can serve its own `__worker` children with the same operations. The mode is not part of
//! any command grammar: a binary recognizes it before it parses its arguments. The mode reads its
//! request from stdin and writes its response to stdout, and it writes nothing else. Every failure
//! exits with status 1 and no diagnostics.

use std::ffi::OsStr;
use std::process::ExitCode;

use jobniac_infra::workers::{WorkerKind, run_from_process, run_from_process_with};

use crate::worker_pdf;

/// The argument that selects the worker mode.
pub const WORKER_MODE: &str = "__worker";

/// Whether `first` selects the worker mode.
#[must_use]
pub fn selects_worker_mode(first: Option<&OsStr>) -> bool {
    first.is_some_and(|first| first == OsStr::new(WORKER_MODE))
}

/// Runs the worker kind named by `arguments`, which must hold exactly one kind name. The caller
/// passes the arguments that follow [`WORKER_MODE`].
pub fn run(arguments: &[&OsStr]) -> ExitCode {
    let kind = match arguments {
        [name] => name.to_str().and_then(WorkerKind::from_name),
        _ => None,
    };
    let Some(kind) = kind else {
        return ExitCode::FAILURE;
    };
    let result = match kind {
        WorkerKind::PdfExtract => run_from_process_with(kind, worker_pdf::extract),
        WorkerKind::PdfRender => run_from_process_with(kind, worker_pdf::render),
        WorkerKind::Probe | WorkerKind::FetchLink | WorkerKind::FetchArbeitnow => {
            run_from_process(kind)
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
