//! Reading order of hand-built geometry pages, and the joining of wrapped fragments.

use jobniac_core::profiles::extract::lines::{LineDocument, LineGeometry, TextLine};

const PAGE_W: i32 = 59_500;
const PAGE_H: i32 = 84_200;

/// A 10 pt line whose width follows its text.
fn line(page: u32, text: &str, x0: i32, y0: i32) -> TextLine {
    let width = i32::try_from(text.chars().count()).unwrap_or(0) * 500;
    TextLine::new(
        page,
        0,
        text,
        Some(LineGeometry {
            x0,
            y0,
            x1: x0 + width,
            y1: y0 + 1200,
            font_size_centipoints: 1000,
            bold: false,
            italic: false,
            page_width_centipoints: PAGE_W,
            page_height_centipoints: PAGE_H,
        }),
    )
}

fn texts(document: &LineDocument) -> Vec<&str> {
    document.lines.iter().map(|l| l.text.as_str()).collect()
}

/// A Europass-style page: a sidebar at x0 2800 and a main column at x0 21000, given row by row
/// (interleaved) the way a row-wise extractor would return them.
fn sidebar_page() -> Vec<TextLine> {
    let side = [
        "Alex Sample",
        "alex@example.com",
        "(202) 555-0147",
        "Languages",
        "English",
        "German",
    ];
    let main = [
        "About me",
        "Builds reliable services.",
        "Work experience",
        "Software Engineer",
        "Example Systems Inc.",
        "Education",
    ];
    let mut lines = Vec::new();
    for (row, (left, right)) in side.iter().zip(main.iter()).enumerate() {
        let y = 5_000 + i32::try_from(row).unwrap_or(0) * 1_400;
        lines.push(line(0, left, 2_800, y));
        lines.push(line(0, right, 21_000, y));
    }
    lines
}

#[test]
fn sidebar_page_reads_sidebar_then_main() {
    let document = LineDocument::from_lines(sidebar_page());
    assert_eq!(
        texts(&document),
        vec![
            "Alex Sample",
            "alex@example.com",
            "(202) 555-0147",
            "Languages",
            "English",
            "German",
            "About me",
            "Builds reliable services.",
            "Work experience",
            "Software Engineer",
            "Example Systems Inc.",
            "Education",
        ]
    );
    assert_eq!(document.columns, vec![1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2]);
    let indexes: Vec<u32> = document.lines.iter().map(|l| l.index).collect();
    assert_eq!(indexes, (0..12).collect::<Vec<u32>>());
}

#[test]
fn a_spanning_header_comes_before_both_columns() {
    let mut lines = sidebar_page();
    lines.push(line(
        0,
        "Alex Sample - Curriculum Vitae Of A Very Long Header Line",
        2_800,
        2_000,
    ));
    lines.push(line(
        0,
        "Page footer text that spans the whole width of the page here",
        2_800,
        20_000,
    ));
    let document = LineDocument::from_lines(lines);
    let seen = texts(&document);
    assert_eq!(
        seen[0],
        "Alex Sample - Curriculum Vitae Of A Very Long Header Line"
    );
    assert_eq!(seen[1], "Alex Sample");
    assert_eq!(seen[7], "About me");
    assert_eq!(
        seen.last().copied(),
        Some("Page footer text that spans the whole width of the page here")
    );
    assert_eq!(document.columns[0], 0);
    assert_eq!(document.columns[1], 1);
    assert_eq!(document.columns[7], 2);
}

#[test]
fn two_equal_columns_read_left_then_right() {
    let mut lines = Vec::new();
    for row in 0..5 {
        let y = 5_000 + row * 1_400;
        // Ragged lines, as running text is.
        let pad = "x".repeat(usize::try_from(row).unwrap_or(0) * 3);
        lines.push(line(0, &format!("left {row}"), 4_000, y));
        lines.push(line(0, &format!("right {row} {pad}"), 31_000, y));
    }
    let document = LineDocument::from_lines(lines);
    assert_eq!(
        texts(&document),
        vec![
            "left 0",
            "left 1",
            "left 2",
            "left 3",
            "left 4",
            "right 0",
            "right 1 xxx",
            "right 2 xxxxxx",
            "right 3 xxxxxxxxx",
            "right 4 xxxxxxxxxxxx",
        ]
    );
}

#[test]
fn a_single_column_keeps_top_to_bottom_order_and_rows_left_to_right() {
    let mut lines = vec![
        line(0, "Software Engineer", 5_400, 9_000),
        line(0, "Jan 2020 - Present", 45_000, 9_000),
        line(0, "Name", 5_400, 5_000),
        line(0, "Built things.", 5_400, 10_400),
        line(0, "Second job", 5_400, 12_000),
        line(0, "Mar 2018 - Dec 2019", 44_000, 12_000),
        line(0, "Tested things.", 5_400, 13_400),
    ];
    lines.reverse();
    let document = LineDocument::from_lines(lines);
    assert_eq!(
        texts(&document),
        vec![
            "Name",
            "Software Engineer",
            "Jan 2020 - Present",
            "Built things.",
            "Second job",
            "Mar 2018 - Dec 2019",
            "Tested things.",
        ]
    );
    assert!(document.columns.iter().all(|c| *c == 0));
}

#[test]
fn pages_are_read_in_page_order() {
    let lines = vec![
        line(1, "second page", 5_400, 5_000),
        line(0, "first page, last", 5_400, 9_000),
        line(0, "first page, first", 5_400, 5_000),
    ];
    let document = LineDocument::from_lines(lines);
    assert_eq!(
        texts(&document),
        vec!["first page, first", "first page, last", "second page"]
    );
}

#[test]
fn a_page_without_geometry_keeps_input_order() {
    let mut lines = vec![line(0, "b", 5_400, 9_000), line(0, "a", 5_400, 5_000)];
    lines.push(TextLine::new(0, 0, "c", None));
    let document = LineDocument::from_lines(lines);
    assert_eq!(texts(&document), vec!["b", "a", "c"]);
}

#[test]
fn a_wrapped_url_is_rejoined() {
    let lines = vec![
        line(0, "LinkedIn", 5_400, 5_000),
        line(0, "https://example.com/alex-sa", 5_400, 6_400),
        line(0, "mple", 5_400, 7_800),
        line(0, "Skills", 5_400, 9_200),
    ];
    let document = LineDocument::from_lines(lines);
    assert_eq!(
        texts(&document),
        vec!["LinkedIn", "https://example.com/alex-sample", "Skills"]
    );
    let joined = document.lines[1].geometry;
    assert_eq!(joined.map(|g| (g.y0, g.y1)), Some((6_400, 9_000)));

    let plain = LineDocument::from_text("https://example.com/alex-sa\nmple\nSkills");
    assert_eq!(
        texts(&plain),
        vec!["https://example.com/alex-sample", "Skills"]
    );
}

#[test]
fn a_url_is_not_joined_to_a_heading_or_a_sentence() {
    for next in [
        "Skills",
        "skills",
        "Rust",
        "next paragraph here",
        "https://example.org/b",
    ] {
        let text = format!("https://example.com/alex-sample\n{next}");
        let document = LineDocument::from_text(&text);
        assert_eq!(document.lines.len(), 2, "{next}");
    }
    let short = LineDocument::from_text("https://x.example/a\nmple");
    assert_eq!(short.lines.len(), 2);
}

#[test]
fn a_wrapped_address_is_rejoined_only_when_it_is_clearly_one_field() {
    let plain =
        LineDocument::from_text("10 Example Street,\nSpringfield, IL 62701\nalex@example.com");
    assert_eq!(
        texts(&plain),
        vec![
            "10 Example Street, Springfield, IL 62701",
            "alex@example.com"
        ]
    );
    let german = LineDocument::from_text("Beispielstraße 12,\n10115 Berlin");
    assert_eq!(texts(&german), vec!["Beispielstraße 12, 10115 Berlin"]);
    // No trailing comma: two address lines of a letter block stay two lines.
    let block = LineDocument::from_text("10 Example Street\nSpringfield, IL 62701");
    assert_eq!(block.lines.len(), 2);
    // A salutation that ends in a comma is not an address.
    let salutation = LineDocument::from_text("Dear Ms. Example,\n10115 Berlin office");
    assert_eq!(salutation.lines.len(), 2);

    let far = vec![
        line(0, "10 Example Street,", 5_400, 5_000),
        line(0, "Springfield, IL 62701", 5_400, 9_000),
    ];
    assert_eq!(LineDocument::from_lines(far).lines.len(), 2);
    let near = vec![
        line(0, "10 Example Street,", 5_400, 5_000),
        line(0, "Springfield, IL 62701", 5_400, 6_400),
    ];
    assert_eq!(
        texts(&LineDocument::from_lines(near)),
        vec!["10 Example Street, Springfield, IL 62701"]
    );
}

#[test]
fn a_right_sidebar_is_read_after_the_main_column() {
    // Sidebar at 73 % of the page width, main column from 5 %.
    let side = ["Contact", "alex@example.com", "Skills", "Rust", "Go", "SQL"];
    let main = [
        "Summary",
        "Builds reliable services for small teams.",
        "Experience",
        "Software Engineer at Example Systems",
        "Education",
        "B.S. in Computer Science",
    ];
    let mut lines = Vec::new();
    for (row, (right, left)) in side.iter().zip(main.iter()).enumerate() {
        let y = 5_000 + i32::try_from(row).unwrap_or(0) * 1_400;
        lines.push(line(0, left, 3_000, y));
        lines.push(line(0, right, 43_500, y));
    }
    let document = LineDocument::from_lines(lines);
    let seen = texts(&document);
    assert_eq!(seen[..6], main);
    assert_eq!(seen[6..], side);
    assert_eq!(document.columns, vec![1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2]);
}

#[test]
fn an_unrelated_url_neighbour_is_never_joined() {
    // A URL followed by a domain, a phone number, a year or a date.
    let cases = [
        "https://example.com/ada-lovelace\ngithub.com/example-ada\n202-555-0147",
        "https://example.com/ada-lovelace\n202-555-0147",
        "https://example.com/ada-lovelace\n2019",
        "https://example.com/ada-lovelace\n\nmple",
        "https://example.com/ada-lovelace\nmple.org/x",
        "https://example.com/ada-lovelace\nada@example.org",
        "https://example.com/ada-lovelace\nmplesomething-long",
        "LinkedIn: https://example.com/ada-lovelace\nmple",
    ];
    for text in cases {
        let lines = text.split('\n').filter(|l| !l.is_empty()).count();
        let document = LineDocument::from_text(text);
        assert_eq!(document.lines.len(), lines, "{text}");
    }
    // With geometry: a tail that is a domain, or a URL that does not reach the right margin.
    let domain = vec![
        line(0, "https://example.com/ada-lovelace", 5_400, 5_000),
        line(0, "github.com/example-ada", 5_400, 6_400),
    ];
    assert_eq!(LineDocument::from_lines(domain).lines.len(), 2);
    let short = vec![
        line(0, "https://example.com/ada", 5_400, 5_000),
        line(0, "mple", 5_400, 6_400),
        line(
            0,
            "A much longer line that sets the right margin of this column wide open",
            5_400,
            7_800,
        ),
    ];
    assert_eq!(LineDocument::from_lines(short).lines.len(), 3);
    let indented = vec![
        line(0, "https://example.com/ada-lovelace", 5_400, 5_000),
        line(0, "mple", 12_000, 6_400),
    ];
    assert_eq!(LineDocument::from_lines(indented).lines.len(), 2);
    let after_gap = vec![
        line(0, "https://example.com/ada-lovelace", 5_400, 5_000),
        line(0, "mple", 5_400, 9_000),
    ];
    assert_eq!(LineDocument::from_lines(after_gap).lines.len(), 2);
}

#[test]
fn extreme_coordinates_do_not_overflow() {
    let extreme = |text: &str, x0: i32, y0: i32| {
        TextLine::new(
            0,
            0,
            text,
            Some(LineGeometry {
                x0,
                y0,
                x1: i32::MAX,
                y1: i32::MAX,
                font_size_centipoints: i32::MAX,
                bold: false,
                italic: true,
                page_width_centipoints: i32::MAX,
                page_height_centipoints: i32::MAX,
            }),
        )
    };
    let lines = (0..10)
        .map(|n| {
            extreme(
                &format!("line {n}"),
                if n % 2 == 0 { i32::MIN } else { i32::MAX / 2 },
                i32::MIN + n,
            )
        })
        .collect::<Vec<_>>();
    let document = LineDocument::from_lines(lines);
    let rows = jobniac_core::profiles::extract::lines::line_features(&document);
    assert_eq!(rows.len(), 10);
    assert!(rows.iter().all(|row| row.iter().all(|v| !v.is_nan())));
}
