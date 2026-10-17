//! The worker kind table: names, compiled limits (including the rendering limits), markers, and the
//! admission checks of the supervisor that run before any child exists.

use std::error::Error;
use std::ffi::OsString;
use std::future::pending;
use std::path::PathBuf;
use std::time::Duration;

use jobniac_infra::workers::child::marker_matches;
use jobniac_infra::workers::{WorkerErrorKind, WorkerKind, WorkerSupervisor};

type TestResult = Result<(), Box<dyn Error>>;

const MIB: u64 = 1024 * 1024;

#[test]
fn names_round_trip_and_unknown_names_are_refused() {
    for kind in WorkerKind::ALL {
        assert_eq!(WorkerKind::from_name(kind.name()), Some(kind));
    }
    assert_eq!(WorkerKind::from_name("pdf"), None);
    assert_eq!(WorkerKind::from_name("fetch"), None);
    assert_eq!(WorkerKind::from_name("fetch-link "), None);
    assert_eq!(WorkerKind::from_name("FETCH-LINK"), None);
    assert_eq!(WorkerKind::from_name(""), None);
}

#[test]
fn extraction_limits_are_the_compiled_limits() {
    let limits = WorkerKind::PdfExtract.limits();
    assert_eq!(limits.memory_bytes, 512 * MIB);
    assert_eq!(limits.cpu_seconds, 10);
    assert_eq!(limits.wall, Duration::from_secs(15));
    assert_eq!(limits.output_bytes, 16 * MIB);
    assert_eq!(limits.open_files, 64);
}

#[test]
fn extraction_request_limit_admits_a_full_import_sized_pdf() -> TestResult {
    // The transport bound must admit the header, the largest directory and a PDF of exactly the
    // import limit, plus one byte so that an oversized PDF reaches the size check.
    let header = 1 + 2 + jobniac_infra::workers::pdf::MAX_PDFIUM_DIR_BYTES;
    let import_limit = usize::try_from(jobniac_infra::files::import_root::MAX_IMPORT_BYTES)?;
    assert_eq!(
        WorkerKind::PdfExtract.request_limit(),
        header + import_limit + 1
    );
    Ok(())
}

#[test]
fn rendering_limits_are_those_of_the_preview_worker() {
    let limits = WorkerKind::PdfRender.limits();
    assert_eq!(limits.memory_bytes, 1024 * MIB);
    assert_eq!(limits.cpu_seconds, 10);
    assert_eq!(limits.wall, Duration::from_secs(15));
    // MAX_RESPONSE_BYTES (1600 * 1600 * 4 + 65536 + 12) plus the four-byte frame prefix.
    assert_eq!(limits.output_bytes, 10_305_552);
}

#[test]
fn every_kind_has_a_positive_bounded_profile() {
    for kind in WorkerKind::ALL {
        let limits = kind.limits();
        assert!(limits.memory_bytes > 0, "{}", kind.name());
        assert!(limits.cpu_seconds > 0, "{}", kind.name());
        assert!(!limits.wall.is_zero(), "{}", kind.name());
        assert!(limits.output_bytes > 4, "{}", kind.name());
        assert!(kind.max_concurrent() >= 1, "{}", kind.name());
        assert!(kind.request_limit() < usize::try_from(u32::MAX).unwrap_or(0));
    }
}

#[test]
fn only_the_probe_has_an_operation_in_this_build() {
    assert!(WorkerKind::Probe.handler().is_some());
    assert!(WorkerKind::PdfExtract.handler().is_none());
    assert!(WorkerKind::PdfRender.handler().is_none());
}

#[test]
fn marker_identifies_exactly_its_own_kind() {
    for kind in WorkerKind::ALL {
        let marker = OsString::from(kind.marker_value());
        assert!(marker_matches(kind, Some(&marker)), "{}", kind.name());
        for other in WorkerKind::ALL.into_iter().filter(|other| *other != kind) {
            assert!(!marker_matches(other, Some(&marker)));
        }
    }
    assert!(!marker_matches(WorkerKind::Probe, None));
    assert!(!marker_matches(
        WorkerKind::Probe,
        Some(&OsString::from("probe"))
    ));
}

#[tokio::test]
async fn oversized_request_is_refused_before_any_child_starts() -> TestResult {
    let supervisor = WorkerSupervisor::new(PathBuf::from("/nonexistent/jobniac"), Vec::new());
    let request = vec![b' '; WorkerKind::PdfRender.request_limit() + 1];
    let result = supervisor
        .run(WorkerKind::PdfRender, &request, pending())
        .await;
    assert_eq!(
        result.err().map(|error| error.kind()),
        Some(WorkerErrorKind::InvalidRequest)
    );
    assert_eq!(
        supervisor.available_permits(WorkerKind::PdfRender),
        WorkerKind::PdfRender.max_concurrent()
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_before_admission_starts_nothing() -> TestResult {
    let supervisor = WorkerSupervisor::new(PathBuf::from("/nonexistent/jobniac"), Vec::new());
    let result = supervisor
        .run(WorkerKind::Probe, b"{}", std::future::ready(()))
        .await;
    assert_eq!(
        result.err().map(|error| error.kind()),
        Some(WorkerErrorKind::Cancelled)
    );
    assert_eq!(
        supervisor.available_permits(WorkerKind::Probe),
        WorkerKind::Probe.max_concurrent()
    );
    Ok(())
}

/// Without a SYSTEMROOT directory the Windows child is not started (fail closed).
#[cfg(windows)]
#[tokio::test]
async fn missing_system_root_fails_closed_before_spawning() -> TestResult {
    let environment: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| !name.to_string_lossy().eq_ignore_ascii_case("SYSTEMROOT"))
        .collect();
    let program = std::env::temp_dir().join("jobniac-no-such-worker-binary");
    let supervisor = WorkerSupervisor::new(program, environment);
    let result = supervisor.run(WorkerKind::Probe, b"{}", pending()).await;
    assert_eq!(
        result.err().map(|error| error.kind()),
        Some(WorkerErrorKind::SetupFailed)
    );
    Ok(())
}

#[tokio::test]
async fn missing_program_is_a_spawn_failure_and_releases_its_permit() -> TestResult {
    let environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let program = std::env::temp_dir().join("jobniac-no-such-worker-binary");
    let supervisor = WorkerSupervisor::new(program, environment);
    let result = supervisor.run(WorkerKind::Probe, b"{}", pending()).await;
    let kind = result.err().map(|error| error.kind());
    assert!(
        matches!(
            kind,
            Some(WorkerErrorKind::SpawnFailed | WorkerErrorKind::SetupFailed)
        ),
        "unexpected category {kind:?}"
    );
    assert_eq!(
        supervisor.available_permits(WorkerKind::Probe),
        WorkerKind::Probe.max_concurrent()
    );
    Ok(())
}
