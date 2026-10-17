//! `scan_arbeitnow_jobs` bounds over the MCP wire, against recorded results.
//!
//! The vectors in `golden/arbeitnow-scan-limits.json` were recorded by calling the tool through
//! the SDK with the real scan service over a fake board.
//! This test runs the same arguments through the Rust tool with the real core scan service over an
//! empty synthetic board. Only the bounds are compared: a success control checks the call succeeds.
//!
//! One documented limitation remains: an integer above `u64` (here `10^30`) is a float to serde_json
//! without `arbitrary_precision`, so it is refused as `int_type` before the service is reached.
//! The test pins that refusal explicitly.

mod common;

use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::arbeitnow::board::Page;
use jobniac_core::arbeitnow::candidates::CandidateCodec;
use jobniac_core::arbeitnow::cursor::CursorKey;
use jobniac_core::arbeitnow::failure::BoardFailure;
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use jobniac_core::arbeitnow::scan::{
    ArbeitnowBoard, ArbeitnowScanService, ScanRequest, ScanResult,
};
use jobniac_core::errors::ApplicationError;
use jobniac_mcp::tools::arbeitnow::{ArbeitnowPorts, ArbeitnowScanPort};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Map, Value};

type TestResult = Result<(), Box<dyn Error>>;

const SCAN: &str = "scan_arbeitnow_jobs";
const CASES: &str = include_str!("golden/arbeitnow-scan-limits.json");
/// Integers above `u64`: serde_json holds them as floats, so the Rust build refuses them as `int_type`.
const BEYOND_U64: &[&str] = &["limit-ten-pow-thirty", "max-pages-ten-pow-thirty"];

/// A board with no jobs: every bound check happens before any page is read.
#[derive(Debug)]
struct EmptyBoard;

impl ArbeitnowBoard for EmptyBoard {
    fn fetch(&self, _page: u32, _visa: Option<bool>) -> Result<Page, BoardFailure> {
        Ok(Page {
            jobs: Vec::new(),
            another_page_may_remain: false,
        })
    }
}

/// The real core scan service behind the MCP port.
struct CoreScan {
    service: ArbeitnowScanService<EmptyBoard, CandidateCodec>,
}

impl std::fmt::Debug for CoreScan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CoreScan")
    }
}

#[async_trait]
impl ArbeitnowScanPort for CoreScan {
    async fn scan(&self, request: ScanRequest) -> Result<ScanResult, ApplicationError> {
        self.service.scan(&request)
    }
}

fn core_services() -> Result<jobniac_mcp::McpServices, Box<dyn Error>> {
    let codec = CandidateCodec::with_instant_source(&[7u8; 32], || {
        ArbeitnowInstant::new(1_791_547_200, 0)
    })?;
    let cursor = CursorKey::new(&[9u8; 32])?;
    let service = ArbeitnowScanService::new(EmptyBoard, cursor, codec);
    let mut services = common::services();
    services.arbeitnow = ArbeitnowPorts {
        scan: Some(Arc::new(CoreScan { service }) as Arc<dyn ArbeitnowScanPort>),
        import: None,
    };
    Ok(services)
}

async fn call(arguments: Value) -> Result<CallToolResult, Box<dyn Error>> {
    let client = common::connect(core_services()?).await?;
    let map: Map<String, Value> = match arguments {
        Value::Object(map) => map,
        _ => return Err("arguments must be an object".into()),
    };
    Ok(client
        .call_tool(CallToolRequestParams::new(SCAN).with_arguments(map))
        .await?)
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text())
        .map(|block| block.text.clone())
        .collect()
}

#[tokio::test]
async fn scan_bounds_match_the_recorded_results_over_the_wire() -> TestResult {
    let cases: Vec<Value> = serde_json::from_str(CASES)?;
    assert!(cases.len() >= 20, "the golden holds the bound cases");
    for case in cases {
        let id = case["id"].as_str().ok_or("case id")?;
        let result = call(case["arguments"].clone()).await?;
        let expected = &case["result"];
        let expected_error = expected["isError"].as_bool() == Some(true);
        if BEYOND_U64.contains(&id) {
            // The recording reached the service (validation_error); serde_json has no integer here.
            assert_eq!(result.is_error, Some(true), "{id}");
            assert!(
                text(&result).contains("[type=int_type]"),
                "{id}: {}",
                text(&result)
            );
            continue;
        }
        assert_eq!(result.is_error == Some(true), expected_error, "{id}");
        if !expected_error {
            // A successful scan: the board data differs from the recording's, so only the outcome is compared.
            continue;
        }
        let expected_text = expected["content"][0]["text"]
            .as_str()
            .ok_or("expected text")?;
        assert_eq!(text(&result), expected_text, "{id}");
        match expected.get("structuredContent") {
            Some(structured) => {
                assert_eq!(result.structured_content.as_ref(), Some(structured), "{id}");
            }
            None => assert_eq!(result.structured_content, None, "{id}"),
        }
    }
    Ok(())
}
