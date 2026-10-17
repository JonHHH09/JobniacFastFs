//! Argument decoding of the generation tools.
//!
//! Every tool has one `request` argument, a required object whose keys are all required and
//! nullable. Text and identity fields are strict strings (identifiers are checked as strings only;
//! the service checks the UUID form), and `includeProjects` is a strict boolean. The request
//! handed to the service keeps only the declared keys.

use serde_json::{Map, Value};

use crate::args::{
    ArgDecoder, ArgErrors, ArgIssue, REQUIRED, REQUIRED_NULLABLE, declared_keys, strict_bool,
    strict_string, typed_dict,
};
use crate::registry::Arguments;

/// The declared keys of `PdfFileRequest`.
const PDF_FILE_KEYS: &[&str] = &["fileName", "title", "body"];
/// The declared keys of `ProfileResumeRequest`.
const PROFILE_KEYS: &[&str] = &["profileId"];
/// The declared keys of `GermanResumeRequest`.
const GERMAN_RESUME_KEYS: &[&str] = &["profileId", "jobId", "includeProjects"];
/// The declared keys of `GermanCoverLetterRequest`.
const GERMAN_LETTER_KEYS: &[&str] = &["profileId", "jobId", "resumeId"];

/// Decode the `request` argument of `tool` in the shape of its `TypedDict`.
///
/// # Errors
///
/// The argument errors of the request object, in declaration order.
pub fn request(tool: &'static str, arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new(tool);
    let shape: ItemShape = match tool {
        "generate_pdf_file" => pdf_file_request,
        "generate_german_tailored_resume" => german_resume_request,
        "generate_german_cover_letter" => german_letter_request,
        _ => profile_request,
    };
    let request = decoder.field(arguments, "", "request", REQUIRED, shape);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// The decoder of one `TypedDict` argument: the value is validated and its declared keys kept.
type ItemShape = fn(&str, &Value, &mut Vec<ArgIssue>) -> Option<Value>;

/// `PdfFileRequest`: `fileName`, `title` and `body`, all strings or null.
fn pdf_file_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in PDF_FILE_KEYS {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
    })?;
    Some(declared(value, PDF_FILE_KEYS))
}

/// `ProfileResumeRequest`: `profileId`, a string or null.
fn profile_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(
            object,
            parent,
            "profileId",
            REQUIRED_NULLABLE,
            strict_string,
        );
    })?;
    Some(declared(value, PROFILE_KEYS))
}

/// `GermanResumeRequest`: `profileId`, `jobId` (strings or null) and `includeProjects` (a strict
/// boolean or null).
fn german_resume_request(
    location: &str,
    value: &Value,
    issues: &mut Vec<ArgIssue>,
) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(
            object,
            parent,
            "profileId",
            REQUIRED_NULLABLE,
            strict_string,
        );
        decoder.field(object, parent, "jobId", REQUIRED_NULLABLE, strict_string);
        decoder.field(
            object,
            parent,
            "includeProjects",
            REQUIRED_NULLABLE,
            strict_bool,
        );
    })?;
    Some(declared(value, GERMAN_RESUME_KEYS))
}

/// `GermanCoverLetterRequest`: `profileId`, `jobId` and `resumeId`, strings or null.
fn german_letter_request(
    location: &str,
    value: &Value,
    issues: &mut Vec<ArgIssue>,
) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in GERMAN_LETTER_KEYS {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
    })?;
    Some(declared(value, GERMAN_LETTER_KEYS))
}

/// The declared keys of a validated `TypedDict` object, with their values unchanged.
fn declared(value: &Value, keys: &[&str]) -> Value {
    value
        .as_object()
        .map_or(Value::Null, |object: &Map<String, Value>| {
            Value::Object(declared_keys(object, keys))
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn arguments(value: Value) -> Arguments {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn pdf_file_request_keeps_declared_keys_with_nulls() {
        let decoded = request(
            "generate_pdf_file",
            &arguments(json!({"request": {"fileName": null, "title": "T", "body": "B", "x": 1}})),
        );
        assert_eq!(
            decoded.ok(),
            Some(json!({"fileName": null, "title": "T", "body": "B"}))
        );
    }

    #[test]
    fn missing_keys_are_reported_for_each_typed_dict() {
        for tool in [
            "generate_pdf_file",
            "generate_pdf_resume",
            "generate_canadian_pdf_resume",
            "generate_canadian_french_pdf_resume",
            "generate_german_tailored_resume",
            "generate_german_cover_letter",
        ] {
            let Err(errors) = request(tool, &arguments(json!({"request": {}}))) else {
                panic!("{tool}: missing keys must be rejected");
            };
            assert!(!errors.issues().is_empty(), "{tool}");
        }
    }

    #[test]
    fn include_projects_is_a_strict_boolean_and_identities_are_strings() {
        assert!(
            request(
                "generate_german_tailored_resume",
                &arguments(
                    json!({"request": {"profileId": "p", "jobId": "j", "includeProjects": 1}})
                ),
            )
            .is_err()
        );
        assert!(
            request(
                "generate_german_tailored_resume",
                &arguments(
                    json!({"request": {"profileId": null, "jobId": null, "includeProjects": null}})
                ),
            )
            .is_ok()
        );
        assert!(
            request(
                "generate_pdf_resume",
                &arguments(json!({"request": {"profileId": 7}})),
            )
            .is_err()
        );
    }

    #[test]
    fn a_non_object_request_is_refused() {
        assert!(
            request(
                "generate_german_cover_letter",
                &arguments(json!({"request": "x"}))
            )
            .is_err()
        );
    }
}
