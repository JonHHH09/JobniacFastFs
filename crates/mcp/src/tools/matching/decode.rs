//! Argument decoding of the matching tools, in the shapes of the tool signatures.
//!
//! Every `TypedDict` argument is decoded key by key in declaration order, and the decoded request
//! handed to the service keeps only the declared keys.
//! `UuidText` is a plain `StrictStr` in the signatures, so an identifier that is not a
//! string is `string_type`, and any string reaches the service, which admits it as a UUID (a bad
//! one is the service's `validation_error`).

use serde_json::Value;

use crate::args::{
    ArgDecoder, ArgErrors, ArgIssue, OPTIONAL_NULLABLE, REQUIRED, REQUIRED_NULLABLE, declared_keys,
    domain_int, strict_string, typed_dict,
};
use crate::registry::Arguments;

/// The declared keys of `AnalyzeRequest`.
const ANALYZE_KEYS: &[&str] = &["profileId", "jobId"];
/// The declared keys of `BatchRequest`.
const BATCH_KEYS: &[&str] = &["profileId", "limit", "cursor"];
/// The declared keys of `ReportRequest`.
const REPORT_KEYS: &[&str] = &["id"];
/// The declared keys of `ReportFilter`.
const FILTER_KEYS: &[&str] = &["profileId", "jobId", "limit", "cursor"];

/// Decode the `request` of `analyze_job_match` (`AnalyzeRequest`: `profileId` and `jobId`, both
/// required and nullable).
pub fn analyze_job_match(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("analyze_job_match");
    let request = decoder.field(arguments, "", "request", REQUIRED, analyze_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// Decode the `request` of `analyze_all_job_matches` (`BatchRequest`: `profileId` required, `limit`
/// and `cursor` optional and nullable).
pub fn analyze_all_job_matches(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("analyze_all_job_matches");
    let request = decoder.field(arguments, "", "request", REQUIRED, batch_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// Decode the `request` of `get_match_report` (`ReportRequest`: `id` required and nullable).
pub fn get_match_report(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("get_match_report");
    let request = decoder.field(arguments, "", "request", REQUIRED, report_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// Decode the `request` of `list_match_reports` (`ReportFilter`: every key optional and nullable).
pub fn list_match_reports(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("list_match_reports");
    let request = decoder.field(arguments, "", "request", REQUIRED, report_filter);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// `AnalyzeRequest`.
fn analyze_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(
            object,
            parent,
            "profileId",
            REQUIRED_NULLABLE,
            strict_string,
        );
        decoder.field(object, parent, "jobId", REQUIRED_NULLABLE, strict_string);
    })?;
    Some(declared(value, ANALYZE_KEYS))
}

/// `BatchRequest`: `profileId` is required and not nullable (`Required[UuidText]`).
fn batch_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "profileId", REQUIRED, strict_string);
        decoder.field(object, parent, "limit", OPTIONAL_NULLABLE, domain_int);
        decoder.field(object, parent, "cursor", OPTIONAL_NULLABLE, strict_string);
    })?;
    Some(declared(value, BATCH_KEYS))
}

/// `ReportRequest`.
fn report_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "id", REQUIRED_NULLABLE, strict_string);
    })?;
    Some(declared(value, REPORT_KEYS))
}

/// `ReportFilter`: every key is optional and nullable.
fn report_filter(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(
            object,
            parent,
            "profileId",
            OPTIONAL_NULLABLE,
            strict_string,
        );
        decoder.field(object, parent, "jobId", OPTIONAL_NULLABLE, strict_string);
        decoder.field(object, parent, "limit", OPTIONAL_NULLABLE, domain_int);
        decoder.field(object, parent, "cursor", OPTIONAL_NULLABLE, strict_string);
    })?;
    Some(declared(value, FILTER_KEYS))
}

/// The declared keys of a `TypedDict` argument that has passed its shape check.
fn declared(value: &Value, keys: &[&str]) -> Value {
    value.as_object().map_or(Value::Null, |object| {
        Value::Object(declared_keys(object, keys))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;
    use crate::args::IssueKind;

    fn arguments(value: Value) -> Arguments {
        match value {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }

    fn kinds(errors: &ArgErrors) -> Vec<(String, IssueKind)> {
        errors
            .issues()
            .iter()
            .map(|issue| (issue.location().to_owned(), issue.kind()))
            .collect()
    }

    #[test]
    fn analyze_keeps_both_nullable_keys_and_drops_the_rest() {
        let decoded = analyze_job_match(&arguments(json!({
            "request": {"profileId": null, "jobId": "x", "extra": 1}
        })));
        assert_eq!(decoded.ok(), Some(json!({"profileId": null, "jobId": "x"})));
        let rejected = analyze_job_match(&arguments(json!({"request": {"profileId": "p"}})));
        assert_eq!(
            rejected.err().map(|errors| kinds(&errors)),
            Some(vec![("request.jobId".to_owned(), IssueKind::Missing)])
        );
    }

    #[test]
    fn batch_requires_a_non_null_profile_id() {
        let rejected = analyze_all_job_matches(&arguments(json!({
            "request": {"profileId": null, "limit": "5"}
        })));
        assert_eq!(
            rejected.err().map(|errors| kinds(&errors)),
            Some(vec![
                ("request.profileId".to_owned(), IssueKind::StringType),
                ("request.limit".to_owned(), IssueKind::IntType),
            ])
        );
        assert_eq!(
            analyze_all_job_matches(&arguments(json!({"request": {"profileId": "p"}}))).ok(),
            Some(json!({"profileId": "p"}))
        );
    }

    #[test]
    fn report_and_filter_shapes() {
        assert!(get_match_report(&arguments(json!({"request": {"id": null}}))).is_ok());
        assert!(get_match_report(&arguments(json!({"request": {}}))).is_err());
        assert!(get_match_report(&arguments(json!({}))).is_err());
        assert!(list_match_reports(&arguments(json!({"request": {}}))).is_ok());
        assert!(list_match_reports(&arguments(json!({"request": null}))).is_err());
        assert_eq!(
            list_match_reports(&arguments(json!({"request": {"limit": true}})))
                .err()
                .map(|errors| kinds(&errors)),
            Some(vec![("request.limit".to_owned(), IssueKind::IntType)])
        );
    }
}
