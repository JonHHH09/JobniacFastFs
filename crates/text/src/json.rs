//! Strict JSON admission for upstream bodies (the Arbeitnow board and token payloads).
//!
//! It does byte-order-mark and zero-pattern detection of UTF-8, UTF-16 and UTF-32 bodies, then
//! parses with a parser that refuses what lenient JSON parsers admit and the board must not:
//! duplicate keys, `NaN` and `Infinity`, deep nesting, long keys and long numbers.
//! The parser is recursive with a hard depth bound (500 by default), so it cannot overflow the
//! stack on a nesting bomb. Errors never echo input.
//!
//! Refused inputs that a lenient decoder would pass on:
//! - malformed UTF-8 (overlong forms, lead bytes `F5` to `F7`, code points above U+10FFFF);
//! - an unpaired surrogate escape (`\ud800`), because Rust strings cannot hold a lone surrogate;
//! - a UTF-32 body that holds a surrogate code point.

use std::collections::HashSet;
use std::fmt;

/// Why a body was refused. Every message is fixed and never contains input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonAdmissionError {
    /// The bytes are not admissible UTF-8, UTF-16 or UTF-32.
    InvalidEncoding,
    /// The body is exactly the UTF-8 byte-order mark.
    BomOnly,
    /// Arrays and objects nest deeper than the limit.
    TooDeep,
    /// An object key is longer than the limit.
    KeyTooLong,
    /// A number literal has more digits than the limit.
    NumberTooLong,
    /// An object repeats a key.
    DuplicateKey,
    /// `NaN`, `Infinity` or `-Infinity`.
    NonFinite,
    /// An unpaired surrogate escape or code point.
    LoneSurrogate,
    /// Not valid JSON.
    Syntax,
    /// Data follows the first JSON value.
    TrailingData,
}

impl JsonAdmissionError {
    /// Stable camelCase code of the category (used by the golden vectors).
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidEncoding => "invalidEncoding",
            Self::BomOnly => "bomOnly",
            Self::TooDeep => "tooDeep",
            Self::KeyTooLong => "keyTooLong",
            Self::NumberTooLong => "numberTooLong",
            Self::DuplicateKey => "duplicateKey",
            Self::NonFinite => "nonFinite",
            Self::LoneSurrogate => "loneSurrogate",
            Self::Syntax => "syntax",
            Self::TrailingData => "trailingData",
        }
    }
}

impl fmt::Display for JsonAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidEncoding => "JSON body is not admissible text",
            Self::BomOnly => "JSON body holds only a byte-order mark",
            Self::TooDeep => "JSON nests too deeply",
            Self::KeyTooLong => "JSON object key is too long",
            Self::NumberTooLong => "JSON number has too many digits",
            Self::DuplicateKey => "JSON object repeats a key",
            Self::NonFinite => "JSON holds a non-finite number",
            Self::LoneSurrogate => "JSON holds an unpaired surrogate",
            Self::Syntax => "JSON is malformed",
            Self::TrailingData => "JSON has trailing data",
        })
    }
}

impl std::error::Error for JsonAdmissionError {}

/// How the key-length limit counts: UTF-8 bytes for bodies read by the byte parser (UTF-8) and
/// UTF-16 units for UTF-16 and UTF-32 bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMeasure {
    /// UTF-8 bytes of the decoded key.
    Utf8Bytes,
    /// UTF-16 code units of the decoded key.
    Utf16Units,
}

/// Bounds of the strict parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrictLimits {
    /// Deepest allowed nesting of arrays and objects.
    pub max_depth: usize,
    /// Longest allowed object key, in `key_measure` units.
    pub max_key: usize,
    /// Most digits a number literal may have.
    pub max_number_digits: usize,
    /// Unit of `max_key`.
    pub key_measure: KeyMeasure,
}

impl Default for StrictLimits {
    fn default() -> Self {
        Self {
            max_depth: 500,
            max_key: 50_000,
            max_number_digits: 1000,
            key_measure: KeyMeasure::Utf8Bytes,
        }
    }
}

/// A decoded body and how it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBody {
    /// The text.
    pub text: String,
    /// True when the body was UTF-8 (byte parser), false for UTF-16 and UTF-32.
    pub utf8: bool,
}

impl DecodedBody {
    /// Limits for this body: default bounds and the key unit of the
    /// path that decoded it.
    pub fn limits(&self) -> StrictLimits {
        StrictLimits {
            key_measure: if self.utf8 {
                KeyMeasure::Utf8Bytes
            } else {
                KeyMeasure::Utf16Units
            },
            ..StrictLimits::default()
        }
    }
}

/// A parsed JSON value. Numbers keep their literal so no precision or range is lost.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer literal (`-?digits`, no fraction or exponent).
    Integer(String),
    /// A literal with a fraction or exponent.
    Float(String),
    /// A string (never holds a lone surrogate).
    String(String),
    /// An array.
    Array(Vec<JsonValue>),
    /// An object in source order; keys are unique.
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    /// The string, if this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// The boolean, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The members, if this is an object.
    pub fn as_object(&self) -> Option<&[(String, JsonValue)]> {
        match self {
            Self::Object(members) => Some(members),
            _ => None,
        }
    }

    /// The value of a key, if this is an object holding it.
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object()?
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// True for `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// The integer, if this is an integer literal that fits `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(literal) => literal.parse().ok(),
            _ => None,
        }
    }

    /// The number as `f64` (out-of-range literals become infinity).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(literal) | Self::Float(literal) => literal.parse().ok(),
            _ => None,
        }
    }
}

/// Decodes a body: UTF-8 byte-order mark, UTF-32 and UTF-16 marks,
/// then zero-pattern detection on the first four bytes, else UTF-8.
pub fn decode_body(bytes: &[u8]) -> Result<DecodedBody, JsonAdmissionError> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return decode_utf8(rest);
    }
    let marks: [(&[u8], Wide); 4] = [
        (&[0x00, 0x00, 0xFE, 0xFF], Wide::Utf32 { big: true }),
        (&[0xFF, 0xFE, 0x00, 0x00], Wide::Utf32 { big: false }),
        (&[0xFE, 0xFF], Wide::Utf16 { big: true }),
        (&[0xFF, 0xFE], Wide::Utf16 { big: false }),
    ];
    let mut found = None;
    for (prefix, kind) in marks {
        if let Some(rest) = bytes.strip_prefix(prefix) {
            found = Some((kind, rest));
            break;
        }
    }
    if found.is_none()
        && let [a, b, c, d, ..] = *bytes
    {
        let kind = if a == 0 && b == 0 && c == 0 {
            Some(Wide::Utf32 { big: true })
        } else if b == 0 && c == 0 && d == 0 {
            Some(Wide::Utf32 { big: false })
        } else if a == 0 && c == 0 {
            Some(Wide::Utf16 { big: true })
        } else if b == 0 && d == 0 {
            Some(Wide::Utf16 { big: false })
        } else {
            None
        };
        found = kind.map(|kind| (kind, bytes));
    }
    match found {
        None => decode_utf8(bytes),
        Some((kind, raw)) => decode_wide(kind, raw),
    }
}

#[derive(Clone, Copy)]
enum Wide {
    Utf16 { big: bool },
    Utf32 { big: bool },
}

fn decode_utf8(bytes: &[u8]) -> Result<DecodedBody, JsonAdmissionError> {
    let text = std::str::from_utf8(bytes).map_err(|_| JsonAdmissionError::InvalidEncoding)?;
    // Non-ASCII is admitted only inside a string and never directly after a backslash.
    let (mut quoted, mut escaped) = (false, false);
    for ch in text.chars() {
        if ch.is_ascii() {
            if escaped {
                escaped = false;
            } else if quoted && ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = !quoted;
            }
        } else if escaped || !quoted {
            return Err(JsonAdmissionError::InvalidEncoding);
        }
    }
    Ok(DecodedBody {
        text: text.to_owned(),
        utf8: true,
    })
}

fn decode_wide(kind: Wide, raw: &[u8]) -> Result<DecodedBody, JsonAdmissionError> {
    let (width, big) = match kind {
        Wide::Utf16 { big } => (2, big),
        Wide::Utf32 { big } => (4, big),
    };
    if !raw.len().is_multiple_of(width) {
        return Err(JsonAdmissionError::InvalidEncoding);
    }
    let units = raw.chunks_exact(width).map(|chunk| {
        let mut value = 0u32;
        if big {
            for byte in chunk {
                value = (value << 8) | u32::from(*byte);
            }
        } else {
            for byte in chunk.iter().rev() {
                value = (value << 8) | u32::from(*byte);
            }
        }
        value
    });
    let mut text = String::new();
    if width == 4 {
        for value in units {
            if value > 0x10_FFFF {
                return Err(JsonAdmissionError::InvalidEncoding);
            }
            text.push(char::from_u32(value).ok_or(JsonAdmissionError::LoneSurrogate)?);
        }
    } else {
        let units: Vec<u32> = units.collect();
        let mut index = 0;
        while index < units.len() {
            let value = units[index];
            index += 1;
            let ch = if (0xD800..=0xDBFF).contains(&value) {
                match units.get(index) {
                    Some(&low) => {
                        index += 1;
                        if (0xDC00..=0xDFFF).contains(&low) {
                            char::from_u32(0x1_0000 + ((value - 0xD800) << 10) + (low - 0xDC00))
                        } else {
                            None
                        }
                    }
                    None => None,
                }
            } else if (0xDC00..=0xDFFF).contains(&value) {
                None
            } else {
                char::from_u32(value)
            };
            text.push(ch.unwrap_or('\u{FFFD}'));
        }
    }
    Ok(DecodedBody { text, utf8: false })
}

/// Decodes and parses a body. `Ok(None)` means the body holds only
/// whitespace (the caller maps it to a schema failure).
pub fn parse_body(bytes: &[u8]) -> Result<Option<JsonValue>, JsonAdmissionError> {
    if bytes == [0xEF, 0xBB, 0xBF] {
        return Err(JsonAdmissionError::BomOnly);
    }
    let decoded = decode_body(bytes)?;
    parse_strict(&decoded.text, decoded.limits())
}

/// Parses text under the strict rules. `Ok(None)` means only JSON whitespace.
pub fn parse_strict(
    text: &str,
    limits: StrictLimits,
) -> Result<Option<JsonValue>, JsonAdmissionError> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        limits,
    };
    parser.skip_whitespace();
    if parser.pos == parser.bytes.len() {
        return Ok(None);
    }
    let value = parser.value(0)?;
    parser.skip_whitespace();
    if parser.pos != parser.bytes.len() {
        return Err(JsonAdmissionError::TrailingData);
    }
    Ok(Some(value))
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    limits: StrictLimits,
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<JsonValue, JsonAdmissionError> {
        match self.peek().ok_or(JsonAdmissionError::Syntax)? {
            b'"' => self.string().map(JsonValue::String),
            b'{' => self.object(depth + 1),
            b'[' => self.array(depth + 1),
            b'-' | b'0'..=b'9' => self.number(),
            b'N' | b'I' => Err(if self.eat_word("NaN") || self.eat_word("Infinity") {
                JsonAdmissionError::NonFinite
            } else {
                JsonAdmissionError::Syntax
            }),
            _ if self.eat_word("null") => Ok(JsonValue::Null),
            _ if self.eat_word("true") => Ok(JsonValue::Bool(true)),
            _ if self.eat_word("false") => Ok(JsonValue::Bool(false)),
            _ => Err(JsonAdmissionError::Syntax),
        }
    }

    fn array(&mut self, depth: usize) -> Result<JsonValue, JsonAdmissionError> {
        if depth > self.limits.max_depth {
            return Err(JsonAdmissionError::TooDeep);
        }
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value(depth)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                _ => return Err(JsonAdmissionError::Syntax),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonValue, JsonAdmissionError> {
        if depth > self.limits.max_depth {
            return Err(JsonAdmissionError::TooDeep);
        }
        self.pos += 1;
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(members));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(JsonAdmissionError::Syntax);
            }
            let key = self.string()?;
            let size = match self.limits.key_measure {
                KeyMeasure::Utf8Bytes => key.len(),
                KeyMeasure::Utf16Units => key.encode_utf16().count(),
            };
            if size > self.limits.max_key {
                return Err(JsonAdmissionError::KeyTooLong);
            }
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(JsonAdmissionError::Syntax);
            }
            self.pos += 1;
            self.skip_whitespace();
            let value = self.value(depth)?;
            if !seen.insert(key.clone()) {
                return Err(JsonAdmissionError::DuplicateKey);
            }
            members.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(members));
                }
                _ => return Err(JsonAdmissionError::Syntax),
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Result<JsonValue, JsonAdmissionError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
            if self.peek() == Some(b'I') {
                return Err(if self.eat_word("Infinity") {
                    JsonAdmissionError::NonFinite
                } else {
                    JsonAdmissionError::Syntax
                });
            }
        }
        let mut count = match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                1
            }
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(JsonAdmissionError::Syntax),
        };
        let mut float = false;
        if self.peek() == Some(b'.') && matches!(self.bytes.get(self.pos + 1), Some(b'0'..=b'9')) {
            self.pos += 1;
            count += self.digits();
            float = true;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mut ahead = self.pos + 1;
            if matches!(self.bytes.get(ahead), Some(b'+' | b'-')) {
                ahead += 1;
            }
            if matches!(self.bytes.get(ahead), Some(b'0'..=b'9')) {
                self.pos = ahead;
                count += self.digits();
                float = true;
            }
        }
        if count > self.limits.max_number_digits {
            return Err(JsonAdmissionError::NumberTooLong);
        }
        let literal = self.text[start..self.pos].to_owned();
        Ok(if float {
            JsonValue::Float(literal)
        } else {
            JsonValue::Integer(literal)
        })
    }

    fn hex4(&mut self) -> Result<u32, JsonAdmissionError> {
        let digits = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or(JsonAdmissionError::Syntax)?;
        let mut value = 0u32;
        for byte in digits {
            let digit = char::from(*byte)
                .to_digit(16)
                .ok_or(JsonAdmissionError::Syntax)?;
            value = (value << 4) | digit;
        }
        self.pos += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, JsonAdmissionError> {
        self.pos += 1;
        let mut out = String::new();
        let mut run = self.pos;
        loop {
            let byte = self.peek().ok_or(JsonAdmissionError::Syntax)?;
            match byte {
                b'"' => {
                    out.push_str(&self.text[run..self.pos]);
                    self.pos += 1;
                    return Ok(out);
                }
                0..=0x1F => return Err(JsonAdmissionError::Syntax),
                b'\\' => {
                    out.push_str(&self.text[run..self.pos]);
                    self.pos += 1;
                    let escape = self.peek().ok_or(JsonAdmissionError::Syntax)?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.hex4()?;
                            out.push(self.scalar(unit)?);
                        }
                        _ => return Err(JsonAdmissionError::Syntax),
                    }
                    run = self.pos;
                }
                _ => self.pos += 1,
            }
        }
    }

    /// Resolves a `\u` unit, pairing a high surrogate with the escape that must follow it.
    fn scalar(&mut self, unit: u32) -> Result<char, JsonAdmissionError> {
        let value = match unit {
            0xD800..=0xDBFF => {
                if !self.eat_word("\\u") {
                    return Err(JsonAdmissionError::LoneSurrogate);
                }
                let low = self.hex4()?;
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return Err(JsonAdmissionError::LoneSurrogate);
                }
                0x1_0000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
            }
            0xDC00..=0xDFFF => return Err(JsonAdmissionError::LoneSurrogate),
            other => other,
        };
        char::from_u32(value).ok_or(JsonAdmissionError::LoneSurrogate)
    }
}
