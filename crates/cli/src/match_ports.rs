//! The MCP matching and review ports over the SQLite-backed use cases.
//!
//! Like [`crate::job_ports`], the composition root is the only place that knows both the MCP ports
//! (`jobniac-mcp`) and the concrete use cases (`jobniac-core` over `jobniac-infra`). Each adapter
//! wraps one concrete use case and converts its typed result into the service dictionary the
//! tools return.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::matching::MatchAnalysisService;
use jobniac_core::reviews::MatchReviewService;
use jobniac_infra::sqlite::match_reviews::SqliteMatchReviewRepository;
use jobniac_infra::sqlite::matching::SqliteMatchingRepository;
use jobniac_mcp::tools::matching::MatchingPort;
use jobniac_mcp::tools::reviews::MatchReviewPort;
use jobniac_runtime::services::MatchingServices;
use serde_json::Value;

use crate::profile_ports::to_dictionary;

/// The deterministic matching use cases over SQLite.
pub type MatchingUseCase = MatchAnalysisService<SqliteMatchingRepository>;
/// The advisory review and disagreement use cases over SQLite.
pub type MatchReviewUseCase = MatchReviewService<SqliteMatchReviewRepository>;

/// The MCP ports of the matching and review tools. An absent port leaves its tools unregistered.
#[derive(Debug, Clone, Default)]
pub struct MatchPorts {
    /// The analysis, batch, report read and report history behind the four matching tools.
    pub matching: Option<Arc<dyn MatchingPort>>,
    /// The review, disagreement and status use cases behind the six review tools.
    pub reviews: Option<Arc<dyn MatchReviewPort>>,
}

/// The ports of the composed matching and review services.
pub fn from_services(matching: &MatchingServices) -> MatchPorts {
    build(
        Arc::clone(&matching.analysis),
        Arc::clone(&matching.reviews),
    )
}

/// The ports over the given use cases.
fn build(analysis: Arc<MatchingUseCase>, reviews: Arc<MatchReviewUseCase>) -> MatchPorts {
    MatchPorts {
        matching: Some(Arc::new(MatchingAdapter(analysis)) as Arc<dyn MatchingPort>),
        reviews: Some(Arc::new(ReviewAdapter(reviews)) as Arc<dyn MatchReviewPort>),
    }
}

/// The four matching tools over the analysis use case.
#[derive(Debug)]
struct MatchingAdapter(Arc<MatchingUseCase>);

#[async_trait]
impl MatchingPort for MatchingAdapter {
    async fn analyze_job_match(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.analyze_job_match(request).await?)
    }

    async fn analyze_all_job_matches(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.analyze_all_job_matches(request).await?)
    }

    async fn get_match_report(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.get_match_report(request).await?)
    }

    async fn list_match_reports(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.list_match_reports(request).await?)
    }
}

/// The six review tools over the review use case.
#[derive(Debug)]
struct ReviewAdapter(Arc<MatchReviewUseCase>);

#[async_trait]
impl MatchReviewPort for ReviewAdapter {
    async fn submit_match_review(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.submit_match_review(request).await?)
    }

    async fn get_match_review(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.get_match_review(request).await?)
    }

    async fn list_match_reviews(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.list_match_reviews(request).await?)
    }

    async fn list_match_disagreements(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.list_match_disagreements(request).await?)
    }

    async fn acknowledge_match_disagreement(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.acknowledge_match_disagreement(request).await?)
    }

    async fn link_match_disagreement(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.link_match_disagreement(request).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use jobniac_core::clock::{Clock, SystemClock};
    use jobniac_core::jobs::JobService;
    use jobniac_core::matching::MatchAnalysisService;
    use jobniac_core::pagination::{CursorCodec, CursorKey};
    use jobniac_core::profiles::ProfileService;
    use jobniac_core::reviews::MatchReviewService;
    use jobniac_infra::sqlite::jobs::SqliteJobRepository;
    use jobniac_infra::sqlite::match_reviews::SqliteMatchReviewRepository;
    use jobniac_infra::sqlite::matching::SqliteMatchingRepository;
    use jobniac_infra::sqlite::migrations::migrate;
    use jobniac_infra::sqlite::pool::SqlitePool;
    use jobniac_infra::sqlite::profiles::SqliteProfileRepository;
    use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
    use jobniac_mcp::catalog::Catalog;
    use jobniac_mcp::registry::{Arguments, ToolRegistry};
    use jobniac_mcp::result::ToolResult;
    use jobniac_mcp::tools::{matching, reviews};
    use jobniac_testkit::sqlite::TempDatabase;
    use serde_json::{Value, json};

    use super::{MatchPorts, build};

    type TestResult = Result<(), Box<dyn Error>>;

    const CURSOR_SECRET: &str = "match-ports-cursor-secret-0123456789abcdef";
    const MISSING_REPORT_ID: &str = "00000000-0000-4000-8000-0000000000ff";

    /// Calls one tool through the registry and returns its outcome.
    async fn call(
        registry: &ToolRegistry,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, String> {
        let Value::Object(map) = arguments else {
            return Err("arguments must be an object".to_owned());
        };
        let map: Arguments = map;
        registry
            .call(name, &map)
            .await
            .ok_or_else(|| format!("{name} is not registered"))
    }

    /// The structured success of a call, or the outcome as a message.
    fn success(outcome: ToolResult) -> Result<Value, String> {
        match outcome {
            ToolResult::Success(value) => Ok(value),
            other => Err(format!("expected a success, got {other:?}")),
        }
    }

    /// The `code` of a failure envelope, or the outcome as a message.
    fn failure_code(outcome: ToolResult) -> Result<String, String> {
        match outcome {
            ToolResult::Failure { envelope, .. } => Ok(envelope.code.code().to_owned()),
            other => Err(format!("expected a failure, got {other:?}")),
        }
    }

    /// Follows `nextCursor` from the first page of a listing and returns every page in order. The
    /// first request carries `cursor: null`, and the walk stops at the first page without a cursor.
    async fn pages(
        registry: &ToolRegistry,
        name: &str,
        request: Value,
    ) -> Result<Vec<Value>, Box<dyn Error>> {
        let mut collected = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut request = request.clone();
            request["cursor"] = cursor.clone().map_or(Value::Null, Value::String);
            let page = success(call(registry, name, json!({ "request": request })).await?)?;
            let next = page["nextCursor"].as_str().map(str::to_owned);
            collected.push(page);
            match next {
                Some(next) if collected.len() < 10 => cursor = Some(next),
                Some(_) => return Err(format!("{name} did not end its cursor walk").into()),
                None => return Ok(collected),
            }
        }
    }

    /// The id of a record, as text.
    fn id_of(value: &Value) -> Result<String, String> {
        value["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("no id in {value}"))
    }

    /// A synthetic profile write with one skill and one project.
    fn profile_request() -> Value {
        json!({
            "fullName": "Example Candidate",
            "email": "candidate@example.test",
            "summary": "Synthetic summary",
            "contacts": [{
                "id": null, "contactType": "email",
                "contactValue": "candidate@example.test", "label": null
            }],
            "links": null,
            "skills": [{
                "id": null, "skill": "Rust", "normalizedSkill": "rust",
                "category": "Languages", "displayOrder": 0
            }],
            "languages": null,
            "education": null,
            "experiences": null,
            "projects": null,
        })
    }

    /// A synthetic job capture whose skills overlap the profile.
    fn job_request() -> Value {
        json!({
            "text": "Synthetic posting text: Rust and SQLite in a small team",
            "sourceLabel": "synthetic-board",
            "title": "Backend engineer",
            "company": "Example Corp",
            "location": null,
            "skills": ["Rust", "SQLite"],
        })
    }

    /// A second synthetic job, so that a batch needs more than one page.
    fn second_job_request() -> Value {
        json!({
            "text": "Synthetic posting text: Go and SQLite for a platform team",
            "sourceLabel": "synthetic-board",
            "title": "Platform engineer",
            "company": "Example Corp",
            "location": null,
            "skills": ["Go"],
        })
    }

    /// One complete `SubmitRequest` with the given review fields, for the review tools.
    fn review_request(report_id: &str, model: &str, outcome: &str) -> Value {
        json!({
            "reportId": report_id,
            "reviewer": "synthetic-reviewer",
            "model": model,
            "reviewVersion": "v1",
            "overallScore": 50,
            "outcome": outcome,
            "blockerMismatch": false,
            "components": [],
            "evidence": [],
            "summary": "outcome_adjustment",
        })
    }

    /// The matching and review tools through the MCP handler layer, over a disposable migrated
    /// database. The walk covers analysis and reuse, a job update that makes a report stale, batch
    /// and history cursors, a review that diverges from its report (with an identical resubmission
    /// and a second model that reuses its disagreement), the issue reference, and the rejections
    /// that never reach the service.
    #[tokio::test]
    async fn matching_and_review_tools_run_through_the_mcp_handlers() -> TestResult {
        let database = TempDatabase::new()?;
        migrate(database.path()).map_err(|error| error.0)?;

        let uow = SqliteUnitOfWork::new(SqlitePool::open(database.path())?);
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let profiles =
            ProfileService::new(uow.clone(), SqliteProfileRepository, Arc::clone(&clock));
        let jobs = JobService::new(
            uow.clone(),
            SqliteJobRepository,
            Arc::clone(&clock),
            CursorCodec::new(CursorKey::from_secret(CURSOR_SECRET)?),
        );
        let analysis = Arc::new(MatchAnalysisService::new(
            SqliteMatchingRepository::new(uow.clone()),
            CursorCodec::new(CursorKey::from_secret(CURSOR_SECRET)?),
            Arc::clone(&clock),
        ));
        let review_service = Arc::new(MatchReviewService::new(
            SqliteMatchReviewRepository::new(uow.clone()),
            CursorCodec::new(CursorKey::from_secret(CURSOR_SECRET)?),
            clock,
        ));
        let ports: MatchPorts = build(analysis, review_service);
        let mut registry = ToolRegistry::new(Catalog::load()?);
        matching::register(&mut registry, ports.matching)?;
        reviews::register(&mut registry, ports.reviews)?;

        let profile = serde_json::to_value(profiles.create_profile(&profile_request()).await?)?;
        let profile_id = profile["profile"]["id"]
            .as_str()
            .ok_or("no profile id")?
            .to_owned();
        let job = serde_json::to_value(jobs.add_job_from_text(&job_request()).await?)?;
        let job_id = job["job"]["job"]["id"]
            .as_str()
            .ok_or("no job id")?
            .to_owned();
        let job_revision = job["job"]["job"]["revision"]
            .as_i64()
            .ok_or("no job revision")?;
        jobs.add_job_from_text(&second_job_request()).await?;

        // Analyze: the report is created, and the same pair reuses it while it is current.
        let analyze = json!({ "request": { "profileId": profile_id, "jobId": job_id } });
        let analyzed = success(call(&registry, "analyze_job_match", analyze.clone()).await?)?;
        assert_eq!(analyzed["stale"], Value::Bool(false));
        let first_report = analyzed["report"].clone();
        let first_report_id = id_of(&first_report)?;
        let again = success(call(&registry, "analyze_job_match", analyze).await?)?;
        assert_eq!(again["report"]["id"], first_report_id.as_str());

        // Staleness: a job edit makes the report stale, and a fresh analysis is a new report.
        jobs.update_job(&json!({
            "jobId": job_id,
            "expectedRevision": job_revision,
            "skills": [],
        }))
        .await?;
        let stale = success(
            call(
                &registry,
                "get_match_report",
                json!({ "request": { "id": first_report_id } }),
            )
            .await?,
        )?;
        assert_eq!(stale["stale"], Value::Bool(true));
        assert_eq!(stale["report"]["id"], first_report_id.as_str());
        let fresh = success(
            call(
                &registry,
                "analyze_job_match",
                json!({ "request": { "profileId": profile_id, "jobId": job_id } }),
            )
            .await?,
        )?;
        let report = fresh["report"].clone();
        let report_id = id_of(&report)?;
        assert_ne!(report_id, first_report_id);
        assert_eq!(report["outcome"], "INSUFFICIENT_EVIDENCE");

        // Batch: two jobs in two pages of one, the second page without a cursor.
        let batch = pages(
            &registry,
            "analyze_all_job_matches",
            json!({ "profileId": profile_id, "limit": 1 }),
        )
        .await?;
        assert_eq!(batch.len(), 2, "two jobs in pages of one");
        for page in &batch {
            assert_eq!(page["succeeded"].as_array().map(Vec::len), Some(1));
            assert_eq!(page["failed"].as_array().map(Vec::len), Some(0));
        }
        assert!(batch[1]["nextCursor"].is_null());

        // History: every report of the profile, across pages of one, without duplicates.
        let history = pages(
            &registry,
            "list_match_reports",
            json!({ "profileId": profile_id, "jobId": null, "limit": 1 }),
        )
        .await?;
        let mut history_ids = Vec::new();
        for page in &history {
            for report in page["reports"].as_array().ok_or("no reports list")? {
                history_ids.push(id_of(&report["report"])?);
            }
        }
        assert!(history_ids.contains(&first_report_id));
        assert!(history_ids.contains(&report_id));
        assert_eq!(
            history_ids.len(),
            3,
            "two reports of the first job and one of the second"
        );
        let mut unique = history_ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), history_ids.len());

        // Reads: the report by identifier in upper case, and a missing report.
        let fetched = success(
            call(
                &registry,
                "get_match_report",
                json!({ "request": { "id": report_id.to_ascii_uppercase() } }),
            )
            .await?,
        )?;
        assert_eq!(fetched["report"]["id"], report_id.as_str());
        let missing = call(
            &registry,
            "get_match_report",
            json!({ "request": { "id": MISSING_REPORT_ID } }),
        )
        .await?;
        assert_eq!(failure_code(missing)?, "validation_error");

        // Review: an outcome that differs from the current report creates one disagreement.
        let reviewed_outcome = if report["outcome"] == "STRONG_MATCH" {
            "WEAK_MATCH"
        } else {
            "STRONG_MATCH"
        };
        let draft = review_request(&report_id, "synthetic-model", reviewed_outcome);
        let submitted = success(
            call(
                &registry,
                "submit_match_review",
                json!({ "request": draft.clone() }),
            )
            .await?,
        )?;
        let review_id = id_of(&submitted["review"])?;
        let disagreement_id = submitted["disagreement"]["id"]
            .as_str()
            .ok_or("the divergent review created no disagreement")?
            .to_owned();
        let resubmitted = success(
            call(
                &registry,
                "submit_match_review",
                json!({ "request": draft }),
            )
            .await?,
        )?;
        assert_eq!(
            resubmitted, submitted,
            "an identical resubmission returns the stored review"
        );
        let other_model = success(
            call(
                &registry,
                "submit_match_review",
                json!({ "request": review_request(&report_id, "other-model", reviewed_outcome) }),
            )
            .await?,
        )?;
        assert_ne!(id_of(&other_model["review"])?, review_id);
        assert_eq!(
            other_model["disagreement"]["id"].as_str(),
            Some(disagreement_id.as_str()),
            "a second review of the same divergence reuses its disagreement"
        );

        let review = success(
            call(
                &registry,
                "get_match_review",
                json!({ "request": { "id": review_id } }),
            )
            .await?,
        )?;
        assert_eq!(review["reportId"], report_id.as_str());
        let review_pages = pages(
            &registry,
            "list_match_reviews",
            json!({ "reportId": report_id, "limit": 1 }),
        )
        .await?;
        assert_eq!(review_pages.len(), 2, "two reviews in pages of one");
        assert_eq!(review_pages[0]["reviews"].as_array().map(Vec::len), Some(1));
        assert!(review_pages[1]["nextCursor"].is_null());
        let listed_disagreements = success(
            call(
                &registry,
                "list_match_disagreements",
                json!({ "request": { "reportId": report_id } }),
            )
            .await?,
        )?;
        assert_eq!(
            listed_disagreements["disagreements"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "one divergence, one disagreement"
        );

        // Disagreement status: the issue reference is trimmed, and an acknowledgement without an
        // issue keeps the one already linked.
        let linked = success(
            call(
                &registry,
                "link_match_disagreement",
                json!({ "request": { "disagreementId": disagreement_id, "linearIssueId": " JOB-35 " } }),
            )
            .await?,
        )?;
        assert_eq!(linked["linearIssueId"], "JOB-35");
        let acknowledged = success(
            call(
                &registry,
                "acknowledge_match_disagreement",
                json!({ "request": { "disagreementId": disagreement_id, "linearIssueId": null } }),
            )
            .await?,
        )?;
        assert_eq!(acknowledged["id"], disagreement_id.as_str());
        assert_eq!(acknowledged["linearIssueId"], "JOB-35");
        let after = success(
            call(
                &registry,
                "list_match_disagreements",
                json!({ "request": { "reportId": report_id } }),
            )
            .await?,
        )?;
        assert_eq!(
            after["disagreements"],
            json!([acknowledged]),
            "the listing holds the acknowledged row"
        );

        // A malformed identifier is refused by the service and never echoed; a rejected shape never
        // reaches the service.
        let malformed = call(
            &registry,
            "analyze_job_match",
            json!({ "request": { "profileId": "not-a-uuid", "jobId": job_id } }),
        )
        .await?;
        assert_eq!(failure_code(malformed.clone())?, "validation_error");
        let wire = serde_json::to_value(malformed.into_call_result())?;
        assert!(
            !wire.to_string().contains("not-a-uuid"),
            "the malformed identifier was echoed: {wire}"
        );
        let bad_enum = call(
            &registry,
            "submit_match_review",
            json!({ "request": review_request(&report_id, "synthetic-model", "WEAK") }),
        )
        .await?;
        let ToolResult::InvalidArguments(errors) = bad_enum else {
            return Err("a value outside the literal set must be an argument error".into());
        };
        let text = errors.render();
        assert!(
            text.contains("request.outcome") && text.contains("literal_error"),
            "the rejection must name the outcome literal: {text}"
        );
        Ok(())
    }
}
