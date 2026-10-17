//! Structural readback of a rendered PDF with `lopdf`.
//!
//! The checks are: strict parse, no unsafe keys in the catalog or on a page,
//! page count, MediaBox, a `/Resources` dictionary whose `/Font` holds exactly `FBody` and
//! `FBold` as indirect Type1 Standard-14 fonts with WinAnsi encoding, and page contents.

use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};

/// Keys that must never appear in the catalog or on a page.
const UNSAFE_KEYS: [&[u8]; 11] = [
    b"A",
    b"AA",
    b"OpenAction",
    b"JS",
    b"JavaScript",
    b"Names",
    b"EmbeddedFiles",
    b"Annots",
    b"AF",
    b"AcroForm",
    b"XFA",
];

/// The expected font resources and their base fonts.
const EXPECTED_FONTS: [(&[u8], &[u8]); 2] =
    [(b"FBody", b"Helvetica"), (b"FBold", b"Helvetica-Bold")];

/// A structural check that failed. The text is fixed and names no document content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadbackError(pub &'static str);

impl std::fmt::Display for ReadbackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for ReadbackError {}

fn unexpected(reason: &'static str) -> ReadbackError {
    ReadbackError(reason)
}

/// Checks a rendered document and returns its page count.
///
/// `page_width` and `page_height` give the expected MediaBox. Exact comparison is used for
/// 612 by 792 points, and a tolerance of 1e-6 otherwise.
///
/// # Errors
///
/// Returns a [`ReadbackError`] for the first failed check.
pub fn verify_structure(
    data: &[u8],
    expected_pages: usize,
    page_width: f64,
    page_height: f64,
) -> Result<usize, ReadbackError> {
    let options = LoadOptions {
        strict: true,
        ..LoadOptions::default()
    };
    let document = Document::load_mem_with_options(data, options)
        .map_err(|_| unexpected("PDF could not be parsed strictly"))?;

    let catalog = document
        .catalog()
        .map_err(|_| unexpected("PDF catalog is missing"))?;
    if has_unsafe_key(catalog) {
        return Err(unexpected("Unexpected PDF structure"));
    }

    let pages = document.get_pages();
    if pages.len() != expected_pages {
        return Err(unexpected("Unexpected PDF structure"));
    }
    let tolerance = if (page_width, page_height) == (612.0, 792.0) {
        0.0
    } else {
        0.000_001
    };
    for page_id in pages.values() {
        verify_page(&document, *page_id, page_width, page_height, tolerance)?;
    }
    Ok(pages.len())
}

fn has_unsafe_key(dictionary: &Dictionary) -> bool {
    dictionary
        .iter()
        .any(|(key, _)| UNSAFE_KEYS.contains(&key.as_slice()))
}

fn verify_page(
    document: &Document,
    page_id: ObjectId,
    page_width: f64,
    page_height: f64,
    tolerance: f64,
) -> Result<(), ReadbackError> {
    let page = document
        .get_dictionary(page_id)
        .map_err(|_| unexpected("Unexpected PDF page"))?;
    if has_unsafe_key(page) {
        return Err(unexpected("Unexpected PDF page"));
    }
    verify_media_box(document, page_id, page_width, page_height, tolerance)?;

    let resources = deref_dictionary(document, page.get(b"Resources"))
        .ok_or_else(|| unexpected("Unexpected PDF resources"))?;
    let fonts = deref_dictionary(document, resources.get(b"Font"))
        .ok_or_else(|| unexpected("Unexpected PDF fonts"))?;
    if fonts.len() != EXPECTED_FONTS.len()
        || !page.has(b"Contents")
        || EXPECTED_FONTS.iter().any(|(name, _)| !fonts.has(name))
    {
        return Err(unexpected("Unexpected PDF resources"));
    }
    for (name, base_font) in EXPECTED_FONTS {
        match fonts.get(name) {
            Ok(Object::Reference(_)) => {}
            _ => return Err(unexpected("Expected indirect font")),
        }
        let font = deref_dictionary(document, fonts.get(name))
            .ok_or_else(|| unexpected("Unexpected PDF font"))?;
        if !name_equals(font.get(b"Type").ok(), b"Font")
            || !name_equals(font.get(b"Subtype").ok(), b"Type1")
            || !name_equals(font.get(b"BaseFont").ok(), base_font)
            || !name_equals(font.get(b"Encoding").ok(), b"WinAnsiEncoding")
        {
            return Err(unexpected("Unexpected PDF font"));
        }
    }
    Ok(())
}

/// The MediaBox, inherited from the page tree when the page does not carry one.
fn verify_media_box(
    document: &Document,
    page_id: ObjectId,
    page_width: f64,
    page_height: f64,
    tolerance: f64,
) -> Result<(), ReadbackError> {
    let mut current = Some(page_id);
    let mut depth = 0;
    while let Some(id) = current {
        let dictionary = document
            .get_dictionary(id)
            .map_err(|_| unexpected("Unexpected PDF page"))?;
        if let Ok(object) = dictionary.get(b"MediaBox") {
            let (_, resolved) = document
                .dereference(object)
                .map_err(|_| unexpected("Unexpected PDF page"))?;
            let values = resolved
                .as_array()
                .map_err(|_| unexpected("Unexpected PDF page"))?;
            if values.len() != 4 {
                return Err(unexpected("Unexpected PDF page"));
            }
            let wanted = [0.0, 0.0, page_width, page_height];
            for (value, expected) in values.iter().zip(wanted) {
                let actual = match value {
                    Object::Integer(number) => *number as f64,
                    Object::Real(number) => f64::from(*number),
                    _ => return Err(unexpected("Unexpected PDF page")),
                };
                if (actual - expected).abs() > tolerance {
                    return Err(unexpected("Unexpected PDF page"));
                }
            }
            return Ok(());
        }
        depth += 1;
        current = match dictionary.get(b"Parent") {
            Ok(Object::Reference(parent)) if depth < 64 => Some(*parent),
            _ => None,
        };
    }
    Err(unexpected("Unexpected PDF page"))
}

fn deref_dictionary<'a>(
    document: &'a Document,
    object: Result<&'a Object, lopdf::Error>,
) -> Option<&'a Dictionary> {
    let object = object.ok()?;
    let (_, resolved) = document.dereference(object).ok()?;
    resolved.as_dict().ok()
}

fn name_equals(object: Option<&Object>, expected: &[u8]) -> bool {
    matches!(object, Some(Object::Name(name)) if name.as_slice() == expected)
}
