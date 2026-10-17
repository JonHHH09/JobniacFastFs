//! HTML title and description extraction and content admission for fetched job pages.
//!
//! Extraction follows the pinned Java selection rules: the first non-empty `og:title`, `<title>`
//! or `<h1>`; the description meta tag, else the visible text of the whole page, truncated to 20000
//! UTF-16 units without splitting a character. Patterns run over bytes with ASCII-only case
//! folding (`re.ASCII | re.IGNORECASE` semantics). Admission refuses before the body is
//! copied: duplicate or conflicting framing headers, header blocks over 16 KiB, declared bodies over
//! 2 MiB and any content encoding other than identity.

use std::collections::BTreeSet;

use jobniac_text::java::java_strip;
use regex::bytes::Regex;

use crate::job_links::{JobLinkFetchFailure, JobLinkFetchResult};

/// Largest body accepted from a job page (2 MiB).
pub const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;
/// Largest raw header block, interim and final.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Largest single network read.
pub const NETWORK_READ_BYTES: usize = 64 * 1024;
/// Longest description, in UTF-16 code units.
pub const MAX_DESCRIPTION_UNITS: usize = 20_000;

/// The ASCII whitespace that `re.ASCII` `\s` matches.
fn is_regex_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C | 0x0B)
}

struct Patterns {
    og_title: Regex,
    title: Regex,
    h1: Regex,
    description: Regex,
}

fn compile() -> Option<Patterns> {
    Some(Patterns {
        og_title: Regex::new(
            r#"(?is-u)<meta[^>]+property=["']og:title["'][^>]+content=["'](.*?)["'][^>]*>"#,
        )
        .ok()?,
        title: Regex::new(r"(?is-u)<title[^>]*>(.*?)</title>").ok()?,
        h1: Regex::new(r"(?is-u)<h1[^>]*>(.*?)</h1>").ok()?,
        description: Regex::new(
            r#"(?is-u)<meta[^>]+name=["']description["'][^>]+content=["'](.*?)["'][^>]*>"#,
        )
        .ok()?,
    })
}

/// Answers "first occurrence at or after `start`" queries over a haystack, with ASCII-only case
/// folding when `ignore_case` is set. The last scan is kept: a query that starts inside it is
/// answered from it, and a failed scan answers every later query (a later start is a subset of the
/// searched range). Without this, every unmatched `<` or `<script` rescans to the end of the body,
/// which is quadratic on large bodies. The answers are exactly those of a fresh scan.
struct Finder<'a> {
    haystack: &'a [u8],
    needle: &'a [u8],
    ignore_case: bool,
    /// The start of the last scan and its result (`None` when no occurrence follows it).
    last: Option<(usize, Option<usize>)>,
}

impl<'a> Finder<'a> {
    fn new(haystack: &'a [u8], needle: &'a [u8], ignore_case: bool) -> Self {
        Self {
            haystack,
            needle,
            ignore_case,
            last: None,
        }
    }

    fn matches_at(&self, position: usize) -> bool {
        let window = &self.haystack[position..position + self.needle.len()];
        if self.ignore_case {
            window.eq_ignore_ascii_case(self.needle)
        } else {
            window == self.needle
        }
    }

    /// The first position `>= start` where the needle occurs, if any.
    fn find_from(&mut self, start: usize) -> Option<usize> {
        if let Some((scan_start, result)) = self.last
            && start >= scan_start
        {
            match result {
                None => return None,
                Some(position) if position >= start => return Some(position),
                Some(_) => {}
            }
        }
        let found = match self.haystack.len().checked_sub(self.needle.len()) {
            Some(limit) => (start..=limit).find(|&position| self.matches_at(position)),
            None => None,
        };
        self.last = Some((start, found));
        found
    }
}

/// The searches of [`strip_script_and_style`]: the end of an opening tag and the two closing tags.
struct ScriptSearch<'a> {
    open_end: Finder<'a>,
    script_close: Finder<'a>,
    style_close: Finder<'a>,
}

impl<'a> ScriptSearch<'a> {
    fn new(html: &'a [u8]) -> Self {
        Self {
            open_end: Finder::new(html, b">", false),
            script_close: Finder::new(html, b"</script>", true),
            style_close: Finder::new(html, b"</style>", true),
        }
    }
}

/// Removes `<script ...>...</script>` and `<style ...>...</style>` (the
/// closing tag must repeat the opening name). Each removed span becomes one space.
fn strip_script_and_style(html: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(html.len());
    let mut search = ScriptSearch::new(html);
    let mut index = 0;
    while index < html.len() {
        if html[index] == b'<'
            && let Some(end) = script_or_style_end(html, index, &mut search)
        {
            output.push(b' ');
            index = end;
            continue;
        }
        output.push(html[index]);
        index += 1;
    }
    output
}

fn starts_with_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

fn script_or_style_end(html: &[u8], start: usize, search: &mut ScriptSearch<'_>) -> Option<usize> {
    let rest = &html[start + 1..];
    let (name, close): (&[u8], &mut Finder<'_>) = if starts_with_ascii_ci(rest, b"script") {
        (b"script", &mut search.script_close)
    } else if starts_with_ascii_ci(rest, b"style") {
        (b"style", &mut search.style_close)
    } else {
        return None;
    };
    let after_name = start + 1 + name.len();
    let open_end = search.open_end.find_from(after_name)?;
    let body_start = open_end + 1;
    let found = close.find_from(body_start)?;
    Some(found + close.needle.len())
}

/// Removes `<[^>]+>`. Each removed tag becomes one space.
fn strip_tags(html: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(html.len());
    let mut close = Finder::new(html, b">", false);
    let mut index = 0;
    while index < html.len() {
        if html[index] == b'<'
            && let Some(found) = close.find_from(index + 1)
            && found > index + 1
        {
            // `<[^>]+>` needs at least one character between the brackets.
            output.push(b' ');
            index = found + 1;
            continue;
        }
        output.push(html[index]);
        index += 1;
    }
    output
}

/// The visible text of a page: scripts, styles and tags become spaces, six entities are replaced in
/// order, runs of ASCII whitespace collapse to one space and the Java whitespace set is trimmed.
fn visible_text(html: &[u8]) -> String {
    let without_tags = strip_tags(&strip_script_and_style(html));
    let text = String::from_utf8_lossy(&without_tags)
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let mut collapsed = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        let is_space = character.is_ascii() && is_regex_space(character as u8);
        if is_space {
            if !in_space {
                collapsed.push(' ');
            }
        } else {
            collapsed.push(character);
        }
        in_space = is_space;
    }
    java_strip(&collapsed).to_owned()
}

/// The first pattern (in order) whose first match has non-empty visible text.
fn first_match(html: &[u8], patterns: &[&Regex]) -> Option<String> {
    for pattern in patterns {
        if let Some(captures) = pattern.captures(html)
            && let Some(group) = captures.get(1)
        {
            let text = visible_text(group.as_bytes());
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Keeps characters while the UTF-16 length stays within 20000.
fn description_prefix(text: String) -> String {
    let mut units = 0usize;
    for (index, character) in text.char_indices() {
        units += if u32::from(character) > 0xFFFF { 2 } else { 1 };
        if units > MAX_DESCRIPTION_UNITS {
            return java_strip(&text[..index]).to_owned();
        }
    }
    text
}

/// Surrogate pre-pass: an encoded surrogate (one or two bytes of its prefix) becomes a
/// single replacement character, as in OpenJDK.
fn replace_encoded_surrogates(body: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(body.len());
    let mut index = 0;
    while index < body.len() {
        let surrogate_start = body[index] == 0xED
            && body
                .get(index + 1)
                .is_some_and(|byte| (0xA0..=0xBF).contains(byte));
        if surrogate_start {
            output.extend_from_slice("\u{FFFD}".as_bytes());
            let third_is_continuation = body
                .get(index + 2)
                .is_some_and(|byte| (0x80..=0xBF).contains(byte));
            index += if third_is_continuation { 3 } else { 2 };
        } else {
            output.push(body[index]);
            index += 1;
        }
    }
    output
}

/// Parses an already bounded response body and its status.
pub fn parse_job_html(body: &[u8], status: i64) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
    let unavailable = JobLinkFetchFailure::Unavailable;
    if body.len() as u64 > MAX_BODY_BYTES || !(100..=599).contains(&status) {
        return Err(unavailable);
    }
    let patterns = compile().ok_or(unavailable)?;
    let html_bytes = replace_encoded_surrogates(body);
    let html = String::from_utf8_lossy(&html_bytes).into_owned();
    let bytes = html.as_bytes();
    let title = first_match(bytes, &[&patterns.og_title, &patterns.title, &patterns.h1]);
    let description = first_match(bytes, &[&patterns.description]).or_else(|| {
        let visible = visible_text(bytes);
        if visible.is_empty() {
            None
        } else {
            Some(visible)
        }
    });
    Ok(JobLinkFetchResult {
        title,
        description: description.map(description_prefix),
        http_status: status,
    })
}

/// Header value with ASCII whitespace trimmed, lowercased.
fn trimmed_lower(value: &[u8]) -> Vec<u8> {
    let start = value
        .iter()
        .position(|byte| !is_bytes_space(*byte))
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !is_bytes_space(*byte))
        .map_or(start, |index| index + 1);
    value[start..end.max(start)].to_ascii_lowercase()
}

fn is_bytes_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

fn is_ascii_digits(value: &[u8]) -> bool {
    !value.is_empty() && value.iter().all(u8::is_ascii_digit)
}

fn parse_decimal(value: &[u8]) -> Option<u64> {
    if !is_ascii_digits(value) || value.len() > 10 {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse::<u64>().ok()
}

const CONTENT_LENGTH: &[u8] = b"content-length";
const TRANSFER_ENCODING: &[u8] = b"transfer-encoding";
const CONTENT_ENCODING: &[u8] = b"content-encoding";

/// Admits a final response's raw headers. Returns the declared body
/// length when there is a `Content-Length`. Names are matched case-insensitively. The body limit
/// is the job page limit; [`admit_response_headers_with_limit`] takes another one.
pub fn admit_response_headers(
    headers: &[(&[u8], &[u8])],
) -> Result<Option<u64>, JobLinkFetchFailure> {
    admit_response_headers_with_limit(headers, MAX_BODY_BYTES)
}

/// [`admit_response_headers`] with the largest declared `Content-Length` supplied by the caller
/// (the Arbeitnow board admits four mebibytes where a job page admits two).
pub fn admit_response_headers_with_limit(
    headers: &[(&[u8], &[u8])],
    body_limit: u64,
) -> Result<Option<u64>, JobLinkFetchFailure> {
    admit_response_headers_with_limits(headers, body_limit, JOB_PAGE_HEADER_SLACK)
}

/// Bytes that the job page fetcher adds to the header size before comparing it with
/// [`MAX_HEADER_BYTES`]. The Arbeitnow transport adds none.
pub const JOB_PAGE_HEADER_SLACK: usize = 16;

/// [`admit_response_headers_with_limit`] with the header size slack supplied as well: the job
/// page fetcher compares `size + 16` with the header limit and the Arbeitnow transport compares
/// `size` alone.
pub fn admit_response_headers_with_limits(
    headers: &[(&[u8], &[u8])],
    body_limit: u64,
    header_slack: usize,
) -> Result<Option<u64>, JobLinkFetchFailure> {
    let unavailable = JobLinkFetchFailure::Unavailable;
    let size: usize = headers
        .iter()
        .map(|(name, value)| name.len() + value.len() + 4)
        .sum();
    if size + header_slack > MAX_HEADER_BYTES {
        return Err(unavailable);
    }
    let mut framing: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if [CONTENT_LENGTH, TRANSFER_ENCODING, CONTENT_ENCODING].contains(&lower.as_slice()) {
            if framing.iter().any(|(seen, _)| *seen == lower) {
                return Err(unavailable);
            }
            framing.push((lower, trimmed_lower(value)));
        }
    }
    let find = |key: &[u8]| {
        framing
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, v)| v.as_slice())
    };
    if find(CONTENT_ENCODING).unwrap_or(b"identity") != b"identity" {
        return Err(unavailable);
    }
    let length = find(CONTENT_LENGTH);
    let transfer = find(TRANSFER_ENCODING);
    if let Some(transfer) = transfer
        && (length.is_some() || transfer != b"chunked")
    {
        return Err(unavailable);
    }
    match length {
        Some(length) => {
            let declared = parse_decimal(length).ok_or(unavailable)?;
            if declared > body_limit {
                return Err(unavailable);
            }
            Ok(Some(declared))
        }
        None => Ok(None),
    }
}

/// Bounds the raw header block before the HTTP parser sees it. Interim
/// (1xx) blocks are skipped and counted; the final block completes the guard.
#[derive(Debug)]
pub struct RawHeaderGuard {
    pending: Vec<u8>,
    total: usize,
    complete: bool,
    body_limit: u64,
}

impl Default for RawHeaderGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl RawHeaderGuard {
    /// A guard that has seen no bytes yet and admits a declared body of up to the job page limit.
    pub fn new() -> Self {
        Self::with_body_limit(MAX_BODY_BYTES)
    }

    /// A guard that admits a declared `Content-Length` of up to `body_limit` bytes.
    pub fn with_body_limit(body_limit: u64) -> Self {
        Self {
            pending: Vec::new(),
            total: 0,
            complete: false,
            body_limit,
        }
    }

    /// True once the final header block has been seen.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// The largest read the caller may request next.
    pub fn read_limit(&self, requested: usize) -> usize {
        if self.complete {
            requested.min(NETWORK_READ_BYTES)
        } else {
            let room = (MAX_HEADER_BYTES + 1).saturating_sub(self.total + self.pending.len());
            requested.min(room)
        }
    }

    /// Admits one chunk of raw bytes read from the socket.
    pub fn check(&mut self, data: &[u8]) -> Result<(), JobLinkFetchFailure> {
        let unavailable = JobLinkFetchFailure::Unavailable;
        if self.complete {
            return Ok(());
        }
        if data.len() > self.read_limit(NETWORK_READ_BYTES) {
            return Err(unavailable);
        }
        self.pending.extend_from_slice(data);
        loop {
            let Some(found) = find_bytes(&self.pending, b"\r\n\r\n") else {
                if self.total + self.pending.len() > MAX_HEADER_BYTES {
                    return Err(unavailable);
                }
                return Ok(());
            };
            let end = found + 4;
            self.total += end;
            if self.total > MAX_HEADER_BYTES {
                return Err(unavailable);
            }
            admit_header_block(&self.pending[..end], self.body_limit)?;
            let first_line_end = find_bytes(&self.pending, b"\r\n").unwrap_or(0);
            if is_interim_status_line(&self.pending[..first_line_end]) {
                self.pending.drain(..end);
                continue;
            }
            self.complete = true;
            self.pending.clear();
            return Ok(());
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Framing rules applied to one complete block (status line included).
fn admit_header_block(block: &[u8], body_limit: u64) -> Result<(), JobLinkFetchFailure> {
    let unavailable = JobLinkFetchFailure::Unavailable;
    let mut framing: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut previous_name: Vec<u8> = Vec::new();
    let mut lines: Vec<&[u8]> = Vec::new();
    let mut start = 0;
    while let Some(offset) = find_bytes(&block[start..], b"\r\n") {
        lines.push(&block[start..start + offset]);
        start += offset + 2;
    }
    lines.push(&block[start..]);
    for line in lines.iter().skip(1) {
        if matches!(line.first(), Some(b' ') | Some(b'\t')) && framing.contains(&previous_name) {
            return Err(unavailable);
        }
        let (raw_name, value) = match line.iter().position(|&byte| byte == b':') {
            Some(colon) => (&line[..colon], &line[colon + 1..]),
            None => (&line[..], &line[line.len()..]),
        };
        let name = raw_name.to_ascii_lowercase();
        previous_name = name.clone();
        if name == CONTENT_LENGTH || name == TRANSFER_ENCODING || name == CONTENT_ENCODING {
            if framing.contains(&name) {
                return Err(unavailable);
            }
            framing.insert(name.clone());
            let value = trimmed_lower(value);
            if name == CONTENT_LENGTH {
                let declared = parse_decimal(&value).ok_or(unavailable)?;
                if declared > body_limit {
                    return Err(unavailable);
                }
            } else if (name == TRANSFER_ENCODING && value != b"chunked")
                || (name == CONTENT_ENCODING && value != b"identity")
            {
                return Err(unavailable);
            }
        }
    }
    if framing.contains(CONTENT_LENGTH) && framing.contains(TRANSFER_ENCODING) {
        return Err(unavailable);
    }
    Ok(())
}

/// Full match of `HTTP/1\.[01] 1[0-9]{2}(?: .*)?` on the status line.
fn is_interim_status_line(line: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(b"HTTP/1.") else {
        return false;
    };
    let [minor, rest @ ..] = rest else {
        return false;
    };
    if !matches!(minor, b'0' | b'1') || rest.len() < 4 {
        return false;
    }
    // " 1" then two digits: the status code 100 to 199.
    let code = &rest[..4];
    if code[0] != b' ' || code[1] != b'1' || !code[2].is_ascii_digit() || !code[3].is_ascii_digit()
    {
        return false;
    }
    let tail = &rest[4..];
    tail.is_empty() || tail[0] == b' '
}

#[cfg(test)]
mod tests {
    //! The linear search of `Finder` must give the output of the straightforward scans it replaces.
    //! The reference below is that original code (quadratic on unmatched `<`), kept only here.

    use super::*;

    fn reference_find_ascii_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window.eq_ignore_ascii_case(needle))
    }

    fn reference_script_or_style_end(html: &[u8], start: usize) -> Option<usize> {
        let rest = &html[start + 1..];
        let name: &[u8] = if starts_with_ascii_ci(rest, b"script") {
            b"script"
        } else if starts_with_ascii_ci(rest, b"style") {
            b"style"
        } else {
            return None;
        };
        let after_name = start + 1 + name.len();
        let open_end = html[after_name..].iter().position(|&byte| byte == b'>')? + after_name;
        let body_start = open_end + 1;
        let mut closing = Vec::with_capacity(name.len() + 3);
        closing.extend_from_slice(b"</");
        closing.extend_from_slice(name);
        closing.push(b'>');
        let found = reference_find_ascii_ci(&html[body_start..], &closing)?;
        Some(body_start + found + closing.len())
    }

    fn reference_strip_script_and_style(html: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(html.len());
        let mut index = 0;
        while index < html.len() {
            if html[index] == b'<'
                && let Some(end) = reference_script_or_style_end(html, index)
            {
                output.push(b' ');
                index = end;
                continue;
            }
            output.push(html[index]);
            index += 1;
        }
        output
    }

    fn reference_strip_tags(html: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(html.len());
        let mut index = 0;
        while index < html.len() {
            if html[index] == b'<'
                && let Some(offset) = html[index + 1..].iter().position(|&byte| byte == b'>')
                && offset > 0
            {
                output.push(b' ');
                index = index + 1 + offset + 1;
                continue;
            }
            output.push(html[index]);
            index += 1;
        }
        output
    }

    fn reference_visible_text(html: &[u8]) -> String {
        let without_tags = reference_strip_tags(&reference_strip_script_and_style(html));
        let text = String::from_utf8_lossy(&without_tags)
            .replace("&nbsp;", " ")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'");
        let mut collapsed = String::with_capacity(text.len());
        let mut in_space = false;
        for character in text.chars() {
            let is_space = character.is_ascii() && is_regex_space(character as u8);
            if is_space {
                if !in_space {
                    collapsed.push(' ');
                }
            } else {
                collapsed.push(character);
            }
            in_space = is_space;
        }
        java_strip(&collapsed).to_owned()
    }

    fn assert_matches_reference(html: &[u8]) {
        assert_eq!(
            strip_script_and_style(html),
            reference_strip_script_and_style(html),
            "script and style removal of {:?}",
            String::from_utf8_lossy(html)
        );
        assert_eq!(
            strip_tags(html),
            reference_strip_tags(html),
            "tag removal of {:?}",
            String::from_utf8_lossy(html)
        );
        assert_eq!(
            visible_text(html),
            reference_visible_text(html),
            "visible text of {:?}",
            String::from_utf8_lossy(html)
        );
    }

    const TOKENS: &[&str] = &[
        "<",
        ">",
        "<>",
        "</",
        "<script",
        "<SCRIPT",
        "</script>",
        "</SCRIPT >",
        "<style",
        "</style>",
        "</Style>",
        "<a",
        "<a ",
        "x",
        " ",
        "\n",
        "script",
        "style",
        "&amp;",
        "&nbsp;",
        "<!--",
        "é",
        "\u{1F600}",
        "<script>",
        "<style>",
        "<h1>",
    ];

    #[test]
    fn exhaustive_small_inputs_match_the_reference() {
        let alphabet = *b"<>/sSa";
        let mut buffer = Vec::new();
        for length in 0..=7usize {
            let count = alphabet.len().pow(length as u32);
            for code in 0..count {
                buffer.clear();
                let mut rest = code;
                for _ in 0..length {
                    buffer.push(alphabet[rest % alphabet.len()]);
                    rest /= alphabet.len();
                }
                assert_matches_reference(&buffer);
            }
        }
    }

    #[test]
    fn random_token_soup_matches_the_reference() {
        // xorshift64: a fixed seed keeps the cases reproducible.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let pieces = (next() % 40) as usize;
            let mut html = Vec::new();
            for _ in 0..pieces {
                let token = TOKENS[(next() % TOKENS.len() as u64) as usize];
                html.extend_from_slice(token.as_bytes());
            }
            assert_matches_reference(&html);
        }
    }

    #[test]
    fn random_bytes_match_the_reference() {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let palette = b"<>/sScriptyle \n&";
        for _ in 0..5_000 {
            let length = (next() % 64) as usize;
            let html: Vec<u8> = (0..length)
                .map(|_| palette[(next() % palette.len() as u64) as usize])
                .collect();
            assert_matches_reference(&html);
        }
    }

    #[test]
    fn finder_answers_match_a_fresh_scan_for_any_query_order() {
        let html = b"<a>x</style>y<b>z</script>>";
        let needles: [(&[u8], bool); 3] =
            [(b">", false), (b"</script>", true), (b"</style>", true)];
        let mut state: u64 = 0xDEAD_BEEF_CAFE_F00D;
        for (needle, ignore_case) in needles {
            let mut finder = Finder::new(html, needle, ignore_case);
            for _ in 0..500 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let start = (state % (html.len() as u64 + 2)) as usize;
                let expected = (start..=html.len().saturating_sub(needle.len()))
                    .filter(|&position| position + needle.len() <= html.len())
                    .find(|&position| {
                        let window = &html[position..position + needle.len()];
                        if ignore_case {
                            window.eq_ignore_ascii_case(needle)
                        } else {
                            window == needle
                        }
                    });
                assert_eq!(finder.find_from(start), expected, "start {start}");
            }
        }
    }
}
