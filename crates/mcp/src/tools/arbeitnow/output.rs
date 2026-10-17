//! The camelCase output shapes of the Arbeitnow tools (`_scan_result`, `_scanned_job` and the import
//! `{status, job}` result).

use jobniac_core::arbeitnow::import::ImportOutcome;
use jobniac_core::arbeitnow::instant::format_java_instant;
use jobniac_core::arbeitnow::scan::{ScanResult, ScannedJob};
use jobniac_core::errors::AppError;
use serde_json::{Map, Value, json};

/// One scanned job. `postedAt` is the Java instant text or `null`.
fn scanned_job(job: &ScannedJob) -> Value {
    json!({
        "slug": job.slug,
        "canonicalUrl": job.canonical_url,
        "company": job.company,
        "title": job.title,
        "location": job.location,
        "remote": job.remote,
        "postedAt": job.posted_at.map(format_java_instant),
        "tags": job.tags,
        "jobTypes": job.job_types,
        "descriptionExcerpt": job.description_excerpt,
        "candidateToken": job.candidate_token,
    })
}

/// The scan envelope, with its keys in a fixed order.
pub(super) fn scan_result(result: &ScanResult) -> Value {
    json!({
        "source": result.source,
        "sourceDocsUrl": result.source_docs_url,
        "jobs": result.jobs.iter().map(scanned_job).collect::<Vec<_>>(),
        "pagesScanned": result.pages_scanned,
        "recordsInspected": result.records_inspected,
        "nextCursor": result.next_cursor,
        "consistencyNotice": result.consistency_notice,
    })
}

/// The import result `{status, job}`, the job being the aggregate as the job tools serialize it.
pub(super) fn import_outcome(outcome: &ImportOutcome) -> Result<Value, AppError> {
    let job = serde_json::to_value(&outcome.job).map_err(|_| AppError::Internal)?;
    let mut object = Map::new();
    object.insert(
        "status".to_owned(),
        Value::String(outcome.status.as_str().to_owned()),
    );
    object.insert("job".to_owned(), job);
    Ok(Value::Object(object))
}
