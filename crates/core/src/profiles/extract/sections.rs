//! Section splitting of resume text. Lines are split with `split_lines` and stripped with `strip_text`.

use jobniac_text::java::truncate_utf16;
use jobniac_text::search::lower_text;
use jobniac_text::str_rules::{is_text_alnum, is_text_alpha, split_lines, strip_text};

use super::find_phone;
use super::pattern::{DATE_RANGE, EMAIL, ENTRY_SEPARATOR, SECTION_HEADER};

/// The UTF-16 budget of a summary line or a description.
pub(super) const TEXT_LIMIT: usize = 500;

/// The raw section buffers of one resume text.
#[derive(Debug, Default)]
pub(super) struct Sections {
    /// Every summary-type line, each followed by a newline.
    pub(super) summary: String,
    /// Every skills-type line, each followed by a newline.
    pub(super) skills: String,
    /// Every language-type line, each followed by a newline.
    pub(super) languages: String,
    /// Education entries, each a list of lines (header first).
    pub(super) education: Vec<Vec<String>>,
    /// Experience entries, each a list of lines (header first).
    pub(super) experience: Vec<Vec<String>>,
    /// Project entries, each a list of lines (header first).
    pub(super) projects: Vec<Vec<String>>,
}

fn is_summary(section: &str) -> bool {
    matches!(section, "summary" | "profile" | "professional summary")
}

fn is_skills(section: &str) -> bool {
    matches!(
        section,
        "skills" | "technical skills" | "technologies" | "technology stack"
    )
}

fn is_languages(section: &str) -> bool {
    matches!(section, "languages" | "language skills")
}

fn is_experience(section: &str) -> bool {
    matches!(section, "experience" | "work experience")
}

fn is_structured(section: &str) -> bool {
    is_experience(section) || matches!(section, "projects" | "education")
}

/// A blank line ends the current entry, a header line (lower-cased, full
/// match) starts a section, and other lines go to the buffer of their section.
pub(super) fn split(text: &str) -> Sections {
    let mut result = Sections::default();
    let mut section = String::new();
    let mut current: Vec<String> = Vec::new();
    for raw_line in split_lines(text) {
        let line = strip_text(raw_line);
        if line.is_empty() {
            flush(&mut result, &section, &mut current);
            continue;
        }
        if let Some(header) = SECTION_HEADER.captures(&lower_text(line)) {
            flush(&mut result, &section, &mut current);
            section = header
                .get(1)
                .map_or_else(String::new, |name| name.as_str().to_owned());
        } else if is_summary(&section) {
            result.summary.push_str(line);
            result.summary.push('\n');
        } else if is_skills(&section) {
            result.skills.push_str(line);
            result.skills.push('\n');
        } else if is_languages(&section) {
            result.languages.push_str(line);
            result.languages.push('\n');
        } else if is_structured(&section) {
            if !current.is_empty() && starts_entry(line) {
                flush(&mut result, &section, &mut current);
            }
            current.push(line.to_owned());
        }
    }
    flush(&mut result, &section, &mut current);
    result
}

/// The current entry goes to its list when the section keeps entries, and the
/// buffer is cleared in every case.
fn flush(result: &mut Sections, section: &str, current: &mut Vec<String>) {
    let target = if is_experience(section) {
        "experience"
    } else {
        section
    };
    if !current.is_empty() {
        let entry = std::mem::take(current);
        match target {
            "education" => result.education.push(entry),
            "experience" => result.experience.push(entry),
            "projects" => result.projects.push(entry),
            _ => {}
        }
    }
    current.clear();
}

/// A date range, a `|`, or `head - tail` where the head is a short
/// alphanumeric label.
pub(super) fn starts_entry(line: &str) -> bool {
    if DATE_RANGE.is_match(line) || line.contains('|') {
        return true;
    }
    let Some(separator) = ENTRY_SEPARATOR.find(line) else {
        return false;
    };
    let head = &line[..separator.start()];
    head.chars().count() >= 2
        && head.chars().next().is_some_and(is_text_alnum)
        && head
            .chars()
            .all(|character| is_text_alpha(character) || "0123456789 .'-".contains(character))
}

/// One leading bullet and the surrounding whitespace are removed from
/// each line, the non-empty lines are joined with newlines, and the result is bounded.
pub(super) fn description(lines: &[String]) -> Option<String> {
    let cleaned: Vec<&str> = lines
        .iter()
        .map(|line| strip_text(strip_bullet(line)))
        .filter(|line| !line.is_empty())
        .collect();
    let value = cleaned.join("\n");
    if value.is_empty() {
        None
    } else {
        Some(truncate_utf16(&value, TEXT_LIMIT).to_owned())
    }
}

/// Removes one leading `•`, `*` or `-`.
fn strip_bullet(line: &str) -> &str {
    line.strip_prefix(['•', '*', '-']).unwrap_or(line)
}

/// The first line of the summary section, or else the second line of the text
/// that is not contact or profile-link text (the first line is taken as the name).
pub(super) fn summary(text: &str, section: &str) -> Option<String> {
    if !strip_text(section).is_empty() {
        let first = split_lines(section)
            .first()
            .map_or("", |line| strip_text(line));
        return Some(truncate_utf16(first, TEXT_LIMIT).to_owned());
    }
    let lines: Vec<&str> = split_lines(text)
        .into_iter()
        .filter(|raw| {
            let lower = lower_text(raw);
            !strip_text(raw).is_empty()
                && !EMAIL.is_match(raw)
                && find_phone(raw).is_none()
                && !lower.contains("linkedin")
                && !lower.contains("github")
        })
        .map(strip_text)
        .collect();
    lines
        .get(1)
        .map(|line| truncate_utf16(line, TEXT_LIMIT).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_starts_follow_the_rules() {
        assert!(starts_entry("Jan 2020 - Present"));
        assert!(starts_entry("Role | Co"));
        assert!(starts_entry("Engineer - Example Corp"));
        assert!(starts_entry("A1 - B"));
        assert!(!starts_entry("a - b"));
        assert!(!starts_entry("- bullet"));
        assert!(!starts_entry("plain text"));
    }

    #[test]
    fn bullets_are_removed_once() {
        let lines = vec!["• one".to_owned(), "- - two".to_owned(), "  ".to_owned()];
        assert_eq!(description(&lines).as_deref(), Some("one\n- two"));
        assert_eq!(description(&["  ".to_owned()]), None);
    }
}
