//! Replays the profile text golden vectors (`golden/profile-text.json`) against the deterministic
//! extractor of the core crate.
//!
//! Every vector is synthetic. For the start-date defect, the recorded run raises a plain
//! `ValueError`; the vector then carries the result of the spaced variant, and this crate must
//! produce that result for both the original and the spaced text.

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::profiles::extract::DeterministicProfileTextExtractor;
use serde_json::{Map, Value};

const VECTORS: &str = include_str!("golden/profile-text.json");

fn vectors() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(VECTORS)?)
}

/// Compares one extraction outcome with a recorded result.
fn compare(label: &str, actual: Result<Value, AppError>, expected: &Value) -> Result<(), String> {
    if expected["ok"] == Value::Bool(true) {
        let value = match actual {
            Ok(value) => value,
            Err(error) => {
                return Err(format!(
                    "{label}: expected a value, got {:?}",
                    error_envelope(&error)
                ));
            }
        };
        if value != expected["value"] {
            return Err(format!(
                "{label}: value differs\n rust: {value}\n recorded: {}",
                expected["value"]
            ));
        }
        return Ok(());
    }
    let error = match actual {
        Ok(value) => return Err(format!("{label}: expected an error, got {value}")),
        Err(error) => error,
    };
    let envelope = error_envelope(&error);
    if envelope.code.code() != expected["code"].as_str().unwrap_or_default() {
        return Err(format!("{label}: code {}", envelope.code.code()));
    }
    if envelope.message != expected["message"].as_str().unwrap_or_default() {
        return Err(format!("{label}: message {}", envelope.message));
    }
    let details: Map<String, Value> = envelope
        .details
        .iter()
        .map(|(key, value)| (key.clone(), Value::from(value.clone())))
        .collect();
    if Value::Object(details) != expected["details"] {
        return Err(format!("{label}: details differ"));
    }
    Ok(())
}

#[test]
fn extraction_matches_the_golden_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let all = vectors()?;
    let cases = all["extract"].as_array().ok_or("missing extract section")?;
    assert!(cases.len() >= 400, "the corpus shrank: {}", cases.len());
    let extractor = DeterministicProfileTextExtractor;
    let mut failures = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let text = case["text"].as_str().ok_or("text")?;
        let label = format!("case {index}");
        let result = &case["result"];
        if result["exception"].as_str() == Some("ValueError") {
            // The recorded crash becomes the spaced-variant result.
            let fixed = &case["fixed"];
            let variant = case["fixedVariant"].as_str().ok_or("fixedVariant")?;
            if let Err(message) = compare(
                &format!("{label} (defect)"),
                extractor.extract_profile(text),
                fixed,
            ) {
                failures.push(message);
            }
            if let Err(message) = compare(
                &format!("{label} (spaced)"),
                extractor.extract_profile(variant),
                fixed,
            ) {
                failures.push(message);
            }
        } else if let Err(message) = compare(&label, extractor.extract_profile(text), result) {
            failures.push(message);
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches, first: {}",
        failures.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

#[test]
fn the_corpus_covers_every_error_kind() -> Result<(), Box<dyn std::error::Error>> {
    let all = vectors()?;
    let cases = all["extract"].as_array().ok_or("missing extract section")?;
    let mut fields = std::collections::BTreeSet::new();
    let mut defects = 0;
    for case in cases {
        if case["result"]["exception"].as_str() == Some("ValueError") {
            defects += 1;
        }
        if let Some(field) = case["result"]["details"]["field"].as_str() {
            fields.insert(field.to_owned());
        }
    }
    assert!(defects >= 10, "the start-date defect cases are missing");
    assert_eq!(
        fields.into_iter().collect::<Vec<_>>(),
        vec!["email", "fullName", "text"]
    );
    Ok(())
}
