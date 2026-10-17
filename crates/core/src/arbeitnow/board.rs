//! Pure response admission and page parsing of the fixed Arbeitnow board.
//!
//! Covers the request check, status, header and `Content-Type`/`Content-Length` admission with the
//! `Retry-After` parser (decimal delta and HTTP-date forms), and the mapping of a response body to a
//! [`Page`] (body decoding and strict JSON through [`jobniac_text::json`], schema, job mapping and
//! next-link validation). There is no IO and no dependency on the transport: the worker hands over
//! the status, the raw header pairs and the body.
//!
//! Limits: 4 MiB body, 500 jobs per page and
//! 64 list elements.
//!
//! Notes: a text truncated to a UTF-16 unit cap drops a dangling high surrogate, since a Rust
//! string cannot hold the lone unit; the body decoders refuse what
//! [`jobniac_text::json`] refuses (see its documentation). Header names and values are bytes, as the
//! real transport delivers them.

use jobniac_text::json::{self, JsonValue};

use super::failure::{BoardFailure, RetryAfterSeconds};
use super::instant::ArbeitnowInstant;
use crate::outbound::USER_AGENT;

/// Largest accepted response body, in bytes (four mebibytes).
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Most jobs one page may carry.
pub const MAX_JOBS_PER_PAGE: usize = 500;

/// Most elements a `tags` or `job_types` list may carry.
pub const MAX_LIST_ELEMENTS: usize = 64;

/// Path of the board endpoint.
pub const ENDPOINT_PATH: &str = "/api/job-board-api";

/// Host of the board endpoint.
pub const HOST: &str = "www.arbeitnow.com";

/// Timeout of one board request, in seconds.
pub const ARBEITNOW_TIMEOUT_SECONDS: u64 = 10;

/// The fixed request headers.
pub const REQUEST_HEADERS: [(&str, &str); 2] =
    [("Accept", "application/json"), ("User-Agent", USER_AGENT)];

/// Largest page number (`2^31 - 1`).
const MAX_PAGE: i64 = 2_147_483_647;

/// Longest accepted next link, in UTF-16 units.
const MAX_NEXT_LINK_UNITS: usize = 2048;

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Weekdays in the order of `(epoch_day + 3) % 7` (epoch day 0 is a Thursday).
const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

const SECONDS_PER_DAY: i64 = 86_400;

/// One job of an upstream page, bounded and validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamJob {
    /// The job slug (at most 256 code points).
    pub slug: String,
    /// The company name (at most 256 UTF-16 units).
    pub company: String,
    /// The title (at most 256 UTF-16 units).
    pub title: String,
    /// The HTML description (at most 4096 UTF-16 units).
    pub html_description: String,
    /// Whether the job is remote, when stated.
    pub remote: Option<bool>,
    /// The upstream URL (at most 4096 UTF-16 units; validated, not used downstream).
    pub url: String,
    /// Tags (at most 64, each at most 128 UTF-16 units).
    pub tags: Vec<String>,
    /// Job types (at most 64, each at most 128 UTF-16 units).
    pub job_types: Vec<String>,
    /// The location (at most 256 UTF-16 units), when stated.
    pub location: Option<String>,
    /// Creation time as epoch seconds in `0..=2^63-1`, when stated.
    pub created_at: Option<i64>,
}

/// One parsed board page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The jobs in source order.
    pub jobs: Vec<UpstreamJob>,
    /// Whether the scan may request the next page: a next link is present and either `last_page`
    /// is stated or the page holds jobs.
    pub another_page_may_remain: bool,
}

const fn http_status(retryable: bool) -> BoardFailure {
    BoardFailure::HttpStatus { retryable }
}

/// Checks the request and returns the page number.
///
/// # Errors
///
/// Returns a non-retryable [`BoardFailure::HttpStatus`] for a page outside `1..=2147483647`.
/// The visa filter is `Option<bool>`, so no other value is representable.
pub fn validate_request(page: i64, visa: Option<bool>) -> Result<u32, BoardFailure> {
    let _ = visa;
    if (1..=MAX_PAGE).contains(&page) {
        u32::try_from(page).map_err(|_| http_status(false))
    } else {
        Err(http_status(false))
    }
}

/// The first header with `name` (ASCII case-insensitive); `name` is lowercase.
fn first_header<'a>(headers: &'a [(Vec<u8>, Vec<u8>)], name: &[u8]) -> Option<&'a [u8]> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_slice())
}

/// An optionally signed decimal that fits a signed 64-bit integer. Header bytes are
/// Latin-1, where only `0`-`9` are decimal digits.
fn parse_long(value: &[u8]) -> Option<i64> {
    let negative = value.first() == Some(&b'-');
    let digits = if matches!(value.first(), Some(b'+' | b'-')) {
        &value[1..]
    } else {
        value
    };
    if digits.is_empty() {
        return None;
    }
    let mut result: i128 = 0;
    for byte in digits {
        if !byte.is_ascii_digit() {
            return None;
        }
        result = result * 10 + i128::from(byte - b'0');
        let limit = if negative {
            1i128 << 63
        } else {
            (1i128 << 63) - 1
        };
        if result > limit {
            return None;
        }
    }
    i64::try_from(if negative { -result } else { result }).ok()
}

fn ascii_number(value: &[u8]) -> Option<i64> {
    if value.is_empty() || value.len() > 9 || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    value
        .iter()
        .try_fold(0i64, |acc, byte| Some(acc * 10 + i64::from(byte - b'0')))
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Epoch seconds of an HTTP-date (the `_DATE` form), or `None`.
///
/// The day is clamped to the month length, a weekday when present must
/// match, hour `24:00:00` is the next midnight and the zone is `GMT` or `+/-HH[MM]`.
fn date_second(value: &[u8]) -> Option<i64> {
    let mut parts = value;
    let mut weekday = None;
    if parts.first().is_some_and(u8::is_ascii_alphabetic) {
        if parts.len() < 5 || &parts[3..5] != b", " {
            return None;
        }
        let name = parts[..3].to_ascii_lowercase();
        weekday = Some(WEEKDAYS.iter().position(|day| day.as_bytes() == name)?);
        parts = &parts[5..];
    }
    let mut fields = parts.split(|byte| *byte == b' ');
    let day = ascii_number(fields.next()?)?;
    let month_name = fields.next()?.to_ascii_lowercase();
    let month = i64::try_from(
        MONTHS
            .iter()
            .position(|name| name.as_bytes() == month_name)?
            + 1,
    )
    .ok()?;
    let year = ascii_number(fields.next()?)?;
    let clock = fields.next()?;
    let zone = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let mut clock_fields = clock.split(|byte| *byte == b':');
    let mut hour = ascii_number(clock_fields.next()?)?;
    let minute = ascii_number(clock_fields.next()?)?;
    let second = match clock_fields.next() {
        Some(text) => ascii_number(text)?,
        None => 0,
    };
    if clock_fields.next().is_some()
        || !(0..=999_999_999).contains(&year)
        || !(1..=31).contains(&day)
    {
        return None;
    }
    let month_length = match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let day = day.min(month_length);
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let mut epoch_day = era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100
        + day_of_year
        - 719_468;
    if let Some(expected) = weekday
        && usize::try_from((epoch_day + 3).rem_euclid(7)).ok() != Some(expected)
    {
        return None;
    }
    if hour == 24 && minute == 0 && second == 0 {
        hour = 0;
        epoch_day += 1;
    }
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return None;
    }
    let offset = if zone.eq_ignore_ascii_case(b"GMT") {
        0
    } else {
        let sign = match zone.first()? {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let digits = &zone[1..];
        if digits.len() != 2 && digits.len() != 4 {
            return None;
        }
        let hours = ascii_number(&digits[..2])?;
        let minutes = if digits.len() == 4 {
            ascii_number(&digits[2..])?
        } else {
            0
        };
        if hours > 18 || minutes > 59 || (hours == 18 && minutes != 0) {
            return None;
        }
        sign * (hours * 3600 + minutes * 60)
    };
    Some(epoch_day * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second - offset)
}

/// The bounded `Retry-After` seconds, or `None` when absent or invalid.
fn retry_after(value: Option<&[u8]>, now: ArbeitnowInstant) -> Option<RetryAfterSeconds> {
    let value = value.filter(|text| !text.is_empty())?;
    let seconds = if value.iter().all(u8::is_ascii_digit) {
        parse_long(value)?
    } else {
        date_second(value)?
            .saturating_sub(now.epoch_second())
            .saturating_sub(i64::from(now.nanosecond() > 0))
    };
    RetryAfterSeconds::new(u32::try_from(seconds).ok()?)
}

/// Whether the `Content-Type` value names JSON: media type before `;`, trimmed of bytes `0x00` to
/// `0x20`, lowercased, equal to `application/json` or `application/*+json`.
fn is_json_media(content_type: Option<&[u8]>) -> bool {
    let Some(value) = content_type else {
        return false;
    };
    let media = value.split(|byte| *byte == b';').next().unwrap_or_default();
    let start = media.iter().position(|byte| *byte > 0x20);
    let media = match start {
        Some(start) => {
            let end = media
                .iter()
                .rposition(|byte| *byte > 0x20)
                .map_or(start, |i| i + 1);
            media[start..end].to_ascii_lowercase()
        }
        None => Vec::new(),
    };
    media == b"application/json"
        || (media.starts_with(b"application/") && media.ends_with(b"+json"))
}

/// Admits status, media type and declared length before any body is read. Returns the
/// declared `Content-Length`, when one was sent.
///
/// A 429 is a rate-limited failure carrying the bounded `Retry-After`; another non-200 status is an
/// `http_status` failure, retryable for `500..=599`. For a 200 the first `Content-Type` must name
/// JSON and the first `Content-Length`, when present, must be a decimal in `0..=4 MiB`.
///
/// # Errors
///
/// Returns the classified [`BoardFailure`].
pub fn admit_response(
    status: u16,
    headers: &[(Vec<u8>, Vec<u8>)],
    now: ArbeitnowInstant,
) -> Result<Option<u64>, BoardFailure> {
    match status {
        429 => Err(BoardFailure::RateLimited {
            retry_after: retry_after(first_header(headers, b"retry-after"), now),
        }),
        200 => {
            if !is_json_media(first_header(headers, b"content-type")) {
                return Err(BoardFailure::ContentType);
            }
            let Some(value) = first_header(headers, b"content-length") else {
                return Ok(None);
            };
            let declared = parse_long(value)
                .and_then(|length| u64::try_from(length).ok())
                .filter(|length| *length <= MAX_BODY_BYTES as u64)
                .ok_or(BoardFailure::BodySize)?;
            Ok(Some(declared))
        }
        other => Err(http_status((500..=599).contains(&other))),
    }
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// Truncates to at most `maximum` UTF-16 units. A cut inside a surrogate pair drops the dangling
/// high surrogate (a Rust string cannot hold the lone unit).
fn bound(value: &str, maximum: usize) -> String {
    let mut units = 0;
    let mut result = String::new();
    for ch in value.chars() {
        units += ch.len_utf16();
        if units > maximum {
            break;
        }
        result.push(ch);
    }
    result
}

fn required<'a>(parent: &'a JsonValue, name: &str) -> Result<&'a JsonValue, BoardFailure> {
    parent.get(name).ok_or(BoardFailure::Schema)
}

fn text(parent: &JsonValue, name: &str, maximum: usize) -> Result<String, BoardFailure> {
    match required(parent, name)? {
        JsonValue::String(value) => Ok(bound(value, maximum)),
        _ => Err(BoardFailure::Schema),
    }
}

fn optional_text(
    parent: &JsonValue,
    name: &str,
    maximum: usize,
) -> Result<Option<String>, BoardFailure> {
    match parent.get(name) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => Ok(Some(bound(value, maximum))),
        Some(_) => Err(BoardFailure::Schema),
    }
}

fn list(parent: &JsonValue, name: &str) -> Result<Vec<String>, BoardFailure> {
    match parent.get(name) {
        None | Some(JsonValue::Null) => Ok(Vec::new()),
        Some(JsonValue::Array(items)) if items.len() <= MAX_LIST_ELEMENTS => items
            .iter()
            .map(|item| match item {
                JsonValue::String(value) => Ok(bound(value, 128)),
                _ => Err(BoardFailure::Schema),
            })
            .collect(),
        Some(_) => Err(BoardFailure::Schema),
    }
}

/// A page counter: an integer in `1..=2^31-1`.
fn positive(value: &JsonValue) -> Result<u32, BoardFailure> {
    value
        .as_i64()
        .filter(|number| (1..=MAX_PAGE).contains(number))
        .and_then(|number| u32::try_from(number).ok())
        .ok_or(BoardFailure::Schema)
}

fn job(value: &JsonValue) -> Result<UpstreamJob, BoardFailure> {
    if value.as_object().is_none() {
        return Err(BoardFailure::Schema);
    }
    let slug = match required(value, "slug")? {
        JsonValue::String(slug) if slug.chars().count() <= 256 => slug.clone(),
        _ => return Err(BoardFailure::Schema),
    };
    let remote = match value.get("remote") {
        None | Some(JsonValue::Null) => None,
        Some(JsonValue::Bool(flag)) => Some(*flag),
        Some(_) => return Err(BoardFailure::Schema),
    };
    let created_at = match value.get("created_at") {
        None | Some(JsonValue::Null) => None,
        Some(epoch) => Some(
            epoch
                .as_i64()
                .filter(|seconds| *seconds >= 0)
                .ok_or(BoardFailure::Schema)?,
        ),
    };
    Ok(UpstreamJob {
        slug,
        company: text(value, "company_name", 256)?,
        title: text(value, "title", 256)?,
        html_description: text(value, "description", 4096)?,
        remote,
        url: text(value, "url", 4096)?,
        tags: list(value, "tags")?,
        job_types: list(value, "job_types")?,
        location: optional_text(value, "location", 256)?,
        created_at,
    })
}

/// Whether the next link is the same-host board URL for the expected page.
fn next_link_is_valid(raw: &str, expected: i64, last: Option<u32>, visa: Option<bool>) -> bool {
    if last.is_some_and(|last| expected > i64::from(last)) || utf16_len(raw) > MAX_NEXT_LINK_UNITS {
        return false;
    }
    // Java URI rejects raw ASCII illegal characters and malformed percent escapes.
    if raw
        .chars()
        .any(|ch| ch <= ' ' || ch == '\u{7f}' || "<>\"{}|\\^`".contains(ch))
    {
        return false;
    }
    let bytes = raw.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !(bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit))
        {
            return false;
        }
    }
    if raw.contains('#') {
        return false;
    }
    // urlsplit: the scheme is the text before the first colon.
    let Some((scheme, rest)) = raw.split_once(':') else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("//") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let netloc_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (netloc, rest) = rest.split_at(netloc_end);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    if path != ENDPOINT_PATH || netloc.contains(['@', '[', ']']) || !host_and_port_are_valid(netloc)
    {
        return false;
    }
    let mut page = None;
    let mut visa_value = None;
    for pair in query.split('&') {
        let mut halves = pair.split('=');
        let (Some(key), Some(value), None) = (halves.next(), halves.next(), halves.next()) else {
            return false;
        };
        let slot = match key {
            "page" => &mut page,
            "visa_sponsorship" => &mut visa_value,
            _ => return false,
        };
        if value.is_empty() || slot.is_some() {
            return false;
        }
        *slot = Some(value);
    }
    let expected_visa = match visa {
        None => None,
        Some(true) => Some("true"),
        Some(false) => Some("false"),
    };
    page == Some(expected.to_string().as_str()) && visa_value == expected_visa
}

/// The host is the board host (or its parent) and the port is absent or 443 (an empty port
/// counts as absent, a non-numeric one is an error).
fn host_and_port_are_valid(netloc: &str) -> bool {
    let (host, port) = match netloc.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (netloc, None),
    };
    if !host.eq_ignore_ascii_case(HOST) && !host.eq_ignore_ascii_case("arbeitnow.com") {
        return false;
    }
    match port {
        None => true,
        Some(port) => {
            let trimmed = port.trim_start_matches('0');
            port.is_empty() || (port.bytes().all(|byte| byte.is_ascii_digit()) && trimmed == "443")
        }
    }
}

/// Maps a response body to a [`Page`].
///
/// The order is: request check, body bound, decoding and strict JSON, root object, `data`, `links`
/// and `meta`, page counters, all jobs, then the pagination rules. `meta.last_page` is optional
/// (the live API omits it): then `links.next` and an empty page drive pagination.
///
/// # Errors
///
/// Returns the classified [`BoardFailure`]: `body_size`, `json`, `schema` or `pagination`.
pub fn parse_page(
    body: &[u8],
    requested_page: u32,
    visa: Option<bool>,
) -> Result<Page, BoardFailure> {
    validate_request(i64::from(requested_page), visa)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(BoardFailure::BodySize);
    }
    let root = json::parse_body(body)
        .map_err(|_| BoardFailure::Json)?
        .ok_or(BoardFailure::Schema)?;
    if root.as_object().is_none() {
        return Err(BoardFailure::Schema);
    }
    let data = required(&root, "data")?;
    let links = required(&root, "links")?;
    let meta = required(&root, "meta")?;
    let (Some(data), true, true) = (
        data.as_array()
            .filter(|rows| rows.len() <= MAX_JOBS_PER_PAGE),
        links.as_object().is_some(),
        meta.as_object().is_some(),
    ) else {
        return Err(BoardFailure::Schema);
    };
    let current = positive(required(meta, "current_page")?)?;
    let last = meta.get("last_page").map(positive).transpose()?;
    if current != requested_page || last.is_some_and(|last| last < current) {
        return Err(BoardFailure::Schema);
    }
    let jobs = data.iter().map(job).collect::<Result<Vec<_>, _>>()?;
    let next = required(links, "next")?;
    let pagination_ok = match next {
        JsonValue::Null => last.is_none_or(|last| last == current),
        JsonValue::String(link) => {
            // The overflow of the last page wraps into `-2^31`.
            let expected = if i64::from(requested_page) < MAX_PAGE {
                i64::from(requested_page) + 1
            } else {
                -2_147_483_648
            };
            next_link_is_valid(link, expected, last, visa)
        }
        _ => false,
    };
    if !pagination_ok {
        return Err(BoardFailure::Pagination);
    }
    // Without last_page an empty page ends the scan even if a next link is advertised.
    let another_page_may_remain = !next.is_null() && (last.is_some() || !jobs.is_empty());
    Ok(Page {
        jobs,
        another_page_may_remain,
    })
}
