//! Job URL identity and the URL check before any fetch. The URL parser is hand-written: the `url` crate is not used.
//!
//! Identity: `safe_display_url` keeps scheme, host, port and path only; `canonical_source_url` adds
//! the source-qualified identity parameters; `validate_persisted_job_urls` checks stored provenance.
//! None of these authorizes a network destination. Errors are the fixed validation envelope and
//! never echo the rejected URL, which may carry userinfo.

use jobniac_text::java::java_strip;

use crate::errors::{ApplicationError, validation_error};
use crate::job_links::JobLinkFetchFailure;
use crate::outbound::address::{compressed_ipv6, parse_ip, parse_ipv6};
use crate::outbound::{is_public_address, normalize_hostname};

const MUST_BE_ABSOLUTE: &str = "must be an absolute http(s) URL";
const MUST_BE_VALID: &str = "must be a valid absolute http(s) URL";
const NO_USERINFO: &str = "must not include userinfo";
/// Retrieval URLs longer than this are refused before any copy.
pub const MAX_URL_BYTES: usize = 8192;
const MAX_PORT: u64 = 2_147_483_647;
/// The identity parameters admitted for a host (source-specific keys before generic ones).
fn allowed_keys(host: &str) -> &'static [&'static str] {
    if host == "greenhouse.io" || host.ends_with(".greenhouse.io") {
        &["gh_jid"]
    } else if host == "indeed.com" || host.ends_with(".indeed.com") {
        &["jk"]
    } else {
        &["id", "job_id", "jobid"]
    }
}

fn invalid(reason: &str) -> ApplicationError {
    validation_error("Invalid job request", Some("url"), Some(reason))
}

/// The safe display form: scheme, host, port and path without query, fragment or userinfo.
pub fn safe_display_url(raw_url: &str) -> Result<String, ApplicationError> {
    parse(raw_url, true)
        .map(|parsed| parsed.base)
        .map_err(invalid)
}

/// The canonical source identity: the display form plus the allowed identity parameters.
pub fn canonical_source_url(raw_url: &str) -> Result<String, ApplicationError> {
    let parsed = parse(raw_url, true).map_err(invalid)?;
    Ok(canonical_query(&parsed.base, &parsed.host, &parsed.query))
}

/// Validates already normalized provenance (display and canonical forms) without trimming a
/// trailing slash a second time.
pub fn validate_persisted_job_urls(
    display_url: &str,
    normalized_url: &str,
) -> Result<(), ApplicationError> {
    let valid = match (parse(display_url, false), parse(normalized_url, false)) {
        (Ok(display), Ok(canonical)) => {
            display_url == display.base
                && normalized_url
                    == canonical_query(&canonical.base, &canonical.host, &canonical.query)
                && display.base == canonical.base
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(validation_error(
            "Invalid job link provenance",
            Some("url"),
            Some("must contain matching safe job URLs"),
        ))
    }
}

/// True when the text has a `%` not followed by two hexadecimal digits.
fn has_invalid_escape(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(index, &byte)| {
        byte == b'%'
            && !(bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit))
    })
}

/// The Unicode categories Cc, Cs, Zs, Zl and Zp, written out (their members are stable).
fn is_refused_control_or_separator(character: char) -> bool {
    let code = u32::from(character);
    code <= 0x1F
        || (0x7F..=0x9F).contains(&code)
        || matches!(
            character,
            ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{2028}' | '\u{2029}'
        )
}

struct Parsed {
    base: String,
    host: String,
    query: String,
}

fn parse(raw_url: &str, trim_trailing_slash: bool) -> Result<Parsed, &'static str> {
    let value = java_strip(raw_url);
    if value.chars().any(|character| {
        "<>\"{}|\\^`".contains(character) || is_refused_control_or_separator(character)
    }) {
        return Err(MUST_BE_VALID);
    }
    let (scheme, remainder) = split_scheme(value).ok_or(MUST_BE_ABSOLUTE)?;
    let after_slashes = remainder.strip_prefix("//").ok_or(MUST_BE_ABSOLUTE)?;
    let (authority_and_path, fragment) =
        after_slashes.split_once('#').unwrap_or((after_slashes, ""));
    let (authority_and_path, query) = authority_and_path
        .split_once('?')
        .unwrap_or((authority_and_path, ""));
    let (authority, path) = match authority_and_path.find('/') {
        Some(index) => (&authority_and_path[..index], &authority_and_path[index..]),
        None => (authority_and_path, ""),
    };
    if has_invalid_escape(&format!("{path}?{query}#{fragment}"))
        || path.contains('[')
        || path.contains(']')
        || fragment.contains('#')
    {
        return Err(MUST_BE_VALID);
    }
    if authority.contains('@') {
        return Err(NO_USERINFO);
    }
    let (host, port) = authority_host_and_port(authority)?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(MUST_BE_ABSOLUTE);
    }
    let mut path = if path.is_empty() { "/" } else { path };
    if trim_trailing_slash && path.len() > 1 && path.ends_with('/') {
        path = &path[..path.len() - 1];
    }
    let host = host.to_ascii_lowercase();
    let base = format!("{}://{}{}{}", scheme.to_ascii_lowercase(), host, port, path);
    Ok(Parsed {
        base,
        host,
        query: query.to_owned(),
    })
}

/// Splits `scheme:rest` where the scheme matches `[A-Za-z][A-Za-z0-9+.-]*`.
fn split_scheme(value: &str) -> Option<(&str, &str)> {
    let (scheme, remainder) = value.split_once(':')?;
    if is_scheme(scheme) {
        Some((scheme, remainder))
    } else {
        None
    }
}

/// True when the whole text matches `[A-Za-z][A-Za-z0-9+.-]*` (the scheme group of a wire URL).
fn is_scheme(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '.' | '-')
        })
}

/// True when the label matches `[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?`.
fn is_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let is_alnum = |byte: &u8| byte.is_ascii_alphanumeric();
    match (bytes.first(), bytes.last()) {
        (Some(first), Some(last)) => {
            is_alnum(first)
                && is_alnum(last)
                && bytes.iter().all(|byte| is_alnum(byte) || *byte == b'-')
        }
        _ => false,
    }
}

/// An IPv4 octet of the dotted form that identity admission accepts: decimal digits, at most
/// three significant digits, value up to 255. Leading zeros are allowed here.
fn is_padded_octet(octet: &str) -> bool {
    if octet.is_empty() || !octet.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let significant = octet.trim_start_matches('0');
    if significant.len() > 3 {
        return false;
    }
    significant.parse::<u32>().unwrap_or(0) <= 255
}

/// `_valid_ipv6_address`: the padded embedded IPv4 octets are normalized in a validation copy only.
fn valid_ipv6_identity(address: &str) -> bool {
    let normalized: String;
    let address = if address.contains('.') {
        let Some((prefix, suffix)) = address.rsplit_once(':') else {
            return false;
        };
        let octets: Vec<&str> = suffix.split('.').collect();
        if octets.len() != 4 || !octets.iter().all(|octet| is_padded_octet(octet)) {
            return false;
        }
        let decimal: Vec<String> = octets
            .iter()
            .map(|octet| {
                octet
                    .trim_start_matches('0')
                    .parse::<u32>()
                    .unwrap_or(0)
                    .to_string()
            })
            .collect();
        normalized = format!("{prefix}:{}", decimal.join("."));
        normalized.as_str()
    } else {
        address
    };
    parse_ipv6(address).is_some()
}

/// Returns the host text (brackets and zone kept, as the caller spelled them)
/// and the normalized port suffix (`:80`, or empty when no port was written).
fn authority_host_and_port(authority: &str) -> Result<(String, String), &'static str> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (literal, tail) = rest.split_once(']').ok_or(MUST_BE_VALID)?;
        let (address, zone) = match literal.split_once('%') {
            Some((address, zone)) => (address, Some(zone)),
            None => (literal, None),
        };
        let zone_ok = zone.is_none_or(|zone| {
            !zone.is_empty()
                && zone
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
        });
        if !valid_ipv6_identity(address) || !zone_ok {
            return Err(MUST_BE_VALID);
        }
        if !tail.is_empty() && !tail.starts_with(':') {
            return Err(MUST_BE_VALID);
        }
        let port = if tail.is_empty() { "" } else { &tail[1..] };
        (format!("[{literal}]"), port)
    } else {
        let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
        let labels: Vec<&str> = host.strip_suffix('.').unwrap_or(host).split('.').collect();
        if host.is_empty() || !labels.iter().all(|label| is_dns_label(label)) {
            return Err(MUST_BE_ABSOLUTE);
        }
        let last = labels[labels.len() - 1];
        let last_starts_with_letter = last
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic());
        if labels.len() > 1 && !last_starts_with_letter {
            let dotted_quad =
                labels.len() == 4 && labels.iter().all(|label| is_padded_octet(label));
            if host.ends_with('.') || !dotted_quad {
                return Err(MUST_BE_ABSOLUTE);
            }
        }
        (host.to_owned(), port)
    };
    if port.is_empty() {
        return Ok((host, String::new()));
    }
    if !port.bytes().all(|byte| byte.is_ascii_digit()) || port.trim_start_matches('0').len() > 10 {
        return Err(MUST_BE_ABSOLUTE);
    }
    let value = port.trim_start_matches('0').parse::<u64>().unwrap_or(0);
    if value > MAX_PORT {
        return Err(MUST_BE_ABSOLUTE);
    }
    Ok((host, format!(":{value}")))
}

fn canonical_query(base: &str, host: &str, query: &str) -> String {
    let allowed = allowed_keys(host);
    let mut parameters: Vec<(String, String)> = Vec::new();
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = java_strip(&unquote_plus(name)).to_lowercase();
        if !allowed.contains(&name.as_str()) {
            continue;
        }
        let value = java_strip(&unquote_plus(value)).to_owned();
        if is_identity_value(&value) {
            parameters.push((name, value));
        }
    }
    if parameters.is_empty() {
        return base.to_owned();
    }
    parameters.sort();
    let encoded: Vec<String> = parameters
        .iter()
        .map(|(name, value)| format!("{name}={}", value.replace('~', "%7E")))
        .collect();
    format!("{base}?{}", encoded.join("&"))
}

/// True when the value matches `[A-Za-z0-9][A-Za-z0-9._~-]{0,127}`.
fn is_identity_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    match bytes.first() {
        Some(first) if first.is_ascii_alphanumeric() => {
            bytes.len() <= 128
                && bytes[1..].iter().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-')
                })
        }
        _ => false,
    }
}

/// `unquote_plus` with replacement: `+` becomes a space, and each run of
/// percent-encoded bytes is decoded as UTF-8 with replacement.
pub fn unquote_plus(text: &str) -> String {
    let characters: Vec<char> = text
        .chars()
        .map(|c| if c == '+' { ' ' } else { c })
        .collect();
    let mut output = String::new();
    let mut run: Vec<u8> = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '%'
            && index + 2 < characters.len()
            && characters[index + 1].is_ascii_hexdigit()
            && characters[index + 2].is_ascii_hexdigit()
        {
            let high = characters[index + 1].to_digit(16).unwrap_or(0);
            let low = characters[index + 2].to_digit(16).unwrap_or(0);
            run.push(u8::try_from(high * 16 + low).unwrap_or(0));
            index += 3;
            continue;
        }
        flush_run(&mut run, &mut output);
        output.push(character);
        index += 1;
    }
    flush_run(&mut run, &mut output);
    output
}

fn flush_run(run: &mut Vec<u8>, output: &mut String) {
    if !run.is_empty() {
        output.push_str(&String::from_utf8_lossy(run));
        run.clear();
    }
}

/// The retrieval URL in wire form, after the pre-fetch checks.
///
/// Hosts must be IP literals, except that an `https` host name is admitted in its normalized form
/// (the resolved addresses are checked at connect time). The failure names the first check that
/// failed; no part of the URL is kept in it.
pub fn wire_url(retrieval_url: &str) -> Result<String, JobLinkFetchFailure> {
    let mut failure = JobLinkFetchFailure::InvalidUrl;
    match wire_candidate(retrieval_url, &mut failure) {
        Some(wire) => Ok(wire),
        None => Err(failure),
    }
}

fn wire_candidate(url: &str, failure: &mut JobLinkFetchFailure) -> Option<String> {
    if url.chars().count() > MAX_URL_BYTES || url.len() > MAX_URL_BYTES {
        return None;
    }
    if url
        .chars()
        .any(|character| u32::from(character) <= 32 || u32::from(character) == 127)
    {
        return None;
    }
    if url.contains('\\') || has_invalid_escape(url) {
        return None;
    }
    let (scheme_text, after) = url.split_once("://")?;
    if !is_scheme(scheme_text) {
        return None;
    }
    let scheme = scheme_text.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..authority_end];
    if authority.is_empty() || authority.contains(['@', '%', '\\']) {
        return None;
    }
    let (host_text, port_text): (&str, Option<&str>) =
        if let Some(rest) = authority.strip_prefix('[') {
            let end = rest.find(']')?;
            let host = &rest[..end];
            let suffix = &rest[end + 1..];
            if suffix.is_empty() {
                (host, None)
            } else {
                (host, Some(suffix.strip_prefix(':')?))
            }
        } else {
            if authority.matches(':').count() > 1 || authority.contains(['[', ']']) {
                return None;
            }
            match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
    let port_value = match port_text {
        Some(port) => Some(checked_port(port)?),
        None => None,
    };
    *failure = JobLinkFetchFailure::IpLiteralRequired;
    let mut host = host_text.to_owned();
    let (prefix, dotted, has_colon) = match host.rfind(':') {
        Some(index) => (host[..index].to_owned(), host[index + 1..].to_owned(), true),
        None => (String::new(), host.clone(), false),
    };
    if is_padded_dotted_quad(&dotted) {
        *failure = JobLinkFetchFailure::InvalidUrl;
        let decimal: Vec<String> = dotted
            .split('.')
            .map(|octet| octet.parse::<u32>().unwrap_or(0).to_string())
            .collect();
        host = if has_colon {
            format!("{prefix}:{}", decimal.join("."))
        } else {
            decimal.join(".")
        };
    } else if host.contains(':') {
        *failure = JobLinkFetchFailure::InvalidUrl;
    }
    let mut numeric = if !host.contains(':') && parse_ip(&host).is_none() {
        // A bracketed host must be an IPv6 literal, never a DNS name.
        if scheme != "https" || authority.starts_with('[') {
            return None;
        }
        normalize_hostname(&host).ok()?
    } else {
        let address = parse_ip(&host)?;
        if authority.starts_with('[') && !matches!(address, std::net::IpAddr::V6(_)) {
            return None;
        }
        *failure = JobLinkFetchFailure::UnsafeAddress;
        if !is_public_address(address) {
            return None;
        }
        match address {
            std::net::IpAddr::V6(address) => format!("[{}]", compressed_ipv6(u128::from(address))),
            std::net::IpAddr::V4(address) => address.to_string(),
        }
    };
    if let Some(port) = port_value {
        numeric.push_str(&format!(":{port}"));
    }
    let after_host = &after[authority.len()..];
    let tail = after_host.split('#').next().unwrap_or("");
    Some(format!("{scheme}://{numeric}{tail}"))
}

/// True when the text fully matches `[0-9]{1,3}(?:\.[0-9]{1,3}){3}`.
fn is_padded_dotted_quad(text: &str) -> bool {
    let octets: Vec<&str> = text.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|octet| {
            (1..=3).contains(&octet.len()) && octet.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// A port of 1 to 65535 with its leading zeros removed; anything else refuses the URL.
fn checked_port(port: &str) -> Option<u64> {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let significant = port.trim_start_matches('0');
    if significant.len() > 5 {
        return None;
    }
    let value = significant.parse::<u64>().unwrap_or(0);
    if (1..=65535).contains(&value) {
        Some(value)
    } else {
        None
    }
}
