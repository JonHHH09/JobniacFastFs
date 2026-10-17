//! Writes and single reads of the advisory reviews.
//!
//! A submission is one transaction: the report is loaded, the review factory runs on it, the review
//! is stored, the disagreement factory runs on the stored review, and the disagreement is stored.
//! Both inserts are `ON CONFLICT DO NOTHING` and read back by their content fingerprint, so a
//! resubmission returns the stored row.

use jobniac_core::clock::format_instant;
use jobniac_core::errors::AppError;
use jobniac_core::reviews::{
    DisagreementFactory, DisagreementStatus, MatchDisagreement, MatchReview, ReviewFactory,
    ReviewSubmission, disagreement_fingerprint, review_fingerprint, validate_match_disagreement,
    validate_match_review,
};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use tokio_postgres::{Client, Row};
use uuid::Uuid;

use crate::postgres::matching::store::{raw_json, report_from_row};
use crate::postgres::matching::value::{
    Failure, column, components_json, corrupted, evidence_json, json_array, outcome_text,
};

/// The report of a submission, or `match report not found`.
const SELECT_REPORT_SQL: &str = "SELECT * FROM match.reports WHERE id = $1";

/// The insert of a review, deduplicated by its fingerprint.
const INSERT_REVIEW_SQL: &str = "
        INSERT INTO match.reviews
            (id,fingerprint,report_id,reviewer,model,review_version,overall_score,outcome,
             blocker_mismatch,components,evidence,summary,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10::jsonb,$11::jsonb,$12,$13)
        ON CONFLICT DO NOTHING
        ";

/// The stored review with the fingerprint of the inserted one.
const SELECT_REVIEW_BY_FINGERPRINT_SQL: &str = "SELECT * FROM match.reviews WHERE fingerprint = $1";

/// One review by identifier.
const SELECT_REVIEW_SQL: &str = "SELECT * FROM match.reviews WHERE id = $1";

/// The insert of a disagreement, deduplicated by its fingerprint.
const INSERT_DISAGREEMENT_SQL: &str = "
        INSERT INTO match.disagreements
            (id,fingerprint,report_id,review_id,policy_version,reasons,evidence_defect_codes,
             status,linear_issue_id,created_at,updated_at)
        VALUES ($1,$2,$3,$4,$5,$6::jsonb,$7::jsonb,$8,$9,$10,$11)
        ON CONFLICT DO NOTHING
        ";

/// The stored disagreement with the fingerprint of the inserted one.
const SELECT_DISAGREEMENT_BY_FINGERPRINT_SQL: &str =
    "SELECT * FROM match.disagreements WHERE fingerprint = $1";

/// The status, the issue (kept when absent) and `updated_at`.
const CHANGE_DISAGREEMENT_SQL: &str = "UPDATE match.disagreements SET status = $1, \
    linear_issue_id = COALESCE($2, linear_issue_id), updated_at = $3 \
    WHERE id = $4 RETURNING *";

/// One submission, in the caller's transaction.
pub(crate) async fn submit(
    client: &Client,
    report_id: Uuid,
    review_factory: &ReviewFactory<'_>,
    disagreement_factory: &DisagreementFactory<'_>,
) -> Result<ReviewSubmission, Failure> {
    let Some(row) = client.query_opt(SELECT_REPORT_SQL, &[&report_id]).await? else {
        return Err(Failure::App(AppError::DomainValue(format!(
            "match report not found: {report_id}"
        ))));
    };
    let report = report_from_row(&row)?;
    let review = save_review(client, review_factory(&report)?).await?;
    let disagreement = match disagreement_factory(&report, &review) {
        None => None,
        Some(candidate) => Some(save_disagreement(client, candidate).await?),
    };
    Ok(ReviewSubmission {
        review,
        disagreement,
    })
}

/// The stored review, or `None`.
pub(crate) async fn get_review(
    client: &Client,
    review_id: Uuid,
) -> Result<Option<MatchReview>, Failure> {
    let row = client.query_opt(SELECT_REVIEW_SQL, &[&review_id]).await?;
    row.as_ref().map(review_from_row).transpose()
}

/// Inserts the review unless its fingerprint is stored, then returns the
/// stored row (with its original identifier and creation instant).
pub(crate) async fn save_review(
    client: &Client,
    review: MatchReview,
) -> Result<MatchReview, Failure> {
    let fingerprint = review_fingerprint(&review);
    let overall_score =
        i32::try_from(review.overall_score).map_err(|_| Failure::App(AppError::Internal))?;
    let components = components_json(&review.components);
    let evidence = evidence_json(&review.evidence);
    client
        .execute(
            INSERT_REVIEW_SQL,
            &[
                &review.id,
                &fingerprint,
                &review.report_id,
                &review.reviewer,
                &review.model,
                &review.review_version,
                &overall_score,
                &outcome_text(review.outcome),
                &review.blocker_mismatch,
                &components,
                &evidence,
                &review.summary.as_str(),
                &review.created_at,
            ],
        )
        .await?;
    let row = client
        .query_opt(SELECT_REVIEW_BY_FINGERPRINT_SQL, &[&fingerprint])
        .await?
        .ok_or_else(corrupted)?;
    review_from_row(&row)
}

/// Inserts the disagreement unless its dedupe key is stored, then returns
/// the stored row. A stored disagreement keeps its status and linked issue.
pub(crate) async fn save_disagreement(
    client: &Client,
    disagreement: MatchDisagreement,
) -> Result<MatchDisagreement, Failure> {
    let fingerprint = disagreement_fingerprint(
        disagreement.report_id,
        &disagreement.reasons,
        &disagreement.evidence_defect_codes,
        &disagreement.policy_version,
    );
    let reasons = Value::Array(
        disagreement
            .reasons
            .iter()
            .map(|reason| Value::String(reason.as_str().to_owned()))
            .collect(),
    );
    let codes = Value::Array(
        disagreement
            .evidence_defect_codes
            .iter()
            .map(|code| Value::String(code.as_str().to_owned()))
            .collect(),
    );
    client
        .execute(
            INSERT_DISAGREEMENT_SQL,
            &[
                &disagreement.id,
                &fingerprint,
                &disagreement.report_id,
                &disagreement.review_id,
                &disagreement.policy_version,
                &reasons,
                &codes,
                &disagreement.status.as_str(),
                &disagreement.linear_issue_id,
                &disagreement.created_at,
                &disagreement.updated_at,
            ],
        )
        .await?;
    let row = client
        .query_opt(SELECT_DISAGREEMENT_BY_FINGERPRINT_SQL, &[&fingerprint])
        .await?
        .ok_or_else(corrupted)?;
    disagreement_from_row(&row)
}

/// The changed row, or `None` when it does not exist.
pub(crate) async fn change_disagreement(
    client: &Client,
    disagreement_id: Uuid,
    status: DisagreementStatus,
    issue: Option<&str>,
    now: OffsetDateTime,
) -> Result<Option<MatchDisagreement>, Failure> {
    let row = client
        .query_opt(
            CHANGE_DISAGREEMENT_SQL,
            &[&status.as_str(), &issue, &now, &disagreement_id],
        )
        .await?;
    row.as_ref().map(disagreement_from_row).transpose()
}

/// A stored review, validated with the persisted-record rules. Any violation is
/// "Local database is unavailable".
pub(crate) fn review_from_row(row: &Row) -> Result<MatchReview, Failure> {
    let id = column::<Uuid>(row, "id")?.to_string();
    let report_id = column::<Uuid>(row, "report_id")?.to_string();
    let reviewer: String = column(row, "reviewer")?;
    let model: String = column(row, "model")?;
    let review_version: String = column(row, "review_version")?;
    let overall_score: i32 = column(row, "overall_score")?;
    let outcome: String = column(row, "outcome")?;
    let blocker_mismatch: bool = column(row, "blocker_mismatch")?;
    let components = json_array(row, "components")?;
    let evidence = json_array(row, "evidence")?;
    let summary: String = column(row, "summary")?;
    let created_at = format_instant(column::<OffsetDateTime>(row, "created_at")?);
    let values = json!({
        "id": id,
        "reportId": report_id,
        "reviewer": reviewer,
        "model": model,
        "reviewVersion": review_version,
        "overallScore": overall_score,
        "outcome": outcome,
        "blockerMismatch": blocker_mismatch,
        "components": components,
        "evidence": evidence,
        "summary": summary,
        "createdAt": created_at,
    });
    let Value::Object(map) = values else {
        return Err(corrupted());
    };
    validate_match_review(&map).map_err(|_| corrupted())
}

/// A stored disagreement, validated with the persisted-record rules.
pub(crate) fn disagreement_from_row(row: &Row) -> Result<MatchDisagreement, Failure> {
    let id = column::<Uuid>(row, "id")?.to_string();
    let report_id = column::<Uuid>(row, "report_id")?.to_string();
    let review_id = column::<Uuid>(row, "review_id")?.to_string();
    let policy_version: String = column(row, "policy_version")?;
    let reasons = raw_json(row, "reasons")?;
    let codes = raw_json(row, "evidence_defect_codes")?;
    let status: String = column(row, "status")?;
    let linear_issue_id: Option<String> = column(row, "linear_issue_id")?;
    let created_at = format_instant(column::<OffsetDateTime>(row, "created_at")?);
    let updated_at = format_instant(column::<OffsetDateTime>(row, "updated_at")?);
    let values: Map<String, Value> = match json!({
        "id": id,
        "reportId": report_id,
        "reviewId": review_id,
        "policyVersion": policy_version,
        "reasons": reasons,
        "evidenceDefectCodes": codes,
        "status": status,
        "linearIssueId": linear_issue_id,
        "createdAt": created_at,
        "updatedAt": updated_at,
    }) {
        Value::Object(map) => map,
        _ => return Err(corrupted()),
    };
    validate_match_disagreement(&values).map_err(|_| corrupted())
}
