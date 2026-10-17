//! Argument decoding of the profile PDF tools.
//!
//! In `ProfilePdfRequest`, `documentId` is required and not nullable, and every other key may be
//! absent or `null`. `get_profile_pdf_source` takes the profile identifier as a string that is
//! checked as a string only.

use serde_json::{Map, Value};

use crate::args::{
    ArgDecoder, ArgErrors, ArgIssue, OPTIONAL_NULLABLE, REQUIRED, declared_keys, domain_int,
    strict_bool, strict_string, typed_dict,
};
use crate::registry::Arguments;

/// The declared keys of `ProfilePdfRequest`.
const INGEST_KEYS: &[&str] = &[
    "documentId",
    "existingProfileId",
    "overwriteExistingProfile",
    "maxCharacters",
    "expectedRevision",
];

/// Decode the `request` of `ingest_profile_from_stored_pdf` (`ProfilePdfRequest`).
pub fn ingest_profile_from_stored_pdf(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("ingest_profile_from_stored_pdf");
    let request = decoder.field(arguments, "", "request", REQUIRED, ingest_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// Decode the `profileId` of `get_profile_pdf_source` (`UuidText`, a required string).
pub fn get_profile_pdf_source(arguments: &Arguments) -> Result<String, ArgErrors> {
    let mut decoder = ArgDecoder::new("get_profile_pdf_source");
    let profile_id = decoder.field(arguments, "", "profileId", REQUIRED, strict_string);
    decoder.check()?;
    Ok(profile_id.into_value().unwrap_or_default())
}

/// `ProfilePdfRequest`: `documentId` required and not nullable; the others optional and nullable.
fn ingest_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "documentId", REQUIRED, strict_string);
        decoder.field(
            object,
            parent,
            "existingProfileId",
            OPTIONAL_NULLABLE,
            strict_string,
        );
        decoder.field(
            object,
            parent,
            "overwriteExistingProfile",
            OPTIONAL_NULLABLE,
            strict_bool,
        );
        decoder.field(
            object,
            parent,
            "maxCharacters",
            OPTIONAL_NULLABLE,
            domain_int,
        );
        decoder.field(
            object,
            parent,
            "expectedRevision",
            OPTIONAL_NULLABLE,
            domain_int,
        );
    })?;
    Some(declared(value, INGEST_KEYS))
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
    fn only_document_id_is_required_in_the_request() {
        let decoded = ingest_profile_from_stored_pdf(&arguments(json!({
            "request": {"documentId": "doc"}
        })));
        assert_eq!(decoded.ok(), Some(json!({"documentId": "doc"})));
    }

    #[test]
    fn document_id_is_not_nullable() {
        assert!(
            ingest_profile_from_stored_pdf(&arguments(json!({
                "request": {"documentId": null}
            })))
            .is_err()
        );
    }

    #[test]
    fn optional_keys_keep_null_and_drop_undeclared_keys() {
        let decoded = ingest_profile_from_stored_pdf(&arguments(json!({
            "request": {
                "documentId": "doc",
                "existingProfileId": null,
                "expectedRevision": 2,
                "other": true
            }
        })));
        assert_eq!(
            decoded.ok(),
            Some(json!({"documentId": "doc", "existingProfileId": null, "expectedRevision": 2}))
        );
    }

    #[test]
    fn profile_id_is_a_string_only() {
        assert!(get_profile_pdf_source(&arguments(json!({"profileId": true}))).is_err());
        assert!(get_profile_pdf_source(&arguments(json!({"profileId": "p"}))).is_ok());
    }
}
