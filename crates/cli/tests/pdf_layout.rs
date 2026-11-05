//! End-to-end `pdf-layout` worker operation: the PDFium child of the built CLI binary against the
//! Europass fixtures and the synthetic corpus.
//!
//! The tests need PDFium: set `JOBNIAC_PDFIUM_DIR` to the directory that holds the library fetched
//! by `scripts/fetch-pdfium.*`. Without it each test prints a skip note and passes locally; in CI
//! (`CI` set) a missing directory fails the test instead.
//!
//! The layout of the Europass CV is recorded in `crates/infra/tests/golden/pdf-layout-europass-cv.json`
//! so that a change to the grouping shows up in review. To record it again after an intended change,
//! run the test with `JOBNIAC_BLESS_GOLDEN=1` (refused on CI; the run then fails once
//! on purpose, so the recorded file is never silently accepted).

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jobniac_core::documents::PdfLayoutReader;
use jobniac_core::documents::model::{LayoutLine, PageLayout, PdfLayout};
use jobniac_infra::workers::WorkerSupervisor;
use jobniac_infra::workers::pdf::PdfError;
use jobniac_infra::workers::pdf_layout::layout_pdf;
use jobniac_runtime::document_ports::{WorkerCancellation, WorkerPdfLayoutReader};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

#[path = "support/pdfium.rs"]
mod pdfium;

/// The fixture root shared with the parent-side tests.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../infra/tests/fixtures/pdf")
}

/// A supervisor for the built CLI binary, with the parent's environment as its source.
fn supervisor() -> Arc<WorkerSupervisor> {
    Arc::new(WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        std::env::vars_os().collect(),
    ))
}

/// The layout of one fixture through the worker.
async fn layout_of(directory: &Path, name: &str) -> Result<PdfLayout, Box<dyn Error>> {
    let content = fs::read(fixture_root().join(name))?;
    layout_pdf(&supervisor(), directory, &content, std::future::pending())
        .await
        .map_err(|error| format!("{name}: {error:?}").into())
}

/// The first line of `page` whose text is exactly `text`.
fn line<'a>(page: &'a PageLayout, text: &str) -> Result<&'a LayoutLine, Box<dyn Error>> {
    page.lines
        .iter()
        .find(|line| line.text == text)
        .ok_or_else(|| format!("no line {text:?}").into())
}

/// The recorded form of a layout: one line per fragment, so that a diff reads line by line.
fn recorded_form(layout: &PdfLayout) -> Result<String, Box<dyn Error>> {
    let mut out = String::new();
    out.push_str(&format!(
        "{{\"pageCount\":{},\"truncated\":{},\"pages\":[\n",
        layout.page_count, layout.truncated
    ));
    for (index, page) in layout.pages.iter().enumerate() {
        out.push_str(&format!(
            "{{\"page\":{},\"width\":{},\"height\":{},\"lines\":[\n",
            page.page_number, page.width_centipoints, page.height_centipoints
        ));
        for (position, line) in page.lines.iter().enumerate() {
            let value = json!({
                "text": line.text,
                "x0": line.x0,
                "y0": line.y0,
                "x1": line.x1,
                "y1": line.y1,
                "size": line.font_size_centipoints,
                "bold": line.bold,
                "italic": line.italic,
            });
            let comma = if position + 1 < page.lines.len() {
                ","
            } else {
                ""
            };
            out.push_str(&format!("{}{comma}\n", serde_json::to_string(&value)?));
        }
        let comma = if index + 1 < layout.pages.len() {
            ","
        } else {
            ""
        };
        out.push_str(&format!("]}}{comma}\n"));
    }
    out.push_str("]}\n");
    Ok(out)
}

/// The two-column Europass CV: sidebar fragments sit left of the main column, headings are larger
/// than body text, bold and dates are recognized, and y grows down the page.
#[tokio::test]
async fn the_europass_cv_layout_shows_columns_sizes_and_bold() -> TestResult {
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let layout = layout_of(&directory, "europass/europass-cv-alex-sample.pdf").await?;
    assert_eq!(layout.page_count, 1);
    assert!(!layout.truncated);
    let page = &layout.pages[0];
    assert_eq!(
        (page.width_centipoints, page.height_centipoints),
        (59_500, 84_200)
    );

    let name = line(page, "Alex Sample")?;
    let contact = line(page, "Contact")?;
    let about = line(page, "ABOUT ME")?;
    let experience = line(page, "WORK EXPERIENCE")?;
    // Sidebar versus main column.
    let sidebar_x = name.x0.max(contact.x0);
    assert!(
        sidebar_x < about.x0,
        "sidebar {sidebar_x} main {}",
        about.x0
    );
    assert!(sidebar_x < experience.x0);
    assert_eq!(
        about.x0, experience.x0,
        "the main column shares one left edge"
    );
    assert!(
        page.lines
            .iter()
            .any(|line| line.text.contains('@') && line.x0 < about.x0)
    );
    // The section heading is larger than body text.
    let body = line(page, "Software QA Engineer")?;
    assert!(
        experience.font_size_centipoints > body.font_size_centipoints,
        "heading {} body {}",
        experience.font_size_centipoints,
        body.font_size_centipoints
    );
    // Bold roles and employers, split from their regular-weight locations, and the date range as
    // its own fragment.
    assert!(body.bold, "role is bold");
    let employer = line(page, "Example Software Laboratory (fictional)")?;
    assert!(employer.bold);
    let location = line(page, "Testville, United States")?;
    assert!(!location.bold);
    assert!(
        location.x0 > employer.x1,
        "the location follows the employer on the line"
    );
    let dates = line(page, "15/03/2023 - Current")?;
    assert!(!dates.bold);
    assert!(dates.font_size_centipoints < body.font_size_centipoints);
    // Coordinates increase down the page: heading, employer, role, dates, first duty.
    let duty = line(
        page,
        "Developed Playwright regression tests for a fictional customer portal.",
    )?;
    let ys = [experience.y0, employer.y0, body.y0, dates.y0, duty.y0];
    assert!(ys.windows(2).all(|pair| pair[0] < pair[1]), "{ys:?}");
    // Every fragment is a valid box on the page.
    for line in &page.lines {
        assert!(line.x0 <= line.x1 && line.y0 <= line.y1, "{}", line.text);
        assert!(line.x1 <= page.width_centipoints && line.y1 <= page.height_centipoints);
        assert!(!line.text.is_empty() && !line.text.contains("  "));
    }
    Ok(())
}

/// The layout of the CV is recorded; a change to the grouping has to show in the golden file.
#[tokio::test]
async fn the_europass_cv_layout_matches_the_golden() -> TestResult {
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let layout = layout_of(&directory, "europass/europass-cv-alex-sample.pdf").await?;
    let actual = recorded_form(&layout)?;
    // The recorded form is itself valid JSON.
    serde_json::from_str::<Value>(&actual)?;
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../infra/tests/golden/pdf-layout-europass-cv.json");
    if std::env::var("JOBNIAC_BLESS_GOLDEN").as_deref() == Ok("1") {
        assert!(
            std::env::var_os("CI").is_none(),
            "the golden file is never recorded on CI"
        );
        fs::write(&path, &actual)?;
        panic!("golden regenerated; rerun without JOBNIAC_BLESS_GOLDEN");
    }
    assert_eq!(
        actual,
        fs::read_to_string(&path)?.replace("\r\n", "\n"),
        "the layout changed; review the lines, then record it with JOBNIAC_BLESS_GOLDEN=1"
    );
    Ok(())
}

/// The cover letter and corpus documents are read, and two runs give identical output.
#[tokio::test]
async fn other_documents_are_read_deterministically() -> TestResult {
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    for (name, pages) in [
        ("europass/europass-cover-letter-alex-sample.pdf", 1_usize),
        ("corpus/resume-simple.pdf", 1),
        ("corpus/two-pages.pdf", 2),
    ] {
        let first = layout_of(&directory, name).await?;
        let second = layout_of(&directory, name).await?;
        assert_eq!(first, second, "{name} is deterministic");
        assert_eq!(first.pages.len(), pages, "{name}");
        assert!(!first.truncated, "{name}");
        assert!(
            first.pages.iter().any(|page| !page.lines.is_empty()),
            "{name} has text"
        );
        for page in &first.pages {
            // Content order is top to bottom for these single-column documents.
            let tops: Vec<u32> = page.lines.iter().map(|line| line.y0).collect();
            assert!(
                tops.windows(2).all(|pair| pair[0] <= pair[1] + 100),
                "{name}: {tops:?}"
            );
        }
    }
    let letter = layout_of(&directory, "europass/europass-cover-letter-alex-sample.pdf").await?;
    assert!(
        letter.pages[0]
            .lines
            .iter()
            .any(|line| line.text.contains("Alex Sample"))
    );
    // A page without text has a size and no lines.
    let blank = layout_of(&directory, "corpus/blank-page.pdf").await?;
    assert_eq!(blank.page_count, 2);
    assert!(!blank.pages[0].lines.is_empty());
    assert!(blank.pages[1].lines.is_empty());
    assert_eq!(blank.pages[1].width_centipoints, 61_200);
    Ok(())
}

/// A document over the page limit and non-PDF bytes are refused like text extraction refuses them.
#[tokio::test]
async fn documents_over_the_limits_are_refused() -> TestResult {
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let content = fs::read(fixture_root().join("hostile/page-limit.pdf"))?;
    let outcome = layout_pdf(&supervisor(), &directory, &content, std::future::pending()).await;
    assert_eq!(outcome, Err(PdfError::PageLimit));
    let outcome = layout_pdf(
        &supervisor(),
        &directory,
        b"not a pdf",
        std::future::pending(),
    )
    .await;
    assert!(matches!(outcome, Err(PdfError::Validation { .. })));
    Ok(())
}

/// A one-page PDF with the standard Helvetica (not embedded), `page_extra` as extra page entries
/// and `content` as the content stream. Offsets are exact, so the structure is strict.
fn synthetic_pdf(page_extra: &str, content: &str, base_font: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R {page_extra} /Resources << /Font << /F1 4 0 R >> >> \
             /Contents 5 0 R >>"
        ),
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /{base_font} /Encoding /WinAnsiEncoding >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

async fn layout_of_bytes(directory: &Path, content: &[u8]) -> Result<PdfLayout, Box<dyn Error>> {
    layout_pdf(&supervisor(), directory, content, std::future::pending())
        .await
        .map_err(|error| format!("{error:?}").into())
}

/// A page whose CropBox or MediaBox does not start at the origin is reported in the displayed
/// frame: the box origin is subtracted, and y grows down from the top of the box.
#[tokio::test]
async fn an_offset_page_box_is_translated_to_the_displayed_page() -> TestResult {
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let content = "BT /F1 20 Tf 100 300 Td (Hello) Tj ET";
    let plain = layout_of_bytes(
        &directory,
        &synthetic_pdf("/MediaBox [0 0 612 792]", content, "Helvetica"),
    )
    .await?;
    let plain_line = &plain.pages[0].lines[0];
    assert_eq!(plain_line.text, "Hello");
    assert_eq!(plain_line.x0, 10_000);
    for extra in [
        "/MediaBox [0 0 612 792] /CropBox [50 100 350 500]",
        "/MediaBox [50 100 350 500]",
    ] {
        let layout =
            layout_of_bytes(&directory, &synthetic_pdf(extra, content, "Helvetica")).await?;
        let page = &layout.pages[0];
        assert_eq!(
            (page.width_centipoints, page.height_centipoints),
            (30_000, 40_000)
        );
        let line = &page.lines[0];
        assert_eq!(line.text, "Hello", "{extra}");
        // The text starts 50 pt right of the box edge, and its baseline lies 200 pt below the top.
        assert_eq!(line.x0, 5_000, "{extra}");
        assert!(
            line.y0 < 20_000 && (20_000..20_700).contains(&line.y1),
            "{extra}: {line:?}"
        );
        // Moving the box top from 792 to 500 pt moves every baseline up by 292 pt.
        assert_eq!(plain_line.y1 - line.y1, 29_200, "{extra}");
    }
    Ok(())
}

/// A rotated page is reported as displayed: the sizes swap and text drawn upright on screen is one
/// horizontal fragment.
#[tokio::test]
async fn a_rotated_page_is_reported_in_the_displayed_frame() -> TestResult {
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    // /Rotate 90 turns the page clockwise, so text running up the page in user space reads
    // left to right on screen.
    let content = "BT /F1 20 Tf 0 1 -1 0 100 300 Tm (Hello World) Tj ET";
    let layout = layout_of_bytes(
        &directory,
        &synthetic_pdf("/MediaBox [0 0 612 792] /Rotate 90", content, "Helvetica"),
    )
    .await?;
    let page = &layout.pages[0];
    assert_eq!(
        (page.width_centipoints, page.height_centipoints),
        (79_200, 61_200)
    );
    assert_eq!(page.lines.len(), 1, "{:?}", page.lines);
    let line = &page.lines[0];
    assert_eq!(line.text, "Hello World");
    assert_eq!(line.x0, 30_000);
    assert!(line.x1 > line.x0 + 5_000 && line.x1 <= page.width_centipoints);
    assert!(
        line.y0 < 10_000 && (10_000..10_700).contains(&line.y1),
        "{line:?}"
    );
    assert_eq!(line.font_size_centipoints, 2_000);
    Ok(())
}

/// A non-embedded font has no font program to read: nothing is bold or italic.
#[tokio::test]
async fn a_font_that_is_not_embedded_gives_no_font_program_style() -> TestResult {
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let content = "BT /F1 12 Tf 72 700 Td (Plain text) Tj ET";
    let layout = layout_of_bytes(
        &directory,
        &synthetic_pdf("/MediaBox [0 0 612 792]", content, "Helvetica"),
    )
    .await?;
    let line = &layout.pages[0].lines[0];
    assert_eq!(line.text, "Plain text");
    assert!(!line.bold && !line.italic);
    // The font name still counts for a font that says so itself.
    let bold = layout_of_bytes(
        &directory,
        &synthetic_pdf("/MediaBox [0 0 612 792]", content, "Helvetica-Bold"),
    )
    .await?;
    assert!(bold.pages[0].lines[0].bold);
    Ok(())
}

/// A page with more characters than the budget (and its slack) is cut and flagged, not fatal.
#[tokio::test]
async fn a_page_over_the_character_budget_is_truncated() -> TestResult {
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let row = "a".repeat(1_000);
    let mut content = String::from("BT /F1 1 Tf 1.5 TL 10 780 Td\n");
    for _ in 0..400 {
        content.push_str(&format!("({row}) Tj T*\n"));
    }
    content.push_str("ET");
    let started = std::time::Instant::now();
    let layout = layout_of_bytes(
        &directory,
        &synthetic_pdf("/MediaBox [0 0 612 792]", &content, "Helvetica"),
    )
    .await?;
    eprintln!("character budget run took {:?}", started.elapsed());
    assert!(layout.truncated);
    let lines = &layout.pages[0].lines;
    assert_eq!(lines.len(), 250);
    assert_eq!(
        lines.iter().map(|line| line.text.len()).sum::<usize>(),
        250_000
    );
    Ok(())
}

/// The runtime adapter reads a layout, and refuses with the fixed message when unconfigured.
#[tokio::test]
async fn the_adapter_reads_a_layout_and_refuses_when_unconfigured() -> TestResult {
    let unconfigured = WorkerPdfLayoutReader::new(None, None, WorkerCancellation::new());
    let refused = unconfigured.read_layout(b"%PDF-1.4\n").await;
    assert!(refused.is_err());
    let Some(directory) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let reader = WorkerPdfLayoutReader::new(
        Some(supervisor()),
        Some(directory),
        WorkerCancellation::new(),
    );
    let content = fs::read(fixture_root().join("europass/europass-cv-alex-sample.pdf"))?;
    let layout = reader.read_layout(&content).await?;
    assert_eq!(layout.page_count, 1);
    Ok(())
}
