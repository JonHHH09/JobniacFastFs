//! The synthetic labelled corpus: determinism, privacy, held-out vocabulary, structural variety and
//! the reading pipeline over every generated document.

use std::collections::BTreeMap;

use jobniac_core::profiles::extract::lines::corpus::{
    CorpusStyle, DocumentKind, LabelledDocument, synthetic_cover_letters, synthetic_document,
    synthetic_resumes,
};
use jobniac_core::profiles::extract::lines::dates::{RangeOutcome, parse_date_range};
use jobniac_core::profiles::extract::lines::locale::{LocaleHint, PhoneKind, find_phones};
use jobniac_core::profiles::extract::lines::{
    LetterLineRole, LineDocument, LineLabel, ResumeLineRole, TextLine, feature_index,
    line_features, lines_from_text,
};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SUMMARY: &str = include_str!("golden/line-corpus-summary.json");
const SEED: u64 = 20_260_101;

/// A fixed sample: 24 resumes (three of each style) and 15 cover letters (three of each).
fn sample() -> Vec<LabelledDocument> {
    let mut documents = synthetic_resumes(SEED, 24);
    documents.extend(synthetic_cover_letters(SEED, 15));
    documents
}

/// Many more documents for the invariant checks.
fn large_sample() -> Vec<LabelledDocument> {
    let mut documents = Vec::new();
    for seed in 1..=5 {
        documents.extend(synthetic_resumes(seed, 32));
        documents.extend(synthetic_cover_letters(seed, 20));
    }
    documents
}

fn canonical_dump(documents: &[LabelledDocument]) -> String {
    let mut dump = String::new();
    for document in documents {
        dump.push_str(&format!("# {}\n", document.style.name()));
        for labelled in &document.plain {
            dump.push_str(&format!(
                "P|{}|{}|{:?}|{}\n",
                labelled.label.name(),
                u8::from(labelled.line.blank_before),
                labelled.continues,
                labelled.line.text
            ));
        }
        for labelled in &document.geometry {
            let g = labelled.line.geometry;
            dump.push_str(&format!(
                "G|{}|{}|{}|{:?}\n",
                labelled.label.name(),
                labelled.line.page,
                labelled.line.text,
                g.map(|g| (
                    g.x0,
                    g.y0,
                    g.x1,
                    g.y1,
                    g.font_size_centipoints,
                    g.bold,
                    g.italic,
                    g.page_width_centipoints,
                    g.page_height_centipoints
                ))
            ));
        }
    }
    dump
}

/// How many lines of a label carry a lexicon flag (`flagged`) and how many do not (`held_out`).
#[derive(Default, Clone, Copy)]
struct Flagged {
    total: usize,
    flagged: usize,
}

impl Flagged {
    fn held_out_share(self) -> f64 {
        (self.total - self.flagged) as f64 / self.total.max(1) as f64
    }
}

/// The label-versus-lexicon-flag statistics of documents read as plain text (the pipeline view).
fn held_out(documents: &[LabelledDocument]) -> BTreeMap<&'static str, Flagged> {
    let kinds: [(&str, LineLabel, &str); 7] = [
        (
            "section_heading.heading_any",
            LineLabel::Resume(ResumeLineRole::SectionHeading),
            "heading_any",
        ),
        (
            "organization.has_company_suffix",
            LineLabel::Resume(ResumeLineRole::Organization),
            "has_company_suffix",
        ),
        (
            "title.has_title_keyword",
            LineLabel::Resume(ResumeLineRole::Title),
            "has_title_keyword",
        ),
        (
            "salutation.is_salutation",
            LineLabel::Letter(LetterLineRole::Salutation),
            "is_salutation",
        ),
        (
            "closing.is_closing",
            LineLabel::Letter(LetterLineRole::Closing),
            "is_closing",
        ),
        (
            "subject.is_subject",
            LineLabel::Letter(LetterLineRole::Subject),
            "is_subject",
        ),
        (
            "title.has_degree",
            LineLabel::Resume(ResumeLineRole::Title),
            "has_degree",
        ),
    ];
    let mut stats: BTreeMap<&'static str, Flagged> = BTreeMap::new();
    for document in documents {
        let view = document.pipeline_view();
        let reread = LineDocument::from_text(&document.plain_text());
        let rows = line_features(&reread);
        assert_eq!(rows.len(), view.len(), "{}", document.style.name());
        for ((_, label), row) in view.iter().zip(&rows) {
            for (name, wanted, feature) in kinds {
                if *label == wanted {
                    let index = feature_index(feature).unwrap_or_else(|| panic!("{feature}"));
                    let entry = stats.entry(name).or_default();
                    entry.total += 1;
                    if row[index] > 0.5 {
                        entry.flagged += 1;
                    }
                }
            }
        }
    }
    stats
}

fn summary(documents: &[LabelledDocument]) -> Value {
    let mut styles: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    let mut labels: BTreeMap<String, usize> = BTreeMap::new();
    for document in documents {
        let entry = styles.entry(document.style.name()).or_default();
        entry.0 += 1;
        entry.1 += document.plain.len();
        entry.2 += document.pipeline_view().len();
        for labelled in &document.plain {
            let kind = match labelled.label {
                LineLabel::Resume(_) => "resume",
                LineLabel::Letter(_) => "letter",
            };
            *labels.entry(kind.to_owned()).or_default() += 1;
            *labels
                .entry(format!("{kind}.{}", labelled.label.name()))
                .or_default() += 1;
        }
    }
    let held: serde_json::Map<String, Value> = held_out(documents)
        .into_iter()
        .map(|(name, stat)| {
            (
                name.to_owned(),
                json!({"lines": stat.total, "flagged": stat.flagged}),
            )
        })
        .collect();
    json!({
        "seed": SEED,
        "documents": documents.len(),
        "styles": styles
            .iter()
            .map(|(name, (count, lines, joined))| {
                (name.to_string(), json!({"documents": count, "lines": lines, "pipeline_lines": joined}))
            })
            .collect::<serde_json::Map<String, Value>>(),
        "labels": labels,
        "lexicon_flags": held,
        "sha256": Sha256::digest(canonical_dump(documents).as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    })
}

#[test]
#[ignore = "prints the golden summary and the held-out shares; run with --ignored --nocapture"]
fn print_summary() {
    println!(
        "{}",
        serde_json::to_string_pretty(&summary(&sample())).unwrap_or_default()
    );
    for (name, stat) in held_out(&large_sample()) {
        println!(
            "{name}: {} lines, held out {:.3}",
            stat.total,
            stat.held_out_share()
        );
    }
}

#[test]
fn the_same_seed_gives_identical_output_and_matches_the_golden_summary()
-> Result<(), serde_json::Error> {
    let first = sample();
    let second = sample();
    assert_eq!(first, second);
    assert_eq!(canonical_dump(&first), canonical_dump(&second));
    assert_ne!(
        canonical_dump(&first),
        canonical_dump(&synthetic_resumes(SEED + 1, 24))
    );
    let golden: Value = serde_json::from_str(SUMMARY)?;
    assert_eq!(summary(&first), golden);
    Ok(())
}

#[test]
fn every_style_and_every_role_occurs() {
    let documents = large_sample();
    for style in CorpusStyle::RESUMES
        .iter()
        .chain(CorpusStyle::LETTERS.iter())
    {
        assert!(
            documents.iter().any(|d| d.style == *style),
            "{}",
            style.name()
        );
    }
    let labels: Vec<LineLabel> = documents
        .iter()
        .flat_map(LabelledDocument::labels)
        .collect();
    for role in ResumeLineRole::ALL {
        assert!(labels.contains(&LineLabel::Resume(role)), "{}", role.name());
    }
    for role in LetterLineRole::ALL {
        assert!(labels.contains(&LineLabel::Letter(role)), "{}", role.name());
    }
    for role in ResumeLineRole::ALL {
        assert_eq!(ResumeLineRole::from_name(role.name()), Some(role));
    }
    for role in LetterLineRole::ALL {
        assert_eq!(LetterLineRole::from_name(role.name()), Some(role));
    }
}

#[test]
fn generated_contact_data_is_fictional() {
    let email = Regex::new(r"[A-Za-z0-9._%+-]+@([A-Za-z0-9.-]+)").unwrap_or_else(|e| panic!("{e}"));
    let url = Regex::new(r"https?://([^/\s|,)]+)").unwrap_or_else(|e| panic!("{e}"));
    let mut emails = 0;
    let mut urls = 0;
    let mut phones = 0;
    for document in large_sample() {
        for line in document.pipeline_view() {
            let text = line.0;
            for captures in email.captures_iter(&text) {
                emails += 1;
                assert!(
                    matches!(&captures[1], "example.com" | "example.org"),
                    "{text}"
                );
            }
            for captures in url.captures_iter(&text) {
                urls += 1;
                let host = &captures[1];
                assert!(
                    ["example.com", "example.org"]
                        .iter()
                        .any(|d| host == *d || host.ends_with(&format!(".{d}"))),
                    "{text}"
                );
            }
            for phone in find_phones(&text) {
                phones += 1;
                let digits: String = phone.text.chars().filter(char::is_ascii_digit).collect();
                let fictional = match phone.kind {
                    PhoneKind::Nanp => digits[digits.len() - 10..].get(3..8) == Some("55501"),
                    _ => {
                        digits.starts_with("493023125")
                            || digits.starts_with("35569000")
                            || digits.starts_with("33199")
                            || (digits.starts_with('1') && digits.get(4..9) == Some("55501"))
                    }
                };
                assert!(fictional, "{text}");
            }
        }
    }
    assert!(
        emails > 200 && urls > 100 && phones > 200,
        "{emails} {urls} {phones}"
    );
}

#[test]
fn plain_and_geometry_lines_agree() {
    let (mut agree, mut compared) = (0_usize, 0_usize);
    for document in large_sample() {
        let name = document.style.name();
        assert_eq!(document.plain.len(), document.geometry.len(), "{name}");
        for (plain, geometry) in document.plain.iter().zip(&document.geometry) {
            assert_eq!(plain.line.text, geometry.line.text, "{name}");
            assert_eq!(plain.label, geometry.label, "{name}");
            assert_eq!(plain.continues, geometry.continues, "{name}");
            assert!(plain.line.geometry.is_none() && geometry.line.geometry.is_some());
            let g = geometry.line.geometry.unwrap_or_else(|| panic!("{name}"));
            assert!(
                g.x0 >= 0 && g.x1 <= g.page_width_centipoints,
                "{name}: {}",
                geometry.line.text
            );
            assert!(
                g.y0 >= 0 && g.y1 <= g.page_height_centipoints,
                "{name}: {}",
                geometry.line.text
            );
            assert!(g.font_size_centipoints > 0 && g.x1 > g.x0 && g.y1 > g.y0);
        }
        // The plain text round-trips through the plain reader.
        let reread = lines_from_text(&document.plain_text());
        let expected: Vec<(&str, bool)> = document
            .plain
            .iter()
            .map(|l| (l.line.text.as_str(), l.line.blank_before))
            .collect();
        let actual: Vec<(&str, bool)> = reread
            .iter()
            .map(|l| (l.text.as_str(), l.blank_before))
            .collect();
        assert_eq!(actual, expected, "{name}");

        // `blank_before` has one meaning in both inputs: compare the pipeline lines.
        let from_text = LineDocument::from_text(&document.plain_text());
        let from_geometry = LineDocument::from_lines(document.geometry_lines());
        assert_eq!(from_text.lines.len(), from_geometry.lines.len(), "{name}");
        for (a, b) in from_text.lines.iter().zip(&from_geometry.lines) {
            compared += 1;
            agree += usize::from(a.blank_before == b.blank_before);
        }
    }
    let share = agree as f64 / compared as f64;
    assert!(share > 0.97, "blank_before agreement {share}");
}

#[test]
fn date_ranges_of_date_range_lines_parse() {
    for document in large_sample() {
        for (text, label) in document.pipeline_view() {
            if label == LineLabel::Resume(ResumeLineRole::DateRange) {
                assert!(
                    matches!(
                        parse_date_range(&text, LocaleHint::Unknown),
                        RangeOutcome::Range(_)
                    ),
                    "{}: {text}",
                    document.style.name()
                );
            }
        }
    }
}

#[test]
fn labels_must_be_learned_from_more_than_the_lexicon_flags() {
    let stats = held_out(&large_sample());
    // About a third of each lexicon-backed kind comes from held-out pools (organizations and
    // titles also include schools and degrees, which never carry a company suffix or a title word).
    let bounds: [(&str, f64, f64); 6] = [
        ("section_heading.heading_any", 0.15, 0.55),
        ("organization.has_company_suffix", 0.25, 0.85),
        ("title.has_title_keyword", 0.25, 0.85),
        ("salutation.is_salutation", 0.15, 0.55),
        ("closing.is_closing", 0.15, 0.55),
        ("subject.is_subject", 0.10, 0.55),
    ];
    for (name, low, high) in bounds {
        let stat = stats.get(name).copied().unwrap_or_default();
        let share = stat.held_out_share();
        assert!(
            stat.total >= 80 && (low..=high).contains(&share),
            "{name}: {share} of {}",
            stat.total
        );
    }
    // The flags also occur on other labels: a region on organization and contact lines, a company
    // suffix on title lines, a year or a date on non-date lines.
    let mut region_off_location = 0;
    let mut suffix_off_organization = 0;
    for document in large_sample() {
        let reread = LineDocument::from_text(&document.plain_text());
        let rows = line_features(&reread);
        let view = document.pipeline_view();
        let region = feature_index("has_region").unwrap_or(0);
        let suffix = feature_index("has_company_suffix").unwrap_or(0);
        for ((_, label), row) in view.iter().zip(&rows) {
            if row[region] > 0.5 && *label != LineLabel::Resume(ResumeLineRole::Location) {
                region_off_location += 1;
            }
            if row[suffix] > 0.5 && *label != LineLabel::Resume(ResumeLineRole::Organization) {
                suffix_off_organization += 1;
            }
        }
    }
    assert!(region_off_location > 200, "{region_off_location}");
    assert!(suffix_off_organization > 50, "{suffix_off_organization}");
}

#[test]
fn documents_vary_in_structure() {
    let documents = large_sample();
    let resumes: Vec<&LabelledDocument> = documents
        .iter()
        .filter(|d| d.kind == DocumentKind::Resume)
        .collect();
    let first_label_is_name = resumes
        .iter()
        .filter(|d| d.labels()[0] == LineLabel::Resume(ResumeLineRole::Name))
        .count();
    let share = first_label_is_name as f64 / resumes.len() as f64;
    assert!(
        (0.4..0.95).contains(&share),
        "the name is first in {share} of the resumes"
    );
    let lengths: Vec<usize> = resumes.iter().map(|d| d.plain.len()).collect();
    let (min, max) = (
        lengths.iter().min().copied().unwrap_or(0),
        lengths.iter().max().copied().unwrap_or(0),
    );
    assert!(max > 3 * min, "{min}..{max} lines");
    let has = |role: ResumeLineRole| {
        resumes
            .iter()
            .filter(|d| d.labels().contains(&LineLabel::Resume(role)))
            .count()
    };
    assert!(has(ResumeLineRole::Other) > resumes.len() / 2);
    // Several pages and wrapped fragments occur.
    assert!(
        resumes
            .iter()
            .any(|d| d.geometry.iter().any(|l| l.line.page > 0))
    );
    assert!(
        documents
            .iter()
            .any(|d| d.plain.iter().any(|l| l.continues == Some("")))
    );
    assert!(
        documents
            .iter()
            .any(|d| d.plain.iter().any(|l| l.continues == Some(" ")))
    );
    // Several date separators and ongoing markers.
    let mut separators = std::collections::BTreeSet::new();
    for document in &documents {
        for (text, label) in document.pipeline_view() {
            if label == LineLabel::Resume(ResumeLineRole::DateRange) {
                for separator in [
                    " – ", " - ", " — ", "–", " to ", " bis ", " deri ", " à ", " au ", " until ",
                ] {
                    if text.contains(separator) {
                        separators.insert(separator);
                    }
                }
            }
        }
    }
    assert!(separators.len() >= 6, "{separators:?}");
}

/// Re-sorts geometry lines row by row (page, top, left), as a row-wise extractor would return them.
fn row_wise(lines: &[TextLine]) -> Vec<TextLine> {
    let mut sorted = lines.to_vec();
    sorted.sort_by_key(|l| {
        let g = l.geometry;
        (l.page, g.map_or(0, |g| g.y0), g.map_or(0, |g| g.x0))
    });
    sorted
}

#[test]
fn the_reading_pipeline_recovers_every_labelled_document() {
    for document in large_sample() {
        let name = document.style.name();
        let view = document.pipeline_view();
        let expected: Vec<&str> = view.iter().map(|(text, _)| text.as_str()).collect();

        let from_text = LineDocument::from_text(&document.plain_text());
        let actual: Vec<&str> = from_text.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(actual, expected, "{name}: plain");

        let geometry = document.geometry_lines();
        let ordered = LineDocument::from_lines(geometry.clone());
        let actual: Vec<&str> = ordered.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(actual, expected, "{name}: geometry in reading order");

        let shuffled = LineDocument::from_lines(row_wise(&geometry));
        let actual: Vec<&str> = shuffled.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(actual, expected, "{name}: geometry row by row");

        for features in [line_features(&from_text), line_features(&shuffled)] {
            assert_eq!(features.len(), expected.len());
        }
    }
}

#[test]
fn europass_pages_are_detected_as_two_columns() {
    for document in synthetic_resumes(3, 64) {
        let name = document.style.name();
        let ordered = LineDocument::from_lines(row_wise(&document.geometry_lines()));
        if matches!(
            document.style,
            CorpusStyle::EuropassEnglish
                | CorpusStyle::EuropassGerman
                | CorpusStyle::EuropassAlbanian
                | CorpusStyle::EuropassFrench
        ) {
            assert!(
                ordered.columns.contains(&1) && ordered.columns.contains(&2),
                "{name}"
            );
            assert_eq!(ordered.locale, LocaleHint::Eu, "{name}");
        } else {
            assert!(ordered.columns.iter().all(|c| *c == 0), "{name}");
        }
    }
    let one = synthetic_document(CorpusStyle::UsChronological, 9);
    let locale = LineDocument::from_text(&one.plain_text()).locale;
    assert!(
        matches!(locale, LocaleHint::Us | LocaleHint::Unknown),
        "{locale:?}"
    );
}
