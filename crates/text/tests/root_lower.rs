//! ROOT lowercase compared with the golden vectors.
//!
//! The vector files under `tests/golden/`, except `root-lower-jdk.json`, hold generated expected
//! values. `root-lower-jdk.json` holds recorded OpenJDK 25 observations (see its provenance
//! block). Every vector must pass.

use std::collections::BTreeMap;

use jobniac_text::case::root_lower;
use serde_json::Value;

const CODEPOINTS: &str = include_str!("golden/root-lower-codepoints.json");
const SIGMA: &str = include_str!("golden/root-lower-sigma.json");
const PAIRS: &str = include_str!("golden/root-lower-sigma-pairs.json");
const JDK: &str = include_str!("golden/root-lower-jdk.json");
const RANDOM: &str = include_str!("golden/root-lower-random.json");
const SIGMA_CLASSES: &str = include_str!("golden/root-lower-sigma-classes.json");

const SIGMA_CAPITAL: char = '\u{3a3}';
const SIGMA_SMALL: char = '\u{3c3}';
const SIGMA_FINAL: char = '\u{3c2}';
const ALPHA_CAPITAL: char = '\u{391}';

fn parse(source: &str) -> Value {
    serde_json::from_str(source).expect("vector file is valid JSON")
}

fn vectors(document: &Value) -> Vec<(String, String)> {
    document["vectors"]
        .as_array()
        .expect("vectors array")
        .iter()
        .map(|vector| {
            (
                vector["input"].as_str().expect("input string").to_owned(),
                vector["expected"]
                    .as_str()
                    .expect("expected string")
                    .to_owned(),
            )
        })
        .collect()
}

fn assert_vectors(name: &str, vectors: &[(String, String)]) {
    let failures: Vec<String> = vectors
        .iter()
        .filter_map(|(input, expected)| {
            let observed = root_lower(input);
            (observed != *expected)
                .then(|| format!("{input:?} gave {observed:?}, expected {expected:?}"))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{name}: {} of {} vectors differ; first: {:?}",
        failures.len(),
        vectors.len(),
        failures.iter().take(10).collect::<Vec<_>>()
    );
}

/// The nine probes of the generator, as digits: each probe is evaluated with `root_lower`.
fn probe_bits(character: char) -> String {
    let c = character;
    let s = SIGMA_CAPITAL;
    let a = ALPHA_CAPITAL;
    let final_at_end = |text: String| root_lower(&text).ends_with(SIGMA_FINAL);
    let small_at_end = |text: String| root_lower(&text).ends_with(SIGMA_SMALL);
    let small_at_one = |text: String| root_lower(&text).chars().nth(1) == Some(SIGMA_SMALL);
    let bits = [
        final_at_end(format!("{a}{c}{s}")),
        final_at_end(format!("1{c}{s}")),
        final_at_end(format!("{c}{s}")),
        small_at_one(format!("{a}{s}{c}")),
        final_at_end(format!("A{c}{s}"))
            && final_at_end(format!("A{c}_{s}"))
            && small_at_end(format!("A1{c}_{s}"))
            && small_at_one(format!("A{s}{c}B")),
        small_at_end(format!("A'{c}{s}")),
        small_at_end(format!("A1{c}{s}")),
        small_at_one(format!("A{s}{c}'B")),
        small_at_one(format!("A{s}{c}1B")),
        small_at_end(format!("{c}'{s}")),
    ];
    bits.iter()
        .map(|&bit| if bit { '1' } else { '0' })
        .collect()
}

#[test]
fn codepoint_table_is_well_formed() {
    let document = parse(CODEPOINTS);
    let entries = document["entries"].as_array().expect("entries array");
    let mut previous = None;
    for entry in entries {
        let codepoint = entry[0].as_u64().expect("code point");
        let result = entry[1].as_str().expect("result string");
        let character = u32::try_from(codepoint)
            .ok()
            .and_then(char::from_u32)
            .expect("stored code point is a scalar value");
        assert!(
            previous.is_none_or(|last| last < codepoint),
            "entries are ascending"
        );
        assert_ne!(
            result,
            character.to_string(),
            "identity results are not stored"
        );
        previous = Some(codepoint);
    }
    assert_eq!(
        entries.len(),
        1460,
        "one entry per changed single-character mapping"
    );
}

#[test]
fn every_scalar_value_matches_the_golden_table() {
    let document = parse(CODEPOINTS);
    let mut table = BTreeMap::new();
    for entry in document["entries"].as_array().expect("entries array") {
        let codepoint = entry[0].as_u64().expect("code point");
        let result = entry[1].as_str().expect("result string").to_owned();
        table.insert(codepoint, result);
    }
    let mut failures = Vec::new();
    let mut checked = 0_usize;
    for codepoint in 0..=0x10_FFFF_u32 {
        let Some(character) = char::from_u32(codepoint) else {
            continue;
        };
        checked += 1;
        let input = character.to_string();
        let expected = table
            .get(&u64::from(codepoint))
            .cloned()
            .unwrap_or_else(|| input.clone());
        let observed = root_lower(&input);
        if observed != expected {
            failures.push(format!(
                "U+{codepoint:04X} gave {observed:?}, expected {expected:?}"
            ));
        }
    }
    assert_eq!(checked, 1_112_064, "every Unicode scalar value is checked");
    assert!(
        failures.is_empty(),
        "{} code points differ; first: {:?}",
        failures.len(),
        failures.iter().take(40).collect::<Vec<_>>()
    );
}

#[test]
fn final_sigma_context_vectors_match_the_golden() {
    assert_vectors("sigma", &vectors(&parse(SIGMA)));
}

#[test]
fn final_sigma_pair_vectors_match_the_golden() {
    let document = parse(PAIRS);
    let all = vectors(&document);
    assert_eq!(all.len(), 27_849, "pair corpus size");
    assert_vectors("sigma pairs", &all);
}

#[test]
fn final_sigma_probe_bits_match_the_golden_for_every_scalar_value() {
    let document = parse(SIGMA_CLASSES);
    let mut failures = Vec::new();
    let mut checked = 0_usize;
    for range in document["ranges"].as_array().expect("ranges array") {
        let start = range[0].as_u64().expect("range start");
        let end = range[1].as_u64().expect("range end");
        let bits = range[2].as_str().expect("probe bits");
        for codepoint in start..=end {
            let Some(character) = u32::try_from(codepoint).ok().and_then(char::from_u32) else {
                failures.push(format!("U+{codepoint:04X} is not a scalar value"));
                continue;
            };
            checked += 1;
            let observed = probe_bits(character);
            if observed != bits {
                failures.push(format!(
                    "U+{codepoint:04X} probes {observed}, golden {bits}"
                ));
            }
        }
    }
    assert_eq!(checked, 1_112_064, "every Unicode scalar value is checked");
    assert!(
        failures.is_empty(),
        "{} code points differ; first: {:?}",
        failures.len(),
        failures.iter().take(40).collect::<Vec<_>>()
    );
}

#[test]
fn recorded_jdk_observations_match() {
    let document = parse(JDK);
    let provenance = &document["provenance"];
    let recorded = provenance["recorded_vector_count"]
        .as_u64()
        .expect("recorded count");
    let substituted = provenance["substituted_lone_surrogate_indices"]
        .as_array()
        .expect("substituted indices")
        .len();
    let kept = vectors(&document);
    assert_eq!(
        u64::try_from(kept.len()).ok(),
        Some(recorded),
        "every recorded vector is compared, with lone surrogates substituted"
    );
    assert_eq!(recorded, 111, "the recorded OpenJDK observation count");
    assert_eq!(substituted, 73, "vectors with a substituted lone surrogate");
    assert_vectors("recorded JDK observations", &kept);
}

#[test]
fn random_mixed_strings_match_the_golden() {
    let document = parse(RANDOM);
    let all = vectors(&document);
    assert_eq!(all.len(), 2000, "seeded random corpus size");
    assert_vectors("random mixed strings", &all);
}

#[test]
fn empty_and_ascii_inputs() {
    assert_eq!(root_lower(""), "");
    assert_eq!(root_lower("ABC xyz"), "abc xyz");
}

#[test]
fn sigma_examples_from_review() {
    assert_eq!(root_lower("A\u{3a3}-\u{345}"), "a\u{3c2}-\u{345}");
    assert_eq!(root_lower("\u{2160}'\u{3a3}"), "\u{2170}'\u{3c3}");
    assert_eq!(root_lower("A1\u{200d}.\u{3a3}"), "a1\u{200d}.\u{3c3}");
    assert_eq!(root_lower("1\u{345}\u{3a3}"), "1\u{345}\u{3c2}");
    assert_eq!(root_lower("\u{345}\u{3a3}"), "\u{345}\u{3c3}");
}
