//! PDF layout parent side without PDFium: the grouping of characters into line fragments, the
//! font-style reader, and the validation of the child's response.
//!
//! The end-to-end runs of the PDFium child are in `crates/cli/tests/pdf_layout.rs`.

use std::error::Error;

use jobniac_core::documents::model::{
    LAYOUT_BOUNDS_TOLERANCE, LayoutLine, MAX_CHARACTERS_LIMIT, MAX_LAYOUT_FONT_SIZE,
    MAX_LAYOUT_LINE_CHARS, MAX_LAYOUT_LINES, MAX_PAGE_COUNT, PageLayout, PdfLayout,
};
use jobniac_infra::workers::pdf::{
    OPERATION_LAYOUT, PdfError, PdfOperation, decode_request, encode_page_limit_response,
    encode_request,
};
use jobniac_infra::workers::pdf_layout::{
    FontStyle, LayoutBudget, LayoutChar, centipoints_within, decode_layout_response,
    encode_layout_response, group_page_chars, is_bold_font, sfnt_style,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

/// A 10 pt character at `x` (points) on the baseline `y`, 5 pt wide.
fn glyph(character: char, x: f32, y: f32) -> LayoutChar {
    sized(character, x, y, 10.0)
}

fn sized(character: char, x: f32, y: f32, size: f32) -> LayoutChar {
    LayoutChar {
        character,
        generated: false,
        left: x,
        right: x + size / 2.0,
        bottom: y - size * 0.2,
        top: y + size * 0.8,
        baseline: y,
        font_size: size,
        bold: false,
        italic: false,
    }
}

/// The characters of `text` laid out from `x` at `y`, 5 pt apart (a space takes a slot).
fn run(text: &str, x: f32, y: f32) -> Vec<LayoutChar> {
    text.chars()
        .enumerate()
        .map(|(index, character)| {
            let mut value = glyph(character, x + index as f32 * 5.0, y);
            value.generated = false;
            value
        })
        .collect()
}

fn group(chars: &[LayoutChar]) -> Vec<LayoutLine> {
    group_page_chars(chars, 60_000, 80_000, &mut LayoutBudget::new())
}

fn texts(lines: &[LayoutLine]) -> Vec<&str> {
    lines.iter().map(|line| line.text.as_str()).collect()
}

#[test]
fn characters_on_one_baseline_become_one_fragment_with_a_top_left_box() {
    let lines = group(&run("Hello world", 100.0, 700.0));
    assert_eq!(texts(&lines), ["Hello world"]);
    let line = &lines[0];
    assert_eq!((line.x0, line.x1), (10_000, 15_500));
    // The page is 800 pt high: the top of the glyphs is 8 pt above the baseline, the bottom 2 pt
    // below it.
    assert_eq!((line.y0, line.y1), (9_200, 10_200));
    assert_eq!(line.font_size_centipoints, 1_000);
    assert!(!line.bold && !line.italic);
}

#[test]
fn a_baseline_change_beyond_half_the_font_size_starts_a_new_fragment() {
    let mut chars = run("first", 100.0, 700.0);
    chars.extend(run("second", 100.0, 686.0));
    // A superscript within half a font size stays on the line.
    let mut raised = run("ab", 100.0, 700.0);
    raised.push(glyph('2', 110.0, 704.0));
    assert_eq!(texts(&group(&chars)), ["first", "second"]);
    assert_eq!(texts(&group(&raised)), ["ab2"]);
}

#[test]
fn a_horizontal_gap_over_one_and_a_half_font_sizes_starts_a_new_fragment() {
    // Two columns on one baseline: the second starts 40 pt after the first ends.
    let mut chars = run("left", 100.0, 700.0);
    chars.extend(run("right", 160.0, 700.0));
    let lines = group(&chars);
    assert_eq!(texts(&lines), ["left", "right"]);
    assert!(lines[0].x1 < lines[1].x0);
    // A 12 pt gap (1.2 font sizes) stays one fragment and gets a space.
    let mut near = run("left", 100.0, 700.0);
    near.extend(run("right", 132.0, 700.0));
    assert_eq!(texts(&group(&near)), ["left right"]);
}

#[test]
fn a_character_far_behind_the_previous_one_starts_a_new_fragment() {
    let mut chars = run("abcdef", 100.0, 700.0);
    chars.extend(run("x", 100.0, 700.0));
    assert_eq!(texts(&group(&chars)), ["abcdef", "x"]);
}

#[test]
fn spaces_generated_breaks_and_wide_gaps_separate_words() {
    let mut chars = run("one", 100.0, 700.0);
    let mut space = glyph(' ', 0.0, 0.0);
    space.generated = true;
    chars.push(space);
    chars.extend(run("two", 117.0, 700.0));
    // A generated line break inside the baseline band is only a separator.
    let mut newline = glyph('\n', 0.0, 0.0);
    newline.generated = true;
    chars.push(newline);
    chars.extend(run("three", 140.0, 700.0));
    // No space glyph at all, but a gap of 4 pt (0.4 em) between the words.
    chars.extend(run("four", 169.0, 700.0));
    assert_eq!(texts(&group(&chars)), ["one two three four"]);
    // Whitespace of any kind collapses to one space, and a leading or trailing one is dropped.
    let mut padded = vec![glyph(' ', 95.0, 700.0), glyph('\u{a0}', 100.0, 700.0)];
    padded.extend(run("a", 105.0, 700.0));
    padded.extend([glyph('\t', 110.0, 700.0), glyph(' ', 115.0, 700.0)]);
    padded.extend(run("b", 120.0, 700.0));
    padded.push(glyph(' ', 125.0, 700.0));
    assert_eq!(texts(&group(&padded)), ["a b"]);
}

#[test]
fn unusable_characters_are_ignored() {
    let mut nan = glyph('x', 100.0, 700.0);
    nan.left = f32::NAN;
    let mut zero = glyph('y', 105.0, 700.0);
    zero.font_size = 0.0;
    let mut control = glyph('\u{1}', 110.0, 700.0);
    control.generated = false;
    let mut generated = glyph('-', 115.0, 700.0);
    generated.generated = true;
    let mut chars = vec![nan, zero, control, generated];
    chars.extend(run("ok", 120.0, 700.0));
    assert_eq!(texts(&group(&chars)), ["ok"]);
    assert!(group(&[glyph(' ', 0.0, 0.0)]).is_empty());
}

#[test]
fn a_change_of_bold_or_italic_starts_a_new_fragment_and_the_flags_are_kept() {
    let mut label = run("Label:", 100.0, 700.0);
    label.iter_mut().for_each(|value| value.bold = true);
    let mut value = run("value", 135.0, 700.0);
    value.iter_mut().for_each(|part| part.italic = true);
    label.extend(value);
    let lines = group(&label);
    assert_eq!(texts(&lines), ["Label:", "value"]);
    assert_eq!((lines[0].bold, lines[0].italic), (true, false));
    assert_eq!((lines[1].bold, lines[1].italic), (false, true));
}

#[test]
fn the_font_size_is_the_lower_median_of_the_characters() {
    let mut chars: Vec<LayoutChar> = Vec::new();
    for (index, size) in [10.0_f32, 10.0, 12.0, 20.0].into_iter().enumerate() {
        chars.push(sized('a', 100.0 + index as f32 * 5.0, 700.0, size));
    }
    // Baselines agree, sizes differ: the median of 10, 10, 12, 20 is its lower middle, 10.
    assert_eq!(group(&chars)[0].font_size_centipoints, 1_000);
}

#[test]
fn coordinates_are_clamped_into_the_page() {
    let chars = vec![
        sized('a', -20.0, 805.0, 10.0),
        sized('b', 595.0, -3.0, 10.0),
    ];
    let lines = group_page_chars(&chars, 59_500, 80_000, &mut LayoutBudget::new());
    for line in &lines {
        assert!(line.x0 <= line.x1 && line.y0 <= line.y1);
        assert!(line.x1 <= 59_500 && line.y1 <= 80_000);
    }
    assert_eq!(centipoints_within(f32::NAN, 10), 0);
    assert_eq!(centipoints_within(-4.0, 10), 0);
    assert_eq!(centipoints_within(1e9, 10), 10);
    assert_eq!(centipoints_within(0.125, 1_000), 13);
    assert_eq!(centipoints_within(2.5, 1_000), 250);
}

#[test]
fn a_fragment_is_cut_at_the_character_bound_and_the_rest_of_it_is_dropped() {
    let long = "a".repeat(MAX_LAYOUT_LINE_CHARS + 50);
    let mut chars: Vec<LayoutChar> = long
        .chars()
        .enumerate()
        .map(|(index, character)| glyph(character, index as f32 * 5.0, 700.0))
        .collect();
    chars.extend(run("next", 0.0, 650.0));
    let mut budget = LayoutBudget::new();
    let lines = group_page_chars(&chars, 600_000, 80_000, &mut budget);
    assert_eq!(lines[0].text.chars().count(), MAX_LAYOUT_LINE_CHARS);
    assert_eq!(texts(&lines)[1], "next");
    assert!(budget.truncated);
    assert!(!budget.stopped, "the next fragment is still read");
}

#[test]
fn the_fragment_budget_stops_the_page_and_flags_truncation() {
    let mut chars = Vec::new();
    for index in 0..5 {
        chars.extend(run("row", 100.0, 700.0 - index as f32 * 20.0));
    }
    let mut budget = LayoutBudget {
        lines_left: 3,
        ..LayoutBudget::new()
    };
    let lines = group_page_chars(&chars, 60_000, 80_000, &mut budget);
    assert_eq!(lines.len(), 3);
    assert!(budget.truncated && budget.stopped);
    // Exactly as many fragments as the budget is not a cut.
    let mut exact = LayoutBudget {
        lines_left: 5,
        ..LayoutBudget::new()
    };
    assert_eq!(
        group_page_chars(&chars, 60_000, 80_000, &mut exact).len(),
        5
    );
    assert!(!exact.truncated && !exact.stopped);
}

#[test]
fn the_character_budget_cuts_the_text_and_stops() {
    let mut chars = run("abcdefgh", 100.0, 700.0);
    chars.extend(run("later", 100.0, 600.0));
    let mut budget = LayoutBudget {
        chars_left: 5,
        ..LayoutBudget::new()
    };
    let lines = group_page_chars(&chars, 60_000, 80_000, &mut budget);
    assert_eq!(texts(&lines), ["abcde"]);
    assert_eq!(budget.chars_left, 0);
    assert!(budget.truncated && budget.stopped);
}

#[test]
fn grouping_is_deterministic() {
    let mut chars = run("some text", 100.0, 700.0);
    chars.extend(run("other", 300.0, 700.0));
    assert_eq!(group(&chars), group(&chars));
}

#[test]
fn bold_comes_from_the_weight_the_flag_or_the_name() {
    assert!(is_bold_font(Some(600), false, "Open Sans"));
    assert!(is_bold_font(Some(700), false, ""));
    assert!(!is_bold_font(Some(400), false, "Open Sans"));
    assert!(!is_bold_font(Some(0), false, "Open Sans"));
    assert!(!is_bold_font(None, false, "Open Sans"));
    assert!(is_bold_font(None, true, "Open Sans"));
    for name in [
        "ABCDEF+Arial-BoldMT",
        "Inter-Black",
        "Roboto-HEAVY",
        "Calibri-Semibold",
    ] {
        assert!(is_bold_font(None, false, name), "{name}");
    }
    assert!(!is_bold_font(None, false, "Helvetica-Oblique"));
}

/// A minimal sfnt with the given `OS/2` weight class and `fsSelection`, and `head.macStyle`.
fn sfnt(weight: u16, selection: u16, mac_style: u16, tag: [u8; 4]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&tag);
    data.extend_from_slice(&2_u16.to_be_bytes());
    data.extend_from_slice(&[0; 6]);
    let os2_offset = 12 + 2 * 16;
    let head_offset = os2_offset + 64;
    for (name, offset, length) in [(b"OS/2", os2_offset, 64_u32), (b"head", head_offset, 54)] {
        data.extend_from_slice(name);
        data.extend_from_slice(&0_u32.to_be_bytes());
        data.extend_from_slice(&u32::try_from(offset).unwrap_or(0).to_be_bytes());
        data.extend_from_slice(&length.to_be_bytes());
    }
    let mut os2 = vec![0_u8; 64];
    os2[4..6].copy_from_slice(&weight.to_be_bytes());
    os2[62..64].copy_from_slice(&selection.to_be_bytes());
    data.extend_from_slice(&os2);
    let mut head = vec![0_u8; 54];
    head[44..46].copy_from_slice(&mac_style.to_be_bytes());
    data.extend_from_slice(&head);
    data
}

#[test]
fn the_font_program_declares_weight_bold_and_italic() {
    let tag = [0, 1, 0, 0];
    assert_eq!(
        sfnt_style(&sfnt(700, 0, 0, tag)),
        Some(FontStyle {
            weight: Some(700),
            bold: true,
            italic: false
        })
    );
    // The weight class alone decides: 600 and up is bold, 599 and down is not.
    for (weight, bold) in [
        (100, false),
        (400, false),
        (599, false),
        (600, true),
        (900, true),
    ] {
        assert_eq!(
            sfnt_style(&sfnt(weight, 0, 0, tag)).map(|style| style.bold),
            Some(bold),
            "{weight}"
        );
    }
    assert_eq!(
        sfnt_style(&sfnt(400, 0x20, 0, tag)).map(|style| style.bold),
        Some(true)
    );
    assert_eq!(
        sfnt_style(&sfnt(400, 0, 0x03, *b"OTTO")).map(|style| (style.bold, style.italic)),
        Some((true, true))
    );
    assert_eq!(
        sfnt_style(&sfnt(400, 0x01, 0, tag)).map(|style| style.italic),
        Some(true)
    );
    // Not an sfnt, truncated, or a table directory that points outside the data.
    assert_eq!(sfnt_style(b"%!PS-AdobeFont"), None);
    assert_eq!(sfnt_style(&[]), None);
    let whole = sfnt(700, 0, 0, tag);
    assert_eq!(sfnt_style(&whole[..20]), None);
    assert_eq!(
        sfnt_style(&whole[..100]).map(|style| style.weight),
        Some(Some(700))
    );
}

/// An sfnt with the given table records `(tag, offset, length)` followed by `body`.
fn sfnt_with(tag: [u8; 4], declared: u16, records: &[([u8; 4], u32, u32)], body: &[u8]) -> Vec<u8> {
    let mut data = tag.to_vec();
    data.extend_from_slice(&declared.to_be_bytes());
    data.extend_from_slice(&[0; 6]);
    for (name, offset, length) in records {
        data.extend_from_slice(name);
        data.extend_from_slice(&0_u32.to_be_bytes());
        data.extend_from_slice(&offset.to_be_bytes());
        data.extend_from_slice(&length.to_be_bytes());
    }
    data.extend_from_slice(body);
    data
}

#[test]
fn a_hostile_table_directory_gives_nothing_and_never_panics() {
    let tag = [0, 1, 0, 0];
    // Offsets that point past the data, at its very end, and far outside any address space.
    for offset in [
        u32::MAX,
        u32::MAX - 1,
        1_000_000,
        12 + 32 + 64,
        12 + 32 + 64 + 1,
    ] {
        let data = sfnt_with(
            tag,
            2,
            &[(*b"OS/2", offset, 64), (*b"head", offset, 54)],
            &[0; 64],
        );
        assert_eq!(sfnt_style(&data), None, "offset {offset}");
    }
    // The table count is far beyond the buffer.
    assert_eq!(sfnt_style(&sfnt_with(tag, 0xFFFF, &[], &[])), None);
    assert_eq!(
        sfnt_style(&sfnt_with(tag, 0xFFFF, &[(*b"cmap", 0, 0)], &[0; 8])),
        None
    );
    // No tables at all declares nothing.
    assert_eq!(sfnt_style(&sfnt_with(tag, 0, &[], &[])), None);
    // The directory is cut off inside a record, at every length.
    let whole = sfnt(700, 0x20, 0x01, tag);
    for length in 0..(12 + 2 * 16) {
        assert_eq!(sfnt_style(&whole[..length]), None, "cut at {length}");
    }
    // A table length that overflows when added to a field offset is not trusted.
    let data = sfnt_with(tag, 1, &[(*b"OS/2", 28, u32::MAX)], &[0xFF; 64]);
    assert!(sfnt_style(&data).is_some_and(|style| style.bold));
}

#[test]
fn a_table_shorter_than_the_field_does_not_read_the_next_table() {
    let tag = [0, 1, 0, 0];
    // `OS/2` declares 5 bytes: the weight class (bytes 4 and 5) is not covered, and the bytes that
    // follow (the next table) say bold. Nothing may be read.
    let mut body = vec![0_u8; 5];
    body.extend_from_slice(&[0x02, 0xBC]);
    body.extend_from_slice(&[0xFF; 80]);
    let data = sfnt_with(tag, 1, &[(*b"OS/2", 28, 5)], &body);
    assert_eq!(sfnt_style(&data), None);
    // The weight is covered but `fsSelection` is not: bold only from the weight class.
    let mut body = vec![0_u8; 64];
    body[4..6].copy_from_slice(&400_u16.to_be_bytes());
    body[62..64].copy_from_slice(&0xFFFF_u16.to_be_bytes());
    let data = sfnt_with(tag, 1, &[(*b"OS/2", 28, 62)], &body);
    assert_eq!(
        sfnt_style(&data),
        Some(FontStyle {
            weight: Some(400),
            bold: false,
            italic: false
        })
    );
    // `head` shorter than `macStyle` declares nothing.
    let data = sfnt_with(tag, 1, &[(*b"head", 28, 45)], &[0xFF; 64]);
    assert_eq!(sfnt_style(&data), None);
}

#[test]
fn duplicate_tables_and_collections_follow_the_first_font() {
    let tag = [0, 1, 0, 0];
    let mut body = vec![0_u8; 64];
    body[4..6].copy_from_slice(&400_u16.to_be_bytes());
    let mut bold = vec![0_u8; 64];
    bold[4..6].copy_from_slice(&900_u16.to_be_bytes());
    body.extend_from_slice(&bold);
    // Two `OS/2` entries: the first, regular one counts.
    let data = sfnt_with(
        tag,
        2,
        &[(*b"OS/2", 12 + 32, 64), (*b"OS/2", 12 + 32 + 64, 64)],
        &body,
    );
    assert_eq!(
        sfnt_style(&data).map(|style| (style.weight, style.bold)),
        Some((Some(400), false))
    );
    // A TrueType collection is not read at all.
    assert_eq!(
        sfnt_style(&sfnt_with(*b"ttcf", 1, &[(*b"OS/2", 28, 64)], &[0xFF; 64])),
        None
    );
}

// The parent-side decoder of the `pdf-layout` response.

fn sample_layout() -> PdfLayout {
    let line = |text: &str, x0, y0, x1, y1, bold| LayoutLine {
        text: text.to_owned(),
        x0,
        y0,
        x1,
        y1,
        font_size_centipoints: 1_000,
        bold,
        italic: false,
    };
    PdfLayout {
        page_count: 2,
        pages: vec![
            PageLayout {
                page_number: 1,
                width_centipoints: 59_500,
                height_centipoints: 84_200,
                lines: vec![
                    line("Alex Sample", 1_400, 3_400, 14_000, 5_500, true),
                    line("About me", 20_200, 7_000, 27_000, 8_900, false),
                ],
            },
            PageLayout {
                page_number: 2,
                width_centipoints: 61_200,
                height_centipoints: 79_200,
                lines: Vec::new(),
            },
        ],
        truncated: false,
    }
}

/// The response with `edit` applied to the JSON of the sample layout.
fn body_with(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut body: Value =
        serde_json::from_slice(&encode_layout_response(&sample_layout())).expect("json");
    edit(&mut body["result"]);
    body.to_string().into_bytes()
}

/// Removes a member of a JSON object.
fn remove(value: &mut Value, key: &str) {
    if let Some(map) = value.as_object_mut() {
        map.remove(key);
    }
}

fn refused(body: &[u8]) {
    assert_eq!(decode_layout_response(body), Err(PdfError::Layout));
}

#[test]
fn the_layout_response_round_trips_and_reports_the_page_limit() {
    let layout = sample_layout();
    assert_eq!(
        decode_layout_response(&encode_layout_response(&layout)),
        Ok(layout)
    );
    assert_eq!(
        decode_layout_response(&encode_page_limit_response()),
        Err(PdfError::PageLimit)
    );
}

#[test]
fn the_layout_request_round_trips_with_the_layout_operation() -> TestResult {
    let directory = std::env::temp_dir();
    let body = encode_request(PdfOperation::Layout, &directory, 0, b"%PDF-x")?;
    let decoded = decode_request(&body).ok_or("request")?;
    assert_eq!(decoded.operation, PdfOperation::Layout);
    assert_eq!(decoded.content, b"%PDF-x");
    assert_eq!(body[0], OPERATION_LAYOUT);
    assert_eq!(PdfError::Layout.message(), "PDF layout could not be read");
    assert_eq!(
        PdfError::Cancelled {
            operation: PdfOperation::Layout
        }
        .message(),
        PdfError::Layout.message()
    );
    Ok(())
}

#[test]
fn the_decoder_refuses_malformed_bodies_and_pages() {
    refused(b"not json");
    refused(b"{}");
    refused(br#"{"result":[]}"#);
    refused(br#"{"error":"other"}"#);
    // Page count and page list.
    refused(&body_with(|result| result["pageCount"] = json!(3)));
    refused(&body_with(|result| result["pageCount"] = json!(0)));
    refused(&body_with(|result| result["pageCount"] = json!(-1)));
    refused(&body_with(|result| result["pageCount"] = json!(2.0)));
    refused(&body_with(|result| result["pages"] = json!("x")));
    refused(&body_with(|result| result["pages"] = json!([1, 2])));
    refused(&body_with(|result| result["truncated"] = json!(1)));
    refused(&body_with(|result| remove(result, "truncated")));
    refused(&body_with(|result| result["pages"][1]["page"] = json!(1)));
    refused(&body_with(|result| result["pages"][0]["page"] = json!(2)));
    refused(&body_with(|result| result["pages"][0]["width"] = json!(0)));
    refused(&body_with(|result| {
        result["pages"][0]["height"] = json!(-1)
    }));
    refused(&body_with(|result| {
        result["pages"][0]["width"] = json!(4_294_967_296_u64);
    }));
    refused(&body_with(|result| result["pages"][0]["lines"] = json!({})));
    let pages = usize::try_from(MAX_PAGE_COUNT).unwrap_or(0) + 1;
    refused(&body_with(|result| {
        result["pageCount"] = json!(MAX_PAGE_COUNT + 1);
        result["pages"] = Value::Array(
            (1..=pages)
                .map(|number| json!({"page": number, "width": 100, "height": 100, "lines": []}))
                .collect(),
        );
    }));
}

#[test]
fn the_decoder_refuses_every_bad_line_field() {
    let line =
        |edit: &dyn Fn(&mut Value)| body_with(|result| edit(&mut result["pages"][0]["lines"][0]));
    // Text: empty, not normalized, controls, over the character bound, not a string.
    for text in [
        json!(""),
        json!(" padded"),
        json!("padded "),
        json!("two  spaces"),
        json!("tab\tinside"),
        json!("line\nbreak"),
        json!("nul\u{0}"),
        json!("bell\u{7}"),
        json!("nbsp\u{a0}inside"),
        json!("x".repeat(MAX_LAYOUT_LINE_CHARS + 1)),
        json!(5),
        Value::Null,
    ] {
        refused(&line(&|value| value["text"] = text.clone()));
    }
    // Numbers: floats, negatives, strings, missing and out of range.
    for field in ["x0", "y0", "x1", "y1", "size"] {
        for bad in [json!(1.5), json!(-1), json!("5"), Value::Null] {
            refused(&line(&|value| value[field] = bad.clone()));
        }
        refused(&line(&|value| remove(value, field)));
    }
    refused(&line(&|value| value["x1"] = json!(4_294_967_296_u64)));
    // Inverted boxes.
    refused(&line(&|value| {
        value["x0"] = json!(15_000);
        value["x1"] = json!(14_000);
    }));
    refused(&line(&|value| {
        value["y0"] = json!(6_000);
        value["y1"] = json!(5_500);
    }));
    // Outside the page by more than the tolerance (page 1 is 59,500 by 84,200).
    refused(&line(&|value| {
        value["x1"] = json!(59_500 + LAYOUT_BOUNDS_TOLERANCE + 1)
    }));
    refused(&line(&|value| {
        value["y1"] = json!(84_200 + LAYOUT_BOUNDS_TOLERANCE + 1)
    }));
    // Font size and flags.
    refused(&line(&|value| value["size"] = json!(0)));
    refused(&line(&|value| {
        value["size"] = json!(MAX_LAYOUT_FONT_SIZE + 1)
    }));
    refused(&line(&|value| value["bold"] = json!("true")));
    refused(&line(&|value| value["italic"] = Value::Null));
    refused(&line(&|value| remove(value, "bold")));
    refused(&line(&|value| *value = json!([1, 2, 3])));
    // The edge cases that are accepted.
    let edge = line(&|value| {
        value["x1"] = json!(59_500 + LAYOUT_BOUNDS_TOLERANCE);
        value["y1"] = json!(84_200 + LAYOUT_BOUNDS_TOLERANCE);
        value["size"] = json!(MAX_LAYOUT_FONT_SIZE);
        value["text"] = json!("x".repeat(MAX_LAYOUT_LINE_CHARS));
    });
    assert!(decode_layout_response(&edge).is_ok());
}

#[test]
fn the_decoder_enforces_the_document_bounds() {
    let many_lines = |count: usize, text: &str| {
        let line = json!({"text": text, "x0": 0, "y0": 0, "x1": 1, "y1": 1, "size": 1_000,
            "bold": false, "italic": false});
        body_with(|result| result["pages"][0]["lines"] = Value::Array(vec![line; count]))
    };
    assert!(decode_layout_response(&many_lines(MAX_LAYOUT_LINES, "a")).is_ok());
    refused(&many_lines(MAX_LAYOUT_LINES + 1, "a"));
    // The line bound holds across pages.
    let line = json!({"text": "a", "x0": 0, "y0": 0, "x1": 1, "y1": 1, "size": 1_000,
        "bold": false, "italic": false});
    refused(&body_with(|result| {
        result["pages"][0]["lines"] = Value::Array(vec![line.clone(); MAX_LAYOUT_LINES]);
        result["pages"][1]["lines"] = Value::Array(vec![line.clone(); 1]);
    }));
    // The character bound holds in total: 250 lines of 1,000 characters fill it exactly.
    let full = "y".repeat(MAX_LAYOUT_LINE_CHARS);
    let rows = MAX_CHARACTERS_LIMIT / MAX_LAYOUT_LINE_CHARS;
    assert!(decode_layout_response(&many_lines(rows, &full)).is_ok());
    refused(&many_lines(rows + 1, &full));
    refused(&body_with(|result| {
        let full_line = json!({"text": full, "x0": 0, "y0": 0, "x1": 1, "y1": 1, "size": 1_000,
            "bold": false, "italic": false});
        result["pages"][0]["lines"] = Value::Array(vec![full_line.clone(); rows]);
        result["pages"][1]["lines"] = Value::Array(vec![line.clone()]);
    }));
}
