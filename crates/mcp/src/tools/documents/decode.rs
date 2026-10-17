//! Argument decoding of the document tools.
//!
//! Every argument is a required key whose value may be `null`, so each key is decoded as
//! `REQUIRED_NULLABLE`. The identifiers are checked as strings only; the service checks the UUID
//! form. The integers are `Int32` schema hints (the domain decides the range, see
//! [`crate::args`]). The request handed to the service keeps only the declared keys.

use serde_json::{Map, Value};

use crate::args::{
    ArgDecoder, ArgErrors, REQUIRED, REQUIRED_NULLABLE, declared_keys, domain_int, strict_bool,
    strict_string, typed_dict,
};
use crate::registry::Arguments;

/// The declared keys of `StoreRequest`.
const STORE_KEYS: &[&str] = &["path", "mediaType"];
/// The declared keys of `ExtractRequest`.
const EXTRACT_KEYS: &[&str] = &["path", "maxCharacters", "includePages"];
/// The declared keys of `StoredExtractRequest`.
const STORED_EXTRACT_KEYS: &[&str] = &[
    "documentId",
    "maxCharacters",
    "includePages",
    "persistExtraction",
];

/// Decode the `request` of `store_document_file` (`StoreRequest`).
pub fn store_document_file(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("store_document_file");
    let request = decoder.field(arguments, "", "request", REQUIRED, store_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// Decode the `documentId` of `get_document_metadata` (`UuidText`, a required string).
pub fn get_document_metadata(arguments: &Arguments) -> Result<String, ArgErrors> {
    let mut decoder = ArgDecoder::new("get_document_metadata");
    let document_id = decoder.field(arguments, "", "documentId", REQUIRED, strict_string);
    decoder.check()?;
    Ok(document_id.into_value().unwrap_or_default())
}

/// Decode the `request` of `extract_pdf_text` (`ExtractRequest`).
pub fn extract_pdf_text(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("extract_pdf_text");
    let request = decoder.field(arguments, "", "request", REQUIRED, extract_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// Decode the `request` of `extract_stored_pdf_text` (`StoredExtractRequest`).
pub fn extract_stored_pdf_text(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("extract_stored_pdf_text");
    let request = decoder.field(arguments, "", "request", REQUIRED, stored_extract_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// `StoreRequest`: `path` and `mediaType`, both required and nullable.
fn store_request(
    location: &str,
    value: &Value,
    issues: &mut Vec<crate::args::ArgIssue>,
) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "path", REQUIRED_NULLABLE, strict_string);
        decoder.field(
            object,
            parent,
            "mediaType",
            REQUIRED_NULLABLE,
            strict_string,
        );
    })?;
    Some(declared(value, STORE_KEYS))
}

/// `ExtractRequest`: `path`, `maxCharacters` and `includePages`, all required and nullable.
fn extract_request(
    location: &str,
    value: &Value,
    issues: &mut Vec<crate::args::ArgIssue>,
) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "path", REQUIRED_NULLABLE, strict_string);
        decoder.field(
            object,
            parent,
            "maxCharacters",
            REQUIRED_NULLABLE,
            domain_int,
        );
        decoder.field(
            object,
            parent,
            "includePages",
            REQUIRED_NULLABLE,
            strict_bool,
        );
    })?;
    Some(declared(value, EXTRACT_KEYS))
}

/// `StoredExtractRequest`: four keys, all required and nullable.
fn stored_extract_request(
    location: &str,
    value: &Value,
    issues: &mut Vec<crate::args::ArgIssue>,
) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(
            object,
            parent,
            "documentId",
            REQUIRED_NULLABLE,
            strict_string,
        );
        decoder.field(
            object,
            parent,
            "maxCharacters",
            REQUIRED_NULLABLE,
            domain_int,
        );
        decoder.field(
            object,
            parent,
            "includePages",
            REQUIRED_NULLABLE,
            strict_bool,
        );
        decoder.field(
            object,
            parent,
            "persistExtraction",
            REQUIRED_NULLABLE,
            strict_bool,
        );
    })?;
    Some(declared(value, STORED_EXTRACT_KEYS))
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
    fn store_request_keeps_declared_keys_with_nulls() {
        let decoded = store_document_file(&arguments(json!({
            "request": {"path": "a.pdf", "mediaType": null, "extra": 1}
        })));
        assert_eq!(
            decoded.ok(),
            Some(json!({"path": "a.pdf", "mediaType": null}))
        );
    }

    #[test]
    fn missing_keys_of_a_required_typed_dict_are_reported() {
        let Err(errors) = store_document_file(&arguments(json!({"request": {}}))) else {
            panic!("missing keys must be rejected");
        };
        assert_eq!(errors.issues().len(), 2);
    }

    #[test]
    fn document_id_must_be_a_string_only() {
        assert!(
            get_document_metadata(&arguments(json!({"documentId": "not-checked-here"}))).is_ok()
        );
        assert!(get_document_metadata(&arguments(json!({"documentId": 12}))).is_err());
        assert!(get_document_metadata(&arguments(json!({}))).is_err());
    }

    #[test]
    fn extraction_flags_are_strict_booleans() {
        assert!(
            extract_pdf_text(&arguments(json!({
                "request": {"path": "a.pdf", "maxCharacters": 10, "includePages": 1}
            })))
            .is_err()
        );
        assert!(extract_stored_pdf_text(&arguments(json!({
            "request": {"documentId": null, "maxCharacters": null, "includePages": null, "persistExtraction": false}
        })))
        .is_ok());
    }
}
