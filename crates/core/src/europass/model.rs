//! Typed, lossless model of a parsed Europass document.
//!
//! The model keeps every field the parsers read, so a later Europass renderer can write the same XML
//! back. It is `Serialize` only (golden tests); it never crosses IPC or the MCP wire as is.

use std::fmt;

use serde::{Serialize, Serializer};

/// Days in a month of the proleptic Gregorian calendar.
fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// A date part that is malformed or inconsistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDate;

/// A date that is known to the year, the month or the day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PartialDate {
    year: u16,
    month: Option<u8>,
    day: Option<u8>,
}

impl PartialDate {
    /// Build a date; a day needs a month, the year is 1..=9999 and the month and day must exist.
    pub fn new(year: u16, month: Option<u8>, day: Option<u8>) -> Option<Self> {
        if !(1..=9999).contains(&year) {
            return None;
        }
        match (month, day) {
            (None, None) => {}
            (Some(m), None) if (1..=12).contains(&m) => {}
            (Some(m), Some(d))
                if (1..=12).contains(&m)
                    && (1..=days_in_month(i32::from(year), m)).contains(&d) => {}
            _ => return None,
        }
        Some(Self { year, month, day })
    }

    /// Parse `YYYY`, `YYYY-MM` or `YYYY-MM-DD`, each with an optional time-zone suffix (dropped); a
    /// date-time (`YYYY-MM-DDThh:mm...`) keeps its date.
    pub fn parse(text: &str) -> Option<Self> {
        let text = strip_zone(text.trim());
        let date = match text.split_once('T') {
            Some((date, time)) if date.len() == 10 && !time.is_empty() => date,
            Some(_) => return None,
            None => text,
        };
        let mut parts = date.split('-');
        let year = fixed_digits(parts.next()?, 4)?;
        let month = match parts.next() {
            Some(part) => Some(fixed_digits(part, 2)?),
            None => None,
        };
        let day = match parts.next() {
            Some(part) => Some(fixed_digits(part, 2)?),
            None => None,
        };
        if parts.next().is_some() {
            return None;
        }
        Self::new(
            u16::try_from(year).ok()?,
            month.and_then(|m| u8::try_from(m).ok()),
            day.and_then(|d| u8::try_from(d).ok()),
        )
    }

    /// Build a date from the XSD `gYear`, `gMonth` and `gDay` lexical forms (`2026`, `--11`, `---03`).
    /// Absent parts stay unknown; an invalid or inconsistent combination is an [`InvalidDate`].
    pub fn from_g_parts(
        year: Option<&str>,
        month: Option<&str>,
        day: Option<&str>,
    ) -> Result<Option<Self>, InvalidDate> {
        let year = year.map(|text| parse_g(text, "", 4)).transpose()?;
        let month = month.map(|text| parse_g(text, "--", 2)).transpose()?;
        let day = day.map(|text| parse_g(text, "---", 2)).transpose()?;
        match (year, month, day) {
            (None, None, None) => Ok(None),
            (Some(year), month, day) => Self::new(
                u16::try_from(year).map_err(|_| InvalidDate)?,
                month.and_then(|m| u8::try_from(m).ok()),
                day.and_then(|d| u8::try_from(d).ok()),
            )
            .map(Some)
            .ok_or(InvalidDate),
            _ => Err(InvalidDate),
        }
    }

    pub fn year(&self) -> u16 {
        self.year
    }

    pub fn month(&self) -> Option<u8> {
        self.month
    }

    pub fn day(&self) -> Option<u8> {
        self.day
    }
}

/// Drops an XSD time-zone suffix (`Z`, `+hh:mm`, `-hh:mm`) from a date or date-time.
fn strip_zone(text: &str) -> &str {
    if let Some(rest) = text.strip_suffix('Z') {
        return rest;
    }
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len >= 6
        && bytes[len - 3] == b':'
        && matches!(bytes[len - 6], b'+' | b'-')
        && bytes[len - 5..len - 3].iter().all(u8::is_ascii_digit)
        && bytes[len - 2..].iter().all(u8::is_ascii_digit)
        && text.is_char_boundary(len - 6)
    {
        return &text[..len - 6];
    }
    text
}

/// Exactly `len` ASCII digits as a number.
fn fixed_digits(text: &str, len: usize) -> Option<u32> {
    if text.len() == len && text.bytes().all(|b| b.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

/// A `gYear`/`gMonth`/`gDay` lexical value: the prefix, `len` digits and an optional `Z` or
/// `+hh:mm`/`-hh:mm` zone, which is dropped.
fn parse_g(text: &str, prefix: &str, len: usize) -> Result<u32, InvalidDate> {
    let rest = text.trim().strip_prefix(prefix).ok_or(InvalidDate)?;
    let (digits, zone) = rest.split_at_checked(len).ok_or(InvalidDate)?;
    let zone_ok = zone.is_empty()
        || zone == "Z"
        || (zone.len() == 6
            && matches!(zone.as_bytes()[0], b'+' | b'-')
            && zone.as_bytes()[3] == b':'
            && zone[1..3].bytes().all(|b| b.is_ascii_digit())
            && zone[4..].bytes().all(|b| b.is_ascii_digit()));
    if !zone_ok {
        return Err(InvalidDate);
    }
    fixed_digits(digits, len).ok_or(InvalidDate)
}

impl fmt::Display for PartialDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}", self.year)?;
        if let Some(month) = self.month {
            write!(f, "-{month:02}")?;
            if let Some(day) = self.day {
                write!(f, "-{day:02}")?;
            }
        }
        Ok(())
    }
}

impl Serialize for PartialDate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A Common European Framework of Reference language level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Cefr {
    A1,
    A2,
    B1,
    B2,
    C1,
    C2,
}

impl Cefr {
    /// Parse `A1`..`C2`, ignoring case and surrounding whitespace.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_uppercase().as_str() {
            "A1" => Some(Self::A1),
            "A2" => Some(Self::A2),
            "B1" => Some(Self::B1),
            "B2" => Some(Self::B2),
            "C1" => Some(Self::C1),
            "C2" => Some(Self::C2),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::A1 => "A1",
            Self::A2 => "A2",
            Self::B1 => "B1",
            Self::B2 => "B2",
            Self::C1 => "C1",
            Self::C2 => "C2",
        }
    }
}

/// One block of a rich-text field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "text", rename_all = "camelCase")]
pub enum RichBlock {
    /// A paragraph of plain text (soft line breaks inside it are `\n`).
    Paragraph(String),
    /// An empty paragraph (`<p><br></p>`), a visual gap.
    Blank,
    /// One list item.
    Bullet(String),
}

/// A rich-text field: the plain-text blocks and, when nothing was cut, the original markup.
///
/// The markup is kept because inline formatting such as bold is not representable in the blocks;
/// the blocks are what profile mapping and search use. A field cut at its cap has no markup and is
/// listed in `truncated_fields`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RichText {
    pub markup: Option<String>,
    pub blocks: Vec<RichBlock>,
}

impl RichText {
    /// Plain text: blocks joined by newlines, bullets prefixed with `- `, blanks as empty lines.
    pub fn plain(&self) -> String {
        let lines: Vec<String> = self
            .blocks
            .iter()
            .map(|block| match block {
                RichBlock::Paragraph(text) => text.clone(),
                RichBlock::Blank => String::new(),
                RichBlock::Bullet(text) => format!("- {text}"),
            })
            .collect();
        lines.join("\n")
    }
}

/// A telephone number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phone {
    pub use_code: Option<String>,
    /// Country dialing code without `+` (v4 `CountryDialing`); `None` when `number` is complete.
    pub country_dialing: Option<String>,
    pub number: String,
    pub country_code: Option<String>,
}

impl Phone {
    /// The number in international form: `+`, the dialing code, a space and the number when both
    /// parts are known, otherwise the number as written. The national number is never changed, in
    /// particular a leading trunk `0` is not stripped: it is wrong to drop for some countries
    /// (Italy keeps it) and the editor does not say which case applies.
    pub fn international(&self) -> String {
        let number = self.number.trim();
        match self.country_dialing.as_deref().map(str::trim) {
            Some(dialing) if !dialing.is_empty() && !number.starts_with('+') => {
                let dialing = dialing.trim_start_matches('+');
                format!("+{dialing} {number}")
            }
            _ => number.to_owned(),
        }
    }
}

/// A postal address.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    /// The `type` attribute (`home`) or the communication `UseCode`.
    pub kind: Option<String>,
    pub lines: Vec<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
    pub country_code: Option<String>,
    pub country_label: Option<String>,
}

/// A natural person's contact data.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub emails: Vec<String>,
    pub websites: Vec<String>,
    pub phones: Vec<Phone>,
    pub addresses: Vec<Address>,
    pub birth_date: Option<PartialDate>,
    pub primary_language: Option<String>,
}

/// A time span; `current` is the editor's "present" flag (`CurrentIndicator`/`Ongoing`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Period {
    pub start: Option<PartialDate>,
    pub end: Option<PartialDate>,
    pub current: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Employment {
    pub employer: Option<String>,
    pub employer_city: Option<String>,
    pub employer_country_code: Option<String>,
    pub position: Option<String>,
    pub period: Period,
    pub description: Option<RichText>,
    pub city: Option<String>,
    pub country_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Education {
    pub organization: Option<String>,
    pub city: Option<String>,
    pub country_code: Option<String>,
    pub period: Period,
    pub degree: Option<String>,
    pub description: Option<RichText>,
}

/// A language with its five CEFR self-assessment dimensions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageSkill {
    pub name: String,
    /// The `schemeName` of the language identifier (`FREE_TEXT`, an ISO scheme, ...).
    pub scheme: Option<String>,
    pub listening: Option<Cefr>,
    pub reading: Option<Cefr>,
    pub spoken_interaction: Option<Cefr>,
    pub spoken_production: Option<Cefr>,
    pub writing: Option<Cefr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// The taxonomy (`Digital_Skill`, ...).
    pub taxonomy: Option<String>,
    pub name: String,
}

/// Rendering choices of the Europass editor (template, colour, section order).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Design {
    pub template: Option<String>,
    pub color: Option<String>,
    pub font_size: Option<String>,
    pub logo: Option<String>,
    pub page_numbers: Option<bool>,
    pub sections_order: Vec<String>,
}

/// A parsed v4 `Candidate` CV.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EuropassCv {
    /// `schemeVersionID` of the document id (`4.0`).
    pub schema_version: Option<String>,
    /// `languageCode` of the profile (`en`).
    pub language_code: Option<String>,
    pub person: Person,
    pub summary: Option<RichText>,
    pub employment: Vec<Employment>,
    pub education: Vec<Education>,
    pub languages: Vec<LanguageSkill>,
    pub skills: Vec<Skill>,
    pub design: Option<Design>,
    /// Names of non-empty section elements the parser does not map.
    pub unsupported_sections: Vec<String>,
    /// Indexed paths of fields or lists cut at a cap (`employment[3].position`, `skills`).
    pub truncated_fields: Vec<String>,
    /// Indexed paths of malformed values that were dropped (`person.birthDate`).
    pub invalid_fields: Vec<String>,
}

/// The v3 `DocumentInfo` block.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInfo {
    pub creation_date: Option<String>,
    pub last_update_date: Option<String>,
    pub xsd_version: Option<String>,
    pub generator: Option<String>,
}

/// The recipient of a cover letter.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Addressee {
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub organisation: Option<String>,
    pub address: Option<Address>,
}

/// The opening salutation (`Dear` + recipient name).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Salutation {
    pub label: Option<String>,
    pub name: Option<String>,
}

/// A parsed v3 `SkillsPassport` cover letter (`ECL`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EuropassCoverLetter {
    pub locale: Option<String>,
    pub document_info: DocumentInfo,
    pub sender: Person,
    pub addressee: Addressee,
    pub date: Option<PartialDate>,
    pub place: Option<String>,
    pub subject: Option<String>,
    pub opening: Salutation,
    pub body: Option<RichText>,
    pub closing: Option<String>,
    pub design: Option<Design>,
    pub unsupported_sections: Vec<String>,
    pub truncated_fields: Vec<String>,
    pub invalid_fields: Vec<String>,
}

/// A parsed Europass document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "document", rename_all = "camelCase")]
pub enum EuropassDocument {
    Cv(Box<EuropassCv>),
    CoverLetter(Box<EuropassCoverLetter>),
}
