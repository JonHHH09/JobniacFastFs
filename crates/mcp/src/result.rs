//! Tool results and their `CallToolResult` shapes, as they go on the wire.
//!
//! - [`ToolResult::Success`]: a structured success. The text is the value in the indented layout
//!   without ASCII escaping, `structuredContent` is the value, and `isError` is `false`.
//! - [`ToolResult::Failure`]: an application failure of a `_call` wrapper. The text is the
//!   envelope `{"code", "message", "details"}` in the module's separator style with ASCII escaping,
//!   `structuredContent` is the same envelope and `isError` is `true`.
//! - [`ToolResult::InvalidArguments`]: the SDK's rejection of the arguments. The text is the SDK
//!   error text, there is no `structuredContent`, and `isError` is `true`.

use jobniac_core::errors::{AppError, ErrorEnvelope, error_envelope};
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Map, Value};

use crate::args::ArgErrors;
use crate::wire_json::{Layout, dumps};

/// The separator style of a module's error text (see [`crate::wire_json`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStyle {
    /// Compact separators, used by the profile, job and most other modules and by `list_profiles`.
    Compact,
    /// Default separators, used by the match, review, document and generation modules.
    Default,
}

/// The outcome of one tool call, before it is put on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResult {
    /// A successful call with its structured result.
    Success(Value),
    /// A failed application call with its envelope and the module's text style.
    Failure {
        /// The `{code, message, details}` envelope.
        envelope: ErrorEnvelope,
        /// The separator style of the module's error text.
        style: EnvelopeStyle,
    },
    /// Arguments that the SDK rejects before the tool runs.
    InvalidArguments(ArgErrors),
}

impl ToolResult {
    /// Map a service outcome the way a call wrapper does: a value is a success, and an
    /// error becomes its envelope (see `jobniac_core::errors::error_envelope`).
    pub fn from_outcome(outcome: Result<Value, AppError>, style: EnvelopeStyle) -> Self {
        match outcome {
            Ok(value) => Self::Success(value),
            Err(error) => Self::Failure {
                envelope: error_envelope(&error),
                style,
            },
        }
    }

    /// The `CallToolResult` that the server returns for this outcome.
    pub fn into_call_result(self) -> CallToolResult {
        match self {
            Self::Success(value) => {
                let text = dumps(&value, Layout::Indented, false);
                let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
                result.structured_content = Some(value);
                result
            }
            Self::Failure { envelope, style } => {
                let value = envelope_value(&envelope);
                let layout = match style {
                    EnvelopeStyle::Compact => Layout::Compact,
                    EnvelopeStyle::Default => Layout::Default,
                };
                let text = dumps(&value, layout, true);
                let mut result = CallToolResult::error(vec![ContentBlock::text(text)]);
                result.structured_content = Some(value);
                result
            }
            Self::InvalidArguments(errors) => {
                CallToolResult::error(vec![ContentBlock::text(errors.render())])
            }
        }
    }
}

impl From<ArgErrors> for ToolResult {
    fn from(errors: ArgErrors) -> Self {
        Self::InvalidArguments(errors)
    }
}

/// The envelope as a JSON object with the fixed key order: `code`, `message`, `details`.
fn envelope_value(envelope: &ErrorEnvelope) -> Value {
    let details: Map<String, Value> = envelope
        .details
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    let mut object = Map::new();
    object.insert(
        "code".to_owned(),
        Value::String(envelope.code.code().to_owned()),
    );
    object.insert(
        "message".to_owned(),
        Value::String(envelope.message.clone()),
    );
    object.insert("details".to_owned(), Value::Object(details));
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use jobniac_core::errors::ErrorCode;
    use serde_json::json;

    use super::*;
    use crate::args::{ArgDecoder, REQUIRED, strict_int};

    #[test]
    fn failure_text_is_the_compact_envelope_with_ascii_escapes() {
        let mut details = IndexMap::new();
        details.insert("profileId".to_owned(), "M\u{fc}ller".to_owned());
        let envelope = ErrorEnvelope {
            code: ErrorCode::Conflict,
            message: "Profile revision changed".to_owned(),
            details,
        };
        let result = ToolResult::Failure {
            envelope,
            style: EnvelopeStyle::Compact,
        }
        .into_call_result();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content,
            Some(json!({
                "code": "conflict",
                "message": "Profile revision changed",
                "details": {"profileId": "M\u{fc}ller"}
            }))
        );
        let text = first_text(&result);
        assert_eq!(
            text,
            r#"{"code":"conflict","message":"Profile revision changed","details":{"profileId":"M\u00fcller"}}"#
        );
    }

    #[test]
    fn success_text_is_indented_and_not_ascii_escaped() {
        let value = json!({"fullName": "Zo\u{eb}"});
        let result = ToolResult::Success(value.clone()).into_call_result();
        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.structured_content, Some(value));
        assert_eq!(first_text(&result), "{\n  \"fullName\": \"Zo\u{eb}\"\n}");
    }

    #[test]
    fn argument_rejection_has_no_structured_content() {
        let arguments = Map::new();
        let mut decoder = ArgDecoder::new("t");
        decoder.field(&arguments, "", "a", REQUIRED, strict_int);
        let Err(errors) = decoder.check() else {
            panic!("a missing required key must be rejected");
        };
        let result = ToolResult::from(errors).into_call_result();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, None);
        assert_eq!(
            first_text(&result),
            "Error executing tool t: 1 validation error for tArguments\na\n  Field required [type=missing]\n    For further information visit https://errors.pydantic.dev/2.13/v/missing"
        );
    }

    #[test]
    fn service_outcome_maps_errors_to_their_envelopes() {
        let outcome = ToolResult::from_outcome(Err(AppError::Internal), EnvelopeStyle::Default);
        assert_eq!(
            outcome,
            ToolResult::Failure {
                envelope: ErrorEnvelope {
                    code: ErrorCode::InternalError,
                    message: "Unexpected application error".to_owned(),
                    details: IndexMap::new(),
                },
                style: EnvelopeStyle::Default,
            }
        );
    }

    fn first_text(result: &CallToolResult) -> String {
        result
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|text| text.text.clone())
            .unwrap_or_default()
    }
}
