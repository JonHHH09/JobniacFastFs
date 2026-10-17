//! The generated-asset advisory lock key texts of the Rust adapter, compared byte for byte with the
//! golden vectors in `golden/generated-lock-keys.json`.
//! No database is needed: PostgreSQL hashes each text with `hashtextextended(text, 0)`, and the
//! server-backed tests check that the hash of the Rust text equals the packaged SQL key.

use std::error::Error;

use jobniac_infra::postgres::generated::bindings::{asset_lock_key_input, document_lock_key_input};
use serde_json::Value;
use uuid::Uuid;

const VECTORS: &str = include_str!("golden/generated-lock-keys.json");

#[test]
fn lock_key_texts_are_byte_identical_to_the_golden_vectors() -> Result<(), Box<dyn Error>> {
    let all: Value = serde_json::from_str(VECTORS)?;
    assert_eq!(all["format"], "jobniac-generated-lock-keys");
    assert_eq!(all["hashFunction"], "hashtextextended(text, 0)");
    let cases = all["cases"].as_array().ok_or("missing cases")?;
    assert_eq!(cases.len(), 5);
    for case in cases {
        let id: Uuid = case["id"].as_str().ok_or("id")?.parse()?;
        let raw = case["raw"].as_str().ok_or("raw")?;
        let derived = match case["kind"].as_str().ok_or("kind")? {
            "asset" => asset_lock_key_input(id),
            "document" => document_lock_key_input(id),
            other => return Err(format!("unknown kind {other}").into()),
        };
        assert_eq!(derived, raw, "lock key text for {id}");
    }
    Ok(())
}

#[test]
fn lock_key_prefixes_are_the_golden_literals() -> Result<(), Box<dyn Error>> {
    let all: Value = serde_json::from_str(VECTORS)?;
    let id = Uuid::nil();
    assert_eq!(
        asset_lock_key_input(id),
        format!("{}{id}", all["prefixes"]["asset"].as_str().ok_or("asset")?)
    );
    assert_eq!(
        document_lock_key_input(id),
        format!(
            "{}{id}",
            all["prefixes"]["document"].as_str().ok_or("document")?
        )
    );
    Ok(())
}
