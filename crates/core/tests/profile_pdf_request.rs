//! Replays the `ingestRequest` golden vectors (`golden/profile-text.json`) against
//! [`IngestRequest::parse`].
//!
//! The recorded parser accepts braced, URN and compact UUID text. Only the hyphenated form is
//! admitted here, so those inputs are refused with the normal validation error. Every other vector
//! must match the recorded result exactly: the value fields, or the code, message and details.

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::profiles::pdf_ingest::{IngestRequest, lock_key};
use serde_json::{Map, Value};

const VECTORS: &str = include_str!("golden/profile-text.json");

/// Whether the text is a UUID in a form that is refused (braced, URN or compact).
fn non_canonical_uuid(text: &str) -> bool {
    let canonical = text.len() == 36
        && text.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    !canonical && !text.is_empty()
}

/// The recorded shape of the value of a parsed request.
fn as_recorded_value(request: &IngestRequest) -> Value {
    serde_json::json!({
        "documentId": request.document_id.to_string(),
        "existingProfileId": request.existing_profile_id.map(|id| id.to_string()),
        "overwriteExistingProfile": request.overwrite_existing_profile,
        "maxCharacters": request.max_characters,
        "expectedRevision": request.expected_revision,
    })
}

fn envelope_details(error: &AppError) -> (String, String, Value) {
    let envelope = error_envelope(error);
    let details: Map<String, Value> = envelope
        .details
        .iter()
        .map(|(key, value)| (key.clone(), Value::from(value.clone())))
        .collect();
    (
        envelope.code.code().to_owned(),
        envelope.message,
        Value::Object(details),
    )
}

#[test]
fn ingest_requests_match_the_golden_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let all: Value = serde_json::from_str(VECTORS)?;
    let cases = all["ingestRequest"]
        .as_array()
        .ok_or("missing ingestRequest")?;
    assert!(cases.len() >= 30, "the request corpus shrank");
    let mut failures = Vec::new();
    let mut deviations = 0;
    for (index, case) in cases.iter().enumerate() {
        let request = &case["request"];
        let expected = &case["result"];
        let actual = IngestRequest::parse(request);
        if expected["ok"] == Value::Bool(true) {
            match actual {
                Ok(parsed) => {
                    if as_recorded_value(&parsed) != expected["value"] {
                        failures.push(format!("case {index}: value differs for {request}"));
                    }
                }
                Err(error) => {
                    // Only a non-canonical identifier form may be refused, as a validation
                    // error on the identifier field.
                    let (code, _, details) = envelope_details(&error);
                    let raw_documents = request["documentId"].as_str().unwrap_or_default();
                    let raw_profiles = request["existingProfileId"].as_str().unwrap_or_default();
                    let refused_form =
                        non_canonical_uuid(raw_documents) || non_canonical_uuid(raw_profiles);
                    if refused_form && code == "validation_error" {
                        deviations += 1;
                    } else {
                        failures.push(format!("case {index}: unexpected refusal {details}"));
                    }
                }
            }
            continue;
        }
        match actual {
            Ok(parsed) => failures.push(format!(
                "case {index}: expected {expected}, got {}",
                as_recorded_value(&parsed)
            )),
            Err(error) => {
                let (code, message, details) = envelope_details(&error);
                let expected_details = expected["details"].clone();
                if code != expected["code"].as_str().unwrap_or_default()
                    || message != expected["message"].as_str().unwrap_or_default()
                    || details != expected_details
                {
                    failures.push(format!(
                        "case {index}: got {code} {message} {details}, expected {expected}"
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        deviations >= 3,
        "the refused-spelling cases were not exercised"
    );
    Ok(())
}

#[test]
fn lock_keys_are_byte_identical_to_the_golden_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let all: Value = serde_json::from_str(VECTORS)?;
    let keys = all["lockKeys"].as_array().ok_or("missing lockKeys")?;
    assert_eq!(keys.len(), 6);
    for key in keys {
        let raw = key["raw"].as_str().ok_or("raw")?;
        let hashed = key["hashed"].as_str().ok_or("hashed")?;
        assert_eq!(lock_key(raw), hashed, "lock key {raw}");
    }
    Ok(())
}
