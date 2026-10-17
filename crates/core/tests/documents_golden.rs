//! Replays the document golden vectors (`golden/documents.json`) against the pure document rules of
//! the core crate.

use jobniac_core::documents::{
    PageText, PdfText, extraction_view, safe_basename, validate_pdf_content,
};
use jobniac_core::errors::error_envelope;
use serde_json::{Map, Value};

const VECTORS: &str = include_str!("golden/documents.json");

fn vectors() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(VECTORS)?)
}

fn text_of(value: &Value) -> Result<&str, Box<dyn std::error::Error>> {
    value.as_str().ok_or_else(|| "expected a string".into())
}

/// Checks a Rust error against a recorded error record.
fn assert_error_matches(
    actual: &jobniac_core::errors::AppError,
    expected: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let envelope = error_envelope(actual);
    assert_eq!(envelope.code.code(), text_of(&expected["code"])?);
    assert_eq!(envelope.message, text_of(&expected["message"])?);
    let details: Map<String, Value> = envelope
        .details
        .iter()
        .map(|(key, value)| (key.clone(), Value::from(value.clone())))
        .collect();
    assert_eq!(Value::Object(details), expected["details"]);
    Ok(())
}

#[test]
fn safe_basenames_match_the_golden_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let all = vectors()?;
    for case in all["safeBasename"].as_array().ok_or("missing section")? {
        assert_eq!(
            safe_basename(text_of(&case["input"])?),
            text_of(&case["output"])?,
            "input {}",
            case["input"]
        );
    }
    Ok(())
}

#[test]
fn extraction_views_match_the_golden_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let all = vectors()?;
    for case in all["extractionView"].as_array().ok_or("missing section")? {
        let canonical = &case["canonical"];
        let pages = canonical["pages"]
            .as_array()
            .ok_or("pages")?
            .iter()
            .map(|page| {
                Ok(PageText {
                    page_number: u32::try_from(page["pageNumber"].as_u64().ok_or("number")?)?,
                    text: text_of(&page["text"])?.to_owned(),
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let canonical_text = PdfText {
            file_name: text_of(&canonical["fileName"])?.to_owned(),
            page_count: u32::try_from(canonical["pageCount"].as_u64().ok_or("count")?)?,
            character_count: canonical["characterCount"].as_i64().ok_or("chars")?,
            truncated: false,
            text: text_of(&canonical["text"])?.to_owned(),
            pages,
        };
        let budget = usize::try_from(case["maxCharacters"].as_u64().ok_or("budget")?)?;
        let include = case["includePages"].as_bool().ok_or("include")?;
        let view = extraction_view(&canonical_text, budget, include);
        let expected = &case["view"];
        assert_eq!(view.file_name, text_of(&expected["fileName"])?);
        assert_eq!(
            i64::from(view.page_count),
            expected["pageCount"].as_i64().ok_or("pc")?
        );
        assert_eq!(
            view.character_count,
            expected["characterCount"].as_i64().ok_or("cc")?
        );
        assert_eq!(view.truncated, expected["truncated"].as_bool().ok_or("tr")?);
        assert_eq!(view.text, text_of(&expected["text"])?);
        let view_pages: Vec<Value> = view
            .pages
            .iter()
            .map(|page| serde_json::json!({"pageNumber": page.page_number, "text": page.text}))
            .collect();
        assert_eq!(
            Value::Array(view_pages),
            expected["pages"],
            "budget {budget}"
        );
    }
    Ok(())
}

#[test]
fn pdf_content_checks_match_the_golden_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let all = vectors()?;
    for case in all["validatePdfContent"]
        .as_array()
        .ok_or("missing section")?
    {
        let prefix = text_of(&case["prefix"])?;
        let fill = usize::try_from(case["fill"].as_u64().ok_or("fill")?)?;
        let mut content = prefix.as_bytes().to_vec();
        content.resize(content.len() + fill, 0);
        let storage = case["storage"].as_bool().ok_or("storage")?;
        let result = validate_pdf_content(&content, storage);
        if case["result"]["ok"] == Value::Bool(true) {
            assert!(
                result.is_ok(),
                "prefix {prefix:?} fill {fill} storage {storage}"
            );
        } else {
            let error = result.err().ok_or("expected a refusal")?;
            assert_error_matches(&error, &case["result"])?;
        }
    }
    Ok(())
}

#[test]
fn request_options_match_the_golden_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let all = vectors()?;
    for case in all["options"].as_array().ok_or("missing section")? {
        let request = case["request"].as_object().ok_or("request object")?;
        let result = jobniac_core::documents::view::extraction_options(request);
        if case["result"]["ok"] == Value::Bool(true) {
            let options = result.map_err(|error| error.to_string())?;
            assert_eq!(
                i64::try_from(options.max_characters)?,
                case["result"]["maxCharacters"].as_i64().ok_or("max")?
            );
            assert_eq!(
                options.include_pages,
                case["result"]["includePages"].as_bool().ok_or("inc")?
            );
        } else {
            let error = result.err().ok_or("expected a refusal")?;
            assert_error_matches(&error, &case["result"])?;
        }
    }
    Ok(())
}
