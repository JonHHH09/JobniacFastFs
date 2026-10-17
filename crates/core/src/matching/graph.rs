//! Graph inputs of the scorer (flat structs).
//!
//! The repository builds these from PostgreSQL rows. Identifiers and revisions stay as text, so the
//! scorer applies one fixed parsing and error order.

use uuid::Uuid;

/// A profile graph: the profile root, its normalized skills and whether it has any experience row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGraph {
    /// The profile identifier text.
    pub id: String,
    /// The profile update instant text.
    pub updated_at: String,
    /// The normalized skill of each profile skill row, in stored order.
    pub skills: Vec<String>,
    /// Whether the profile has at least one experience row. Only the presence of a row is
    /// read by the scorer.
    pub has_experience: bool,
}

/// A job graph: the job root and its normalized skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobGraph {
    /// The job identifier text.
    pub id: String,
    /// The job update instant text.
    pub updated_at: String,
    /// `experienceRequirement`. The scorer reads it only when it is text.
    pub experience_requirement: Option<String>,
    /// `seniority`. The scorer reads it only when it is text.
    pub seniority: Option<String>,
    /// The normalized skill of each job skill row, in stored order. Duplicates are kept.
    pub skills: Vec<String>,
}

/// Parses a graph identifier.
///
/// Removes the `urn:` and `uuid:` prefixes, strips the braces and removes every hyphen, and then
/// requires 32 ASCII hexadecimal digits. Surrounding whitespace, a sign, a `0x` prefix and
/// underscores are refused. Graph identifiers come from `uuid` columns, so the
/// adapter never supplies them.
pub fn uuid_from_text(text: &str) -> Option<Uuid> {
    let without_prefix = text.replace("urn:", "").replace("uuid:", "");
    let hex: String = without_prefix
        .trim_matches(['{', '}'])
        .chars()
        .filter(|character| *character != '-')
        .collect();
    if hex.chars().count() != 32 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok().map(Uuid::from_u128)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_lenient_spellings_of_one_identifier() {
        let canonical = "11111111-1111-4111-8111-111111111111";
        let expected = Uuid::from_u128(0x1111_1111_1111_4111_8111_1111_1111_1111);
        for text in [
            canonical,
            "11111111111141118111111111111111",
            "{11111111-1111-4111-8111-111111111111}",
            "urn:uuid:11111111-1111-4111-8111-111111111111",
            "11111111-1111-4111-8111111111111111",
        ] {
            assert_eq!(uuid_from_text(text), Some(expected), "{text}");
        }
    }

    #[test]
    fn refuses_malformed_identifier_text() {
        assert_eq!(uuid_from_text(""), None);
        assert_eq!(uuid_from_text("11111111-1111-4111-8111-11111111111"), None);
        assert_eq!(uuid_from_text("11111111-1111-4111-8111-11111111111g"), None);
    }
}
