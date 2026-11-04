//! Request-parsing helpers shared by the command adapters.
//!
//! These are the small checks every `*_commands` module makes before a use case runs: the database
//! gate, the request JSON text, the revision text, a normalized UUID and the document and page
//! fields of a request. Each takes the failing operation's message id, so the wire code and copy of
//! a refusal stay with the command that calls it.

use jobniac_core::ids::admit_uuid;
use jobniac_runtime::services::Services;
use jobniac_text::str_rules::strip_text;
use serde_json::Value;
use uuid::Uuid;

use crate::error::CommandError;

/// Wire code of a request made while no local database is configured.
pub const NOT_CONFIGURED: &str = "not_configured";

/// Wire code of every refused request shape.
const VALIDATION: &str = "validation_error";

/// Refuses a request before any use case runs while no local database is configured. The
/// composition root keeps the services without a database, so every command makes this check
/// itself and passes its own message id.
pub(crate) fn require_database(
    services: &Services,
    unconfigured: &str,
) -> Result<(), CommandError> {
    match services.database {
        Some(_) => Ok(()),
        None => Err(CommandError::new(NOT_CONFIGURED, unconfigured)),
    }
}

/// Parses the request text the UI sends, whatever JSON value it holds. A lone surrogate escape is not
/// valid JSON text, so it is refused with the operation's validation copy.
pub(crate) fn parse_request(text: &str, message_id: &str) -> Result<Value, CommandError> {
    serde_json::from_str(text).map_err(|_| CommandError::new(VALIDATION, message_id))
}

/// Parses the request object of a command. Malformed JSON, a lone surrogate escape or a value
/// that is not an object is refused with the operation's generic copy.
pub(crate) fn parse_object_request(
    request_json: &str,
    generic: &str,
) -> Result<Value, CommandError> {
    match serde_json::from_str::<Value>(request_json) {
        Ok(value) if value.is_object() => Ok(value),
        _ => Err(CommandError::new(VALIDATION, generic)),
    }
}

/// Parses the revision text: a non-empty run of ASCII digits, as a decimal integer. A sign, a space
/// or any other character is refused (`"+7"` is not the revision 7). The range checks stay in the core.
pub(crate) fn parse_revision(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok()
}

/// Parses a UUID after normalization (white space removed, lowercase); `None` when it is not a
/// canonical UUID.
pub(crate) fn uuid_text(text: &str) -> Option<Uuid> {
    admit_uuid(&strip_text(text).to_ascii_lowercase()).ok()
}

/// Admits an identifier: trimmed, lowercased, then a canonical UUID. A malformed one is a
/// validation error with the operation's copy.
pub(crate) fn admit_id(text: &str, message_id: &str) -> Result<Uuid, CommandError> {
    uuid_text(text).ok_or_else(|| CommandError::new(VALIDATION, message_id))
}

/// A required text field of the request. A missing field or a value that is not text is refused.
pub(crate) fn text_field<'a>(
    request: &'a Value,
    key: &str,
    generic: &str,
) -> Result<&'a str, CommandError> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::new("validation_error", generic))
}

/// The stored document ID of a request: required text, trimmed, lowercased, a canonical UUID.
pub(crate) fn document_id_field(request: &Value, message_id: &str) -> Result<Uuid, CommandError> {
    let text = text_field(request, "documentId", message_id)?;
    admit_id(text, message_id)
}

/// The zero-based page index of a request: a JSON integer that fits in `u32` and, when `max_exclusive`
/// is given, is below it.
pub(crate) fn page_index_field(
    request: &Value,
    max_exclusive: Option<u32>,
    message_id: &str,
) -> Result<u32, CommandError> {
    request
        .get("pageIndex")
        .and_then(Value::as_u64)
        .and_then(|index| u32::try_from(index).ok())
        .filter(|index| max_exclusive.is_none_or(|max| *index < max))
        .ok_or_else(|| CommandError::new(VALIDATION, message_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "00000000-0000-4000-8000-0000000000a1";

    fn refusal(message_id: &str) -> CommandError {
        CommandError::new("validation_error", message_id)
    }

    #[test]
    fn parse_request_accepts_any_json_and_refuses_invalid_text() {
        assert_eq!(parse_request("[1]", "m").ok(), Some(json!([1])));
        assert_eq!(parse_request("{", "m").err(), Some(refusal("m")));
        assert_eq!(
            parse_request(r#"{"a":"\ud800"}"#, "m").err(),
            Some(refusal("m"))
        );
    }

    #[test]
    fn parse_object_request_refuses_non_objects() {
        assert_eq!(
            parse_object_request(r#"{"a":1}"#, "m").ok(),
            Some(json!({"a": 1}))
        );
        assert_eq!(parse_object_request("[]", "m").err(), Some(refusal("m")));
        assert_eq!(parse_object_request("{", "m").err(), Some(refusal("m")));
    }

    #[test]
    fn parse_revision_takes_only_ascii_digits() {
        assert_eq!(parse_revision("7"), Some(7));
        assert_eq!(parse_revision("9223372036854775807"), Some(i64::MAX));
        for refused in ["", "+7", "-7", " 7", "7 ", "1.5", "9223372036854775808"] {
            assert_eq!(parse_revision(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn uuid_text_trims_and_lowercases() {
        let padded = format!("  {}\n", ID.to_ascii_uppercase());
        assert_eq!(
            uuid_text(&padded).map(|id| id.to_string()).as_deref(),
            Some(ID)
        );
        assert_eq!(uuid_text("not-a-uuid"), None);
        assert_eq!(uuid_text(""), None);
    }

    #[test]
    fn admit_id_refuses_with_the_given_copy() -> Result<(), CommandError> {
        let upper = format!(" {} ", ID.to_ascii_uppercase());
        assert_eq!(admit_id(&upper, "m")?.to_string(), ID);
        assert_eq!(admit_id("nope", "copy").err(), Some(refusal("copy")));
        Ok(())
    }

    #[test]
    fn document_id_field_requires_text_that_is_a_uuid() -> Result<(), CommandError> {
        let request = json!({"documentId": format!("  {}  ", ID.to_ascii_uppercase())});
        assert_eq!(document_id_field(&request, "m")?.to_string(), ID);
        assert_eq!(document_id_field(&json!({}), "m").err(), Some(refusal("m")));
        assert_eq!(
            document_id_field(&json!({"documentId": 7}), "m").err(),
            Some(refusal("m"))
        );
        assert_eq!(
            document_id_field(&json!({"documentId": "x"}), "m").err(),
            Some(refusal("m"))
        );
        Ok(())
    }

    #[test]
    fn page_index_bound_applies_only_when_given() {
        let big = json!({"pageIndex": 4_000_000_000_u64});
        assert_eq!(page_index_field(&big, None, "m").ok(), Some(4_000_000_000));
        assert_eq!(
            page_index_field(&big, Some(200), "m").err(),
            Some(refusal("m"))
        );
        let last = json!({"pageIndex": 199});
        assert_eq!(page_index_field(&last, Some(200), "m").ok(), Some(199));
        let past = json!({"pageIndex": 200});
        assert_eq!(
            page_index_field(&past, Some(200), "m").err(),
            Some(refusal("m"))
        );
        for bad in [
            json!({}),
            json!({"pageIndex": -1}),
            json!({"pageIndex": "1"}),
            json!({"pageIndex": 4_294_967_296_u64}),
        ] {
            assert_eq!(
                page_index_field(&bad, None, "m").err(),
                Some(refusal("m")),
                "{bad}"
            );
        }
    }
}
