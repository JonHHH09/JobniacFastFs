//! Maps a parsed Europass CV to a profile write request.
//!
//! The value has the shape of [`super::DeterministicProfileTextExtractor::extract_profile`]: `id`
//! null on every child and the same key names, so `canonicalize_profile_write` accepts it
//! unchanged. The data is taken as written; only the limits of the text extractor are applied
//! (display name, summary and descriptions), by cutting, never by failing. Birth date and postal
//! addresses are personal details and are not mapped here.

use std::collections::HashSet;

use jobniac_text::java::truncate_utf16;
use jobniac_text::str_rules::strip_text;
use serde_json::{Value, json};
use time::{Date, Month, util::days_in_month};

use crate::errors::AppError;
use crate::europass::{
    Cefr, Education, Employment, EuropassCv, LanguageSkill, PartialDate, Period,
};

use super::super::draft::{education_key, experience_key};
use super::super::link_url::canonical_profile_link_url;
use super::super::values::parse_iso_date;
use super::sections::TEXT_LIMIT;
use super::text::normalize_url;
use super::{NAME_LIMIT, invalid};

/// Profile write request of a Europass CV.
///
/// # Errors
///
/// `validation_error` with message `Invalid profile extraction input` and the field `email` or
/// `fullName` when the CV has no email address or no name, as the text extractor reports them.
pub fn profile_write_from_europass_cv(cv: &EuropassCv) -> Result<Value, AppError> {
    let person = &cv.person;
    let email = person
        .emails
        .iter()
        .map(|email| strip_text(email))
        .find(|email| !email.is_empty())
        .ok_or_else(|| invalid("email", "could not be extracted from Europass CV"))?;
    let name = [&person.given_name, &person.family_name]
        .into_iter()
        .filter_map(|part| part.as_deref().map(strip_text))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        return Err(invalid(
            "fullName",
            "could not be extracted from Europass CV",
        ));
    }
    let mut contacts = Vec::new();
    let mut seen = HashSet::new();
    for address in &person.emails {
        push_contact(&mut contacts, &mut seen, "email", address, "Europass email");
    }
    for phone in &person.phones {
        push_contact(
            &mut contacts,
            &mut seen,
            "phone",
            &phone.international(),
            "Europass phone",
        );
    }
    Ok(json!({
        "fullName": truncate_utf16(&name, NAME_LIMIT),
        "email": email,
        "summary": cv.summary.as_ref().and_then(|text| bounded(&text.plain())),
        "contacts": contacts,
        "links": links(&person.websites),
        "skills": skills(cv),
        "languages": languages(cv),
        "education": education_values(cv),
        "experiences": experience_values(cv),
        "projects": [],
    }))
}

/// An optional field as the write canonicalization reads it: stripped, `None` when blank.
fn field(value: Option<&String>) -> Option<String> {
    value
        .map(|text| strip_text(text))
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The date of a mapped `YYYY-MM-DD` string.
fn date(value: Option<&String>) -> Option<Date> {
    value.and_then(|text| parse_iso_date(text))
}

/// Education entries without the ones the write would refuse as duplicates, by the key the write
/// canonicalization uses (the first is kept).
fn education_values(cv: &EuropassCv) -> Vec<Value> {
    let mut seen = HashSet::new();
    cv.education
        .iter()
        .filter(|entry| entry.organization.is_some() || entry.degree.is_some())
        .filter(|entry| {
            let (start, end) = dates(&entry.period);
            seen.insert(education_key(
                &field(entry.organization.as_ref()),
                &field(entry.degree.as_ref()),
                &None,
                date(start.as_ref()),
                date(end.as_ref()),
            ))
        })
        .map(education)
        .collect()
}

/// Experience entries without the ones the write would refuse as duplicates, by the key the write
/// canonicalization uses (the first is kept), numbered in order.
fn experience_values(cv: &EuropassCv) -> Vec<Value> {
    let mut seen = HashSet::new();
    cv.employment
        .iter()
        .filter(|entry| entry.employer.is_some() || entry.position.is_some())
        .filter(|entry| {
            let (start, end) = dates(&entry.period);
            seen.insert(experience_key(
                &field(entry.employer.as_ref()),
                &field(entry.position.as_ref()),
                date(start.as_ref()),
                date(end.as_ref()),
            ))
        })
        .enumerate()
        .map(|(index, entry)| experience(entry, index))
        .collect()
}

/// Text cut at the extractor's budget, or `None` when blank.
fn bounded(text: &str) -> Option<String> {
    let cleaned = strip_text(text);
    (!cleaned.is_empty()).then(|| truncate_utf16(cleaned, TEXT_LIMIT).to_owned())
}

/// A non-blank contact, once per (type, value).
fn push_contact(
    contacts: &mut Vec<Value>,
    seen: &mut HashSet<(&'static str, String)>,
    kind: &'static str,
    value: &str,
    label: &str,
) {
    let value = strip_text(value);
    if !value.is_empty() && seen.insert((kind, value.to_owned())) {
        contacts.push(json!({
            "id": null,
            "contactType": kind,
            "contactValue": value,
            "label": label,
        }));
    }
}

/// Websites classified as LinkedIn, GitHub or other by the host of the canonical URL, once per
/// (type, canonical URL), which is the key the profile write canonicalization de-duplicates on.
fn links(websites: &[String]) -> Vec<Value> {
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    for raw in websites.iter().filter(|raw| !strip_text(raw).is_empty()) {
        let url = normalize_url(raw);
        let canonical = canonical_profile_link_url(&url);
        let (kind, label) = link_kind(&canonical);
        if seen.insert((kind, canonical)) {
            links.push(json!({
                "id": null,
                "linkType": kind,
                "url": url,
                "label": label,
            }));
        }
    }
    links
}

/// LinkedIn profile (`linkedin.com/in/...`), GitHub (`github.com/...`) or other website, judged on
/// the host and path of a canonical (`https://host/path`, lower-case) URL.
fn link_kind(canonical: &str) -> (&'static str, &'static str) {
    let rest = canonical.strip_prefix("https://").unwrap_or(canonical);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host == "linkedin.com" && path.starts_with("in/") && path.len() > 3 {
        ("linkedin", "LinkedIn")
    } else if host == "github.com" && !path.is_empty() {
        ("github", "GitHub")
    } else {
        ("website", "Website")
    }
}

fn skills(cv: &EuropassCv) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut skills = Vec::new();
    for skill in &cv.skills {
        let name = strip_text(&skill.name);
        let normalized = name.to_lowercase();
        if name.is_empty() || !seen.insert(normalized.clone()) {
            continue;
        }
        skills.push(json!({
            "id": null,
            "skill": name,
            "normalizedSkill": normalized,
            "category": skill.taxonomy,
            "displayOrder": skills.len(),
        }));
    }
    skills
}

/// The primary language first (as `native`), then each assessed language once.
fn languages(cv: &EuropassCv) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut languages = Vec::new();
    let mut push = |name: &str, proficiency: Option<String>| {
        let name = strip_text(name);
        let normalized = name.to_lowercase();
        if name.is_empty() || !seen.insert(normalized.clone()) {
            return;
        }
        languages.push(json!({
            "id": null,
            "language": name,
            "normalizedLanguage": normalized,
            "proficiency": proficiency,
            "displayOrder": languages.len(),
        }));
    };
    if let Some(primary) = &cv.person.primary_language {
        push(language_name(primary), Some("native".to_owned()));
    }
    for skill in &cv.languages {
        push(language_name(&skill.name), proficiency(skill));
    }
    languages
}

/// ISO 639-1 codes of the languages a Europass export is likely to name, with their English names.
const LANGUAGE_CODES: [(&str, &str); 37] = [
    ("bg", "Bulgarian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("ga", "Irish"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("it", "Italian"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mt", "Maltese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sv", "Swedish"),
    ("sq", "Albanian"),
    ("no", "Norwegian"),
    ("is", "Icelandic"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ru", "Russian"),
    ("sr", "Serbian"),
    ("bs", "Bosnian"),
    ("mk", "Macedonian"),
    ("ar", "Arabic"),
    ("zh", "Chinese"),
    ("ja", "Japanese"),
    ("hi", "Hindi"),
];

/// The English name of a two-letter language code (any case); other text, including unknown
/// codes, is returned as written.
fn language_name(value: &str) -> &str {
    let text = strip_text(value);
    if text.len() == 2 {
        let code = text.to_ascii_lowercase();
        if let Some((_, name)) = LANGUAGE_CODES.iter().find(|(known, _)| *known == code) {
            return name;
        }
    }
    text
}

/// The one level when the five dimensions agree, otherwise `L B2, R B2, SI B1, SP B1, W B1`
/// (listening, reading, spoken interaction, spoken production, writing) with the known ones only.
fn proficiency(skill: &LanguageSkill) -> Option<String> {
    let levels = [
        ("L", skill.listening),
        ("R", skill.reading),
        ("SI", skill.spoken_interaction),
        ("SP", skill.spoken_production),
        ("W", skill.writing),
    ];
    let known: Vec<(&str, Cefr)> = levels
        .iter()
        .filter_map(|(label, level)| level.map(|level| (*label, level)))
        .collect();
    let (_, first) = *known.first()?;
    if known.len() == levels.len() && known.iter().all(|(_, level)| *level == first) {
        return Some(first.as_str().to_owned());
    }
    Some(
        known
            .iter()
            .map(|(label, level)| format!("{label} {}", level.as_str()))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// `city, COUNTRY` from the parts that are present.
fn location(city: Option<&str>, country_code: Option<&str>) -> Option<String> {
    let city = city.map(strip_text).filter(|city| !city.is_empty());
    let country = country_code
        .map(strip_text)
        .filter(|code| !code.is_empty())
        .map(str::to_uppercase);
    match (city, country) {
        (Some(city), Some(country)) => Some(format!("{city}, {country}")),
        (Some(city), None) => Some(city.to_owned()),
        (None, country) => country,
    }
}

/// The first or last day of the period a partial date names, as `YYYY-MM-DD`.
fn iso(date: PartialDate, last: bool) -> Option<String> {
    let month = date.month().unwrap_or(if last { 12 } else { 1 });
    let year = i32::from(date.year());
    let day = date.day().unwrap_or(if last {
        days_in_month(Month::try_from(month).ok()?, year)
    } else {
        1
    });
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()?;
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// Start and end dates; an ongoing period has no end, and an end before the start is dropped.
fn dates(period: &Period) -> (Option<String>, Option<String>) {
    let start = period.start.and_then(|date| iso(date, false));
    let end = if period.current == Some(true) {
        None
    } else {
        period.end.and_then(|date| iso(date, true))
    };
    let end = match (&start, end) {
        (Some(start), Some(end)) if end < *start => None,
        (_, end) => end,
    };
    (start, end)
}

fn education(entry: &Education) -> Value {
    let (start, end) = dates(&entry.period);
    json!({
        "id": null,
        "institution": entry.organization,
        "degree": entry.degree,
        "field": null,
        "location": location(entry.city.as_deref(), entry.country_code.as_deref()),
        "startDate": start,
        "endDate": end,
        "relevantFocus": entry.description.as_ref().and_then(|text| bounded(&text.plain())),
    })
}

fn experience(entry: &Employment, index: usize) -> Value {
    let (start, end) = dates(&entry.period);
    let city = entry.city.as_deref().or(entry.employer_city.as_deref());
    let country = entry
        .country_code
        .as_deref()
        .or(entry.employer_country_code.as_deref());
    json!({
        "id": null,
        "company": entry.employer,
        "title": entry.position,
        "location": location(city, country),
        "startDate": start,
        "endDate": end,
        "description": entry.description.as_ref().and_then(|text| bounded(&text.plain())),
        "displayOrder": index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_dates_cover_their_whole_period() {
        let year = PartialDate::new(2020, None, None);
        let month = PartialDate::new(2024, Some(2), None);
        assert_eq!(
            year.and_then(|d| iso(d, false)).as_deref(),
            Some("2020-01-01")
        );
        assert_eq!(
            year.and_then(|d| iso(d, true)).as_deref(),
            Some("2020-12-31")
        );
        assert_eq!(
            month.and_then(|d| iso(d, true)).as_deref(),
            Some("2024-02-29")
        );
    }

    #[test]
    fn locations_join_present_parts() {
        assert_eq!(
            location(Some("Rome"), Some("it")).as_deref(),
            Some("Rome, IT")
        );
        assert_eq!(location(None, Some("it")).as_deref(), Some("IT"));
        assert_eq!(location(Some(" "), None), None);
    }
}
