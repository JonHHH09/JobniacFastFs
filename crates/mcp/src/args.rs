//! Argument decoding for MCP tools, matching the MCP SDK's argument validation.
//!
//! Tool arguments are validated against strict models built from the tool signatures (`StrictInt`,
//! `StrictStr`, `StrictBool`, `UUID`, `TypedDict` with `total=True` or `total=False`, and
//! `X | None`). A rejected call returns a tool error whose text is the SDK's rendering of the
//! validation error, with no submitted value in it. This module decodes the JSON in a fixed order,
//! collects every issue, and renders that text (see [`ArgErrors::render`]).
//!
//! Rules:
//!
//! - A key of a required shape must be present, even when its type allows `null` (`Field required`).
//! - A key of an optional shape may be absent; `null` is accepted only where the type allows it.
//! - Strict types: integers are not floats and booleans are not integers; strings are not coerced.
//! - Unknown keys are ignored (`extra="ignore"`).
//! - `Int32` and `Int64` are schema hints only: the adapter does not check their range.
//!   [`strict_int`] therefore accepts every JSON integer that fits in `i64`, and the domain decides.
//!   Integers beyond `i64` are a documented limitation (they are refused here).
//! - Identifiers: [`uuid_arg`] refuses non-strings (`uuid_type`) and admits strings
//!   that pass UUID admission (`uuid_parsing`, a generic text, see [`IssueKind::UuidParsing`]).

use jobniac_core::ids::admit_uuid;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::registry::Arguments;

/// The documentation base URL that the SDK's error text cites for each error type.
const ERROR_DOCS: &str = "https://errors.pydantic.dev/2.13/v/";

/// A kind of argument issue, named by its validation error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// A required key is absent (`missing`).
    Missing,
    /// A value is not a strict integer (`int_type`).
    IntType,
    /// A value is not a string (`string_type`).
    StringType,
    /// A value is not a boolean (`bool_type`).
    BoolType,
    /// A value is not an object (`dict_type`).
    DictType,
    /// A value is not an array (`list_type`).
    ListType,
    /// A UUID value is not a string (`uuid_type`).
    UuidType,
    /// A string is not an admitted UUID (`uuid_parsing`). The SDK's own text names the offending
    /// character, which would echo submitted input; this text is generic instead.
    UuidParsing,
    /// The request held a lone surrogate, which the JSON-RPC line cannot carry as a string
    /// (`string_unicode`). Raised at stdin admission, see [`crate::stdin_admission`].
    StringUnicode,
    /// The request nested deeper than the admitted limit (`json_invalid`). Raised at stdin
    /// admission, see [`crate::stdin_admission`].
    JsonInvalid,
    /// A value is not one of the admitted strings of a `Literal` (`literal_error`). The admitted
    /// values are listed in the text.
    LiteralError(&'static [&'static str]),
}

impl IssueKind {
    /// The message line of the SDK's error text.
    pub fn message(self) -> String {
        match self {
            Self::Missing => "Field required".to_owned(),
            Self::IntType => "Input should be a valid integer".to_owned(),
            Self::StringType => "Input should be a valid string".to_owned(),
            Self::BoolType => "Input should be a valid boolean".to_owned(),
            Self::DictType => "Input should be a valid dictionary".to_owned(),
            Self::ListType => "Input should be a valid list".to_owned(),
            Self::UuidType => "UUID input should be a string, bytes or UUID object".to_owned(),
            Self::UuidParsing => "Input should be a valid UUID".to_owned(),
            Self::StringUnicode => {
                "Input should be a valid string, unable to parse raw data as a unicode string"
                    .to_owned()
            }
            Self::JsonInvalid => "Invalid JSON: recursion limit exceeded".to_owned(),
            Self::LiteralError(expected) => {
                format!("Input should be {}", literal_expectation(expected))
            }
        }
    }

    /// The error type identifier (`[type=...]`).
    pub const fn type_id(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::IntType => "int_type",
            Self::StringType => "string_type",
            Self::BoolType => "bool_type",
            Self::DictType => "dict_type",
            Self::ListType => "list_type",
            Self::UuidType => "uuid_type",
            Self::UuidParsing => "uuid_parsing",
            Self::StringUnicode => "string_unicode",
            Self::JsonInvalid => "json_invalid",
            Self::LiteralError(_) => "literal_error",
        }
    }
}

/// The admitted values of a `Literal`, each quoted, separated by `, `
/// with ` or ` before the last one (`'A', 'B' or 'C'`).
fn literal_expectation(expected: &[&str]) -> String {
    let quoted: Vec<String> = expected.iter().map(|value| format!("'{value}'")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// One issue: the dotted location of the argument (`request.limit`) and its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgIssue {
    location: String,
    kind: IssueKind,
}

impl ArgIssue {
    /// The dotted location of the argument.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// The kind of issue.
    pub const fn kind(&self) -> IssueKind {
        self.kind
    }
}

/// A rejected argument set of one tool call, in validation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgErrors {
    tool: String,
    issues: Vec<ArgIssue>,
}

impl ArgErrors {
    /// The rejection of a whole call whose input could not be admitted (see
    /// [`crate::stdin_admission`]). The single issue has no location, as root-level
    /// errors have none, so the text carries no location line and never echoes the input.
    pub fn input_rejection(tool: &str, kind: IssueKind) -> Self {
        Self {
            tool: tool.to_owned(),
            issues: vec![ArgIssue {
                location: String::new(),
                kind,
            }],
        }
    }

    /// The tool whose arguments were rejected.
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Every issue, in the order the decoder met them.
    pub fn issues(&self) -> &[ArgIssue] {
        &self.issues
    }

    /// The text of the SDK's tool error: `Error executing tool <tool>: <n> validation error(s)
    /// for <tool>Arguments`, then one block per issue with its location (when it has one), message
    /// and error type.
    pub fn render(&self) -> String {
        let count = self.issues.len();
        let plural = if count == 1 { "" } else { "s" };
        let mut text = format!(
            "Error executing tool {tool}: {count} validation error{plural} for {tool}Arguments",
            tool = self.tool
        );
        for issue in &self.issues {
            let type_id = issue.kind.type_id();
            text.push('\n');
            if !issue.location.is_empty() {
                text.push_str(&issue.location);
                text.push('\n');
            }
            text.push_str("  ");
            text.push_str(&issue.kind.message());
            text.push_str(" [type=");
            text.push_str(type_id);
            text.push(']');
            text.push_str("\n    For further information visit ");
            text.push_str(ERROR_DOCS);
            text.push_str(type_id);
        }
        text
    }
}

/// The presence rule of one key: whether it is required and whether `null` is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// The key must be present (`total=True`, or a parameter without a default).
    pub required: bool,
    /// The type allows `null` (`X | None`).
    pub nullable: bool,
}

/// Required key, `null` not allowed.
pub const REQUIRED: Shape = Shape {
    required: true,
    nullable: false,
};

/// Required key that may be `null` (`X | None` without a default; the key must still be sent).
pub const REQUIRED_NULLABLE: Shape = Shape {
    required: true,
    nullable: true,
};

/// Optional key (`total=False`), `null` not allowed.
pub const OPTIONAL: Shape = Shape {
    required: false,
    nullable: false,
};

/// Optional key that may be `null` (`X | None` with a `None` default).
pub const OPTIONAL_NULLABLE: Shape = Shape {
    required: false,
    nullable: true,
};

/// The decoded state of one key.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot<T> {
    /// The key was absent (and optional), or its value was rejected (see the issues).
    Absent,
    /// The key was present with `null`, and the shape allows it.
    Null,
    /// The key was present with a valid value.
    Value(T),
}

impl<T> Slot<T> {
    /// The value, when the key was present and valid.
    pub fn into_value(self) -> Option<T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Absent | Self::Null => None,
        }
    }
}

/// Collects the issues of one argument set while the tool decodes it.
#[derive(Debug)]
pub struct ArgDecoder {
    tool: &'static str,
    issues: Vec<ArgIssue>,
}

impl ArgDecoder {
    /// A decoder for the arguments of `tool`.
    pub const fn new(tool: &'static str) -> Self {
        Self {
            tool,
            issues: Vec::new(),
        }
    }

    /// Decode one key of `object`. `parent` is the dotted location of `object` (empty for the
    /// top-level arguments). A rejected value records its issues and yields [`Slot::Absent`].
    pub fn field<'a, T>(
        &mut self,
        object: &'a Map<String, Value>,
        parent: &str,
        key: &str,
        shape: Shape,
        decode: impl FnOnce(&str, &'a Value, &mut Vec<ArgIssue>) -> Option<T>,
    ) -> Slot<T> {
        let location = if parent.is_empty() {
            key.to_owned()
        } else {
            format!("{parent}.{key}")
        };
        match object.get(key) {
            None => {
                if shape.required {
                    self.issues.push(ArgIssue {
                        location,
                        kind: IssueKind::Missing,
                    });
                }
                Slot::Absent
            }
            Some(Value::Null) if shape.nullable => Slot::Null,
            Some(value) => match decode(&location, value, &mut self.issues) {
                Some(decoded) => Slot::Value(decoded),
                None => Slot::Absent,
            },
        }
    }

    /// Finish decoding: `Ok` when no issue was recorded, otherwise every issue as [`ArgErrors`].
    pub fn check(self) -> Result<(), ArgErrors> {
        if self.issues.is_empty() {
            Ok(())
        } else {
            Err(ArgErrors {
                tool: self.tool.to_owned(),
                issues: self.issues,
            })
        }
    }
}

fn reject(location: &str, kind: IssueKind, issues: &mut Vec<ArgIssue>) {
    issues.push(ArgIssue {
        location: location.to_owned(),
        kind,
    });
}

/// A strict integer (`StrictInt`, `Int32` and `Int64` alike), within `i64`.
pub fn strict_int(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<i64> {
    let decoded = value.as_i64();
    if decoded.is_none() {
        reject(location, IssueKind::IntType, issues);
    }
    decoded
}

/// A domain integer: any JSON integer that serde_json holds as `i64` or `u64`, forwarded unchanged.
///
/// A strict integer admits every integer, and the service decides the range (its messages
/// name the bounds, such as `limit must be between 1 and 100` or `overallScore must be 0-100`).
/// Integers above `u64` are a documented limitation: serde_json holds them as floats
/// and they are refused here as `int_type`.
pub fn domain_int(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    if value.is_i64() || value.is_u64() {
        Some(value.clone())
    } else {
        reject(location, IssueKind::IntType, issues);
        None
    }
}

/// A string (`StrictStr`), not coerced.
pub fn strict_string(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<String> {
    let decoded = value.as_str().map(str::to_owned);
    if decoded.is_none() {
        reject(location, IssueKind::StringType, issues);
    }
    decoded
}

/// A boolean (`StrictBool`), not coerced from integers or strings.
pub fn strict_bool(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<bool> {
    let decoded = value.as_bool();
    if decoded.is_none() {
        reject(location, IssueKind::BoolType, issues);
    }
    decoded
}

/// A `Literal` of strings (`Literal[...]`), not coerced: any value that is not one of `expected`,
/// including a non-string, is `literal_error`.
pub fn literal_str<'v>(
    location: &str,
    value: &'v Value,
    issues: &mut Vec<ArgIssue>,
    expected: &'static [&'static str],
) -> Option<&'v str> {
    match value.as_str() {
        Some(text) if expected.contains(&text) => Some(text),
        _ => {
            reject(location, IssueKind::LiteralError(expected), issues);
            None
        }
    }
}

/// An object (a `TypedDict` argument), borrowed from the arguments.
pub fn dict_arg<'v>(
    location: &str,
    value: &'v Value,
    issues: &mut Vec<ArgIssue>,
) -> Option<&'v Map<String, Value>> {
    let decoded = value.as_object();
    if decoded.is_none() {
        reject(location, IssueKind::DictType, issues);
    }
    decoded
}

/// An array (`list[...]`). Element types are decoded by the caller.
pub fn list_arg<'v>(
    location: &str,
    value: &'v Value,
    issues: &mut Vec<ArgIssue>,
) -> Option<&'v Vec<Value>> {
    let decoded = value.as_array();
    if decoded.is_none() {
        reject(location, IssueKind::ListType, issues);
    }
    decoded
}

/// A UUID argument (`UUID` for `get_profile` and `update_profile`, and the admitted form
/// of `UuidText` elsewhere). Non-strings are `uuid_type`; strings must pass UUID admission.
pub fn uuid_arg(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Uuid> {
    let Some(text) = value.as_str() else {
        reject(location, IssueKind::UuidType, issues);
        return None;
    };
    match admit_uuid(text) {
        Ok(uuid) => Some(uuid),
        Err(_) => {
            reject(location, IssueKind::UuidParsing, issues);
            None
        }
    }
}

/// The name of a nested decoder. Its issues are merged into the caller's decoder, so this name never
/// reaches a client.
const NESTED: &str = "nested";

/// The decoder of one list item: the item location, its value and the issue sink.
pub type ItemDecoder = fn(&str, &Value, &mut Vec<ArgIssue>) -> Option<()>;

/// A `TypedDict` argument at `location` (`dict_type` for a non-object). `fields` decodes its keys in
/// declaration order, each at its dotted location, and the issues of the keys are kept in that order.
pub fn typed_dict<'v>(
    location: &str,
    value: &'v Value,
    issues: &mut Vec<ArgIssue>,
    fields: impl FnOnce(&mut ArgDecoder, &'v Map<String, Value>, &str),
) -> Option<()> {
    let object = dict_arg(location, value, issues)?;
    let mut decoder = ArgDecoder::new(NESTED);
    fields(&mut decoder, object, location);
    if let Err(errors) = decoder.check() {
        issues.extend_from_slice(errors.issues());
    }
    Some(())
}

/// The keys of `keys` that `object` holds, in the order of `keys`, with their values unchanged. A
/// `TypedDict` argument reaches its service this way: only the declared keys are kept.
pub fn declared_keys(object: &Map<String, Value>, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|key| {
            object
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect()
}

/// A `list[...]` at `location`, with `item` applied to each element at `location.index`.
pub fn list_of(
    location: &str,
    value: &Value,
    issues: &mut Vec<ArgIssue>,
    item: ItemDecoder,
) -> Option<()> {
    let items = list_arg(location, value, issues)?;
    for (index, element) in items.iter().enumerate() {
        item(&format!("{location}.{index}"), element, issues);
    }
    Some(())
}

/// The decoded `request` of a keyset listing: the optional page limit and cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    /// The page size, or `None` when absent or null.
    pub limit: Option<i64>,
    /// The cursor of the page, or `None` when absent or null.
    pub cursor: Option<String>,
}

/// Decode the optional `request` of a listing tool (`limit` and `cursor`, both optional and
/// nullable). An absent or null request is `{}`. `tool` names the tool in the rejection text.
pub fn page_request(tool: &'static str, arguments: &Arguments) -> Result<PageRequest, ArgErrors> {
    let mut decoder = ArgDecoder::new(tool);
    let mut decoded = PageRequest {
        limit: None,
        cursor: None,
    };
    if let Slot::Value(request) =
        decoder.field(arguments, "", "request", OPTIONAL_NULLABLE, dict_arg)
    {
        decoded.limit = decoder
            .field(request, "request", "limit", OPTIONAL_NULLABLE, strict_int)
            .into_value();
        decoded.cursor = decoder
            .field(
                request,
                "request",
                "cursor",
                OPTIONAL_NULLABLE,
                strict_string,
            )
            .into_value();
    }
    decoder.check()?;
    Ok(decoded)
}

/// The `request` argument of a tool whose decoder has already checked its shape, as the client
/// sent it. The service validates the object and ignores the keys it does not know.
pub fn request_argument(arguments: &Arguments) -> Value {
    arguments.get("request").cloned().unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: &Value) -> &Map<String, Value> {
        match value.as_object() {
            Some(map) => map,
            None => panic!("test object"),
        }
    }

    #[test]
    fn strict_int_accepts_integers_only() {
        let mut issues = Vec::new();
        assert_eq!(strict_int("x", &json!(5), &mut issues), Some(5));
        assert_eq!(
            strict_int("x", &json!(-3_000_000_000_i64), &mut issues),
            Some(-3_000_000_000)
        );
        assert_eq!(
            strict_int("x", &json!(3_000_000_000_u64), &mut issues),
            Some(3_000_000_000)
        );
        assert!(issues.is_empty());
        for rejected in [json!("5"), json!(true), json!(1.0), json!(null)] {
            assert_eq!(strict_int("x", &rejected, &mut issues), None);
        }
        assert_eq!(issues.len(), 4);
        assert!(
            issues
                .iter()
                .all(|issue| issue.kind() == IssueKind::IntType)
        );
    }

    #[test]
    fn strict_scalars_are_not_coerced() {
        let mut issues = Vec::new();
        assert_eq!(
            strict_string("s", &json!("a"), &mut issues).as_deref(),
            Some("a")
        );
        assert_eq!(strict_string("s", &json!(1), &mut issues), None);
        assert_eq!(strict_bool("b", &json!(false), &mut issues), Some(false));
        assert_eq!(strict_bool("b", &json!(0), &mut issues), None);
        assert_eq!(issues[0].kind(), IssueKind::StringType);
        assert_eq!(issues[1].kind(), IssueKind::BoolType);
    }

    #[test]
    fn required_key_must_be_present_even_when_nullable() {
        let value = json!({"a": null});
        let mut decoder = ArgDecoder::new("t");
        let a = decoder.field(object(&value), "", "a", REQUIRED_NULLABLE, strict_int);
        let b = decoder.field(object(&value), "", "b", REQUIRED_NULLABLE, strict_int);
        assert_eq!(a, Slot::Null);
        assert_eq!(b, Slot::Absent);
        let errors = decoder.check().err();
        let issues = errors.as_ref().map(ArgErrors::issues).unwrap_or_default();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].location(), "b");
        assert_eq!(issues[0].kind(), IssueKind::Missing);
    }

    #[test]
    fn null_is_rejected_where_the_type_does_not_allow_it() {
        let value = json!({"a": null});
        let mut decoder = ArgDecoder::new("t");
        let a = decoder.field(object(&value), "", "a", OPTIONAL, strict_int);
        assert_eq!(a, Slot::Absent);
        let errors = decoder.check().err();
        let issues = errors.as_ref().map(ArgErrors::issues).unwrap_or_default();
        assert_eq!(issues[0].kind(), IssueKind::IntType);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let value = json!({"a": 1, "extra": "ignored"});
        let mut decoder = ArgDecoder::new("t");
        assert_eq!(
            decoder.field(object(&value), "", "a", REQUIRED, strict_int),
            Slot::Value(1)
        );
        assert!(decoder.check().is_ok());
    }

    #[test]
    fn render_matches_the_sdk_text_for_one_and_several_issues() {
        let one = ArgErrors {
            tool: "list_profiles".to_owned(),
            issues: vec![ArgIssue {
                location: "request.limit".to_owned(),
                kind: IssueKind::IntType,
            }],
        };
        assert_eq!(
            one.render(),
            "Error executing tool list_profiles: 1 validation error for list_profilesArguments\n\
             request.limit\n  Input should be a valid integer [type=int_type]\n    \
             For further information visit https://errors.pydantic.dev/2.13/v/int_type"
        );
        let two = ArgErrors {
            tool: "t".to_owned(),
            issues: vec![
                ArgIssue {
                    location: "a".to_owned(),
                    kind: IssueKind::Missing,
                },
                ArgIssue {
                    location: "b.c".to_owned(),
                    kind: IssueKind::DictType,
                },
            ],
        };
        assert!(two.render().starts_with("Error executing tool t: 2 validation errors for tArguments\na\n  Field required [type=missing]"));
        assert!(
            two.render()
                .contains("\nb.c\n  Input should be a valid dictionary [type=dict_type]\n")
        );
    }

    #[test]
    fn literal_errors_list_the_admitted_values_as_the_sdk_does() {
        const OUTCOMES: &[&str] = &["STRONG_MATCH", "PARTIAL_MATCH"];
        const ONE: &[&str] = &["MATCH"];
        const THREE: &[&str] = &["A", "B", "C"];
        let mut issues = Vec::new();
        assert_eq!(
            literal_str("outcome", &json!("STRONG_MATCH"), &mut issues, OUTCOMES),
            Some("STRONG_MATCH")
        );
        assert_eq!(
            literal_str("outcome", &json!("WEAK"), &mut issues, OUTCOMES),
            None
        );
        assert_eq!(
            literal_str("outcome", &json!(3), &mut issues, OUTCOMES),
            None
        );
        assert_eq!(
            IssueKind::LiteralError(OUTCOMES).message(),
            "Input should be 'STRONG_MATCH' or 'PARTIAL_MATCH'"
        );
        assert_eq!(
            IssueKind::LiteralError(ONE).message(),
            "Input should be 'MATCH'"
        );
        assert_eq!(
            IssueKind::LiteralError(THREE).message(),
            "Input should be 'A', 'B' or 'C'"
        );
        assert_eq!(IssueKind::LiteralError(THREE).type_id(), "literal_error");
        assert_eq!(issues.len(), 2);
        assert!(
            issues
                .iter()
                .all(|issue| issue.kind() == IssueKind::LiteralError(OUTCOMES))
        );
    }

    #[test]
    fn uuid_arg_refuses_non_strings_and_malformed_text() {
        let mut issues = Vec::new();
        assert!(
            uuid_arg(
                "id",
                &json!("00000000-0000-4000-8000-0000000000AA"),
                &mut issues
            )
            .is_some()
        );
        assert!(uuid_arg("id", &json!(12345), &mut issues).is_none());
        assert!(
            uuid_arg(
                "id",
                &json!("{00000000-0000-4000-8000-0000000000aa}"),
                &mut issues
            )
            .is_none()
        );
        assert_eq!(issues[0].kind(), IssueKind::UuidType);
        assert_eq!(issues[1].kind(), IssueKind::UuidParsing);
    }
}
