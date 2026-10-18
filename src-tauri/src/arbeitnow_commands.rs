//! Command adapters of the Arbeitnow screen. The fixed-board scan, the next
//! page and the one-row import run in Rust behind the Arbeitnow use cases; React sends filters and
//! opaque row ids and shows the mapped copy.
//!
//! Candidate tokens and the next-page cursor stay in the session table ([`crate::state::ArbeitnowSession`]).
//! A row id is random, issued by a scan and refused once a new scan replaces the table or after its
//! import. Failures carry only a wire code and a copy id, never service text.

use std::future::Future;
use std::sync::Arc;

use jobniac_core::arbeitnow::import::{ImportOutcome, ImportRequest, ImportStatus};
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use jobniac_core::arbeitnow::scan::{CONSISTENCY_NOTICE, ScanRequest, ScanResult, ScannedJob};
use jobniac_core::errors::{ApplicationError, ErrorCode};
use jobniac_runtime::arbeitnow::ArbeitnowServices;
use jobniac_text::str_rules::strip_text;
use tauri::State;
use uuid::Uuid;

use crate::arbeitnow_dto::{
    ArbeitnowCapabilitiesDto, ArbeitnowImportResultDto, ArbeitnowImportStatusDto, ArbeitnowPageDto,
    ArbeitnowRowDto, ArbeitnowRowStatusDto, ArbeitnowScanRequestDto,
};
use crate::error::CommandError;
use crate::job_commands::NOT_CONFIGURED;
use crate::state::{AppState, HeldRow};

/// Message id of a call to an Arbeitnow command while the services are not composed. The page is then
/// hidden, so no copy is shown for it.
pub const ARBEITNOW_UNCONFIGURED: &str = "arbeitnow.unconfigured";
/// Wire code of a refused input (the copy depends on the operation).
const VALIDATION: &str = "validation_error";
/// Jobs per scan page.
const PAGE_LIMIT: i64 = 25;
/// Code points of the description excerpt.
const EXCERPT_LIMIT: usize = 280;

/// Copy of a refused scan filter.
pub const SCAN_VALIDATION: &str = "arbeitnow.error.scan.validation";
/// Copy of a scan that failed without a listed code.
pub const SCAN_GENERIC: &str = "arbeitnow.error.scan.generic";
/// Copy of a rate-limited scan.
pub const SCAN_RATE_LIMITED: &str = "arbeitnow.error.scan.rateLimited";
/// Copy of an unavailable upstream.
pub const SCAN_UNAVAILABLE: &str = "arbeitnow.error.scan.unavailable";
/// Copy of an upstream answer that could not be used.
pub const SCAN_INVALID_RESPONSE: &str = "arbeitnow.error.scan.invalidResponse";
/// Copy of a next page whose results can no longer be continued.
pub const NEXT_VALIDATION: &str = "arbeitnow.error.next.validation";
/// Copy of an import that failed without a listed code.
pub const IMPORT_GENERIC: &str = "arbeitnow.error.import.generic";
/// Copy of an import whose row is unknown, stale, already imported or whose candidate is refused.
pub const IMPORT_VALIDATION: &str = "arbeitnow.error.import.validation";

/// The Arbeitnow use cases as the commands use them. The composed services implement it; tests
/// substitute a fake, so the command logic runs without a database or a network.
pub(crate) trait ArbeitnowPort {
    /// Scans one page of the fixed board for `request`.
    fn scan(
        &self,
        request: ScanRequest,
    ) -> impl Future<Output = Result<ScanResult, ApplicationError>> + Send;

    /// Imports the candidate named by `request`.
    fn import(
        &self,
        request: ImportRequest,
    ) -> impl Future<Output = Result<ImportOutcome, ApplicationError>> + Send;
}

impl ArbeitnowPort for ArbeitnowServices {
    fn scan(
        &self,
        request: ScanRequest,
    ) -> impl Future<Output = Result<ScanResult, ApplicationError>> + Send {
        ArbeitnowServices::scan(self, request)
    }

    fn import(
        &self,
        request: ImportRequest,
    ) -> impl Future<Output = Result<ImportOutcome, ApplicationError>> + Send {
        ArbeitnowServices::import(self, request)
    }
}

/// Whether the Arbeitnow services are composed. Answered by the runtime; a missing service hides the
/// page in React.
#[tauri::command]
#[specta::specta]
pub async fn arbeitnow_capabilities_get(
    state: State<'_, AppState>,
) -> Result<ArbeitnowCapabilitiesDto, CommandError> {
    Ok(capabilities(&state))
}

/// The capability of the state: the page is shown exactly when the services are composed.
pub(crate) fn capabilities(state: &AppState) -> ArbeitnowCapabilitiesDto {
    ArbeitnowCapabilitiesDto {
        arbeitnow: state.arbeitnow().is_some(),
    }
}

/// Scans the fixed board with the given filters. A successful scan replaces the held rows; the first
/// page of rows comes back with opaque row ids.
#[tauri::command]
#[specta::specta]
pub async fn arbeitnow_scan(
    state: State<'_, AppState>,
    request: ArbeitnowScanRequestDto,
) -> Result<ArbeitnowPageDto, CommandError> {
    let service = service(&state)?;
    run_scan(&state, service.as_ref(), &request).await
}

/// Loads the next page of the last successful scan with its filters. Only the new rows come back.
#[tauri::command]
#[specta::specta]
pub async fn arbeitnow_next_page(
    state: State<'_, AppState>,
) -> Result<ArbeitnowPageDto, CommandError> {
    let service = service(&state)?;
    run_next(&state, service.as_ref()).await
}

/// Imports the held row named by `row_id`. A row is imported once; the candidate token is used by
/// Rust and never returned.
#[tauri::command]
#[specta::specta]
pub async fn arbeitnow_import_row(
    state: State<'_, AppState>,
    row_id: String,
) -> Result<ArbeitnowImportResultDto, CommandError> {
    let service = service(&state)?;
    run_import(&state, service.as_ref(), &row_id).await
}

/// The composed services, or the unconfigured refusal.
fn service(state: &AppState) -> Result<Arc<ArbeitnowServices>, CommandError> {
    state
        .arbeitnow()
        .cloned()
        .ok_or_else(|| CommandError::new(NOT_CONFIGURED, ARBEITNOW_UNCONFIGURED))
}

/// Runs one scan under the Arbeitnow slot and replaces the session on success. A failed scan keeps
/// the earlier rows, row ids and cursor.
pub(crate) async fn run_scan<P: ArbeitnowPort + ?Sized>(
    state: &AppState,
    port: &P,
    request: &ArbeitnowScanRequestDto,
) -> Result<ArbeitnowPageDto, CommandError> {
    let _slot = state.begin_arbeitnow()?;
    let filters = scan_filters(request);
    let page = ScanRequest {
        limit: Some(PAGE_LIMIT),
        ..filters.clone()
    };
    let result = port
        .scan(page)
        .await
        .map_err(|error| scan_error(&error, SCAN_VALIDATION))?;
    let (held, rows) = held_rows(&result.jobs);
    let cursor = result.next_cursor.clone();
    let mut session = state.arbeitnow_session();
    session.replace(held, filters, cursor);
    let has_next = session.has_next();
    drop(session);
    Ok(page_dto(rows, has_next, result.consistency_notice))
}

/// Runs the next page of the held scan under the Arbeitnow slot. A call
/// without a held cursor (or with the table at its bound) starts nothing: it returns no rows and no
/// error. Only the rows of the new page come back.
pub(crate) async fn run_next<P: ArbeitnowPort + ?Sized>(
    state: &AppState,
    port: &P,
) -> Result<ArbeitnowPageDto, CommandError> {
    // The held cursor is read under the slot, like every other session mutation: a scan that replaced
    // the session between an earlier read and the take would otherwise be continued with the old
    // filters and cursor, and its rows appended to the new table. A busy slot with nothing to
    // continue still answers the empty page (that call is a no-op).
    let slot = state.begin_arbeitnow();
    if slot.is_err() && state.arbeitnow_session().continuation().is_none() {
        return Ok(page_dto(Vec::new(), false, CONSISTENCY_NOTICE));
    }
    let _slot = slot?;
    let Some((filters, cursor, room)) = state.arbeitnow_session().continuation() else {
        return Ok(page_dto(Vec::new(), false, CONSISTENCY_NOTICE));
    };
    // The page size, reduced to the room left under the bound (at least one row, by `continuation`).
    let limit = i64::try_from(room).map_or(PAGE_LIMIT, |room| room.min(PAGE_LIMIT));
    let page = ScanRequest {
        cursor: Some(cursor),
        limit: Some(limit),
        ..filters
    };
    let result = port
        .scan(page)
        .await
        .map_err(|error| scan_error(&error, NEXT_VALIDATION))?;
    let (held, rows) = held_rows(&result.jobs);
    let cursor = result.next_cursor.clone();
    let mut session = state.arbeitnow_session();
    session.append(held, cursor);
    let has_next = session.has_next();
    drop(session);
    Ok(page_dto(rows, has_next, result.consistency_notice))
}

/// Imports one held row under the Arbeitnow slot. The row is marked imported on success, so a replay
/// of its id is refused.
pub(crate) async fn run_import<P: ArbeitnowPort + ?Sized>(
    state: &AppState,
    port: &P,
    row_id: &str,
) -> Result<ArbeitnowImportResultDto, CommandError> {
    let _slot = state.begin_arbeitnow()?;
    let token = state
        .arbeitnow_session()
        .unimported_token(row_id)
        .ok_or_else(|| CommandError::new(VALIDATION, IMPORT_VALIDATION))?;
    let outcome = port
        .import(ImportRequest {
            candidate_token: Some(token),
        })
        .await
        .map_err(|error| import_error(&error))?;
    state.arbeitnow_session().mark_imported(row_id);
    Ok(ArbeitnowImportResultDto {
        row_id: row_id.to_owned(),
        status: import_status(outcome.status),
        job_id: outcome.job.job.id.to_string(),
    })
}

/// The scan filters of the form: query and location trimmed (blank is absent), flags sent only when
/// true, tags and job types split on commas with empties dropped.
fn scan_filters(request: &ArbeitnowScanRequestDto) -> ScanRequest {
    ScanRequest {
        query: trimmed(request.query.as_deref()),
        location: trimmed(request.location.as_deref()),
        remote_only: request.remote_only.then_some(true),
        visa_sponsorship: request.visa_sponsorship.then_some(true),
        tags: split_list(&request.tags),
        job_types: split_list(&request.job_types),
        limit: None,
        max_pages: None,
        cursor: None,
    }
}

/// The stripped text, or `None` when nothing remains.
fn trimmed(text: Option<&str>) -> Option<String> {
    text.map(strip_text)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The comma-separated items, each stripped, empties dropped; `None` when none remains.
fn split_list(text: &str) -> Option<Vec<String>> {
    let items: Vec<String> = text
        .split(',')
        .map(strip_text)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect();
    (!items.is_empty()).then_some(items)
}

/// Assigns a fresh random row id to each job and keeps its token in the held row. The DTO carries the
/// id and the public fields only.
fn held_rows(jobs: &[ScannedJob]) -> (Vec<HeldRow>, Vec<ArbeitnowRowDto>) {
    jobs.iter()
        .map(|job| {
            let row_id = Uuid::new_v4().to_string();
            let dto = row_dto(job, row_id.clone());
            let held = HeldRow {
                row_id,
                token: job.candidate_token.clone(),
                imported: false,
            };
            (held, dto)
        })
        .unzip()
}

fn row_dto(job: &ScannedJob, row_id: String) -> ArbeitnowRowDto {
    ArbeitnowRowDto {
        row_id,
        title: job.title.clone(),
        company: job.company.clone(),
        location: job.location.clone(),
        remote: job.remote,
        posted_at: job.posted_at.as_ref().and_then(posted_date),
        excerpt: job
            .description_excerpt
            .chars()
            .take(EXCERPT_LIMIT)
            .collect(),
        tags: job.tags.clone(),
        job_types: job.job_types.clone(),
        import_status: ArbeitnowRowStatusDto::None,
    }
}

/// The UTC date `YYYY-MM-DD` of an instant within years 1 to 9999; `None` otherwise.
fn posted_date(instant: &ArbeitnowInstant) -> Option<String> {
    let moment = time::OffsetDateTime::from_unix_timestamp(instant.epoch_second()).ok()?;
    let date = moment.date();
    (1..=9999).contains(&date.year()).then(|| {
        format!(
            "{:04}-{:02}-{:02}",
            date.year(),
            u8::from(date.month()),
            date.day()
        )
    })
}

fn page_dto(rows: Vec<ArbeitnowRowDto>, has_next_page: bool, notice: &str) -> ArbeitnowPageDto {
    ArbeitnowPageDto {
        rows,
        has_next_page,
        consistency_notice: notice.to_owned(),
    }
}

fn import_status(status: ImportStatus) -> ArbeitnowImportStatusDto {
    match status {
        ImportStatus::CreatedJob => ArbeitnowImportStatusDto::CreatedJob,
        ImportStatus::ReusedExistingJob => ArbeitnowImportStatusDto::ReusedExistingJob,
    }
}

/// Maps a scan failure to its copy. `validation` is the copy of a refused input (the scan form or
/// the next page). The wire code is kept; the service message is not.
fn scan_error(error: &ApplicationError, validation: &'static str) -> CommandError {
    let code = error.code();
    let copy = match code {
        ErrorCode::ValidationError => validation,
        ErrorCode::UpstreamRateLimited => SCAN_RATE_LIMITED,
        ErrorCode::UpstreamUnavailable => SCAN_UNAVAILABLE,
        ErrorCode::UpstreamInvalidResponse => SCAN_INVALID_RESPONSE,
        _ => SCAN_GENERIC,
    };
    CommandError::new(code.code(), copy)
}

/// Maps an import failure to its copy: a refused candidate has the validation copy, anything else the
/// generic one.
fn import_error(error: &ApplicationError) -> CommandError {
    let code = error.code();
    let copy = match code {
        ErrorCode::ValidationError => IMPORT_VALIDATION,
        _ => IMPORT_GENERIC,
    };
    CommandError::new(code.code(), copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ARBEITNOW_BUSY, ARBEITNOW_CLOSE};
    use jobniac_core::env::Environment;
    use jobniac_core::errors::validation_error;
    use jobniac_core::jobs::model::{JobAggregate, JobRecord};
    use jobniac_runtime::services::StartupError;

    /// Synthetic signing settings for the composition test (32 distinct bytes, a 40-byte cursor secret).
    const SYNTHETIC_CANDIDATE_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    const SYNTHETIC_CURSOR_SECRET: &str = "cursor-secret-0123456789abcdefghijklmnop";
    use serde_json::Value;
    use std::sync::Mutex;
    use time::OffsetDateTime;

    /// Synthetic values that must never reach a DTO, an error or a copy id.
    const TOKEN_A: &str = "synthetic-candidate-token-aaaa";
    const TOKEN_B: &str = "synthetic-candidate-token-bbbb";
    const CURSOR: &str = "synthetic-cursor-value";
    const JOB_UUID: &str = "00000000-0000-4000-8000-0000000000d1";

    /// A fake board: answers scans and imports from queues and records every request it receives.
    #[derive(Default)]
    struct FakePort {
        scans: Mutex<Vec<Result<ScanResult, ApplicationError>>>,
        imports: Mutex<Vec<Result<ImportOutcome, ApplicationError>>>,
        scan_requests: Mutex<Vec<ScanRequest>>,
        import_tokens: Mutex<Vec<Option<String>>>,
    }

    impl FakePort {
        fn with_scans(scans: Vec<Result<ScanResult, ApplicationError>>) -> Self {
            Self {
                scans: Mutex::new(scans),
                ..Self::default()
            }
        }

        fn with_imports(imports: Vec<Result<ImportOutcome, ApplicationError>>) -> Self {
            Self {
                imports: Mutex::new(imports),
                ..Self::default()
            }
        }

        fn scan_requests(&self) -> Vec<ScanRequest> {
            lock(&self.scan_requests)
        }

        fn import_tokens(&self) -> Vec<Option<String>> {
            lock(&self.import_tokens)
        }
    }

    fn lock<T: Clone>(mutex: &Mutex<T>) -> T {
        match mutex.lock() {
            Ok(guard) => (*guard).clone(),
            Err(poisoned) => (*poisoned.into_inner()).clone(),
        }
    }

    impl ArbeitnowPort for FakePort {
        fn scan(
            &self,
            request: ScanRequest,
        ) -> impl Future<Output = Result<ScanResult, ApplicationError>> + Send {
            if let Ok(mut requests) = self.scan_requests.lock() {
                requests.push(request);
            }
            let answer = self.scans.lock().ok().and_then(|mut queue| {
                if queue.is_empty() {
                    None
                } else {
                    Some(queue.remove(0))
                }
            });
            std::future::ready(
                answer.unwrap_or_else(|| Err(ApplicationError::of(ErrorCode::InternalError))),
            )
        }

        fn import(
            &self,
            request: ImportRequest,
        ) -> impl Future<Output = Result<ImportOutcome, ApplicationError>> + Send {
            if let Ok(mut tokens) = self.import_tokens.lock() {
                tokens.push(request.candidate_token);
            }
            let answer = self.imports.lock().ok().and_then(|mut queue| {
                if queue.is_empty() {
                    None
                } else {
                    Some(queue.remove(0))
                }
            });
            std::future::ready(
                answer.unwrap_or_else(|| Err(ApplicationError::of(ErrorCode::InternalError))),
            )
        }
    }

    fn job(title: &str, token: &str, excerpt: &str) -> ScannedJob {
        ScannedJob {
            slug: "example-slug".to_owned(),
            canonical_url: "https://www.arbeitnow.com/jobs/example-slug".to_owned(),
            company: "Example Labs".to_owned(),
            title: title.to_owned(),
            location: "Berlin".to_owned(),
            remote: true,
            posted_at: ArbeitnowInstant::new(0, 0).ok(),
            tags: vec!["rust".to_owned()],
            job_types: vec!["full-time".to_owned()],
            description_excerpt: excerpt.to_owned(),
            candidate_token: token.to_owned(),
        }
    }

    fn scan_result(jobs: Vec<ScannedJob>, next: Option<&str>) -> ScanResult {
        ScanResult {
            source: "arbeitnow",
            source_docs_url: "https://www.arbeitnow.com/api/job-board-api",
            jobs,
            pages_scanned: 1,
            records_inspected: 1,
            next_cursor: next.map(str::to_owned),
            consistency_notice: "Upstream results are mutable; this scan is not a snapshot.",
        }
    }

    fn outcome(status: ImportStatus) -> ImportOutcome {
        let when = OffsetDateTime::UNIX_EPOCH;
        ImportOutcome {
            status,
            job: JobAggregate {
                job: JobRecord {
                    id: Uuid::parse_str(JOB_UUID).unwrap_or(Uuid::nil()),
                    source_method: "link".to_owned(),
                    source_label: None,
                    title: "Synthetic Engineer".to_owned(),
                    company: Some("Example Labs".to_owned()),
                    location: None,
                    description: "Synthetic description".to_owned(),
                    experience_requirement: None,
                    employment_type: None,
                    seniority: None,
                    posted_at: None,
                    canonical_fingerprint: "fingerprint".to_owned(),
                    created_at: when,
                    updated_at: when,
                    revision: 1,
                },
                skills: Vec::new(),
                link_ingestion: None,
                text_ingestion: None,
            },
        }
    }

    fn form() -> ArbeitnowScanRequestDto {
        ArbeitnowScanRequestDto {
            query: None,
            location: None,
            remote_only: false,
            visa_sponsorship: false,
            tags: String::new(),
            job_types: String::new(),
        }
    }

    fn state() -> AppState {
        AppState::new(Err(StartupError))
    }

    fn row_ids(page: &ArbeitnowPageDto) -> Vec<String> {
        page.rows.iter().map(|row| row.row_id.clone()).collect()
    }

    fn json_of<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap_or_default()
    }

    fn error_json(error: &CommandError) -> String {
        json_of(error)
    }

    #[tokio::test]
    async fn a_scan_returns_rows_with_opaque_ids_and_no_token_or_cursor() -> Result<(), CommandError>
    {
        let state = state();
        let port = FakePort::with_scans(vec![Ok(scan_result(
            vec![job("Rust Engineer", TOKEN_A, "Short")],
            Some(CURSOR),
        ))]);
        let page = run_scan(&state, &port, &form()).await?;
        assert_eq!(page.rows.len(), 1);
        assert!(page.has_next_page);
        assert_ne!(page.rows[0].row_id, TOKEN_A);
        let serialized = json_of(&page);
        assert!(!serialized.contains(TOKEN_A));
        assert!(!serialized.contains(CURSOR));
        assert!(!serialized.contains("candidateToken"));
        assert!(!serialized.contains("nextCursor"));
        assert!(!serialized.contains("url"));
        Ok(())
    }

    #[tokio::test]
    async fn the_scan_form_is_mapped_to_trimmed_filters() -> Result<(), CommandError> {
        let state = state();
        let port = FakePort::with_scans(vec![Ok(scan_result(Vec::new(), None))]);
        let request = ArbeitnowScanRequestDto {
            query: Some("  rust  ".to_owned()),
            location: Some("   ".to_owned()),
            remote_only: true,
            visa_sponsorship: false,
            tags: " rust , ,go,".to_owned(),
            job_types: " , ".to_owned(),
        };
        run_scan(&state, &port, &request).await?;
        let sent = port.scan_requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].query.as_deref(), Some("rust"));
        assert_eq!(sent[0].location, None);
        assert_eq!(sent[0].remote_only, Some(true));
        assert_eq!(sent[0].visa_sponsorship, None);
        assert_eq!(sent[0].tags, Some(vec!["rust".to_owned(), "go".to_owned()]));
        assert_eq!(sent[0].job_types, None);
        assert_eq!(sent[0].limit, Some(PAGE_LIMIT));
        assert_eq!(sent[0].cursor, None);
        Ok(())
    }

    #[tokio::test]
    async fn the_next_page_uses_the_held_cursor_and_appends_rows() -> Result<(), CommandError> {
        let state = state();
        let port = FakePort::with_scans(vec![
            Ok(scan_result(vec![job("First", TOKEN_A, "")], Some(CURSOR))),
            Ok(scan_result(vec![job("Second", TOKEN_B, "")], None)),
        ]);
        let first = run_scan(&state, &port, &form()).await?;
        let second = run_next(&state, &port).await?;
        let sent = port.scan_requests();
        assert_eq!(sent[1].cursor.as_deref(), Some(CURSOR));
        assert_eq!(sent[1].limit, Some(PAGE_LIMIT));
        assert_eq!(second.rows.len(), 1);
        assert!(!second.has_next_page);
        assert!(!json_of(&second).contains(CURSOR));
        // Both pages keep their own ids; the first page's id is still importable.
        assert_ne!(row_ids(&first)[0], row_ids(&second)[0]);
        Ok(())
    }

    #[tokio::test]
    async fn a_next_page_without_a_cursor_is_a_noop() -> Result<(), CommandError> {
        // A next page without a cursor starts no operation: no rows, no error, no call.
        let state = state();
        let port =
            FakePort::with_scans(vec![Ok(scan_result(vec![job("Only", TOKEN_A, "")], None))]);
        run_scan(&state, &port, &form()).await?;
        let page = run_next(&state, &port).await?;
        assert!(page.rows.is_empty());
        assert!(!page.has_next_page);
        assert_eq!(page.consistency_notice, CONSISTENCY_NOTICE);
        assert_eq!(port.scan_requests().len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn held_rows_stop_at_the_bound_and_the_last_page_gets_the_room_left()
    -> Result<(), CommandError> {
        // Five pages of 100 rows reach MAX_HELD_ROWS (500); the next call starts nothing.
        let state = state();
        let batch = |prefix: &str| -> Vec<ScannedJob> {
            (0..100)
                .map(|index| {
                    job(
                        &format!("{prefix}{index}"),
                        &format!("{prefix}{index}.t"),
                        "",
                    )
                })
                .collect()
        };
        let port = FakePort::with_scans(
            (0..5)
                .map(|page| Ok(scan_result(batch(&format!("p{page}-")), Some(CURSOR))))
                .collect(),
        );
        run_scan(&state, &port, &form()).await?;
        let mut last = None;
        for _ in 0..4 {
            last = Some(run_next(&state, &port).await?);
        }
        let last = last.ok_or_else(|| CommandError::new(VALIDATION, NEXT_VALIDATION))?;
        assert!(!last.has_next_page);
        let noop = run_next(&state, &port).await?;
        assert!(noop.rows.is_empty());
        assert!(!noop.has_next_page);
        // Four next pages were requested; the bound is checked before the port is called.
        let sent = port.scan_requests();
        assert_eq!(sent.len(), 5);
        assert!(
            sent[1..]
                .iter()
                .all(|request| request.limit == Some(PAGE_LIMIT))
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_page_limit_shrinks_to_the_room_left_under_the_bound() -> Result<(), CommandError> {
        let state = state();
        let batch = |prefix: &str, count: i64| -> Vec<ScannedJob> {
            (0..count)
                .map(|index| {
                    job(
                        &format!("{prefix}{index}"),
                        &format!("{prefix}{index}.t"),
                        "",
                    )
                })
                .collect()
        };
        // The scan and three next pages hold 400 rows, the fourth next page 80 (480 rows). The fifth
        // asks for the 20 rows left under the bound and gets them: the table is full.
        let port = FakePort::with_scans(vec![
            Ok(scan_result(batch("a", 100), Some(CURSOR))),
            Ok(scan_result(batch("b", 100), Some(CURSOR))),
            Ok(scan_result(batch("c", 100), Some(CURSOR))),
            Ok(scan_result(batch("d", 100), Some(CURSOR))),
            Ok(scan_result(batch("e", 80), Some(CURSOR))),
            Ok(scan_result(batch("f", 20), Some(CURSOR))),
        ]);
        run_scan(&state, &port, &form()).await?;
        for _ in 0..4 {
            run_next(&state, &port).await?;
        }
        let page = run_next(&state, &port).await?;
        assert_eq!(page.rows.len(), 20);
        assert!(!page.has_next_page);
        let sent = port.scan_requests();
        assert_eq!(sent.len(), 6);
        assert_eq!(sent[5].limit, Some(20));
        Ok(())
    }

    #[tokio::test]
    async fn a_stale_row_id_after_a_new_scan_is_refused() -> Result<(), CommandError> {
        let state = state();
        let port = FakePort::with_scans(vec![
            Ok(scan_result(vec![job("Old", TOKEN_A, "")], None)),
            Ok(scan_result(vec![job("New", TOKEN_B, "")], None)),
        ]);
        let old = run_scan(&state, &port, &form()).await?;
        let _new = run_scan(&state, &port, &form()).await?;
        let stale = run_import(&state, &FakePort::default(), &old.rows[0].row_id)
            .await
            .err();
        assert_eq!(
            stale,
            Some(CommandError::new(VALIDATION, IMPORT_VALIDATION))
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_replayed_row_id_is_refused_and_the_token_is_used_once() -> Result<(), CommandError> {
        let state = state();
        let scans =
            FakePort::with_scans(vec![Ok(scan_result(vec![job("Once", TOKEN_A, "")], None))]);
        let page = run_scan(&state, &scans, &form()).await?;
        let row_id = page.rows[0].row_id.clone();
        let port = FakePort::with_imports(vec![Ok(outcome(ImportStatus::CreatedJob))]);
        let result = run_import(&state, &port, &row_id).await?;
        assert_eq!(result.status, ArbeitnowImportStatusDto::CreatedJob);
        assert_eq!(result.job_id, JOB_UUID);
        let replay = run_import(&state, &port, &row_id).await.err();
        assert_eq!(
            replay,
            Some(CommandError::new(VALIDATION, IMPORT_VALIDATION))
        );
        assert_eq!(port.import_tokens(), vec![Some(TOKEN_A.to_owned())]);
        Ok(())
    }

    #[tokio::test]
    async fn an_unknown_row_id_is_refused_before_the_port() {
        let state = state();
        let port = FakePort::with_imports(Vec::new());
        let error = run_import(&state, &port, "not-a-held-row").await.err();
        assert_eq!(
            error,
            Some(CommandError::new(VALIDATION, IMPORT_VALIDATION))
        );
        assert!(port.import_tokens().is_empty());
    }

    #[tokio::test]
    async fn a_reused_import_reports_the_reused_status_without_a_token() -> Result<(), CommandError>
    {
        let state = state();
        let scans =
            FakePort::with_scans(vec![Ok(scan_result(vec![job("Saved", TOKEN_A, "")], None))]);
        let page = run_scan(&state, &scans, &form()).await?;
        let port = FakePort::with_imports(vec![Ok(outcome(ImportStatus::ReusedExistingJob))]);
        let result = run_import(&state, &port, &page.rows[0].row_id).await?;
        assert_eq!(result.status, ArbeitnowImportStatusDto::ReusedExistingJob);
        assert!(!json_of(&result).contains(TOKEN_A));
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_scan_keeps_the_held_rows_and_their_ids() -> Result<(), CommandError> {
        let state = state();
        let port = FakePort::with_scans(vec![
            Ok(scan_result(vec![job("Kept", TOKEN_A, "")], Some(CURSOR))),
            Err(ApplicationError::of(ErrorCode::UpstreamUnavailable)),
        ]);
        let page = run_scan(&state, &port, &form()).await?;
        let error = run_scan(&state, &port, &form()).await.err();
        assert_eq!(
            error,
            Some(CommandError::new("upstream_unavailable", SCAN_UNAVAILABLE))
        );
        // The earlier row is still importable and the cursor still continues.
        let continued = run_next(
            &state,
            &FakePort::with_scans(vec![Ok(scan_result(Vec::new(), None))]),
        )
        .await?;
        assert!(continued.rows.is_empty());
        let import = FakePort::with_imports(vec![Ok(outcome(ImportStatus::CreatedJob))]);
        assert!(
            run_import(&state, &import, &page.rows[0].row_id)
                .await
                .is_ok()
        );
        Ok(())
    }

    #[tokio::test]
    async fn every_scan_failure_has_its_own_copy() {
        let cases = [
            (ErrorCode::ValidationError, VALIDATION, SCAN_VALIDATION),
            (
                ErrorCode::UpstreamRateLimited,
                "upstream_rate_limited",
                SCAN_RATE_LIMITED,
            ),
            (
                ErrorCode::UpstreamUnavailable,
                "upstream_unavailable",
                SCAN_UNAVAILABLE,
            ),
            (
                ErrorCode::UpstreamInvalidResponse,
                "upstream_invalid_response",
                SCAN_INVALID_RESPONSE,
            ),
            (ErrorCode::InternalError, "internal_error", SCAN_GENERIC),
        ];
        for (code, wire, copy) in cases {
            let state = state();
            let port = FakePort::with_scans(vec![Err(ApplicationError::of(code))]);
            let error = run_scan(&state, &port, &form()).await.err();
            assert_eq!(error, Some(CommandError::new(wire, copy)));
        }
    }

    #[tokio::test]
    async fn a_next_page_validation_failure_has_the_continuation_copy() {
        let state = state();
        let port = FakePort::with_scans(vec![
            Ok(scan_result(vec![job("Rows", TOKEN_A, "")], Some(CURSOR))),
            Err(ApplicationError::new(
                ErrorCode::ValidationError,
                Some("cursor refused"),
                std::iter::empty::<(&str, &str)>(),
            )),
        ]);
        assert!(run_scan(&state, &port, &form()).await.is_ok());
        let error = run_next(&state, &port).await.err();
        assert_eq!(error, Some(CommandError::new(VALIDATION, NEXT_VALIDATION)));
    }

    #[tokio::test]
    async fn an_import_failure_has_the_import_copy_and_no_service_text() -> Result<(), CommandError>
    {
        let state = state();
        let scans =
            FakePort::with_scans(vec![Ok(scan_result(vec![job("Row", TOKEN_A, "")], None))]);
        let page = run_scan(&state, &scans, &form()).await?;
        let port = FakePort::with_imports(vec![Err(validation_error(
            &format!("expired {TOKEN_A}"),
            Some("candidateToken"),
            Some("expired"),
        ))]);
        let error = run_import(&state, &port, &page.rows[0].row_id).await.err();
        let error = error.unwrap_or_else(|| CommandError::new("", ""));
        assert_eq!(error, CommandError::new(VALIDATION, IMPORT_VALIDATION));
        assert!(!error_json(&error).contains(TOKEN_A));
        // A failed import leaves the row importable.
        let retry = FakePort::with_imports(vec![Ok(outcome(ImportStatus::CreatedJob))]);
        assert!(
            run_import(&state, &retry, &page.rows[0].row_id)
                .await
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn the_capability_follows_the_composition() -> Result<(), StartupError> {
        // Not composed: the flag is absent, so the page stays hidden.
        assert!(!capabilities(&AppState::new(Err(StartupError))).arbeitnow);
        let plain = jobniac_runtime::services::compose(&Environment::from_pairs(
            std::iter::empty::<(&str, &str)>(),
        ))?;
        assert!(!capabilities(&AppState::new(Ok(plain))).arbeitnow);

        // Composed: the flag is exactly 1, the keys are valid and the database opens.
        let directory = tempfile::tempdir().map_err(|_| StartupError)?;
        let file = directory.path().join("jobniac.sqlite3");
        let pairs = [
            ("JOBNIAC_ARBEITNOW_ENABLED", "1"),
            ("JOBNIAC_ARBEITNOW_CANDIDATE_KEY", SYNTHETIC_CANDIDATE_KEY),
            ("JOBNIAC_CURSOR_HMAC_SECRET", SYNTHETIC_CURSOR_SECRET),
            ("JOBNIAC_DB_PATH", file.to_str().ok_or(StartupError)?),
        ];
        let environment = Environment::from_pairs(pairs);
        let services = jobniac_runtime::services::compose(&environment)?;
        let arbeitnow =
            jobniac_runtime::arbeitnow::compose_arbeitnow(&environment, services.database.as_ref())
                .map_err(|_| StartupError)?;
        let state = AppState::new(Ok(services)).with_arbeitnow(arbeitnow);
        assert!(capabilities(&state).arbeitnow);
        Ok(())
    }

    #[tokio::test]
    async fn a_next_page_without_a_cursor_is_the_empty_page_even_while_the_slot_is_busy()
    -> Result<(), CommandError> {
        let state = state();
        let port = FakePort::with_scans(Vec::new());
        let _held = state.begin_arbeitnow()?;
        let page = run_next(&state, &port).await?;
        assert!(page.rows.is_empty() && !page.has_next_page);
        assert!(port.scan_requests().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_busy_slot_refuses_every_operation_and_releases_it() -> Result<(), CommandError> {
        let state = state();
        let port = FakePort::with_scans(vec![
            Ok(scan_result(vec![job("Row", TOKEN_A, "")], Some(CURSOR))),
            Ok(scan_result(vec![job("Again", TOKEN_B, "")], None)),
        ]);
        run_scan(&state, &port, &form()).await?;
        {
            let _held = state.begin_arbeitnow()?;
            assert_eq!(
                run_scan(&state, &port, &form()).await.err(),
                Some(CommandError::busy(ARBEITNOW_BUSY))
            );
            // A held cursor makes the next page an operation, so the busy slot refuses it.
            assert_eq!(
                run_next(&state, &port).await.err(),
                Some(CommandError::busy(ARBEITNOW_BUSY))
            );
            assert_eq!(
                run_import(&state, &port, "any").await.err(),
                Some(CommandError::busy(ARBEITNOW_BUSY))
            );
        }
        assert_eq!(port.scan_requests().len(), 1);
        assert!(run_scan(&state, &port, &form()).await.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn a_running_operation_refuses_the_close_first_in_the_gate() -> Result<(), CommandError> {
        let state = state();
        assert!(state.close_refusal().is_none());
        let slot = state.begin_arbeitnow()?;
        let refusal = state.close_refusal();
        assert_eq!(refusal.as_ref().map(|r| r.workspace), Some("arbeitnow"));
        assert_eq!(
            refusal.as_ref().map(|r| r.message_id),
            Some(ARBEITNOW_CLOSE)
        );
        drop(slot);
        assert!(state.close_refusal().is_none());
        Ok(())
    }

    #[tokio::test]
    async fn clearing_the_session_drops_rows_and_cursor() -> Result<(), CommandError> {
        let state = state();
        let port = FakePort::with_scans(vec![Ok(scan_result(
            vec![job("Row", TOKEN_A, "")],
            Some(CURSOR),
        ))]);
        let page = run_scan(&state, &port, &form()).await?;
        state.clear_arbeitnow_session();
        let import = FakePort::with_imports(Vec::new());
        assert_eq!(
            run_import(&state, &import, &page.rows[0].row_id)
                .await
                .err(),
            Some(CommandError::new(VALIDATION, IMPORT_VALIDATION))
        );
        let after = run_next(&state, &port).await?;
        assert!(after.rows.is_empty());
        assert_eq!(port.scan_requests().len(), 1);
        Ok(())
    }

    #[test]
    fn the_session_debug_text_redacts_tokens_and_cursors() {
        let mut session = crate::state::ArbeitnowSession::default();
        session.replace(
            vec![HeldRow {
                row_id: "row".to_owned(),
                token: TOKEN_A.to_owned(),
                imported: false,
            }],
            ScanRequest::default(),
            Some(CURSOR.to_owned()),
        );
        let text = format!("{session:?}");
        assert!(!text.contains(TOKEN_A));
        assert!(!text.contains(CURSOR));
    }

    #[test]
    fn the_posted_date_is_utc_and_bounded_to_years_one_to_nine_thousand_nine_hundred_ninety_nine() {
        let epoch = ArbeitnowInstant::new(0, 0).ok();
        assert_eq!(
            epoch.as_ref().and_then(posted_date).as_deref(),
            Some("1970-01-01")
        );
        let late = ArbeitnowInstant::new(253_402_300_799, 0).ok();
        assert_eq!(
            late.as_ref().and_then(posted_date).as_deref(),
            Some("9999-12-31")
        );
        let beyond = ArbeitnowInstant::new(253_402_300_800, 0).ok();
        assert_eq!(beyond.as_ref().and_then(posted_date), None);
    }

    #[test]
    fn the_excerpt_is_cut_to_280_code_points() {
        let long: String = "é".repeat(300);
        let row = row_dto(&job("Title", TOKEN_A, &long), "id".to_owned());
        assert_eq!(row.excerpt.chars().count(), EXCERPT_LIMIT);
    }

    #[test]
    fn the_service_absence_is_the_unconfigured_refusal() {
        let state = state();
        assert_eq!(
            service(&state).err(),
            Some(CommandError::new(NOT_CONFIGURED, ARBEITNOW_UNCONFIGURED))
        );
    }

    #[test]
    fn a_json_value_of_a_row_has_only_the_public_keys() -> Result<(), serde_json::Error> {
        let row = row_dto(&job("Title", TOKEN_A, "text"), "id".to_owned());
        let value: Value = serde_json::to_value(&row)?;
        let keys: Vec<&str> = value
            .as_object()
            .map(|map| map.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert!(keys.contains(&"rowId"));
        assert!(!keys.contains(&"candidateToken"));
        assert!(!keys.contains(&"url"));
        Ok(())
    }
}
