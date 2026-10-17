//! Canonical profile link URLs.
//!
//! The canonicalizer splits the URL into netloc and path and falls back to a string cut when the
//! split is invalid. The steps that decide the result are: the `\t`, `\r` and `\n` removal, the
//! bracketed-host and NFKC netloc checks, `hostname` and `port` from the last `@`, and the
//! fallback regular expression `[?#].*$`. The golden vectors in `tests/golden/profile-links.json`
//! pin the behaviour on synthetic URLs.

use std::net::Ipv6Addr;

use jobniac_text::str_rules::strip_text;
use unicode_normalization::UnicodeNormalization;

/// Canonicalizes a profile link URL.
///
/// The result is always a string: a URL that cannot be split falls back to a string cut.
/// Scheme is always `https`; a default port (80 or 443) is dropped; the host is lower-cased;
/// the query and fragment are dropped; the path loses trailing `/`; and the result is lower-cased.
pub fn canonical_profile_link_url(raw_url: &str) -> String {
    let prepared = prepare(raw_url);
    match canonical_from_parts(&prepared) {
        Some(url) => url,
        None => fallback(&prepared),
    }
}

/// Strips the input, drops trailing `.`, `,` and `;`, and adds `https://` unless the text already
/// starts with `http://` or `https://` (case-insensitively). An `http://` prefix becomes `https://`.
fn prepare(raw_url: &str) -> String {
    let stripped = strip_text(raw_url).trim_end_matches(['.', ',', ';']);
    let mut prepared = stripped.to_owned();
    let lowered = prepared.to_lowercase();
    if !lowered.starts_with("http://") && !lowered.starts_with("https://") {
        return format!("https://{prepared}");
    }
    if first_chars(&prepared, 7).to_lowercase() == "http://" {
        prepared = format!("https://{}", skip_chars(&prepared, 7));
    }
    prepared
}

/// The first `count` code points of `text`.
fn first_chars(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

/// `text` without its first `count` code points.
fn skip_chars(text: &str, count: usize) -> &str {
    match text.char_indices().nth(count) {
        Some((index, _)) => &text[index..],
        None => "",
    }
}

/// The canonical URL when the split and the attribute checks succeed; `None` when the URL is
/// invalid.
fn canonical_from_parts(prepared: &str) -> Option<String> {
    let (netloc, path) = split_netloc_and_path(prepared)?;
    let (hostname, port) = hostinfo(&netloc);
    if hostname.is_empty() {
        return None;
    }
    let host = lower_host(&hostname);
    let port = port_number(port)?;
    let port = match port {
        Some(80 | 443) | None => None,
        Some(number) => Some(number),
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let netloc = match port {
        Some(number) => format!("{host}:{number}"),
        None => host,
    };
    let path = path.trim_end_matches('/');
    Some(format!("https://{netloc}{path}").to_lowercase())
}

/// Lower-cases the host, keeping an IPv6 zone identifier (after `%`) as written.
fn lower_host(hostname: &str) -> String {
    match hostname.split_once('%') {
        Some((host, zone)) => format!("{}%{zone}", host.to_lowercase()),
        None => hostname.to_lowercase(),
    }
}

/// Splits the URLs the canonicalizer produces. Returns the netloc and the path
/// (text before the first `?` or `#`), or `None` when the URL is invalid.
fn split_netloc_and_path(prepared: &str) -> Option<(String, String)> {
    let mut url: String = prepared
        .trim_start_matches(|character: char| character <= ' ')
        .to_owned();
    url.retain(|character| !matches!(character, '\t' | '\r' | '\n'));
    if let Some(index) = url.find(':') {
        let scheme = &url[..index];
        let valid_scheme = index > 0
            && scheme.starts_with(|character: char| character.is_ascii_alphabetic())
            && scheme.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
            });
        if valid_scheme {
            url = url[index + 1..].to_owned();
        }
    }
    if !url.starts_with("//") {
        return None;
    }
    let rest = &url[2..];
    let delimiter = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let netloc = &rest[..delimiter];
    let after_netloc = &rest[delimiter..];
    let has_open = netloc.contains('[');
    let has_close = netloc.contains(']');
    if has_open != has_close {
        return None;
    }
    if has_open && check_bracketed_netloc(netloc).is_none() {
        return None;
    }
    check_netloc(netloc)?;
    let path_end = after_netloc.find(['?', '#']).unwrap_or(after_netloc.len());
    Some((netloc.to_owned(), after_netloc[..path_end].to_owned()))
}

/// A non-ASCII netloc must not change under NFKC into a separator.
fn check_netloc(netloc: &str) -> Option<()> {
    if netloc.is_ascii() {
        return Some(());
    }
    let stripped: String = netloc
        .chars()
        .filter(|character| !matches!(character, '@' | ':' | '#' | '?'))
        .collect();
    let normalized: String = stripped.nfkc().collect();
    if stripped == normalized {
        return Some(());
    }
    if normalized.contains(['/', '?', '#', '@', ':']) {
        return None;
    }
    Some(())
}

/// The bracketed host must be a valid IPv6 address or IPvFuture.
fn check_bracketed_netloc(netloc: &str) -> Option<()> {
    let hostname_and_port = netloc.rsplit('@').next().unwrap_or("");
    let hostname = match hostname_and_port.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return None;
            }
            let (hostname, after) = match bracketed.split_once(']') {
                Some((hostname, after)) => (hostname, after),
                None => (bracketed, ""),
            };
            if !after.is_empty() && !after.starts_with(':') {
                return None;
            }
            hostname
        }
        None => hostname_and_port
            .split_once(':')
            .map_or(hostname_and_port, |(hostname, _)| hostname),
    };
    check_bracketed_host(hostname)
}

/// Checks a bracketed host: `v<hex>.<text>` or an IPv6 address (with optional zone).
fn check_bracketed_host(hostname: &str) -> Option<()> {
    if let Some(rest) = hostname.strip_prefix('v') {
        let (hex, tail) = rest.split_once('.')?;
        let valid = !hex.is_empty()
            && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
            && !tail.is_empty()
            && !tail.contains('\n');
        return valid.then_some(());
    }
    let (address, zone) = match hostname.split_once('%') {
        Some((address, zone)) => (address, Some(zone)),
        None => (hostname, None),
    };
    if zone.is_some_and(|zone| zone.is_empty() || zone.contains('%')) {
        return None;
    }
    address.parse::<Ipv6Addr>().ok().map(|_| ())
}

/// The host text (before any port) and the port text (`None` when empty).
fn hostinfo(netloc: &str) -> (String, Option<&str>) {
    let hostinfo = netloc.rsplit('@').next().unwrap_or("");
    let (hostname, port) = match hostinfo.split_once('[') {
        Some((_, bracketed)) => {
            let (hostname, after) = match bracketed.split_once(']') {
                Some((hostname, after)) => (hostname, after),
                None => (bracketed, ""),
            };
            let port = after.split_once(':').map_or("", |(_, port)| port);
            (hostname, port)
        }
        None => hostinfo.split_once(':').unwrap_or((hostinfo, "")),
    };
    let port = (!port.is_empty()).then_some(port);
    (hostname.to_owned(), port)
}

/// The port of a netloc: `Ok(None)` when absent, `Some(number)` for digits in 0 to 65535.
/// Returns `None` (the outer option) when the port is invalid.
fn port_number(port: Option<&str>) -> Option<Option<u32>> {
    let Some(text) = port else {
        return Some(None);
    };
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number: u32 = text.parse().ok()?;
    (number <= 65_535).then_some(Some(number))
}

/// The fallback: cut at the first `?` or `#` whose remainder is one line, then remove a
/// trailing `/`, lower-case, and turn an `http://` prefix into `https://`.
fn fallback(prepared: &str) -> String {
    let cut = cut_query_and_fragment(prepared);
    let trimmed = cut.trim_end_matches('/').to_lowercase();
    match trimmed.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => trimmed,
    }
}

/// `re.sub(r'[?#].*$', '', text)` for one match: the first `?` or `#` whose remainder has no line
/// break, except a single final one (`.` does not match a newline, and `$` matches before a final one).
fn cut_query_and_fragment(text: &str) -> String {
    for (index, character) in text.char_indices() {
        if character != '?' && character != '#' {
            continue;
        }
        let tail = &text[index + character.len_utf8()..];
        match tail.find('\n') {
            None => return text[..index].to_owned(),
            Some(position) if position + 1 == tail.len() => {
                return format!("{}{}", &text[..index], &tail[position..]);
            }
            Some(_) => {}
        }
    }
    text.to_owned()
}
