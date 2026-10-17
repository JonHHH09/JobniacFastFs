//! Argument decoding of the job tools, in the shapes of the tool signatures.
//!
//! Every `TypedDict` argument is decoded key by key in declaration order. Two differences from the
//! profile tools follow from the signatures. `UuidText` is a plain `StrictStr` here, so a
//! `jobId` that is not a string is `string_type`, and any string reaches the service, which admits
//! it as a UUID (a bad one is the service's `validation_error`). The handlers pass the request
//! object to the service unchanged, so the service applies its own canonicalization and ignores
//! the keys it does not know.

use serde_json::Value;

use crate::args::{
    ArgDecoder, ArgErrors, ArgIssue, OPTIONAL_NULLABLE, PageRequest, REQUIRED, list_of,
    page_request, strict_int, strict_string, typed_dict,
};
use crate::registry::Arguments;

/// Decode the `request` of `add_job_from_text` (`JobTextRequest`, `text` required).
pub fn add_job_from_text(arguments: &Arguments) -> Result<(), ArgErrors> {
    let mut decoder = ArgDecoder::new("add_job_from_text");
    decoder.field(arguments, "", "request", REQUIRED, job_text_request);
    decoder.check()
}

/// Decode the `jobId` of `get_job`. The text is passed on as written; the service admits it.
pub fn get_job(arguments: &Arguments) -> Result<String, ArgErrors> {
    let mut decoder = ArgDecoder::new("get_job");
    let job_id = decoder.field(arguments, "", "jobId", REQUIRED, strict_string);
    decoder.check()?;
    Ok(job_id.into_value().unwrap_or_default())
}

/// Decode the optional `request` of `list_jobs` (`JobListRequest`). An absent or null request is `{}`.
pub fn list_jobs(arguments: &Arguments) -> Result<PageRequest, ArgErrors> {
    page_request("list_jobs", arguments)
}

/// Decode the `request` of `search_jobs` (`JobSearchRequest`, `query` required).
pub fn search_jobs(arguments: &Arguments) -> Result<(), ArgErrors> {
    let mut decoder = ArgDecoder::new("search_jobs");
    decoder.field(arguments, "", "request", REQUIRED, job_search_request);
    decoder.check()
}

/// Decode the `request` of `update_job` (`JobUpdateRequest`, `jobId` and `expectedRevision` required).
pub fn update_job(arguments: &Arguments) -> Result<(), ArgErrors> {
    let mut decoder = ArgDecoder::new("update_job");
    decoder.field(arguments, "", "request", REQUIRED, job_update_request);
    decoder.check()
}

/// `JobTextRequest`: `text` is required; every other key is optional and nullable, and `skills` is
/// a list of strings.
fn job_text_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "text", REQUIRED, strict_string);
        for key in ["sourceLabel", "title", "company", "location", "description"] {
            decoder.field(object, parent, key, OPTIONAL_NULLABLE, strict_string);
        }
        decoder.field(
            object,
            parent,
            "skills",
            OPTIONAL_NULLABLE,
            |location, value, issues| list_of(location, value, issues, string_item),
        );
        for key in [
            "experienceRequirement",
            "employmentType",
            "seniority",
            "postedAt",
        ] {
            decoder.field(object, parent, key, OPTIONAL_NULLABLE, strict_string);
        }
    })
}

/// `JobSearchRequest`: `query` is a required string, `limit` an optional nullable integer.
fn job_search_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "query", REQUIRED, strict_string);
        decoder.field(object, parent, "limit", OPTIONAL_NULLABLE, strict_int);
    })
}

/// `JobUpdateRequest`: `jobId` and `expectedRevision` are required; the other keys are optional and
/// nullable, and `skills` is a list whose items may be null (`list[str | None]`).
fn job_update_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "jobId", REQUIRED, strict_string);
        decoder.field(object, parent, "expectedRevision", REQUIRED, strict_int);
        for key in ["sourceLabel", "title", "company", "location", "description"] {
            decoder.field(object, parent, key, OPTIONAL_NULLABLE, strict_string);
        }
        decoder.field(
            object,
            parent,
            "skills",
            OPTIONAL_NULLABLE,
            |location, value, issues| list_of(location, value, issues, nullable_string_item),
        );
        for key in [
            "experienceRequirement",
            "employmentType",
            "seniority",
            "postedAt",
        ] {
            decoder.field(object, parent, key, OPTIONAL_NULLABLE, strict_string);
        }
    })
}

/// A string list item (`str`).
fn string_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    strict_string(location, value, issues).map(|_| ())
}

/// A string list item that may be null (`str | None`).
fn nullable_string_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    if value.is_null() {
        return Some(());
    }
    strict_string(location, value, issues).map(|_| ())
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
    fn job_text_requires_text_and_checks_skill_items() {
        let rejected = add_job_from_text(&arguments(json!({
            "request": {"title": "Engineer", "skills": ["Rust", 5]}
        })));
        let Err(errors) = rejected else {
            panic!("a missing text and a non-string skill must be rejected");
        };
        assert_eq!(
            kinds(&errors),
            vec![
                ("request.text".to_owned(), IssueKind::Missing),
                ("request.skills.1".to_owned(), IssueKind::StringType),
            ]
        );
    }

    #[test]
    fn update_skills_admit_null_items() {
        assert!(
            update_job(&arguments(json!({
                "request": {"jobId": "x", "expectedRevision": 1, "skills": [null, "Rust"]}
            })))
            .is_ok()
        );
        let rejected = update_job(&arguments(json!({
            "request": {"jobId": "x", "expectedRevision": 1, "skills": [null, 5]}
        })));
        let Err(errors) = rejected else {
            panic!("a non-string skill item must be rejected");
        };
        assert_eq!(
            kinds(&errors),
            vec![("request.skills.1".to_owned(), IssueKind::StringType)]
        );
    }

    #[test]
    fn job_id_is_a_plain_string_for_the_service_to_admit() {
        assert_eq!(
            get_job(&arguments(json!({"jobId": "not-a-uuid"}))).ok(),
            Some("not-a-uuid".to_owned())
        );
        let rejected = get_job(&arguments(json!({"jobId": 12})));
        assert_eq!(
            rejected.err().map(|errors| kinds(&errors)),
            Some(vec![("jobId".to_owned(), IssueKind::StringType)])
        );
    }

    #[test]
    fn list_arguments_treat_an_absent_or_null_request_as_empty() {
        assert!(list_jobs(&arguments(json!({}))).is_ok());
        assert!(list_jobs(&arguments(json!({"request": null}))).is_ok());
        assert!(list_jobs(&arguments(json!({"request": {"limit": true}}))).is_err());
    }
}
