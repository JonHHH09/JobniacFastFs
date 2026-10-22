//! The page-image render through the real PDF worker and the in-flight
//! cancellation of worker calls.
//!
//! The render test needs PDFium: set `JOBNIAC_PDFIUM_DIR` to the directory that holds the library
//! fetched by `scripts/fetch-pdfium.*`. Without it the test prints a skip note and passes locally;
//! in CI (`CI` set) a missing directory fails the test. The cancellation test needs no PDFium: it
//! runs the probe worker. The PDF is a synthetic fixture of the extraction corpus.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jobniac_core::documents::PdfRenderer;
use jobniac_infra::workers::{WorkerErrorKind, WorkerKind, WorkerSupervisor};
use jobniac_runtime::document_ports::{WorkerCancellation, WorkerPdfRenderer};

type TestResult = Result<(), Box<dyn Error>>;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The PDFium directory, or `None` when the render test must skip. In CI a missing directory is a
/// failure, not a skip.
fn pdfium_dir() -> Option<PathBuf> {
    match std::env::var_os("JOBNIAC_PDFIUM_DIR") {
        Some(value) => Some(PathBuf::from(value)),
        None => {
            assert!(
                std::env::var_os("CI").is_none(),
                "JOBNIAC_PDFIUM_DIR must be set in CI so that the PDFium render test runs"
            );
            eprintln!("skipped: JOBNIAC_PDFIUM_DIR is not set; the render test needs PDFium");
            None
        }
    }
}

/// A supervisor that runs the CLI binary as its worker.
fn supervisor() -> WorkerSupervisor {
    WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        std::env::vars_os().collect(),
    )
}

/// The two-page synthetic PDF of the extraction corpus.
fn two_page_pdf() -> Result<Vec<u8>, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../infra/tests/fixtures/pdf/corpus/two-pages.pdf");
    Ok(std::fs::read(path)?)
}

#[tokio::test]
async fn a_page_of_a_synthetic_pdf_renders_to_a_bounded_png() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let content = two_page_pdf()?;
    let renderer = WorkerPdfRenderer::new(
        Some(Arc::new(supervisor())),
        Some(directory),
        WorkerCancellation::new(),
    );
    let rendered = renderer.render(&content, 0).await?;
    assert_eq!(rendered.page_count, 2);
    assert!(rendered.png.starts_with(&PNG_SIGNATURE));
    assert!(rendered.width >= 1 && rendered.width <= 1600);
    assert!(rendered.height >= 1 && rendered.height <= 1600);
    assert_eq!(rendered.width.max(rendered.height), 1600);
    Ok(())
}

/// The fixture draws its text with the standard Helvetica font, which it does not embed, so the
/// confined render child must find a system font. A blank page or a fault would fail here.
#[tokio::test]
async fn text_in_a_non_embedded_standard_font_is_drawn_under_the_render_profile() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let content = two_page_pdf()?;
    let renderer = WorkerPdfRenderer::new(
        Some(Arc::new(supervisor())),
        Some(directory),
        WorkerCancellation::new(),
    );
    let rendered = renderer.render(&content, 0).await?;
    let image = image::load_from_memory_with_format(&rendered.png, image::ImageFormat::Png)?;
    let dark = image
        .to_luma8()
        .pixels()
        .filter(|pixel| pixel.0[0] < 128)
        .count();
    assert!(dark > 500, "the page is blank: {dark} dark pixels");
    Ok(())
}

#[tokio::test]
async fn the_second_page_renders_and_a_page_past_the_end_is_refused() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let content = two_page_pdf()?;
    let renderer = WorkerPdfRenderer::new(
        Some(Arc::new(supervisor())),
        Some(directory),
        WorkerCancellation::new(),
    );
    assert_eq!(renderer.render(&content, 1).await?.page_count, 2);
    assert!(renderer.render(&content, 2).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_cancel_while_the_child_runs_kills_it_and_reports_cancelled() -> TestResult {
    let cancellation = WorkerCancellation::new();
    let canceller = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        canceller.cancel_in_flight();
    });
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        supervisor().run(
            WorkerKind::Probe,
            br#"{"op":"hang"}"#,
            cancellation.signal(),
        ),
    )
    .await?;
    match outcome {
        Err(error) => assert!(matches!(error.kind(), WorkerErrorKind::Cancelled)),
        Ok(_) => return Err("a hanging child must not return a response".into()),
    }
    Ok(())
}
