//! Report model of the deterministic matcher: components, evidence, the immutable report, its
//! content-addressed identity and staleness against the current source revisions.
//!
//! Serialized field order is fixed, so the JSON of a report reads in the same
//! order as the MCP wire output. Instants serialize with [`crate::clock::format_instant`].

use serde::{Serialize, Serializer};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::{format_instant, truncate_to_micros};
use crate::ids::name_uuid;

/// Algorithm identifier stored with every report.
pub const ALGORITHM_VERSION: &str = "deterministic-v1";

/// The scored components, declared in the order the report lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Component {
    /// Skill overlap, 40 points.
    Technical,
    /// Dated experience against a stated requirement, 25 points.
    ExperienceSeniority,
    /// Domain fit, 15 points (never computed in `deterministic-v1`).
    Domain,
    /// Delivery fit, 10 points (never computed in `deterministic-v1`).
    Delivery,
    /// Hard requirements, 10 points (never computed in `deterministic-v1`).
    HardRequirements,
}

impl Component {
    /// Every component in report order.
    pub const ALL: [Self; 5] = [
        Self::Technical,
        Self::ExperienceSeniority,
        Self::Domain,
        Self::Delivery,
        Self::HardRequirements,
    ];

    /// The wire name of the component.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Technical => "TECHNICAL",
            Self::ExperienceSeniority => "EXPERIENCE_SENIORITY",
            Self::Domain => "DOMAIN",
            Self::Delivery => "DELIVERY",
            Self::HardRequirements => "HARD_REQUIREMENTS",
        }
    }

    /// The component named by its wire name, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|component| component.as_str() == text)
    }

    /// The points the component can earn (the configured weight).
    pub const fn available_points(self) -> i64 {
        match self {
            Self::Technical => 40,
            Self::ExperienceSeniority => 25,
            Self::Domain => 15,
            Self::Delivery | Self::HardRequirements => 10,
        }
    }
}

/// Status of one component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComponentStatus {
    /// Every required fact is present.
    Match,
    /// Some but not all required facts are present.
    Partial,
    /// No required fact is present.
    Mismatch,
    /// The component was not evaluated; it does not count toward confidence.
    Unknown,
}

impl ComponentStatus {
    /// The wire name of the status.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Partial => "PARTIAL",
            Self::Mismatch => "MISMATCH",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// The status named by its wire name, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Match, Self::Partial, Self::Mismatch, Self::Unknown]
            .into_iter()
            .find(|status| status.as_str() == text)
    }
}

/// Outcome band of a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    /// Score of at least 75.
    StrongMatch,
    /// Score of at least 45 and below 75.
    PartialMatch,
    /// Score below 45.
    WeakMatch,
    /// No component has a known status.
    InsufficientEvidence,
}

/// One scored component: earned and available points and the status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchComponent {
    /// Which component this is.
    pub component: Component,
    /// Points earned, from 0 to `available_points`.
    pub earned_points: i64,
    /// The configured weight of the component.
    pub available_points: i64,
    /// Status of the component.
    pub status: ComponentStatus,
}

/// One evidence item. Only normalized skills and fixed fact strings are ever evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchEvidence {
    /// The component the evidence supports.
    pub component: Component,
    /// The status the evidence supports.
    pub status: ComponentStatus,
    /// The source label, for example `normalized_skill`.
    pub source_type: String,
    /// The fact text (a canonical skill or a fixed sentence).
    pub fact: String,
    /// Whether the evidence changes the outcome. Always `false` in `deterministic-v1`.
    pub outcome_changing_defect: bool,
}

/// An immutable match report.
///
/// The identity is content-addressed: the same algorithm, profile revision and job revision always
/// give the same [`MatchReport::id`]. `created_at` is not part of the identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchReport {
    /// Content-addressed identifier (UUIDv3 over [`report_identity`]).
    pub id: Uuid,
    /// The profile the report scores.
    pub profile_id: Uuid,
    /// The job the report scores.
    pub job_id: Uuid,
    /// Profile update instant the report was computed from.
    #[serde(serialize_with = "serialize_instant")]
    pub profile_revision: OffsetDateTime,
    /// Job update instant the report was computed from.
    #[serde(serialize_with = "serialize_instant")]
    pub job_revision: OffsetDateTime,
    /// Algorithm identifier, [`ALGORITHM_VERSION`] for reports this build creates.
    pub algorithm_version: String,
    /// Overall score from 0 to 100.
    pub overall_score: i64,
    /// Sum of the available points of the known components.
    pub confidence: i64,
    /// Outcome band.
    pub outcome: Outcome,
    /// Whether a blocking mismatch was found. Always `false` in `deterministic-v1`.
    pub blocker_mismatch: bool,
    /// The five components in report order.
    pub components: Vec<MatchComponent>,
    /// Evidence in the order it was produced.
    pub evidence: Vec<MatchEvidence>,
    /// When the report was computed (truncated to microseconds, not part of the identity).
    #[serde(serialize_with = "serialize_instant")]
    pub created_at: OffsetDateTime,
}

/// A report together with its staleness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchView {
    /// The immutable report.
    pub report: MatchReport,
    /// Whether a source revision changed after the report was computed.
    pub stale: bool,
}

/// The identity string the report identifier is derived from:
/// `deterministic-v1|<profileId>|<profileRevision>|<jobId>|<jobRevision>`.
pub fn report_identity(
    profile_id: Uuid,
    profile_revision: OffsetDateTime,
    job_id: Uuid,
    job_revision: OffsetDateTime,
) -> String {
    format!(
        "{ALGORITHM_VERSION}|{profile_id}|{}|{job_id}|{}",
        format_instant(profile_revision),
        format_instant(job_revision)
    )
}

/// The content-addressed report identifier of an identity string (MD5 name-based UUIDv3).
pub fn report_id(identity: &str) -> Uuid {
    name_uuid(identity)
}

/// Whether a report is stale: a source revision differs from the report's, or the source is gone
/// (a missing current revision counts as changed).
pub fn is_stale(
    report: &MatchReport,
    current_profile_revision: Option<OffsetDateTime>,
    current_job_revision: Option<OffsetDateTime>,
) -> bool {
    !current_profile_revision.is_some_and(|current| same_instant(current, report.profile_revision))
        || !current_job_revision.is_some_and(|current| same_instant(current, report.job_revision))
}

/// Compares two instants by their UTC value, not by offset representation.
pub fn same_instant(left: OffsetDateTime, right: OffsetDateTime) -> bool {
    truncate_to_micros(left).unix_timestamp_nanos()
        == truncate_to_micros(right).unix_timestamp_nanos()
}

fn serialize_instant<S: Serializer>(
    value: &OffsetDateTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format_instant(*value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn component_order_and_weights_are_fixed() {
        let weights: Vec<i64> = Component::ALL
            .iter()
            .map(|component| component.available_points())
            .collect();
        assert_eq!(weights, vec![40, 25, 15, 10, 10]);
        assert_eq!(weights.iter().sum::<i64>(), 100);
        assert_eq!(
            Component::parse("HARD_REQUIREMENTS"),
            Some(Component::HardRequirements)
        );
        assert_eq!(Component::parse("hard_requirements"), None);
        assert_eq!(
            ComponentStatus::parse("UNKNOWN"),
            Some(ComponentStatus::Unknown)
        );
    }

    #[test]
    fn identity_uses_the_formatted_revisions() {
        let profile_id = Uuid::from_u128(1);
        let job_id = Uuid::from_u128(2);
        let identity = report_identity(
            profile_id,
            datetime!(2025-01-02 03:04:05.1 UTC),
            job_id,
            datetime!(2025-02-03 04:05:06.654321 UTC),
        );
        assert_eq!(
            identity,
            "deterministic-v1|00000000-0000-0000-0000-000000000001|2025-01-02T03:04:05.100Z|\
             00000000-0000-0000-0000-000000000002|2025-02-03T04:05:06.654321Z"
        );
    }

    #[test]
    fn same_instant_ignores_the_offset_representation() {
        assert!(same_instant(
            datetime!(2025-01-02 03:04:05 UTC),
            datetime!(2025-01-02 04:04:05 +01:00)
        ));
        assert!(!same_instant(
            datetime!(2025-01-02 03:04:05.000001 UTC),
            datetime!(2025-01-02 03:04:05 UTC)
        ));
    }
}
