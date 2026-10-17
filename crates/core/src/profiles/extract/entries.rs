//! Links, phrase matches and the entry builders of the deterministic extractor.

use std::collections::HashSet;
use std::sync::LazyLock;

use jobniac_text::search::lower_text;
use jobniac_text::str_rules::{is_text_whitespace, strip_text};
use regex::Regex;
use serde_json::{Value, json};

use super::pattern::{
    DATE_RANGE, DEGREE, GITHUB, LINKEDIN, PART_SEPARATOR, URL, phrase_pattern, phrase_present,
};
use super::sections::description;
use super::text::{normalize_url, parse_date, title_case, url_matches};

/// The known skills, in tuple order. Output keeps this order, not the text order.
pub(super) const KNOWN_SKILLS: [&str; 22] = [
    "kotlin multiplatform",
    "spring boot",
    "spring cloud",
    "spring ai",
    "testcontainers",
    "typescript",
    "javascript",
    "postgresql",
    "next.js",
    "pyside6",
    "sqlite",
    "python",
    "flyway",
    "docker",
    "kotlin",
    "react",
    "java",
    "jdbc",
    "mcp",
    "qt",
    "qml",
    "aws",
];

/// The human languages, in tuple order.
pub(super) const HUMAN_LANGUAGES: [&str; 15] = [
    "albanian",
    "english",
    "french",
    "spanish",
    "german",
    "italian",
    "portuguese",
    "arabic",
    "mandarin",
    "chinese",
    "japanese",
    "korean",
    "hindi",
    "russian",
    "ukrainian",
];

/// Display names of skills. Any other phrase is shown in title case.
const DISPLAY_NAMES: [(&str, &str); 15] = [
    ("spring boot", "Spring Boot"),
    ("spring cloud", "Spring Cloud"),
    ("spring ai", "Spring AI"),
    ("kotlin multiplatform", "Kotlin Multiplatform"),
    ("postgresql", "PostgreSQL"),
    ("jdbc", "JDBC"),
    ("mcp", "MCP"),
    ("next.js", "Next.js"),
    ("typescript", "TypeScript"),
    ("javascript", "JavaScript"),
    ("qt", "Qt"),
    ("qml", "QML"),
    ("pyside6", "PySide6"),
    ("sqlite", "SQLite"),
    ("aws", "AWS"),
];

/// The compiled boundary-checked patterns of the known skills.
static SKILL_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    KNOWN_SKILLS
        .iter()
        .map(|phrase| (*phrase, phrase_pattern(phrase)))
        .collect()
});

/// The compiled boundary-checked patterns of the human languages.
static LANGUAGE_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    HUMAN_LANGUAGES
        .iter()
        .map(|phrase| (*phrase, phrase_pattern(phrase)))
        .collect()
});

/// The display form of a normalized phrase.
pub(super) fn display(value: &str) -> String {
    DISPLAY_NAMES
        .iter()
        .find(|(key, _)| *key == value)
        .map_or_else(|| title_case(value), |(_, label)| (*label).to_owned())
}

/// The known skills found in `text`, in tuple order.
pub(super) fn skills_in(text: &str) -> Vec<&'static str> {
    SKILL_PATTERNS
        .iter()
        .filter(|(_, pattern)| phrase_present(text, pattern))
        .map(|(phrase, _)| *phrase)
        .collect()
}

/// The human languages found in `text`, in tuple order.
pub(super) fn languages_in(text: &str) -> Vec<&'static str> {
    LANGUAGE_PATTERNS
        .iter()
        .filter(|(_, pattern)| phrase_present(text, pattern))
        .map(|(phrase, _)| *phrase)
        .collect()
}

/// LinkedIn, then GitHub, then every other `http(s)` URL as a website. Each
/// (type, URL) pair is kept once, in first-seen order.
pub(super) fn links(text: &str) -> Vec<Value> {
    let mut links = Vec::new();
    let mut seen: HashSet<(&'static str, String)> = HashSet::new();
    add_link(
        &mut links,
        &mut seen,
        "linkedin",
        LINKEDIN.find(text).map(|found| found.as_str()),
        "LinkedIn",
    );
    add_link(
        &mut links,
        &mut seen,
        "github",
        GITHUB.find(text).map(|found| found.as_str()),
        "GitHub",
    );
    for raw in url_matches(text) {
        let url = normalize_url(raw);
        let lower = lower_text(&url);
        if !lower.contains("linkedin.com/in/") && !lower.contains("github.com/") {
            add_link(&mut links, &mut seen, "website", Some(&url), "Website");
        }
    }
    links
}

fn add_link(
    links: &mut Vec<Value>,
    seen: &mut HashSet<(&'static str, String)>,
    kind: &'static str,
    raw: Option<&str>,
    label: &str,
) {
    let Some(raw) = raw else {
        return;
    };
    let normalized = normalize_url(raw);
    if seen.insert((kind, normalized.clone())) {
        links.push(json!({
            "id": null,
            "linkType": kind,
            "url": normalized,
            "label": label,
        }));
    }
}

/// Skills as write-request skills: category `extracted`, display names.
pub(super) fn skill_values(text: &str) -> Vec<Value> {
    skills_in(text)
        .into_iter()
        .enumerate()
        .map(|(index, skill)| {
            json!({
                "id": null,
                "skill": display(skill),
                "normalizedSkill": skill,
                "category": "extracted",
                "displayOrder": index,
            })
        })
        .collect()
}

/// The header split on `|`, with spaced en and em dashes as separators.
pub(super) fn parts(header: &str) -> Vec<String> {
    let normalized = PART_SEPARATOR.replace_all(header, " | ");
    normalized
        .split('|')
        .map(strip_text)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The start and end of the first date range (`None` for `present` or
/// `current`), and the header parts left after the range and the trailing separators are removed.
pub(super) fn dated_header(line: &str) -> (Vec<String>, Option<String>, Option<String>) {
    let (start, end, header) = match DATE_RANGE.captures(line) {
        Some(captures) => {
            let whole = captures.get(0);
            let start = captures.get(1).and_then(|found| parse_date(found.as_str()));
            let end = captures.get(2).and_then(|found| parse_date(found.as_str()));
            let remaining = whole.map_or_else(
                || line.to_owned(),
                |found| format!("{}{}", &line[..found.start()], &line[found.end()..]),
            );
            (start, end, remaining)
        }
        None => (None, None, line.to_owned()),
    };
    (parts(strip_trailing_separators(&header)), start, end)
}

/// Removes trailing separators (`\s*[|,;-]+\s*$`) from the header and strips it.
fn strip_trailing_separators(header: &str) -> &str {
    let without_space = header.trim_end_matches(is_text_whitespace);
    let without_marks = without_space.trim_end_matches(['|', ',', ';', '-']);
    if without_marks.len() < without_space.len() {
        strip_text(without_marks)
    } else {
        strip_text(header)
    }
}

/// The part at `index`, or `None`.
fn part(parts: &[String], index: usize) -> Option<&str> {
    parts.get(index).map(String::as_str)
}

/// The education entry of a section.
pub(super) fn education(entry: &[String]) -> Value {
    let (parts, start, end) = dated_header(&entry[0]);
    let degree = part(&parts, 1);
    let matched = degree.and_then(|value| DEGREE.captures(value));
    let (degree_value, field) = match matched {
        Some(captures) => (
            captures.get(1).map(|found| found.as_str().to_owned()),
            captures.get(2).map(|found| found.as_str().to_owned()),
        ),
        None => (degree.map(str::to_owned), None),
    };
    json!({
        "id": null,
        "institution": part(&parts, 0),
        "degree": degree_value,
        "field": field,
        "location": part(&parts, 2),
        "startDate": start,
        "endDate": end,
        "relevantFocus": description(&entry[1..]),
    })
}

/// The experience entry of a section.
pub(super) fn experience(entry: &[String], index: usize) -> Value {
    let (parts, start, end) = dated_header(&entry[0]);
    json!({
        "id": null,
        "company": part(&parts, 1),
        "title": part(&parts, 0),
        "location": part(&parts, 2),
        "startDate": start,
        "endDate": end,
        "description": description(&entry[1..]),
        "displayOrder": index,
    })
}

/// The header has no dates. The URL is the first part that contains an
/// `http(s)` URL, and the technologies are the known skills of the whole entry.
pub(super) fn project(entry: &[String], index: usize) -> Value {
    let parts = parts(&entry[0]);
    let raw_url = parts.iter().find(|part| URL.is_match(part));
    let url = raw_url.map(|raw| normalize_url(raw));
    let technologies: Vec<Value> = skills_in(&entry.join("\n"))
        .into_iter()
        .enumerate()
        .map(|(order, skill)| {
            json!({
                "id": null,
                "technology": display(skill),
                "normalizedTechnology": skill,
                "displayOrder": order,
            })
        })
        .collect();
    json!({
        "id": null,
        "name": part(&parts, 0),
        "url": url,
        "description": description(&entry[1..]),
        "displayOrder": index,
        "technologies": technologies,
    })
}

/// The languages of a section as write-request languages.
pub(super) fn language_values(text: &str) -> Vec<Value> {
    languages_in(text)
        .into_iter()
        .enumerate()
        .map(|(index, language)| {
            json!({
                "id": null,
                "language": display(language),
                "normalizedLanguage": language,
                "proficiency": null,
                "displayOrder": index,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_follow_the_table() {
        assert_eq!(display("kotlin"), "Kotlin");
        assert_eq!(display("next.js"), "Next.js");
        assert_eq!(display("jdbc"), "JDBC");
        assert_eq!(display("testcontainers"), "Testcontainers");
    }

    #[test]
    fn skills_keep_the_tuple_order() {
        assert_eq!(
            skills_in("python and kotlin, Kotlin Multiplatform"),
            vec!["kotlin multiplatform", "python", "kotlin"]
        );
    }

    #[test]
    fn link_pairs_are_deduplicated() {
        let found = links("https://github.com/a and https://github.com/a/ https://example.com");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0]["linkType"], "github");
        assert_eq!(found[1]["linkType"], "website");
    }
}
