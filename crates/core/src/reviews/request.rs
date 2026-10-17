//! Request admission shared by the review validators and the use cases.
//!
//! They cover the request object, required and optional identifiers and the page request, with
//! fixed `DomainValue` messages and a fixed check order. The matching module keeps its own private
//! copies of the same rules; they are kept in step by the golden vectors.

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::errors::AppError;
use crate::ids::{UuidRefusal, admit_uuid};
use crate::pagination::{CursorCodec, MAX_LIMIT, PageRequest};

const REQUEST_MESSAGE: &str = "request must not be null";

/// The request object; any other JSON value is refused.
pub(crate) fn request_object(request: &Value) -> Result<&Map<String, Value>, AppError> {
    request
        .as_object()
        .ok_or_else(|| AppError::DomainValue(REQUEST_MESSAGE.to_owned()))
}

/// Whether a field is absent or JSON null.
pub(crate) fn is_absent(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

/// A required identifier: absent or null, a non-string, or a non-canonical UUID are refused with the
/// `DomainValue` messages (`<field> must not be null`, `<field> must be a valid UUID`).
pub(crate) fn required_id(value: Option<&Value>, field: &str) -> Result<Uuid, AppError> {
    let text = match value {
        None | Some(Value::Null) => return Err(UuidRefusal::Null.domain_value(field)),
        Some(Value::String(text)) => text,
        Some(_) => return Err(UuidRefusal::Malformed.domain_value(field)),
    };
    admit_uuid(text).map_err(|refusal| refusal.domain_value(field))
}

/// An optional identifier: absent or null is `None`, anything else must be a required identifier.
pub(crate) fn optional_id(value: Option<&Value>, field: &str) -> Result<Option<Uuid>, AppError> {
    if is_absent(value) {
        Ok(None)
    } else {
        required_id(value, field).map(Some)
    }
}

/// Validates the page request in this order: the cursor is decoded first, then the limit is
/// checked, then the scope and the filter fingerprint. A limit must be a JSON integer.
pub(crate) fn page_request_for<'a>(
    codec: &'a CursorCodec,
    values: &Map<String, Value>,
    scope: &str,
    filter_identity: &str,
) -> Result<PageRequest<'a>, AppError> {
    let cursor = match values.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.as_str()),
        Some(_) => return Err(crate::pagination::invalid_cursor()),
    };
    codec.decode(cursor)?;
    let limit = match values.get("limit") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => Some(number.as_i64().ok_or_else(limit_error)?),
        Some(_) => return Err(limit_error()),
    };
    codec.page_request(limit, cursor, scope, filter_identity)
}

fn limit_error() -> AppError {
    AppError::DomainValue(format!("limit must be between 1 and {MAX_LIMIT}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn required_id_refusals_use_the_fixed_messages() {
        assert_eq!(
            required_id(None, "reportId"),
            Err(AppError::DomainValue(
                "reportId must not be null".to_owned()
            ))
        );
        assert_eq!(
            required_id(Some(&json!(5)), "reportId"),
            Err(AppError::DomainValue(
                "reportId must be a valid UUID".to_owned()
            ))
        );
        assert_eq!(
            required_id(
                Some(&json!("{00000000-0000-4000-8000-000000000001}")),
                "reportId"
            ),
            Err(AppError::DomainValue(
                "reportId must be a valid UUID".to_owned()
            ))
        );
        assert!(
            required_id(
                Some(&json!("00000000-0000-4000-8000-00000000000A")),
                "reportId"
            )
            .is_ok()
        );
    }

    #[test]
    fn optional_id_accepts_null_and_absent() {
        assert_eq!(optional_id(None, "reportId"), Ok(None));
        assert_eq!(optional_id(Some(&Value::Null), "reportId"), Ok(None));
    }
}
