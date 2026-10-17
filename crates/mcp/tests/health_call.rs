//! The `health` tool result must match the golden `call-health.json` (structured and text).

mod common;

use std::error::Error;

use rmcp::model::CallToolRequestParams;

#[tokio::test]
async fn health_call_matches_the_golden() -> Result<(), Box<dyn Error>> {
    let client = common::connect(common::services()).await?;
    let result = client
        .call_tool(CallToolRequestParams::new("health"))
        .await?;

    let golden = common::golden(include_str!("golden/call-health.json"))?;
    assert_eq!(
        serde_json::to_value(&result.structured_content)?,
        golden["structuredContent"]
    );
    assert_eq!(result.is_error, Some(false));

    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.as_str())
        .ok_or("health result has no text content")?;
    let expected_text = golden["content"][0]["text"]
        .as_str()
        .ok_or("call-health.json has no text content")?;
    assert_eq!(text, expected_text);
    Ok(())
}

#[tokio::test]
async fn unknown_tool_is_a_tool_error_with_the_expected_message() -> Result<(), Box<dyn Error>> {
    let client = common::connect(common::services()).await?;
    let result = client
        .call_tool(CallToolRequestParams::new("does-not-exist"))
        .await?;

    assert_eq!(result.is_error, Some(true));
    assert_eq!(result.structured_content, None);
    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.as_str());
    assert_eq!(text, Some("Unknown tool: does-not-exist"));
    Ok(())
}
