//! In-process client-server roundtrip: `initialize` and `tools/list` over the wire.

mod common;

use std::error::Error;

use serde_json::Value;

#[tokio::test]
async fn initialize_result_matches_the_golden() -> Result<(), Box<dyn Error>> {
    let client = common::connect(common::services()).await?;
    let info = client.peer_info().ok_or("no initialize result")?;
    let actual = serde_json::to_value(&*info)?;

    let expected = common::golden(include_str!("golden/initialize.json"))?;
    assert_eq!(actual, expected);
    Ok(())
}

#[tokio::test]
async fn tools_list_names_the_registered_tools() -> Result<(), Box<dyn Error>> {
    let client = common::connect(common::services()).await?;
    let tools = client.list_all_tools().await?;
    let names: Vec<Value> = tools
        .iter()
        .map(|tool| Value::String(tool.name.to_string()))
        .collect();
    let expected = [
        "health",
        "list_profiles",
        "get_profile",
        "create_profile",
        "update_profile",
        "update_profile_project",
        "search_profiles",
        "search_jobs",
        "update_job",
        "list_jobs",
        "get_job",
        "add_job_from_text",
        "analyze_job_match",
        "analyze_all_job_matches",
        "get_match_report",
        "list_match_reports",
        "submit_match_review",
        "get_match_review",
        "list_match_reviews",
        "list_match_disagreements",
        "acknowledge_match_disagreement",
        "link_match_disagreement",
        "store_document_file",
        "get_document_metadata",
        "extract_pdf_text",
        "extract_stored_pdf_text",
        "ingest_profile_from_stored_pdf",
        "get_profile_pdf_source",
        "generate_pdf_file",
        "generate_pdf_resume",
        "generate_canadian_pdf_resume",
        "generate_canadian_french_pdf_resume",
        "generate_german_tailored_resume",
        "generate_german_cover_letter",
    ];
    assert_eq!(
        names,
        expected
            .iter()
            .map(|name| Value::String((*name).to_owned()))
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[tokio::test]
async fn prompts_and_resources_lists_are_empty() -> Result<(), Box<dyn Error>> {
    let client = common::connect(common::services()).await?;
    assert!(client.list_all_prompts().await?.is_empty());
    assert!(client.list_all_resources().await?.is_empty());
    Ok(())
}
