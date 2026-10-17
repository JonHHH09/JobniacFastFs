//! `tools/list` over the wire must equal the golden entries of the implemented tools.

mod common;

use std::error::Error;

use serde_json::Value;

/// Tools checked here: `health`, the profile tools, the job tools and the matching and review
/// tools. The golden lists more.
const IMPLEMENTED: &[&str] = &[
    "health",
    "store_document_file",
    "get_document_metadata",
    "extract_pdf_text",
    "extract_stored_pdf_text",
    "ingest_profile_from_stored_pdf",
    "get_profile_pdf_source",
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
    "generate_pdf_file",
    "generate_pdf_resume",
    "generate_canadian_pdf_resume",
    "generate_canadian_french_pdf_resume",
    "generate_german_tailored_resume",
    "generate_german_cover_letter",
];

#[tokio::test]
async fn tools_list_matches_the_golden_for_implemented_tools() -> Result<(), Box<dyn Error>> {
    let client = common::connect(common::services()).await?;
    let tools = client.list_all_tools().await?;
    let actual = serde_json::to_value(&tools)?;

    let golden = common::golden(include_str!("golden/tools-list-default.json"))?;
    let expected: Vec<Value> = golden
        .as_array()
        .ok_or("tools-list-default.json is not an array")?
        .iter()
        .filter(|tool| {
            tool["name"]
                .as_str()
                .is_some_and(|name| IMPLEMENTED.contains(&name))
        })
        .cloned()
        .collect();

    assert_eq!(actual, Value::Array(expected));
    Ok(())
}

/// Each catalog configuration, the opt-in groups its flags compose, and the `tools/list`
/// golden it must reproduce (tool by tool and in registration order). The default has
/// no opt-in group, each single flag adds exactly its own tools, and `full` has all of them.
const CONFIGURATIONS: &[(&str, &[&str], &str)] = &[
    (
        "default",
        &[],
        include_str!("golden/tools-list-default.json"),
    ),
    (
        "deletion",
        &["deletion"],
        include_str!("golden/tools-list-deletion.json"),
    ),
    (
        "link",
        &["link"],
        include_str!("golden/tools-list-link.json"),
    ),
    (
        "arbeitnow",
        &["arbeitnow"],
        include_str!("golden/tools-list-arbeitnow.json"),
    ),
    (
        "artifacts",
        &["artifacts"],
        include_str!("golden/tools-list-artifacts.json"),
    ),
    (
        "full",
        &["deletion", "link", "arbeitnow", "artifacts"],
        include_str!("golden/tools-list-full.json"),
    ),
];

/// The tool names of one configuration, in listing order.
async fn listed_names(groups: &[&str]) -> Result<Vec<String>, Box<dyn Error>> {
    let client = common::connect(common::services_for(groups)).await?;
    let tools = client.list_all_tools().await?;
    Ok(tools.iter().map(|tool| tool.name.to_string()).collect())
}

#[tokio::test]
async fn every_configuration_lists_its_golden_tools_list() -> Result<(), Box<dyn Error>> {
    for (name, groups, golden) in CONFIGURATIONS {
        let client = common::connect(common::services_for(groups)).await?;
        let tools = client.list_all_tools().await?;
        let actual = serde_json::to_value(&tools)?;
        let expected = common::golden(golden)?;
        assert_eq!(actual, expected, "configuration {name}");
    }
    Ok(())
}

#[tokio::test]
async fn each_flag_adds_exactly_its_tools_to_the_default_listing() -> Result<(), Box<dyn Error>> {
    let expected_extra: &[(&str, &[&str])] = &[
        ("deletion", &["delete_profile", "delete_job"]),
        ("link", &["add_job_from_link"]),
        (
            "arbeitnow",
            &["scan_arbeitnow_jobs", "import_arbeitnow_job"],
        ),
        ("artifacts", &["get_document_artifact"]),
    ];
    let default = listed_names(&[]).await?;
    assert_eq!(default.len(), 34, "the default catalog lists 34 tools");
    for (group, tools) in expected_extra {
        let listed = listed_names(&[*group]).await?;
        let added: Vec<&str> = listed
            .iter()
            .filter(|name| !default.iter().any(|known| known == *name))
            .map(String::as_str)
            .collect();
        assert_eq!(added, *tools, "group {group}");
        assert_eq!(listed.len(), default.len() + tools.len(), "group {group}");
    }
    let all = listed_names(&["deletion", "link", "arbeitnow", "artifacts"]).await?;
    assert_eq!(all.len(), 40, "all opt-in groups together list 40 tools");
    Ok(())
}
