//! The import-root reader and the desktop picked-file grant: a granted file is read without an
//! import root; any other path is refused; a configured root always applies. All values are
//! synthetic.

use std::error::Error;
use std::path::Path;

use jobniac_core::documents::DocumentFileReader;
use jobniac_runtime::document_ports::{ImportRootReader, PickedFileGrant};

type TestResult = Result<(), Box<dyn Error>>;

const CONTENT: &[u8] = b"%PDF-1.4 synthetic\n";

fn text(path: &Path) -> Result<&str, Box<dyn Error>> {
    Ok(path.to_str().ok_or("temporary path is not Unicode")?)
}

fn write_pdf(directory: &Path, name: &str) -> Result<String, Box<dyn Error>> {
    let path = directory.join(name);
    std::fs::write(&path, CONTENT)?;
    Ok(text(&path)?.to_owned())
}

#[tokio::test]
async fn a_granted_file_outside_any_root_is_read_when_no_root_is_set() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = write_pdf(directory.path(), "cv.pdf")?;
    let grant = PickedFileGrant::new();
    let reader = ImportRootReader::with_picked(None, grant.clone());
    grant.grant(&path);
    let snapshot = reader.read(&path).await?;
    assert_eq!(snapshot.file_name, "cv.pdf");
    assert_eq!(snapshot.content, CONTENT);
    Ok(())
}

#[tokio::test]
async fn an_empty_root_counts_as_unset() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = write_pdf(directory.path(), "cv.pdf")?;
    let grant = PickedFileGrant::new();
    let reader = ImportRootReader::with_picked(Some(String::new()), grant.clone());
    grant.grant(&path);
    assert!(reader.read(&path).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_path_that_was_not_picked_is_refused_when_no_root_is_set() -> TestResult {
    let directory = tempfile::tempdir()?;
    let picked = write_pdf(directory.path(), "picked.pdf")?;
    let other = write_pdf(directory.path(), "other.pdf")?;
    let grant = PickedFileGrant::new();
    let reader = ImportRootReader::with_picked(None, grant.clone());
    assert!(reader.read(&picked).await.is_err(), "nothing granted yet");
    grant.grant(&picked);
    assert!(reader.read(&other).await.is_err(), "another file");
    grant.revoke();
    assert!(reader.read(&picked).await.is_err(), "revoked grant");
    // A reader built without a grant (the CLI and MCP composition of tests) admits nothing.
    assert!(ImportRootReader::new(None).read(&picked).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_picked_file_outside_a_configured_root_is_still_refused() -> TestResult {
    let root = tempfile::tempdir()?;
    let elsewhere = tempfile::tempdir()?;
    let outside = write_pdf(elsewhere.path(), "cv.pdf")?;
    let inside = write_pdf(root.path(), "cv.pdf")?;
    let grant = PickedFileGrant::new();
    let reader = ImportRootReader::with_picked(Some(text(root.path())?.to_owned()), grant.clone());
    grant.grant(&outside);
    assert!(reader.read(&outside).await.is_err());
    assert!(
        reader.read(&inside).await.is_ok(),
        "the root still serves its files"
    );
    Ok(())
}

#[tokio::test]
async fn a_granted_traversal_or_relative_path_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    write_pdf(directory.path(), "cv.pdf")?;
    let grant = PickedFileGrant::new();
    let reader = ImportRootReader::with_picked(None, grant.clone());
    let traversal = format!("{}/sub/../cv.pdf", text(directory.path())?);
    for path in [traversal.as_str(), "cv.pdf"] {
        grant.grant(path);
        assert!(reader.read(path).await.is_err(), "path {path}");
    }
    Ok(())
}
