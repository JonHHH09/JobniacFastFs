//! Identifier golden vectors. Admission must accept the same canonical spellings (normalized to
//! lowercase) and refuse the rest with the same envelope at each request boundary, and the
//! name-based identities must be byte-identical UUIDv3 values.

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::ids::{UuidRefusal, admit_optional_uuid, admit_uuid, name_uuid};
use serde_json::Value;

const VECTORS: &str = include_str!("golden/ids.json");

fn load() -> Value {
    match serde_json::from_str(VECTORS) {
        Ok(value) => value,
        Err(error) => panic!("ids.json does not parse: {error}"),
    }
}

fn array<'a>(document: &'a Value, key: &str) -> &'a [Value] {
    match document[key].as_array() {
        Some(items) => items,
        None => panic!("missing array {key}"),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    match value[key].as_str() {
        Some(found) => found,
        None => panic!("missing string field {key} in {value}"),
    }
}

fn envelope_json(error: &AppError) -> Value {
    match serde_json::to_value(error_envelope(error)) {
        Ok(envelope) => envelope,
        Err(error) => panic!("envelope does not serialize: {error}"),
    }
}

/// The value of a vector row: a string, or `None` for JSON null.
fn optional_text(value: &Value) -> Option<&str> {
    value.as_str()
}

#[test]
fn admission_matches_the_golden_boundary() {
    let document = load();
    let field = text(&document, "admissionField");
    let message = text(&document, "admissionMessage");
    for case in array(&document, "admission") {
        let value = text(case, "value");
        // The deletion boundary (`entity_deletion._parse_id`) refuses with the validation envelope.
        let admitted = admit_uuid(value).map_err(|refusal| refusal.validation(message, field));
        if case["ok"] == Value::Bool(true) {
            match admitted {
                Ok(id) => assert_eq!(id.to_string(), text(case, "id"), "{value:?}"),
                Err(error) => panic!("{value:?}: the vector admits, Rust refuses: {error}"),
            }
        } else {
            match admitted {
                Ok(id) => panic!("{value:?}: the vector refuses, Rust admits {id}"),
                Err(error) => assert_eq!(envelope_json(&error), case["error"], "{value:?}"),
            }
        }
    }
}

#[test]
fn request_boundaries_map_refusals_to_their_own_envelopes() {
    let document = load();
    let message = text(&document, "admissionMessage");
    let field = text(&document, "admissionField");
    for case in array(&document, "boundaries") {
        let boundary = text(case, "boundary");
        let value = optional_text(&case["value"]);
        // Each boundary maps the same refusal differently; the vectors record the table.
        let outcome = match (boundary, value) {
            ("entityDeletion", Some(text)) => {
                admit_uuid(text).map_err(|r| r.validation(message, field))
            }
            ("entityDeletion", None) => Err(UuidRefusal::Malformed.validation(message, field)),
            ("generationIdentity", _) => {
                admit_optional_uuid(value).map_err(|r| r.validation(message, field))
            }
            ("matchingRequiredId", _) => {
                admit_optional_uuid(value).map_err(|r| r.domain_value(field))
            }
            (other, _) => panic!("unknown boundary {other}"),
        };
        if case["ok"] == Value::Bool(true) {
            match outcome {
                Ok(id) => assert_eq!(id.to_string(), text(case, "id"), "{boundary} {value:?}"),
                Err(error) => {
                    panic!("{boundary} {value:?}: the vector admits, Rust refuses: {error}")
                }
            }
        } else {
            match outcome {
                Ok(id) => panic!("{boundary} {value:?}: the vector refuses, Rust admits {id}"),
                Err(error) => {
                    assert_eq!(envelope_json(&error), case["error"], "{boundary} {value:?}")
                }
            }
        }
    }
}

#[test]
fn name_based_identities_match_the_golden_vectors() {
    let document = load();
    for case in array(&document, "nameUuid") {
        let identity = text(case, "identity");
        assert_eq!(
            name_uuid(identity).to_string(),
            text(case, "id"),
            "{identity:?}"
        );
    }
}
